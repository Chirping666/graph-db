//! Persistent storage engine internals.
//!
//! This module contains the core storage engine for the embedded graph database,
//! including page management, buffer pool, CoW B+ tree operations, record
//! serialization, and the dual-superblock file format.
//!
//! All modules in `storage/` require the `std` feature and are gated behind
//! `#[cfg(feature = "std")]` at the crate root.

pub mod page;
pub mod btree;
pub mod buffer_pool;
pub mod allocator;
pub mod format;
pub mod serialization;
pub mod snapshot;

/// Converts a HAL backend error into a crate-level [`StorageError`](crate::error::StorageError).
pub(crate) fn map_hal_err<E: crate::hal::StorageError>(e: E) -> crate::error::StorageError {
    crate::error::StorageError {
        message: alloc::format!("{e}"),
        source: None,
    }
}

extern crate alloc;

#[cfg(test)]
pub(crate) mod test_utils {
    //! Test-only in-memory backend for storage tests.

    use std::sync::Mutex;

    use crate::hal::error::{StorageErrorKind, StorageErrorType};
    use crate::hal::{ReadAt, WriteAt};

    /// A simple in-memory storage backend error.
    #[derive(Debug)]
    pub struct TestError {
        pub kind: StorageErrorKind,
        pub message: String,
    }

    impl core::fmt::Display for TestError {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            write!(f, "{}: {}", self.kind, self.message)
        }
    }

    impl std::error::Error for TestError {}

    impl crate::hal::error::StorageError for TestError {
        fn kind(&self) -> StorageErrorKind {
            self.kind
        }
    }

    /// A test-only in-memory storage backend.
    ///
    /// Uses a `Vec<u8>` as the backing store. Thread-safe via `Mutex`.
    pub struct TestBackend {
        data: Mutex<Vec<u8>>,
    }

    impl TestBackend {
        /// Creates a new empty `TestBackend`.
        pub fn new() -> Self {
            Self {
                data: Mutex::new(Vec::new()),
            }
        }

        /// Returns a copy of the current backing data.
        pub fn data(&self) -> Vec<u8> {
            self.data.lock().unwrap().clone()
        }
    }

    impl StorageErrorType for TestBackend {
        type Error = TestError;
    }

    impl ReadAt for TestBackend {
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), TestError> {
            let data = self.data.lock().unwrap();
            let start = offset as usize;
            let end = start + buf.len();
            if end > data.len() {
                return Err(TestError {
                    kind: StorageErrorKind::OutOfBounds,
                    message: format!(
                        "read_at: offset={offset}, len={}, file_size={}",
                        buf.len(),
                        data.len()
                    ),
                });
            }
            buf.copy_from_slice(&data[start..end]);
            Ok(())
        }

        fn len(&self) -> Result<u64, TestError> {
            Ok(self.data.lock().unwrap().len() as u64)
        }
    }

    impl WriteAt for TestBackend {
        fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<(), TestError> {
            let mut data = self.data.lock().unwrap();
            let start = offset as usize;
            let end = start + buf.len();
            if end > data.len() {
                data.resize(end, 0);
            }
            data[start..end].copy_from_slice(buf);
            Ok(())
        }

        fn set_len(&mut self, new_size: u64) -> Result<(), TestError> {
            let mut data = self.data.lock().unwrap();
            data.resize(new_size as usize, 0);
            Ok(())
        }
    }

    impl crate::hal::Sync for TestBackend {
        fn sync_data(&mut self) -> Result<(), TestError> {
            Ok(())
        }

        fn sync_all(&mut self) -> Result<(), TestError> {
            Ok(())
        }
    }
}
