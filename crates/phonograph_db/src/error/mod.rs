//! Error types for the database engine.
//!
//! This module defines the unified [`Error`] enum along with
//! [`StorageError`] and [`TransactionError`]. The vocabulary-level
//! error types (`SchemaError`, `NotFoundError`, `InferenceError`) are
//! re-exported from [`phonograph`].

use alloc::{string::String, vec::Vec};

use core::fmt;

use phonograph::constraint::ConstraintViolation;

// ---------------------------------------------------------------------------
// Storage errors
// ---------------------------------------------------------------------------

/// Errors from the storage layer.
///
/// # Examples
///
/// ```
/// use phonograph_db::error::StorageError;
///
/// let err = StorageError {
///     message: "disk full".into(),
///     source: None,
/// };
/// assert!(format!("{err}").contains("disk full"));
/// ```
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
///
/// # Examples
///
/// ```
/// use phonograph_db::error::TransactionError;
///
/// let err = TransactionError::ReadOnly;
/// assert!(format!("{err}").contains("read-only"));
/// ```
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
// Top-level error enum
// ---------------------------------------------------------------------------

/// The unified error type for the graph database.
///
/// All public API methods return `Result<T, Error>`. Each variant wraps
/// a more specific error type.
///
/// # Examples
///
/// ```
/// use phonograph_db::error::{Error, TransactionError};
///
/// let err: Error = TransactionError::ReadOnly.into();
/// assert!(matches!(err, Error::Transaction(_)));
/// ```
#[derive(Debug)]
pub enum Error {
    /// A schema/type-system error.
    Schema(phonograph::SchemaError),
    /// One or more constraint violations prevented the transaction from committing.
    ConstraintViolation(Vec<ConstraintViolation>),
    /// A storage-layer error.
    Storage(StorageError),
    /// A requested entity was not found.
    NotFound(phonograph::NotFoundError),
    /// A transaction lifecycle error.
    Transaction(TransactionError),
    /// An inference subsystem error.
    Inference(phonograph::InferenceError),
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

impl From<phonograph::SchemaError> for Error {
    fn from(e: phonograph::SchemaError) -> Self {
        Error::Schema(e)
    }
}

impl From<phonograph::NotFoundError> for Error {
    fn from(e: phonograph::NotFoundError) -> Self {
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

impl From<phonograph::InferenceError> for Error {
    fn from(e: phonograph::InferenceError) -> Self {
        Error::Inference(e)
    }
}

// ---------------------------------------------------------------------------
// std::error::Error implementations
// ---------------------------------------------------------------------------

#[cfg(feature = "std")]
impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|e| e as &(dyn std::error::Error + 'static))
    }
}

#[cfg(feature = "std")]
impl std::error::Error for TransactionError {}

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
    use phonograph::types::{NodeId, TypeId};

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

    #[test]
    fn transaction_error_display() {
        assert!(format!("{}", TransactionError::ReadOnly).contains("read-only"));
        assert!(format!("{}", TransactionError::AlreadyFinished).contains("already"));
        assert!(format!("{}", TransactionError::WriteLockTimeout).contains("timed out"));
    }

    #[test]
    fn from_schema_error() {
        let e: Error = phonograph::SchemaError::TypeNotFound(TypeId(1)).into();
        assert!(matches!(e, Error::Schema(_)));
    }

    #[test]
    fn from_not_found_error() {
        let e: Error = phonograph::NotFoundError::Node(NodeId(1)).into();
        assert!(matches!(e, Error::NotFound(_)));
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
        let e: Error = phonograph::InferenceError::RuleNotFound("x".into()).into();
        assert!(matches!(e, Error::Inference(_)));
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

        let e: Error = phonograph::SchemaError::TypeNotFound(TypeId(1)).into();
        assert!(e.source().is_some());

        let e2 = Error::ConstraintViolation(vec![]);
        assert!(e2.source().is_none());
    }
}
