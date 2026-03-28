//! Integration tests for the storage engine using FileBackend + tempfile.
//!
//! These tests exercise the full persistence path: create file → write data →
//! close → reopen → verify.

use phonograph_db::backend::OpenableBackend;
use phonograph_db::storage::page::DEFAULT_PAGE_SIZE;
use phonograph_db::storage::snapshot::SnapshotRoots;
use phonograph_db::storage::{StorageEngine, StorageEngineConfig};
use phonograph_std::backend_std::{FileBackend, FileBackendConfig};

fn config() -> StorageEngineConfig {
    StorageEngineConfig {
        page_size: DEFAULT_PAGE_SIZE,
        buffer_pool_frames: 64,
        application_id: 0,
    }
}

fn make_key(n: u64) -> [u8; 8] {
    n.to_be_bytes()
}

fn make_value(n: u64) -> Vec<u8> {
    format!("node-data-{n}").into_bytes()
}

#[test]
fn end_to_end_insert_close_reopen_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");

    // Create and insert 50 nodes
    {
        let backend =
            FileBackend::create(FileBackendConfig { path: path.clone(), read_only: false })
                .unwrap();
        let mut engine = StorageEngine::create(backend, config()).unwrap();
        let snap = engine.current_snapshot();

        let mut root = snap.roots.node_store;
        let mut freed = Vec::new();
        for i in 0..50u64 {
            let r = engine
                .insert(root, &make_key(i), &make_value(i), snap.transaction_id + 1)
                .unwrap();
            freed.extend(r.freed_pages.into_iter().map(|p| (snap.transaction_id + 1, p)));
            root = r.new_root;
        }
        let new_roots = SnapshotRoots {
            node_store: root,
            ..snap.roots
        };
        engine.commit(new_roots, freed).unwrap();
    }

    // Reopen and verify
    {
        let backend =
            FileBackend::open(FileBackendConfig { path: path.clone(), read_only: false }).unwrap();
        let mut engine = StorageEngine::open(backend, config()).unwrap();
        let snap = engine.current_snapshot();

        for i in 0..50u64 {
            let found = engine.search(snap.roots.node_store, &make_key(i)).unwrap();
            assert_eq!(found, Some(make_value(i)), "node {i} missing after reopen");
        }
    }
}

#[test]
fn multi_transaction_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");

    {
        let backend =
            FileBackend::create(FileBackendConfig { path: path.clone(), read_only: false })
                .unwrap();
        let mut engine = StorageEngine::create(backend, config()).unwrap();

        // Transaction 1: insert nodes 1-10
        let snap = engine.current_snapshot();
        let mut root = snap.roots.node_store;
        let mut freed = Vec::new();
        for i in 1..=10u64 {
            let r = engine.insert(root, &make_key(i), &make_value(i), 2).unwrap();
            freed.extend(r.freed_pages.into_iter().map(|p| (2, p)));
            root = r.new_root;
        }
        let roots1 = SnapshotRoots { node_store: root, ..snap.roots };
        let snap2 = engine.commit(roots1, freed).unwrap();

        // Transaction 2: insert 11-20, delete 5
        root = snap2.roots.node_store;
        freed = Vec::new();
        for i in 11..=20u64 {
            let r = engine.insert(root, &make_key(i), &make_value(i), 3).unwrap();
            freed.extend(r.freed_pages.into_iter().map(|p| (3, p)));
            root = r.new_root;
        }
        if let Some(r) = engine.delete(root, &make_key(5), 3).unwrap() {
            freed.extend(r.freed_pages.into_iter().map(|p| (3, p)));
            root = r.new_root;
        }
        let roots2 = SnapshotRoots { node_store: root, ..snap2.roots };
        let snap3 = engine.commit(roots2, freed).unwrap();

        // Transaction 3: insert 21-30
        root = snap3.roots.node_store;
        freed = Vec::new();
        for i in 21..=30u64 {
            let r = engine.insert(root, &make_key(i), &make_value(i), 4).unwrap();
            freed.extend(r.freed_pages.into_iter().map(|p| (4, p)));
            root = r.new_root;
        }
        let roots3 = SnapshotRoots { node_store: root, ..snap3.roots };
        engine.commit(roots3, freed).unwrap();
    }

    // Reopen and verify
    {
        let backend =
            FileBackend::open(FileBackendConfig { path: path.clone(), read_only: false }).unwrap();
        let mut engine = StorageEngine::open(backend, config()).unwrap();
        let snap = engine.current_snapshot();

        // Nodes 1-4, 6-30 present. Node 5 absent.
        for i in 1..=30u64 {
            let found = engine.search(snap.roots.node_store, &make_key(i)).unwrap();
            if i == 5 {
                assert!(found.is_none(), "node 5 should be deleted");
            } else {
                assert!(found.is_some(), "node {i} missing");
            }
        }
    }
}

#[test]
fn file_growth_under_sustained_inserts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");

    let backend =
        FileBackend::create(FileBackendConfig { path: path.clone(), read_only: false }).unwrap();
    let mut engine = StorageEngine::create(backend, config()).unwrap();

    let initial_size = std::fs::metadata(&path).unwrap().len();

    let mut snap = engine.current_snapshot();
    let mut root = snap.roots.node_store;
    let mut all_freed = Vec::new();

    // Insert 1000 records
    for i in 0..1000u64 {
        let txn_id = snap.transaction_id + 1;
        let r = engine.insert(root, &make_key(i), &make_value(i), txn_id).unwrap();
        all_freed.extend(r.freed_pages.into_iter().map(|p| (txn_id, p)));
        root = r.new_root;
    }
    let new_roots = SnapshotRoots { node_store: root, ..snap.roots };
    engine.commit(new_roots, all_freed).unwrap();

    let final_size = std::fs::metadata(&path).unwrap().len();
    assert!(final_size > initial_size, "file should have grown");

    // All records searchable
    snap = engine.current_snapshot();
    for i in 0..1000u64 {
        let found = engine.search(snap.roots.node_store, &make_key(i)).unwrap();
        assert!(found.is_some(), "record {i} missing after growth");
    }
}
