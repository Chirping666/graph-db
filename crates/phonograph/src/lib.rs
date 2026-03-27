//! Core types, traits, and error hierarchy for typed property graphs.
//!
//! `phonograph` is the `no_std + alloc` vocabulary crate for the Phonograph
//! graph database. It contains:
//!
//! - **`types`** — Core data model: node/edge/type IDs, `Value`, `Node`, `Edge`,
//!   `TypeDefinition`, `PropertyDeclaration`.
//! - **`schema`** — Read-only view traits for the type registry and graph.
//! - **`constraint`** — Pluggable constraint validation trait and change-tracking types.
//! - **`inference`** — Pluggable inference rule trait, provenance, and materialization types.
//! - **`error`** — Error types: `SchemaError`, `NotFoundError`, `InferenceError`.
//!
//! All modules compile under `#![no_std]` with the `alloc` crate. Enable the
//! `std` feature for `std::error::Error` implementations.

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
pub use error::{InferenceError, NotFoundError, SchemaError};
