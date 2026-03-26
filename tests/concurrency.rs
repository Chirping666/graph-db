//! Concurrency tests for the database engine (Task 25, Phase 11).
//!
//! Tests multiple readers, reader/writer isolation, write serialization,
//! and concurrent stress scenarios.

use std::sync::Arc;
use std::thread;

use graph_db::db::builders::{EdgeBuilder, NodeBuilder, TypeDefinitionBuilder};
use graph_db::db::config::DatabaseConfig;
use graph_db::db::database::Database;
use graph_db::types::{NodeId, Value};

use std::sync::atomic::{AtomicBool, Ordering};

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

// =========================================================================
// Snapshot isolation under continuous writes (Task 28, 2.1)
// =========================================================================

#[test]
fn concurrency_snapshot_isolation() {
    let (db, _dir) = open_temp_db();

    // Create 100 nodes with a generation counter property.
    // All 100 nodes start at generation 0.
    let node_type;
    let generation_key;
    let node_ids: Vec<NodeId>;
    {
        let mut wtx = db.write_txn().unwrap();
        node_type = wtx
            .register_type(TypeDefinitionBuilder::node_type("N").build())
            .unwrap();
        generation_key = wtx.get_or_create_property_key("gen").unwrap();

        let mut ids = Vec::new();
        for _ in 0..100 {
            let id = wtx
                .insert_node(
                    NodeBuilder::new()
                        .type_label(node_type)
                        .property(generation_key, Value::I64(0))
                        .build(),
                )
                .unwrap();
            ids.push(id);
        }
        node_ids = ids;
        wtx.commit().unwrap();
    }

    let db = Arc::new(db);

    // Writer thread: 20 transactions, each updating ALL 100 nodes to the
    // next generation. This ensures that within a single committed transaction,
    // all nodes have the same generation value.
    let db_writer = Arc::clone(&db);
    let ids_w = node_ids.clone();
    let writer_handle = thread::spawn(move || {
        for generation in 1..=20i64 {
            let mut wtx = db_writer.write_txn().unwrap();
            for &id in &ids_w {
                wtx.set_node_property(id, generation_key, Value::I64(generation))
                    .unwrap();
            }
            wtx.commit().unwrap();
        }
    });

    // 4 reader threads: each reads all 100 nodes and verifies they all have
    // the SAME generation value within a single read transaction.
    let mut reader_handles = Vec::new();
    for _ in 0..4 {
        let db_r = Arc::clone(&db);
        let ids_r = node_ids.clone();
        reader_handles.push(thread::spawn(move || {
            for _ in 0..50 {
                let rtx = db_r.read_txn().unwrap();
                let count = rtx.node_count().unwrap();
                assert_eq!(count, 100);

                // Read first node's generation
                let first = rtx.get_node(ids_r[0]).unwrap().unwrap();
                let first_gen = match first.properties.get(&generation_key) {
                    Some(Value::I64(g)) => *g,
                    _ => panic!("Expected I64 generation"),
                };

                // ALL nodes should have the same generation in this snapshot
                for &id in &ids_r[1..] {
                    let node = rtx.get_node(id).unwrap().unwrap();
                    let node_gen = match node.properties.get(&generation_key) {
                        Some(Value::I64(g)) => *g,
                        _ => panic!("Expected I64 generation"),
                    };
                    assert_eq!(
                        node_gen, first_gen,
                        "Partial-batch visibility: node {:?} has gen {} but first node has gen {}",
                        id, node_gen, first_gen
                    );
                }
            }
        }));
    }

    writer_handle.join().unwrap();
    for h in reader_handles {
        h.join().unwrap();
    }

    // Final check: all nodes at generation 20
    let rtx = db.read_txn().unwrap();
    for &id in &node_ids {
        let node = rtx.get_node(id).unwrap().unwrap();
        assert_eq!(
            node.properties.get(&generation_key),
            Some(&Value::I64(20))
        );
    }
}

// =========================================================================
// Write serialization under contention — 8 threads (Task 28, 2.2)
// =========================================================================

#[test]
fn concurrency_write_contention_8_threads() {
    let (db, _dir) = open_temp_db();

    let node_type;
    let val_key;
    {
        let mut wtx = db.write_txn().unwrap();
        node_type = wtx
            .register_type(TypeDefinitionBuilder::node_type("N").build())
            .unwrap();
        val_key = wtx.get_or_create_property_key("thread_id").unwrap();
        wtx.commit().unwrap();
    }

    let db = Arc::new(db);
    let mut handles = Vec::new();

    for thread_id in 0..8i64 {
        let db_c = Arc::clone(&db);
        handles.push(thread::spawn(move || {
            let mut wtx = db_c.write_txn().unwrap();
            wtx.insert_node(
                NodeBuilder::new()
                    .type_label(node_type)
                    .property(val_key, Value::I64(thread_id))
                    .build(),
            )
            .unwrap();
            wtx.commit().unwrap();
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    // Verify exactly 8 nodes, each with a unique thread_id
    let rtx = db.read_txn().unwrap();
    assert_eq!(rtx.node_count().unwrap(), 8);

    let mut seen_ids: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let all = rtx.all_nodes().unwrap();
    for node in &all {
        if let Some(Value::I64(tid)) = node.properties.get(&val_key) {
            assert!(seen_ids.insert(*tid), "Duplicate thread_id {tid}");
        }
    }
    assert_eq!(seen_ids.len(), 8);
}

// =========================================================================
// High-throughput mixed read/write stress test (Task 28, 2.3)
// =========================================================================

#[test]
#[ignore] // Stress test: ~3-5 seconds. Run with `cargo test -- --ignored`.
fn concurrency_high_throughput() {
    let (db, _dir) = open_temp_db();

    // Create initial data: 50 nodes, 50 edges
    let node_type;
    let edge_type;
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
        for i in 0..50 {
            wtx.insert_edge(
                EdgeBuilder::new(ids[i], ids[(i + 1) % 50])
                    .type_label(edge_type)
                    .build(),
            )
            .unwrap();
        }
        wtx.commit().unwrap();
    }

    let db = Arc::new(db);
    let done = Arc::new(AtomicBool::new(false));

    // Writer: 50 iterations, each inserting 5 nodes + 10 edges
    let db_w = Arc::clone(&db);
    let done_w = Arc::clone(&done);
    let writer = thread::spawn(move || {
        for _ in 0..50 {
            let mut wtx = db_w.write_txn().unwrap();
            let mut new_ids = Vec::new();
            for _ in 0..5 {
                let id = wtx
                    .insert_node(NodeBuilder::new().type_label(node_type).build())
                    .unwrap();
                new_ids.push(id);
            }
            // Create edges among new nodes
            for i in 0..new_ids.len() {
                let src = new_ids[i];
                let tgt = new_ids[(i + 1) % new_ids.len()];
                wtx.insert_edge(
                    EdgeBuilder::new(src, tgt).type_label(edge_type).build(),
                )
                .unwrap();
                wtx.insert_edge(
                    EdgeBuilder::new(tgt, src).type_label(edge_type).build(),
                )
                .unwrap();
            }
            wtx.commit().unwrap();
        }
        done_w.store(true, Ordering::Release);
    });

    // 6 reader threads: continuously read until writer finishes
    let mut readers = Vec::new();
    for _ in 0..6 {
        let db_r = Arc::clone(&db);
        let done_r = Arc::clone(&done);
        readers.push(thread::spawn(move || {
            let mut iterations = 0u64;
            while !done_r.load(Ordering::Acquire) {
                let rtx = db_r.read_txn().unwrap();
                let n = rtx.node_count().unwrap();
                assert!(n >= 50, "node count {n} < 50");
                let e = rtx.edge_count().unwrap();
                assert!(e >= 50, "edge count {e} < 50");
                iterations += 1;
            }
            iterations
        }));
    }

    writer.join().unwrap();
    for r in readers {
        let iters = r.join().unwrap();
        assert!(iters > 0, "Reader thread did no iterations");
    }

    // Final counts: 50 + 50*5 = 300 nodes, 50 + 50*10 = 550 edges
    let rtx = db.read_txn().unwrap();
    assert_eq!(rtx.node_count().unwrap(), 300);
    assert_eq!(rtx.edge_count().unwrap(), 550);
}
