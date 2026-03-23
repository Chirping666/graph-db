//! An embedded graph database with extensible schema and pluggable inference.
//!
//! `graph_db` is a single-file, embedded graph database engine implemented
//! entirely in Rust. It provides a typed property graph model with support
//! for pluggable constraint validation and inference rules.
//!
//! # Architecture
//!
//! The crate is organized into layers: core types (`types`, `schema`,
//! `constraint`, `inference`, `error`) form the `no_std + alloc` foundation,
//! while the storage engine, buffer pool, and database facade require `std`.
//!
//! # Feature Flags
//!
//! - **`std`** (default) — enables the full database engine, file-backed
//!   storage, and `std::error::Error` implementations. Implies `alloc`.
//! - **`alloc`** — enables core types that require heap allocation
//!   (`String`, `Vec`, `BTreeMap`). This is the minimum feature set for
//!   using the type system, schema traits, and error types in a `no_std`
//!   environment.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod types;
pub mod schema;
pub mod constraint;
pub mod inference;
pub mod error;
pub mod hal;

#[cfg(feature = "std")]
pub mod hal_std;

#[cfg(feature = "std")]
pub mod storage;

// Re-export primary types for convenience.
pub use types::{
    Edge, EdgeId, Node, NodeId, PropertyDeclaration, PropertyKeyId, PropertyMap, TypeDefinition,
    TypeId, TypeKind, Value, ValueTypeDescriptor,
};
pub use schema::{GraphView, PropertyKeyRegistryView, TypeRegistryView};
pub use constraint::{
    ChangeSet, ConstraintValidator, ConstraintViolation, EdgeChange, NodeChange, ViolationSubject,
};
pub use inference::{
    InferenceMode, InferenceResult, InferenceRule, InferredEntity, InferredFact,
    MaterializedMapping, ProvenanceRecord,
};
pub use error::{Error, InferenceError, NotFoundError, SchemaError, StorageError, TransactionError};
pub use hal::{ReadAt, StorageBackend, StorageErrorKind, StorageErrorType, WriteAt};
// Note: hal::Sync is NOT re-exported at the crate root to avoid shadowing
// core::marker::Sync. Access it as graph_db::hal::Sync.
// Note: hal::StorageError (trait) is NOT re-exported here to avoid collision
// with error::StorageError (struct). Access it as graph_db::hal::StorageError.

#[cfg(test)]
mod compile_tests {
    use super::*;

    // Verify Send + Sync on Box<dyn ConstraintValidator>
    fn _assert_validator_send_sync(_: Box<dyn ConstraintValidator>) {}
    // Verify Send + Sync on Box<dyn InferenceRule>
    fn _assert_rule_send_sync(_: Box<dyn InferenceRule>) {}
    // Verify all trait objects are object-safe
    fn _assert_graph_view(_: &dyn GraphView) {}
    fn _assert_type_registry_view(_: &dyn TypeRegistryView) {}
    fn _assert_property_key_registry_view(_: &dyn PropertyKeyRegistryView) {}

    // HAL: StorageBackend is object-safe
    fn _assert_storage_backend_object_safe<E: hal::StorageError>(
        _: &dyn hal::StorageBackend<Error = E>,
    ) {
    }

    // FileBackend satisfies StorageBackend
    #[cfg(feature = "std")]
    fn _assert_file_backend_is_storage_backend() {
        fn _check<T: hal::StorageBackend>() {}
        _check::<hal_std::FileBackend>();
    }

    // FileBackend satisfies OpenableBackend
    #[cfg(feature = "std")]
    fn _assert_file_backend_is_openable() {
        fn _check<T: hal::OpenableBackend>() {}
        _check::<hal_std::FileBackend>();
    }

    // FileBackend satisfies LockableBackend
    #[cfg(feature = "std")]
    fn _assert_file_backend_is_lockable() {
        fn _check<T: hal::LockableBackend>() {}
        _check::<hal_std::FileBackend>();
    }
}
