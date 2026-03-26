//! Core types, traits, and error hierarchy for `graph_db`.
//!
//! This crate provides the `no_std + alloc` foundation used by the full
//! `graph_db` database engine. It contains:
//!
//! - **`types`** — Core data model: node/edge/type IDs, `Value`, `Node`, `Edge`,
//!   `TypeDefinition`, `PropertyDeclaration`.
//! - **`schema`** — Read-only view traits for the type registry and graph.
//! - **`constraint`** — Pluggable constraint validation trait and change-tracking types.
//! - **`inference`** — Pluggable inference rule trait, provenance, and materialization types.
//! - **`error`** — Unified error hierarchy.
//! - **`backend`** — Storage backend trait definitions (`ReadAt`, `WriteAt`, `Durability`).
//! - **`backend_mem`** — In-memory storage backend.
//!
//! All modules (except `backend/lifecycle`) compile under `#![no_std]` with the
//! `alloc` crate. Enable the `std` feature for `std::error::Error` implementations
//! and file I/O helpers on `MemoryBackend`.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

#[cfg(feature = "alloc")]
pub mod types;
pub mod backend;
#[cfg(feature = "alloc")]
pub mod schema;
#[cfg(feature = "alloc")]
pub mod constraint;
#[cfg(feature = "alloc")]
pub mod inference;
#[cfg(feature = "alloc")]
pub mod error;
#[cfg(feature = "alloc")]
pub mod backend_mem;

// Convenience re-exports.
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
