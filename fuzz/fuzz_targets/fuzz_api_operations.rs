//! Fuzz target for API operation sequences.
//!
//! Interprets arbitrary bytes as a sequence of database operations on an
//! in-memory database. All operations are allowed to return errors — only
//! panics are bugs.
//!
//! Run with: `cargo +nightly fuzz run fuzz_api_operations -- -max_total_time=60`

#![no_main]
use libfuzzer_sys::fuzz_target;

use graph_db::db::builders::{EdgeBuilder, NodeBuilder, TypeDefinitionBuilder};
use graph_db::db::config::DatabaseConfig;
use graph_db::db::database::Database;
use graph_db::types::{EdgeId, NodeId, Value};

fuzz_target!(|data: &[u8]| {
    let db = match Database::open(DatabaseConfig::in_memory()) {
        Ok(db) => db,
        Err(_) => return,
    };

    // Setup: register one node type and one edge type.
    let (node_type, edge_type, name_key) = {
        let mut wtx = match db.write_txn() {
            Ok(w) => w,
            Err(_) => return,
        };
        let nt = match wtx.register_type(TypeDefinitionBuilder::node_type("N").build()) {
            Ok(t) => t,
            Err(_) => return,
        };
        let et = match wtx.register_type(TypeDefinitionBuilder::edge_type("E").build()) {
            Ok(t) => t,
            Err(_) => return,
        };
        let nk = match wtx.get_or_create_property_key("name") {
            Ok(k) => k,
            Err(_) => return,
        };
        if wtx.commit().is_err() {
            return;
        }
        (nt, et, nk)
    };

    // Track recently created IDs for edge insertion.
    let mut node_ids: Vec<NodeId> = Vec::new();
    let mut edge_ids: Vec<EdgeId> = Vec::new();
    // Start a write transaction.
    let mut wtx = match db.write_txn() {
        Ok(w) => w,
        Err(_) => return,
    };

    for &byte in data {
        match byte {
            // 0x00..0x3F: Insert node (optionally with a property).
            0x00..=0x3F => {
                let builder = if byte & 0x10 != 0 {
                    NodeBuilder::new()
                        .type_label(node_type)
                        .property(name_key, Value::String(format!("n{byte}")))
                } else {
                    NodeBuilder::new().type_label(node_type)
                };
                if let Ok(id) = wtx.insert_node(builder.build()) {
                    node_ids.push(id);
                }
            }
            // 0x40..0x5F: Insert edge between two recent nodes.
            0x40..=0x5F => {
                if node_ids.len() >= 2 {
                    let src_idx = (byte as usize) % node_ids.len();
                    let tgt_idx = (src_idx + 1) % node_ids.len();
                    let src = node_ids[src_idx];
                    let tgt = node_ids[tgt_idx];
                    if let Ok(id) = wtx.insert_edge(
                        EdgeBuilder::new(src, tgt).type_label(edge_type).build(),
                    ) {
                        edge_ids.push(id);
                    }
                }
            }
            // 0x60..0x6F: Delete most recently inserted node.
            0x60..=0x6F => {
                if let Some(id) = node_ids.pop() {
                    let _ = wtx.delete_node(id);
                }
            }
            // 0x70..0x7F: Delete most recently inserted edge.
            0x70..=0x7F => {
                if let Some(id) = edge_ids.pop() {
                    let _ = wtx.delete_edge(id);
                }
            }
            // 0x80..0x8F: Query node_count.
            0x80..=0x8F => {
                let _ = wtx.node_count();
            }
            // 0x90..0x9F: Get a recent node.
            0x90..=0x9F => {
                if !node_ids.is_empty() {
                    let idx = (byte as usize) % node_ids.len();
                    let _ = wtx.get_node(node_ids[idx]);
                }
            }
            // 0xA0..0xAF: Get outgoing edges for a recent node.
            0xA0..=0xAF => {
                if !node_ids.is_empty() {
                    let idx = (byte as usize) % node_ids.len();
                    let _ = wtx.outgoing_edges(node_ids[idx], None);
                }
            }
            // 0xB0..0xBF: Commit and start new transaction.
            0xB0..=0xBF => {
                let _ = wtx.commit();
                wtx = match db.write_txn() {
                    Ok(w) => w,
                    Err(_) => return,
                };
            }
            // 0xC0..0xCF: Update a node's property.
            0xC0..=0xCF => {
                if !node_ids.is_empty() {
                    let idx = (byte as usize) % node_ids.len();
                    let _ = wtx.set_node_property(
                        node_ids[idx],
                        name_key,
                        Value::I64(byte as i64),
                    );
                }
            }
            // Other bytes: no-op.
            _ => {}
        }
    }

    // Final commit (ignore errors — abort is also fine).
    let _ = wtx.commit();
});
