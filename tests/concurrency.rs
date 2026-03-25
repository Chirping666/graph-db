//! Concurrency tests for the database engine (Task 25, Phase 11).
//!
//! Tests multiple readers, reader/writer isolation, write serialization,
//! and concurrent stress scenarios.

use std::sync::Arc;
use std::thread;

use graph_db::db::builders::{NodeBuilder, TypeDefinitionBuilder, EdgeBuilder};
use graph_db::db::config::DatabaseConfig;
use graph_db::db::database::Database;
use graph_db::types::{NodeId, Value};

/// Helper: creates a temp-dir database.
fn open_temp_db() -> (Database, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    let db = Database::open(DatabaseConfig::persistent(&path)).unwrap();
    (db, dir)
}

// =========================================================================
// 11.1 — Multiple concurrent readers
// =========================================================================

#[test]
fn concurrent_readers() {
    let (db, _dir) = open_temp_db();

    // Insert some data
    let node_type;
    let name_key;
    let node_ids: Vec<NodeId>;

    {
        let mut wtx = db.write_txn().unwrap();
        node_type = wtx
            .register_type(TypeDefinitionBuilder::node_type("Item").build())
            .unwrap();
        name_key = wtx.get_or_create_property_key("name").unwrap();

        let mut ids = Vec::new();
        for i in 0..10 {
            let id = wtx
                .insert_node(
                    NodeBuilder::new()
                        .type_label(node_type)
                        .property(name_key, Value::String(format!("Item{i}")))
                        .build(),
                )
                .unwrap();
            ids.push(id);
        }
        node_ids = ids;
        wtx.commit().unwrap();
    }

    let db = Arc::new(db);
    let mut handles = Vec::new();

    for thread_idx in 0..4 {
        let db = Arc::clone(&db);
        let ids = node_ids.clone();
        handles.push(thread::spawn(move || {
            let rtx = db.read_txn().unwrap();

            // Each reader verifies all nodes
            for (i, &id) in ids.iter().enumerate() {
                let node = rtx.get_node(id).unwrap().unwrap();
                assert_eq!(
                    node.properties.get(&name_key),
                    Some(&Value::String(format!("Item{i}")))
                );
            }

            let count = rtx.node_count().unwrap();
            assert_eq!(count, 10, "thread {thread_idx} saw wrong count");
            count
        }));
    }

    for handle in handles {
        let count = handle.join().unwrap();
        assert_eq!(count, 10);
    }
}

// =========================================================================
// 11.2 — Reader/writer isolation
// =========================================================================

#[test]
fn reader_writer_isolation() {
    let (db, _dir) = open_temp_db();

    // Insert node A
    let a_id;
    let node_type;
    {
        let mut wtx = db.write_txn().unwrap();
        node_type = wtx
            .register_type(TypeDefinitionBuilder::node_type("N").build())
            .unwrap();
        a_id = wtx
            .insert_node(NodeBuilder::new().type_label(node_type).build())
            .unwrap();
        wtx.commit().unwrap();
    }

    let db = Arc::new(db);

    // Spawn reader that holds a snapshot before the write
    let db_reader = Arc::clone(&db);
    let reader_handle = thread::spawn(move || {
        let rtx = db_reader.read_txn().unwrap();

        // Reader sees A
        assert!(rtx.get_node(a_id).unwrap().is_some());

        // Give writer time to commit
        thread::sleep(std::time::Duration::from_millis(50));

        // Reader's snapshot should NOT see the new node B
        // (we only have 1 node in our snapshot)
        let count = rtx.node_count().unwrap();
        assert_eq!(count, 1, "reader should see only 1 node from its snapshot");

        rtx.finish();
    });

    // Small delay to ensure reader acquires snapshot first
    thread::sleep(std::time::Duration::from_millis(10));

    // Writer inserts node B
    {
        let mut wtx = db.write_txn().unwrap();
        wtx.insert_node(NodeBuilder::new().type_label(node_type).build())
            .unwrap();
        wtx.commit().unwrap();
    }

    reader_handle.join().unwrap();

    // New reader should see both nodes
    let rtx = db.read_txn().unwrap();
    assert_eq!(rtx.node_count().unwrap(), 2);
}

// =========================================================================
// 11.3 — Write serialization
// =========================================================================

#[test]
fn write_serialization() {
    let (db, _dir) = open_temp_db();

    let node_type;
    {
        let mut wtx = db.write_txn().unwrap();
        node_type = wtx
            .register_type(TypeDefinitionBuilder::node_type("N").build())
            .unwrap();
        wtx.commit().unwrap();
    }

    let db = Arc::new(db);

    let db1 = Arc::clone(&db);
    let db2 = Arc::clone(&db);

    let h1 = thread::spawn(move || {
        let mut wtx = db1.write_txn().unwrap();
        wtx.insert_node(NodeBuilder::new().type_label(node_type).build())
            .unwrap();
        // Small delay so the second writer has time to block
        thread::sleep(std::time::Duration::from_millis(20));
        wtx.commit().unwrap();
    });

    let h2 = thread::spawn(move || {
        // Small delay so first writer likely gets the lock first
        thread::sleep(std::time::Duration::from_millis(5));
        let mut wtx = db2.write_txn().unwrap();
        wtx.insert_node(NodeBuilder::new().type_label(node_type).build())
            .unwrap();
        wtx.commit().unwrap();
    });

    h1.join().unwrap();
    h2.join().unwrap();

    // Both insertions should be present
    let rtx = db.read_txn().unwrap();
    assert_eq!(rtx.node_count().unwrap(), 2);
}

// =========================================================================
// 11.4 — Concurrent read during write (stress test)
// =========================================================================

#[test]
fn concurrent_stress() {
    let (db, _dir) = open_temp_db();

    // Create initial data: 50 nodes, 100 edges
    let node_type;
    let edge_type;
    let initial_node_ids: Vec<NodeId>;
    {
        let mut wtx = db.write_txn().unwrap();
        node_type = wtx
            .register_type(TypeDefinitionBuilder::node_type("N").build())
            .unwrap();
        edge_type = wtx
            .register_type(TypeDefinitionBuilder::edge_type("E").build())
            .unwrap();

        let mut ids = Vec::new();
        for _ in 0..50 {
            let id = wtx
                .insert_node(NodeBuilder::new().type_label(node_type).build())
                .unwrap();
            ids.push(id);
        }
        // Create edges between consecutive nodes
        for i in 0..ids.len() {
            let src = ids[i];
            let tgt = ids[(i + 1) % ids.len()];
            wtx.insert_edge(
                EdgeBuilder::new(src, tgt).type_label(edge_type).build(),
            )
            .unwrap();
            // Also add a second edge for variety
            wtx.insert_edge(
                EdgeBuilder::new(src, tgt).type_label(edge_type).build(),
            )
            .unwrap();
        }
        initial_node_ids = ids;
        wtx.commit().unwrap();
    }

    let db = Arc::new(db);

    // Writer thread: 5 transactions, each inserting 5 nodes
    let db_writer = Arc::clone(&db);
    let writer_handle = thread::spawn(move || {
        for _ in 0..5 {
            let mut wtx = db_writer.write_txn().unwrap();
            for _ in 0..5 {
                wtx.insert_node(NodeBuilder::new().type_label(node_type).build())
                    .unwrap();
            }
            wtx.commit().unwrap();
        }
    });

    // 3 reader threads that continuously read
    let mut reader_handles = Vec::new();
    for _ in 0..3 {
        let db_r = Arc::clone(&db);
        let ids = initial_node_ids.clone();
        reader_handles.push(thread::spawn(move || {
            for _ in 0..10 {
                let rtx = db_r.read_txn().unwrap();
                let count = rtx.node_count().unwrap();
                // Should be at least initial 50, at most 50 + 25 (5*5)
                assert!(count >= 50, "count was {count}, expected >= 50");
                assert!(count <= 75, "count was {count}, expected <= 75");

                // Verify a known initial node still exists
                let node = rtx.get_node(ids[0]).unwrap();
                assert!(node.is_some(), "initial node should always be visible");

                // Check outgoing edges for consistency
                let edges = rtx.outgoing_edges(ids[0], None).unwrap();
                assert_eq!(edges.len(), 2, "initial node should have 2 outgoing edges");
            }
        }));
    }

    writer_handle.join().unwrap();
    for h in reader_handles {
        h.join().unwrap();
    }

    // Final check: all 75 nodes should be present
    let rtx = db.read_txn().unwrap();
    assert_eq!(rtx.node_count().unwrap(), 75);
}
