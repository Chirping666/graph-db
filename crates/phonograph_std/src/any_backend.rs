//! [`AnyBackend`] — enum dispatch over file and in-memory storage backends.

use phonograph_db::backend::{self, BackendError, BackendErrorType, ReadAt, StorageErrorKind, WriteAt};
use phonograph_db::backend_mem::{MemoryBackend, MemoryError};

use crate::backend_std::{FileBackend, FileError};

// ---------------------------------------------------------------------------
// AnyBackendError
// ---------------------------------------------------------------------------

/// Unified error type for [`AnyBackend`].
#[derive(Debug)]
pub enum AnyBackendError {
    /// Error from the file-backed storage backend.
    File(FileError),
    /// Error from the in-memory storage backend.
    Memory(MemoryError),
}

impl core::fmt::Display for AnyBackendError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AnyBackendError::File(e) => e.fmt(f),
            AnyBackendError::Memory(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for AnyBackendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AnyBackendError::File(e) => e.source(),
            AnyBackendError::Memory(_) => None,
        }
    }
}

impl BackendError for AnyBackendError {
    fn kind(&self) -> StorageErrorKind {
        match self {
            AnyBackendError::File(e) => e.kind(),
            AnyBackendError::Memory(e) => e.kind(),
        }
    }
}

// ---------------------------------------------------------------------------
// AnyBackend
// ---------------------------------------------------------------------------

/// Backend enum supporting both file and in-memory storage.
///
/// Implements all backend traits via match-and-delegate so that
/// `StorageEngine<AnyBackend>` works identically regardless of the
/// underlying backend.
pub enum AnyBackend {
    /// File-backed persistent storage.
    File(FileBackend),
    /// In-memory storage backed by `Vec<u8>`.
    Memory(MemoryBackend),
}

impl BackendErrorType for AnyBackend {
    type Error = AnyBackendError;
}

impl ReadAt for AnyBackend {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), AnyBackendError> {
        match self {
            AnyBackend::File(f) => f.read_at(offset, buf).map_err(AnyBackendError::File),
            AnyBackend::Memory(m) => m.read_at(offset, buf).map_err(AnyBackendError::Memory),
        }
    }

    fn len(&self) -> Result<u64, AnyBackendError> {
        match self {
            AnyBackend::File(f) => f.len().map_err(AnyBackendError::File),
            AnyBackend::Memory(m) => m.len().map_err(AnyBackendError::Memory),
        }
    }
}

impl WriteAt for AnyBackend {
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<(), AnyBackendError> {
        match self {
            AnyBackend::File(f) => f.write_at(offset, buf).map_err(AnyBackendError::File),
            AnyBackend::Memory(m) => m.write_at(offset, buf).map_err(AnyBackendError::Memory),
        }
    }

    fn set_len(&mut self, new_size: u64) -> Result<(), AnyBackendError> {
        match self {
            AnyBackend::File(f) => f.set_len(new_size).map_err(AnyBackendError::File),
            AnyBackend::Memory(m) => m.set_len(new_size).map_err(AnyBackendError::Memory),
        }
    }
}

impl backend::Durability for AnyBackend {
    fn sync_data(&mut self) -> Result<(), AnyBackendError> {
        match self {
            AnyBackend::File(f) => backend::Durability::sync_data(f).map_err(AnyBackendError::File),
            AnyBackend::Memory(m) => {
                backend::Durability::sync_data(m).map_err(AnyBackendError::Memory)
            }
        }
    }

    fn sync_all(&mut self) -> Result<(), AnyBackendError> {
        match self {
            AnyBackend::File(f) => backend::Durability::sync_all(f).map_err(AnyBackendError::File),
            AnyBackend::Memory(m) => {
                backend::Durability::sync_all(m).map_err(AnyBackendError::Memory)
            }
        }
    }
}
