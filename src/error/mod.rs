//! Error types for the graph database.
//!
//! This module defines the unified error hierarchy including schema errors,
//! storage errors, transaction errors, inference errors, and constraint
//! violations.
//!
//! All error types implement [`core::fmt::Display`] and [`core::fmt::Debug`].
//! Under the `std` feature, they additionally implement [`std::error::Error`].

#[cfg(feature = "alloc")]
use alloc::{string::String, vec::Vec};

use core::fmt;

use crate::constraint::ConstraintViolation;
use crate::types::{PropertyKeyId, TypeId, TypeKind};

// ---------------------------------------------------------------------------
// Schema errors
// ---------------------------------------------------------------------------

/// Errors related to the type system and schema.
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
// Storage errors
// ---------------------------------------------------------------------------

/// Errors from the storage layer.
#[derive(Debug)]
pub struct StorageError {
    /// A human-readable description of the error.
    pub message: String,
    /// The underlying I/O error, if any. Only available with the `std` feature.
    #[cfg(feature = "std")]
    pub source: Option<std::io::Error>,
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "storage error: {}", self.message)?;
        #[cfg(feature = "std")]
        if let Some(ref src) = self.source {
            write!(f, " (caused by: {src})")?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Transaction errors
// ---------------------------------------------------------------------------

/// Errors related to transaction lifecycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransactionError {
    /// Attempted a write operation on a read-only transaction.
    ReadOnly,
    /// The transaction has already been committed or rolled back.
    AlreadyFinished,
    /// Timed out waiting to acquire the write lock.
    WriteLockTimeout,
}

impl fmt::Display for TransactionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransactionError::ReadOnly => {
                write!(f, "cannot perform write operation on a read-only transaction")
            }
            TransactionError::AlreadyFinished => {
                write!(f, "transaction has already been committed or rolled back")
            }
            TransactionError::WriteLockTimeout => {
                write!(f, "timed out waiting to acquire the write lock")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Inference errors
// ---------------------------------------------------------------------------

/// Errors related to the inference subsystem.
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
// Top-level error enum
// ---------------------------------------------------------------------------

/// The unified error type for the graph database.
///
/// All public API methods return `Result<T, Error>`. Each variant wraps
/// a more specific error type.
#[derive(Debug)]
pub enum Error {
    /// A schema/type-system error.
    Schema(SchemaError),
    /// One or more constraint violations prevented the transaction from committing.
    ConstraintViolation(Vec<ConstraintViolation>),
    /// A storage-layer error.
    Storage(StorageError),
    /// A requested entity was not found.
    NotFound(NotFoundError),
    /// A transaction lifecycle error.
    Transaction(TransactionError),
    /// An inference subsystem error.
    Inference(InferenceError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Schema(e) => write!(f, "{e}"),
            Error::ConstraintViolation(violations) => {
                write!(f, "{} constraint violation(s)", violations.len())
            }
            Error::Storage(e) => write!(f, "{e}"),
            Error::NotFound(e) => write!(f, "{e}"),
            Error::Transaction(e) => write!(f, "{e}"),
            Error::Inference(e) => write!(f, "{e}"),
        }
    }
}

impl From<SchemaError> for Error {
    fn from(e: SchemaError) -> Self {
        Error::Schema(e)
    }
}

impl From<NotFoundError> for Error {
    fn from(e: NotFoundError) -> Self {
        Error::NotFound(e)
    }
}

impl From<StorageError> for Error {
    fn from(e: StorageError) -> Self {
        Error::Storage(e)
    }
}

impl From<TransactionError> for Error {
    fn from(e: TransactionError) -> Self {
        Error::Transaction(e)
    }
}

impl From<InferenceError> for Error {
    fn from(e: InferenceError) -> Self {
        Error::Inference(e)
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
impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_ref().map(|e| e as &(dyn std::error::Error + 'static))
    }
}

#[cfg(feature = "std")]
impl std::error::Error for TransactionError {}

#[cfg(feature = "std")]
impl std::error::Error for InferenceError {}

#[cfg(feature = "std")]
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Schema(e) => Some(e),
            Error::Storage(e) => Some(e),
            Error::NotFound(e) => Some(e),
            Error::Transaction(e) => Some(e),
            Error::Inference(e) => Some(e),
            Error::ConstraintViolation(_) => None,
        }
    }
}

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

    // === StorageError ===

    #[test]
    fn storage_error_display() {
        let e = StorageError {
            message: "disk full".into(),
            #[cfg(feature = "std")]
            source: None,
        };
        let s = format!("{e}");
        assert!(s.contains("disk full"));
    }

    #[cfg(feature = "std")]
    #[test]
    fn storage_error_with_source() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file gone");
        let e = StorageError {
            message: "read failed".into(),
            source: Some(io_err),
        };
        let s = format!("{e}");
        assert!(s.contains("read failed"));
        assert!(s.contains("file gone"));

        use std::error::Error;
        assert!(e.source().is_some());
    }

    // === TransactionError ===

    #[test]
    fn transaction_error_display() {
        assert!(format!("{}", TransactionError::ReadOnly).contains("read-only"));
        assert!(format!("{}", TransactionError::AlreadyFinished).contains("already"));
        assert!(format!("{}", TransactionError::WriteLockTimeout).contains("timed out"));
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

    // === From conversions ===

    #[test]
    fn from_schema_error() {
        let e: Error = SchemaError::TypeNotFound(TypeId(1)).into();
        assert!(matches!(e, Error::Schema(SchemaError::TypeNotFound(_))));
    }

    #[test]
    fn from_not_found_error() {
        let e: Error = NotFoundError::Node(NodeId(1)).into();
        assert!(matches!(e, Error::NotFound(NotFoundError::Node(_))));
    }

    #[test]
    fn from_storage_error() {
        let se = StorageError {
            message: "test".into(),
            #[cfg(feature = "std")]
            source: None,
        };
        let e: Error = se.into();
        assert!(matches!(e, Error::Storage(_)));
    }

    #[test]
    fn from_transaction_error() {
        let e: Error = TransactionError::ReadOnly.into();
        assert!(matches!(e, Error::Transaction(TransactionError::ReadOnly)));
    }

    #[test]
    fn from_inference_error() {
        let e: Error = InferenceError::RuleNotFound("x".into()).into();
        assert!(matches!(e, Error::Inference(InferenceError::RuleNotFound(_))));
    }

    #[test]
    fn error_display() {
        let e = Error::ConstraintViolation(vec![ConstraintViolation {
            violation_kind: "test".into(),
            message: "msg".into(),
            subject: None,
        }]);
        assert!(format!("{e}").contains("1 constraint violation"));
    }

    #[cfg(feature = "std")]
    #[test]
    fn error_std_source() {
        use std::error::Error as StdError;

        let e: Error = SchemaError::TypeNotFound(TypeId(1)).into();
        assert!(e.source().is_some());

        let e2 = Error::ConstraintViolation(vec![]);
        assert!(e2.source().is_none());
    }
}
