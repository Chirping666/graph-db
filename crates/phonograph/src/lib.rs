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

extern crate alloc;

pub mod types;
pub mod schema;
pub mod constraint;
pub mod inference;
pub mod error;

// Convenience re-exports.
pub use types::{
    Edge, EdgeId, Node, NodeId, PropertyDeclaration, PropertyKeyId, PropertyMap, TypeDefinition,
    TypeId, TypeKind, Value, ValueTypeDescriptor, property_map_total_eq,
};
pub use schema::{GraphView, PropertyKeyRegistryView, TypeRegistryView};
pub use constraint::{
    ChangeSet, ConstraintValidator, ConstraintViolation, EdgeChange, NodeChange, ViolationSubject,
};
pub use inference::{
    InferenceMode, InferenceResult, InferenceRule, InferredEntity, InferredFact,
    MaterializedMapping, ProvenanceRecord,
};
pub use error::{InferenceError, NotFoundError, SchemaError};
