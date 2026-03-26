//! Storage backend error types.
//!
//! Defines [`StorageErrorKind`] for categorizing storage errors,
//! the [`StorageError`] trait that all backend error types implement,
//! and [`StorageErrorType`] for associating an error type with a backend.

use core::fmt;

/// Categorizes storage errors for generic error handling.
///
/// Allows code generic over [`StorageBackend`](super::traits::StorageBackend)
/// to make decisions based on error category without knowing the concrete
/// error type. Follows the `embedded-hal` `ErrorKind` pattern.
///
/// # Examples
///
/// ```
/// use graph_db_core::StorageErrorKind;
///
/// let kind = StorageErrorKind::OutOfBounds;
/// assert_eq!(format!("{kind}"), "out of bounds");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StorageErrorKind {
    /// Read or write referenced an offset beyond the current storage size.
    OutOfBounds,
    /// Underlying I/O operation failed (OS-level error, disk failure, etc.).
    Io,
    /// Storage medium is in read-only mode and a write was attempted.
    ReadOnly,
    /// Cannot grow storage to accommodate the requested operation.
    StorageFull,
    /// Checksum mismatch or structural inconsistency in the storage medium.
    MediaCorruption,
    /// I/O operation was interrupted (e.g., `EINTR` on Unix). Caller may retry.
    Interrupted,
    /// File is locked by another process and exclusive access cannot be obtained.
    LockContention,
    /// Error not covered by the other categories.
    Other,
}

impl fmt::Display for StorageErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StorageErrorKind::OutOfBounds => write!(f, "out of bounds"),
            StorageErrorKind::Io => write!(f, "I/O error"),
            StorageErrorKind::ReadOnly => write!(f, "read-only"),
            StorageErrorKind::StorageFull => write!(f, "storage full"),
            StorageErrorKind::MediaCorruption => write!(f, "media corruption"),
            StorageErrorKind::Interrupted => write!(f, "interrupted"),
            StorageErrorKind::LockContention => write!(f, "lock contention"),
            StorageErrorKind::Other => write!(f, "other error"),
        }
    }
}

/// Trait for storage backend errors.
///
/// Every concrete error type from a storage backend must implement this
/// trait. It replaces `std::error::Error` in `no_std` environments while
/// providing a [`kind`](StorageError::kind) method for categorized error
/// handling.
///
/// This trait is object-safe: `dyn StorageError` is a valid trait object.
pub trait StorageError: fmt::Debug + fmt::Display {
    /// Returns the category of this error.
    fn kind(&self) -> StorageErrorKind;
}

/// Associates a concrete error type with a storage implementation.
///
/// This trait groups the associated error type shared by [`ReadAt`](super::traits::ReadAt),
/// [`WriteAt`](super::traits::WriteAt), and [`Durability`](super::traits::Durability),
/// avoiding repetition. It is a supertrait of all I/O traits.
///
/// This trait is object-safe.
pub trait StorageErrorType {
    /// The error type produced by this backend's I/O operations.
    type Error: StorageError;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_error_kind_display_non_empty() {
        let variants = [
            StorageErrorKind::OutOfBounds,
            StorageErrorKind::Io,
            StorageErrorKind::ReadOnly,
            StorageErrorKind::StorageFull,
            StorageErrorKind::MediaCorruption,
            StorageErrorKind::Interrupted,
            StorageErrorKind::LockContention,
            StorageErrorKind::Other,
        ];
        for v in &variants {
            let s = format!("{v}");
            assert!(!s.is_empty(), "{v:?} has empty Display");
        }
    }

    #[test]
    fn storage_error_kind_equality() {
        assert_eq!(StorageErrorKind::Io, StorageErrorKind::Io);
        assert_ne!(StorageErrorKind::Io, StorageErrorKind::ReadOnly);
    }

    #[test]
    fn storage_error_kind_clone() {
        let a = StorageErrorKind::OutOfBounds;
        let b = a;
        assert_eq!(a, b);
    }

    // StorageErrorKind is #[non_exhaustive] — this cannot be tested from
    // within the crate, but external consumers cannot exhaustively match.

    // Object-safety assertion for StorageError trait
    fn _assert_storage_error_object_safe(_: &dyn StorageError) {}
}
