//! Core storage backend I/O traits.
//!
//! Defines [`ReadAt`], [`WriteAt`], [`Durability`], and [`StorageBackend`].
//! All traits are object-safe and `no_std + alloc` compatible.

use super::error::StorageErrorType;

/// Random-access read from a storage medium.
///
/// # Concurrency
///
/// Takes `&self` (not `&mut self`) to enable concurrent reads. Maps to
/// `pread()` on Unix, which is thread-safe and does not use shared seek
/// state.
///
/// This trait is object-safe.
#[allow(clippy::len_without_is_empty)]
pub trait ReadAt: StorageErrorType {
    /// Reads exactly `buf.len()` bytes starting at byte offset `offset`.
    ///
    /// On success, exactly `buf.len()` bytes have been read into `buf`.
    /// Partial reads are not exposed — the implementation must retry
    /// or return an error.
    ///
    /// # Errors
    ///
    /// - [`OutOfBounds`](super::StorageErrorKind::OutOfBounds) if
    ///   `offset + buf.len()` exceeds the storage size.
    /// - [`Io`](super::StorageErrorKind::Io) on underlying I/O failure.
    /// - [`MediaCorruption`](super::StorageErrorKind::MediaCorruption) if
    ///   the medium detects an integrity error during the read.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Self::Error>;

    /// Returns the current storage size in bytes.
    ///
    /// For file-backed storage this is the file length; for in-memory
    /// storage this is the `Vec` length.
    ///
    /// # Errors
    ///
    /// - [`Io`](super::StorageErrorKind::Io) if the size cannot be determined.
    fn len(&self) -> Result<u64, Self::Error>;
}

/// Random-access write to a storage medium.
///
/// # Concurrency
///
/// Takes `&mut self` to enforce the single-writer invariant at the Rust
/// type level. The storage engine holds `&mut` via an `RwLock` write guard.
///
/// # Durability
///
/// Writes are **not** durable until [`Durability::sync_data`] or
/// [`Durability::sync_all`] is called. Data may be buffered in userspace
/// or OS page cache.
///
/// This trait is object-safe.
pub trait WriteAt: StorageErrorType {
    /// Writes exactly `buf.len()` bytes at byte offset `offset`.
    ///
    /// If `offset + buf.len()` exceeds the current storage size, behavior
    /// is backend-defined: file backends may fail with `OutOfBounds`
    /// (caller must call [`set_len`](WriteAt::set_len) first), while
    /// memory backends may auto-extend.
    ///
    /// On success, exactly `buf.len()` bytes have been written. Partial
    /// writes are not exposed.
    ///
    /// # Errors
    ///
    /// - [`OutOfBounds`](super::StorageErrorKind::OutOfBounds) if the
    ///   write extends beyond storage and the backend does not auto-extend.
    /// - [`Io`](super::StorageErrorKind::Io) on underlying I/O failure.
    /// - [`ReadOnly`](super::StorageErrorKind::ReadOnly) if the backend
    ///   is in read-only mode.
    /// - [`StorageFull`](super::StorageErrorKind::StorageFull) if the
    ///   medium cannot accommodate the write.
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<(), Self::Error>;

    /// Sets the storage size to `new_size` bytes.
    ///
    /// - If `new_size > current`: extends with zero-filled bytes.
    /// - If `new_size < current`: truncates (data beyond `new_size` is lost).
    /// - If `new_size == current`: no-op.
    ///
    /// # fsync note
    ///
    /// After extending a file with `set_len()`, the caller **must** call
    /// [`Durability::sync_all`] (not `sync_data`) to ensure the new file size
    /// metadata is durable.
    ///
    /// # Errors
    ///
    /// - [`StorageFull`](super::StorageErrorKind::StorageFull) if the
    ///   medium cannot grow to the requested size.
    /// - [`Io`](super::StorageErrorKind::Io) on underlying I/O failure.
    /// - [`ReadOnly`](super::StorageErrorKind::ReadOnly) if the backend
    ///   is in read-only mode.
    fn set_len(&mut self, new_size: u64) -> Result<(), Self::Error>;
}

/// Durability control: flush buffered writes to stable storage.
///
/// Provides two sync levels for the commit protocol:
///
/// - [`sync_data`](Durability::sync_data) — data only (faster, omits metadata sync).
/// - [`sync_all`](Durability::sync_all) — data + metadata (required after file extension).
///
/// # Platform mapping
///
/// | Platform | `sync_data()` | `sync_all()` |
/// |----------|---------------|--------------|
/// | Linux    | `fdatasync()` | `fsync()`    |
/// | macOS    | `fcntl(F_FULLFSYNC)` | `fcntl(F_FULLFSYNC)` |
/// | Windows  | `FlushFileBuffers()` | `FlushFileBuffers()` |
/// | Memory   | No-op         | No-op        |
///
/// This trait is object-safe.
pub trait Durability: StorageErrorType {
    /// Flushes all buffered data writes to stable storage.
    ///
    /// After this returns `Ok(())`, all bytes written via
    /// [`WriteAt::write_at`] since the last sync are durable (assuming
    /// the underlying storage correctly implements sync semantics).
    ///
    /// # Errors
    ///
    /// - [`Io`](super::StorageErrorKind::Io) if the sync operation fails.
    fn sync_data(&mut self) -> Result<(), Self::Error>;

    /// Flushes all buffered data **and metadata** to stable storage.
    ///
    /// This is a superset of [`sync_data`](Durability::sync_data) —
    /// additionally ensures file metadata (size, modification time,
    /// directory entry) is durable.
    ///
    /// Call this instead of `sync_data` when the file was extended via
    /// [`WriteAt::set_len`] in the current transaction, to ensure the
    /// new file size is durable before the superblock references pages
    /// in the extended region.
    ///
    /// # Errors
    ///
    /// - [`Io`](super::StorageErrorKind::Io) if the sync operation fails.
    fn sync_all(&mut self) -> Result<(), Self::Error>;
}

/// Full storage backend: readable, writable, and syncable.
///
/// This is the primary trait bound used throughout the storage engine.
/// Any type implementing [`ReadAt`] + [`WriteAt`] + [`Durability`] automatically
/// implements `StorageBackend` via the blanket impl — users never need
/// to write `impl StorageBackend for MyBackend`.
///
/// # Usage in the engine
///
/// ```rust,ignore
/// pub struct Engine<B: StorageBackend> {
///     backend: Arc<RwLock<B>>,
/// }
/// ```
///
/// This trait is object-safe.
pub trait StorageBackend: ReadAt + WriteAt + Durability {}

/// Blanket implementation: any type with all three sub-traits is
/// automatically a [`StorageBackend`].
impl<T: ReadAt + WriteAt + Durability> StorageBackend for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::error::{StorageError, StorageErrorKind};
    use alloc::vec::Vec;

    // --- Mock types for testing ---

    #[derive(Debug)]
    struct MockError(StorageErrorKind);

    impl core::fmt::Display for MockError {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            write!(f, "mock error: {}", self.0)
        }
    }

    impl StorageError for MockError {
        fn kind(&self) -> StorageErrorKind {
            self.0
        }
    }

    struct MockBackend {
        data: Vec<u8>,
    }

    impl MockBackend {
        fn new() -> Self {
            MockBackend { data: Vec::new() }
        }
    }

    impl StorageErrorType for MockBackend {
        type Error = MockError;
    }

    impl ReadAt for MockBackend {
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), MockError> {
            let offset = offset as usize;
            if offset + buf.len() > self.data.len() {
                return Err(MockError(StorageErrorKind::OutOfBounds));
            }
            buf.copy_from_slice(&self.data[offset..offset + buf.len()]);
            Ok(())
        }

        fn len(&self) -> Result<u64, MockError> {
            Ok(self.data.len() as u64)
        }
    }

    impl WriteAt for MockBackend {
        fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<(), MockError> {
            let offset = offset as usize;
            let end = offset + buf.len();
            if end > self.data.len() {
                self.data.resize(end, 0);
            }
            self.data[offset..end].copy_from_slice(buf);
            Ok(())
        }

        fn set_len(&mut self, new_size: u64) -> Result<(), MockError> {
            self.data.resize(new_size as usize, 0);
            Ok(())
        }
    }

    impl Durability for MockBackend {
        fn sync_data(&mut self) -> Result<(), MockError> {
            Ok(())
        }

        fn sync_all(&mut self) -> Result<(), MockError> {
            Ok(())
        }
    }

    // --- Tests ---

    #[test]
    fn mock_write_read_roundtrip() {
        let mut b = MockBackend::new();
        b.write_at(0, b"hello").unwrap();
        let mut buf = [0u8; 5];
        b.read_at(0, &mut buf).unwrap();
        assert_eq!(&buf, b"hello");
    }

    #[test]
    fn mock_len() {
        let mut b = MockBackend::new();
        assert_eq!(b.len().unwrap(), 0);
        b.write_at(0, b"abc").unwrap();
        assert_eq!(b.len().unwrap(), 3);
    }

    #[test]
    fn mock_set_len_expand() {
        let mut b = MockBackend::new();
        b.set_len(10).unwrap();
        assert_eq!(b.len().unwrap(), 10);
        let mut buf = [0xFFu8; 10];
        b.read_at(0, &mut buf).unwrap();
        assert_eq!(buf, [0u8; 10]);
    }

    #[test]
    fn mock_set_len_truncate() {
        let mut b = MockBackend::new();
        b.write_at(0, b"hello world").unwrap();
        b.set_len(5).unwrap();
        assert_eq!(b.len().unwrap(), 5);
    }

    #[test]
    fn mock_read_out_of_bounds() {
        let b = MockBackend::new();
        let mut buf = [0u8; 1];
        let err = b.read_at(0, &mut buf).unwrap_err();
        assert_eq!(err.kind(), StorageErrorKind::OutOfBounds);
    }

    #[test]
    fn mock_sync_succeeds() {
        let mut b = MockBackend::new();
        b.sync_data().unwrap();
        b.sync_all().unwrap();
    }

    #[test]
    fn mock_satisfies_storage_backend() {
        fn _check<T: StorageBackend>() {}
        _check::<MockBackend>();
    }

    #[test]
    fn mock_write_at_offset() {
        let mut b = MockBackend::new();
        b.set_len(10).unwrap();
        b.write_at(5, b"hello").unwrap();
        let mut buf = [0u8; 5];
        b.read_at(5, &mut buf).unwrap();
        assert_eq!(&buf, b"hello");
    }

    #[test]
    fn mock_empty_buffer_operations() {
        let mut b = MockBackend::new();
        b.write_at(0, &[]).unwrap();
        b.read_at(0, &mut []).unwrap();
    }

    // Object-safety assertions
    fn _assert_read_at_object_safe(_: &dyn ReadAt<Error = MockError>) {}
    fn _assert_write_at_object_safe(_: &mut dyn WriteAt<Error = MockError>) {}
    fn _assert_durability_object_safe(_: &mut dyn Durability<Error = MockError>) {}
    fn _assert_storage_backend_object_safe(_: &dyn StorageBackend<Error = MockError>) {}
}
