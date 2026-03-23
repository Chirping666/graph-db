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
}
