//! Error types for the graph vocabulary crate.
//!
//! This module defines error types for schema, not-found, and inference errors.
//!
//! All error types implement [`core::fmt::Display`] and [`core::fmt::Debug`].
//! Under the `std` feature, they additionally implement [`std::error::Error`].

use alloc::string::String;

use core::fmt;

use crate::types::{PropertyKeyId, TypeId, TypeKind};

// ---------------------------------------------------------------------------
// Schema errors
// ---------------------------------------------------------------------------

/// Errors related to the type system and schema.
///
/// # Examples
///
/// ```
/// use phonograph::error::SchemaError;
/// use phonograph::TypeKind;
///
/// let err = SchemaError::DuplicateTypeName {
///     name: "Person".into(),
///     kind: TypeKind::Node,
/// };
/// assert!(format!("{err}").contains("Person"));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaError {
    /// A type with the same name and kind already exists.
    DuplicateTypeName {
        /// The duplicate name.
        name: String,
        /// The kind of type.
        kind: TypeKind,
    },
    /// The referenced type was not found in the registry.
    TypeNotFound(TypeId),
    /// Adding the supertype would create a cycle in the type hierarchy.
    CycleDetected {
        /// The child type that was being modified.
        child: TypeId,
        /// The supertype that would create a cycle.
        would_be_parent: TypeId,
    },
    /// A referenced supertype does not exist.
    SupertypeNotFound(TypeId),
    /// A type kind mismatch (e.g., trying to assign a node type to an edge).
    KindMismatch {
        /// The expected type kind.
        expected: TypeKind,
        /// The actual type kind found.
        found: TypeKind,
    },
    /// A property key with the same name already exists.
    DuplicatePropertyKey {
        /// The duplicate property key name.
        name: String,
    },
    /// The referenced property key was not found.
    PropertyKeyNotFound(PropertyKeyId),
}

impl fmt::Display for SchemaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SchemaError::DuplicateTypeName { name, kind } => {
                write!(f, "duplicate {kind} type name: {name:?}")
            }
            SchemaError::TypeNotFound(id) => {
                write!(f, "type with id {} not found", id.0)
            }
            SchemaError::CycleDetected {
                child,
                would_be_parent,
            } => {
                write!(
                    f,
                    "adding supertype {} to type {} would create a cycle",
                    would_be_parent.0, child.0
                )
            }
            SchemaError::SupertypeNotFound(id) => {
                write!(f, "supertype with id {} not found", id.0)
            }
            SchemaError::KindMismatch { expected, found } => {
                write!(f, "type kind mismatch: expected {expected}, found {found}")
            }
            SchemaError::DuplicatePropertyKey { name } => {
                write!(f, "duplicate property key name: {name:?}")
            }
            SchemaError::PropertyKeyNotFound(id) => {
                write!(f, "property key with id {} not found", id.0)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Not-found errors
// ---------------------------------------------------------------------------

/// Errors indicating a requested entity was not found.
///
/// # Examples
///
/// ```
/// use phonograph::error::NotFoundError;
/// use phonograph::NodeId;
///
/// let err = NotFoundError::Node(NodeId(42));
/// assert!(format!("{err}").contains("42"));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotFoundError {
    /// A node with the given id was not found.
    Node(crate::types::NodeId),
    /// An edge with the given id was not found.
    Edge(crate::types::EdgeId),
    /// A type with the given id was not found.
    Type(TypeId),
    /// A property key with the given id was not found.
    PropertyKey(PropertyKeyId),
}

impl fmt::Display for NotFoundError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NotFoundError::Node(id) => write!(f, "node with id {} not found", id.0),
            NotFoundError::Edge(id) => write!(f, "edge with id {} not found", id.0),
            NotFoundError::Type(id) => write!(f, "type with id {} not found", id.0),
            NotFoundError::PropertyKey(id) => {
                write!(f, "property key with id {} not found", id.0)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Inference errors
// ---------------------------------------------------------------------------

/// Errors related to the inference subsystem.
///
/// # Examples
///
/// ```
/// use phonograph::InferenceError;
///
/// let err = InferenceError::RuleNotFound("my_rule".into());
/// assert!(format!("{err}").contains("my_rule"));
/// ```
#[derive(Clone, Debug)]
pub enum InferenceError {
    /// The requested inference rule was not found.
    RuleNotFound(String),
    /// An inferred fact was invalid.
    InvalidFact {
        /// The name of the rule that produced the invalid fact.
        rule_name: String,
        /// A description of why the fact is invalid.
        message: String,
    },
}

impl fmt::Display for InferenceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InferenceError::RuleNotFound(name) => {
                write!(f, "inference rule not found: {name:?}")
            }
            InferenceError::InvalidFact { rule_name, message } => {
                write!(f, "invalid fact from rule {rule_name:?}: {message}")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// std::error::Error implementations
// ---------------------------------------------------------------------------

#[cfg(feature = "std")]
impl std::error::Error for SchemaError {}

#[cfg(feature = "std")]
impl std::error::Error for NotFoundError {}

#[cfg(feature = "std")]
impl std::error::Error for InferenceError {}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EdgeId, NodeId};

    // === SchemaError ===

    #[test]
    fn schema_error_variants() {
        let _ = SchemaError::DuplicateTypeName {
            name: "Person".into(),
            kind: TypeKind::Node,
        };
        let _ = SchemaError::TypeNotFound(TypeId(1));
        let _ = SchemaError::CycleDetected {
            child: TypeId(1),
            would_be_parent: TypeId(2),
        };
        let _ = SchemaError::SupertypeNotFound(TypeId(3));
        let _ = SchemaError::KindMismatch {
            expected: TypeKind::Node,
            found: TypeKind::Edge,
        };
        let _ = SchemaError::DuplicatePropertyKey {
            name: "name".into(),
        };
        let _ = SchemaError::PropertyKeyNotFound(PropertyKeyId(1));
    }

    #[test]
    fn schema_error_display() {
        let e = SchemaError::DuplicateTypeName {
            name: "Person".into(),
            kind: TypeKind::Node,
        };
        let s = format!("{e}");
        assert!(s.contains("Person"));
        assert!(s.contains("Node"));

        let e2 = SchemaError::TypeNotFound(TypeId(42));
        assert!(format!("{e2}").contains("42"));
    }

    // === NotFoundError ===

    #[test]
    fn not_found_error_variants() {
        let _ = NotFoundError::Node(NodeId(1));
        let _ = NotFoundError::Edge(EdgeId(2));
        let _ = NotFoundError::Type(TypeId(3));
        let _ = NotFoundError::PropertyKey(PropertyKeyId(4));
    }

    #[test]
    fn not_found_error_display() {
        let e = NotFoundError::Node(NodeId(42));
        assert!(format!("{e}").contains("42"));
    }

    // === InferenceError ===

    #[test]
    fn inference_error_display() {
        let e = InferenceError::RuleNotFound("my_rule".into());
        assert!(format!("{e}").contains("my_rule"));

        let e2 = InferenceError::InvalidFact {
            rule_name: "r1".into(),
            message: "bad node ref".into(),
        };
        assert!(format!("{e2}").contains("r1"));
        assert!(format!("{e2}").contains("bad node ref"));
    }
}
