//! An embedded graph database with extensible schema and pluggable inference.
//!
//! `graph_db` is a single-file, embedded graph database engine implemented
//! entirely in Rust. It provides a typed property graph model with pluggable
//! constraint validation and inference rules, designed as a foundation for
//! ontology systems, knowledge graphs, and typed graph applications.
//!
//! The crate provides **mechanism**, not **policy**: you define the types,
//! constraints, and inference rules for your domain. No ontology vocabulary
//! (OWL, RDF, SKOS, etc.) is built in.
//!
//! # Quick Start
//!
//! ```rust
//! use graph_db::db::{Database, DatabaseConfig, NodeBuilder, TypeDefinitionBuilder};
//! use graph_db::{Value, Error};
//!
//! fn main() -> Result<(), Error> {
//!     // Open an in-memory database
//!     let db = Database::open(DatabaseConfig::in_memory())?;
//!
//!     // Register a type and property key, then insert a node
//!     let (person_type, name_key) = {
//!         let mut wtx = db.write_txn()?;
//!         let person = wtx.register_type(
//!             TypeDefinitionBuilder::node_type("Person").build(),
//!         )?;
//!         let name = wtx.get_or_create_property_key("name")?;
//!         wtx.insert_node(
//!             NodeBuilder::new()
//!                 .type_label(person)
//!                 .property(name, Value::String("Alice".into()))
//!                 .build(),
//!         )?;
//!         wtx.commit()?;
//!         (person, name)
//!     };
//!
//!     // Query it back
//!     let rtx = db.read_txn()?;
//!     let people = rtx.nodes_by_type(person_type, false)?;
//!     assert_eq!(people.len(), 1);
//!     assert_eq!(
//!         people[0].properties.get(&name_key),
//!         Some(&Value::String("Alice".into())),
//!     );
//!     Ok(())
//! }
//! ```
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────┐
//! │  Application / Downstream Crate     │
//! ├─────────────────────────────────────┤
//! │  Public API (Database, Transactions)│
//! ├─────────────────────────────────────┤
//! │  Query & Traversal Engine           │
//! ├─────────────────────────────────────┤
//! │  Storage Engine (B+ trees, pages)   │
//! ├─────────────────────────────────────┤
//! │  Storage Backend Traits             │
//! ├──────────────┬──────────────────────┤
//! │  File backend │  In-memory backend  │
//! └──────────────┴──────────────────────┘
//! ```
//!
//! The core types (`types`, `schema`, `constraint`, `inference`, `error`)
//! form the `no_std + alloc` foundation. The storage engine, buffer pool,
//! and database facade require `std`.
//!
//! # Feature Flags
//!
//! - **`std`** (default) — enables the full database engine, file-backed
//!   storage, and `std::error::Error` implementations. Implies `alloc`.
//! - **`alloc`** — enables core types that require heap allocation
//!   (`String`, `Vec`, `BTreeMap`). This is the minimum feature set for
//!   using the type system, schema traits, and error types in a `no_std`
//!   environment.
//!
//! # Thread Safety
//!
//! [`db::Database`] is `Send + Sync` and can be shared across threads
//! (e.g., via `Arc<Database>`). Transactions ([`db::ReadTransaction`],
//! [`db::WriteTransaction`]) are `!Send` and `!Sync` — they hold
//! buffer-pool references and must be used on the thread that created them.
//! Extract owned data (nodes, edges, IDs) to share results across threads.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
pub mod types;
#[cfg(feature = "alloc")]
pub mod schema;
#[cfg(feature = "alloc")]
pub mod constraint;
#[cfg(feature = "alloc")]
pub mod inference;
#[cfg(feature = "alloc")]
pub mod error;
pub mod backend;

#[cfg(feature = "alloc")]
pub mod backend_mem;

#[cfg(feature = "std")]
pub mod backend_std;

#[cfg(feature = "std")]
pub mod storage;

#[cfg(feature = "std")]
pub mod db;

// Re-export database facade types for convenience.
#[cfg(feature = "std")]
pub use db::{
    Database, DatabaseConfig, EdgeBuilder, MissingExtensions, NodeBuilder, ReadTransaction,
    StorageMode, TypeDefinitionBuilder, WriteTransaction,
};

// Re-export primary types for convenience.
#[cfg(feature = "alloc")]
pub use types::{
    Edge, EdgeId, Node, NodeId, PropertyDeclaration, PropertyKeyId, PropertyMap, TypeDefinition,
    TypeId, TypeKind, Value, ValueTypeDescriptor,
};
#[cfg(feature = "alloc")]
pub use schema::{GraphView, PropertyKeyRegistryView, TypeRegistryView};
#[cfg(feature = "alloc")]
pub use constraint::{
    ChangeSet, ConstraintValidator, ConstraintViolation, EdgeChange, NodeChange, ViolationSubject,
};
#[cfg(feature = "alloc")]
pub use inference::{
    InferenceMode, InferenceResult, InferenceRule, InferredEntity, InferredFact,
    MaterializedMapping, ProvenanceRecord,
};
#[cfg(feature = "alloc")]
pub use error::{Error, InferenceError, NotFoundError, SchemaError, StorageError, TransactionError};
pub use backend::{Durability, ReadAt, StorageBackend, StorageErrorKind, StorageErrorType, WriteAt};
#[cfg(feature = "alloc")]
pub use backend_mem::{MemoryBackend, MemoryError};
// Note: backend::StorageError (trait) is NOT re-exported here to avoid collision
// with error::StorageError (struct). Access it as graph_db::backend::StorageError.

#[cfg(test)]
mod compile_tests {
    use super::*;

    // Verify Send + Sync on Box<dyn ConstraintValidator>
    #[cfg(feature = "alloc")]
    fn _assert_validator_send_sync(_: Box<dyn ConstraintValidator>) {}
    // Verify Send + Sync on Box<dyn InferenceRule>
    #[cfg(feature = "alloc")]
    fn _assert_rule_send_sync(_: Box<dyn InferenceRule>) {}
    // Verify all trait objects are object-safe
    #[cfg(feature = "alloc")]
    fn _assert_graph_view(_: &dyn GraphView) {}
    #[cfg(feature = "alloc")]
    fn _assert_type_registry_view(_: &dyn TypeRegistryView) {}
    #[cfg(feature = "alloc")]
    fn _assert_property_key_registry_view(_: &dyn PropertyKeyRegistryView) {}

    // StorageBackend is object-safe
    fn _assert_storage_backend_object_safe<E: backend::StorageError>(
        _: &dyn backend::StorageBackend<Error = E>,
    ) {
    }

    // FileBackend satisfies StorageBackend
    #[cfg(feature = "std")]
    fn _assert_file_backend_is_storage_backend() {
        fn _check<T: backend::StorageBackend>() {}
        _check::<backend_std::FileBackend>();
    }

    // FileBackend satisfies OpenableBackend
    #[cfg(feature = "std")]
    fn _assert_file_backend_is_openable() {
        fn _check<T: backend::OpenableBackend>() {}
        _check::<backend_std::FileBackend>();
    }

    // FileBackend satisfies LockableBackend
    #[cfg(feature = "std")]
    fn _assert_file_backend_is_lockable() {
        fn _check<T: backend::LockableBackend>() {}
        _check::<backend_std::FileBackend>();
    }
}
