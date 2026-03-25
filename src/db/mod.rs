//! Database engine layer.
//!
//! This module provides the complete public API for interacting with the
//! graph database: `Database` for lifecycle management, `ReadTransaction`
//! for snapshot-isolated reads, and `WriteTransaction` for mutations with
//! read-your-own-writes semantics.
//!
//! The `db` module sits on top of the storage engine (`storage/`) and
//! translates high-level graph operations (insert node, traverse edges,
//! query by type) into low-level B-tree operations. It enforces single-writer
//! MVCC concurrency, dispatches constraint validators at commit time, and
//! provides inference rule stubs for Task 26.

pub mod builders;
pub mod config;
pub mod database;
pub mod graph_reader;
pub mod graph_view;
pub mod read_txn;
pub mod schema_cache;
pub mod write_buffer;
pub mod write_txn;

pub use builders::{EdgeBuilder, NodeBuilder, TypeDefinitionBuilder};
pub use config::{DatabaseConfig, StorageMode};
pub use database::{Database, MissingExtensions};
pub use graph_reader::GraphReader;
pub use read_txn::ReadTransaction;
pub use write_txn::WriteTransaction;
