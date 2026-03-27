//! [`MemoryBackend`] implementation and its [`MemoryError`] type.

use alloc::vec::Vec;
use core::fmt;

use crate::backend::{self, BackendErrorType, StorageErrorKind};

/// Error type for the in-memory storage backend.
///
/// In-memory writes to a growable `Vec` cannot fail in the I/O sense.
/// The only failure mode is an out-of-bounds read (attempting to read
/// beyond the current `Vec` length without first extending it).
///
/// In practice, most operations on `MemoryBackend` are infallible.
/// This error type exists to satisfy the trait's associated type
/// requirement.
///
/// # Examples
///
/// ```
/// use phonograph_db::backend_mem::MemoryError;
///
/// let err = MemoryError::OutOfBounds {
///     offset: 100,
///     requested: 10,
///     size: 50,
/// };
/// assert!(format!("{err}").contains("out of bounds"));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryError {
    /// A read was attempted beyond the current storage size.
    OutOfBounds {
        /// Byte offset of the read.
        offset: u64,
        /// Number of bytes requested.
        requested: usize,
        /// Current size of the backing storage.
        size: u64,
    },
}

impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MemoryError::OutOfBounds {
                offset,
                requested,
                size,
            } => write!(
                f,
                "out of bounds: read at offset {offset} with length {requested} exceeds memory size {size}"
            ),
        }
    }
}

impl crate::backend::BackendError for MemoryError {
    fn kind(&self) -> StorageErrorKind {
        match self {
            MemoryError::OutOfBounds { .. } => StorageErrorKind::OutOfBounds,
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for MemoryError {}

// ---------------------------------------------------------------------------
// MemoryBackend
// ---------------------------------------------------------------------------

/// In-memory storage backend backed by a `Vec<u8>`.
///
/// # Use cases
///
/// - **Testing:** fast, deterministic, no filesystem interaction.
/// - **Ephemeral databases:** short-lived data that does not need persistence.
/// - **`no_std + alloc` environments** without a filesystem.
///
/// # Optional snapshot support
///
/// With the `std` feature, the backend supports saving its contents to a file
/// ([`save_to_file`](Self::save_to_file)) and loading from one
/// ([`load_from_file`](Self::load_from_file)). The resulting file is a valid
/// database file and can be opened with
/// `FileBackend` (from `graph_db`).
///
/// # Thread safety
///
/// [`ReadAt::read_at`](crate::backend::ReadAt::read_at) takes `&self`. Because
/// `MemoryBackend` stores data in a plain `Vec<u8>`, immutable access is safe
/// for concurrent reads. The storage engine wraps it in `RwLock` as with any
/// backend.
///
/// # Examples
///
/// ```
/// use phonograph_db::backend_mem::MemoryBackend;
///
/// let backend = MemoryBackend::new();
/// assert!(backend.as_bytes().is_empty());
///
/// let backend = MemoryBackend::with_size(4096);
/// assert_eq!(backend.as_bytes().len(), 4096);
///
/// let backend = MemoryBackend::from_bytes(vec![1, 2, 3]);
/// assert_eq!(backend.as_bytes(), &[1, 2, 3]);
/// ```
pub struct MemoryBackend {
    data: Vec<u8>,
}

impl MemoryBackend {
    /// Creates a new empty in-memory backend.
    ///
    /// The backing storage starts with zero length and grows automatically
    /// on writes.
    pub fn new() -> Self {
        Self { data: Vec::new() }
    }

    /// Creates a new in-memory backend pre-filled with `size` zero bytes.
    ///
    /// This is useful when the caller knows the approximate database size
    /// upfront and wants to avoid repeated reallocations.
    pub fn with_size(size: usize) -> Self {
        Self {
            data: alloc::vec![0u8; size],
        }
    }

    /// Creates a new in-memory backend from existing data.
    ///
    /// This is the "load from snapshot in bytes" entry point. The data should
    /// be a valid database image (e.g., previously obtained via
    /// [`as_bytes`](Self::as_bytes) or [`into_bytes`](Self::into_bytes)).
    pub fn from_bytes(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Returns the backing data as a byte slice.
    ///
    /// This is the "snapshot to bytes" entry point. The returned slice is a
    /// valid database image that can be written to disk or passed to
    /// [`from_bytes`](Self::from_bytes).
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Consumes the backend and returns the backing data.
    ///
    /// This is equivalent to [`as_bytes`](Self::as_bytes) but avoids a copy
    /// when the backend is no longer needed.
    pub fn into_bytes(self) -> Vec<u8> {
        self.data
    }
}

impl Default for MemoryBackend {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Backend trait implementations
// ---------------------------------------------------------------------------

impl BackendErrorType for MemoryBackend {
    type Error = MemoryError;
}

impl backend::ReadAt for MemoryBackend {
    /// Reads exactly `buf.len()` bytes starting at `offset`.
    ///
    /// Returns `Ok(())` immediately for an empty buffer without any bounds
    /// check. Returns [`MemoryError::OutOfBounds`] if the read extends beyond
    /// the current storage size.
    ///
    /// # Errors
    ///
    /// - [`OutOfBounds`](MemoryError::OutOfBounds) if `offset + buf.len()`
    ///   exceeds the storage size or overflows.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), MemoryError> {
        if buf.is_empty() {
            return Ok(());
        }
        let end = offset.checked_add(buf.len() as u64).ok_or(MemoryError::OutOfBounds {
            offset,
            requested: buf.len(),
            size: self.data.len() as u64,
        })?;
        if end > self.data.len() as u64 {
            return Err(MemoryError::OutOfBounds {
                offset,
                requested: buf.len(),
                size: self.data.len() as u64,
            });
        }
        let start = offset as usize;
        buf.copy_from_slice(&self.data[start..start + buf.len()]);
        Ok(())
    }

    /// Returns the current storage size in bytes.
    ///
    /// # Errors
    ///
    /// This method is infallible for `MemoryBackend`.
    fn len(&self) -> Result<u64, MemoryError> {
        Ok(self.data.len() as u64)
    }
}

impl backend::WriteAt for MemoryBackend {
    /// Writes exactly `buf.len()` bytes at `offset`.
    ///
    /// Unlike `FileBackend` (from `graph_db`), this
    /// implementation **auto-extends** the backing storage if the write
    /// extends beyond the current length. Bytes between the old end and
    /// `offset` are zero-filled.
    ///
    /// Returns `Ok(())` immediately for an empty buffer.
    ///
    /// # Errors
    ///
    /// This method is infallible for `MemoryBackend`.
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<(), MemoryError> {
        if buf.is_empty() {
            return Ok(());
        }
        let start = offset as usize;
        let end = start + buf.len();
        if end > self.data.len() {
            self.data.resize(end, 0);
        }
        self.data[start..end].copy_from_slice(buf);
        Ok(())
    }

    /// Sets the storage size to `new_size` bytes.
    ///
    /// Extends with zero-filled bytes or truncates as needed.
    ///
    /// # Errors
    ///
    /// This method is infallible for `MemoryBackend`.
    fn set_len(&mut self, new_size: u64) -> Result<(), MemoryError> {
        self.data.resize(new_size as usize, 0);
        Ok(())
    }
}

impl backend::Durability for MemoryBackend {
    /// No-op: in-memory writes are immediately visible.
    ///
    /// # Errors
    ///
    /// This method is infallible for `MemoryBackend`.
    fn sync_data(&mut self) -> Result<(), MemoryError> {
        Ok(())
    }

    /// No-op: in-memory writes are immediately visible.
    ///
    /// # Errors
    ///
    /// This method is infallible for `MemoryBackend`.
    fn sync_all(&mut self) -> Result<(), MemoryError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Snapshot helpers (std-only)
// ---------------------------------------------------------------------------

#[cfg(feature = "std")]
impl MemoryBackend {
    /// Saves the current contents to a file.
    ///
    /// Writes the raw byte contents of the in-memory storage to the
    /// specified path. The resulting file is a valid database file and can
    /// be opened with `FileBackend` (from `graph_db`).
    ///
    /// # Errors
    ///
    /// Returns [`std::io::Error`] if the file cannot be written.
    ///
    /// # Note
    ///
    /// This operation is **not** atomic. If the process crashes during the
    /// write, the file may be incomplete or corrupt.
    pub fn save_to_file(&self, path: &std::path::Path) -> std::io::Result<()> {
        std::fs::write(path, &self.data)
    }

    /// Loads contents from a file into a new in-memory backend.
    ///
    /// Reads the entire file into memory. The file should be a valid
    /// database file (e.g., one previously saved with
    /// [`save_to_file`](Self::save_to_file) or created by
    /// `FileBackend` (from `graph_db`)).
    ///
    /// # Errors
    ///
    /// Returns [`std::io::Error`] if the file cannot be read.
    pub fn load_from_file(path: &std::path::Path) -> std::io::Result<Self> {
        let data = std::fs::read(path)?;
        Ok(MemoryBackend { data })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{Durability, ReadAt, WriteAt};

    // -- MemoryError tests --

    #[test]
    fn memory_error_display() {
        let err = MemoryError::OutOfBounds {
            offset: 10,
            requested: 5,
            size: 12,
        };
        let s = format!("{err}");
        assert_eq!(
            s,
            "out of bounds: read at offset 10 with length 5 exceeds memory size 12"
        );
    }

    #[test]
    fn memory_error_kind() {
        use crate::backend::BackendError as _;
        let err = MemoryError::OutOfBounds {
            offset: 0,
            requested: 1,
            size: 0,
        };
        assert_eq!(err.kind(), StorageErrorKind::OutOfBounds);
    }

    #[test]
    fn memory_error_clone_copy_eq() {
        let a = MemoryError::OutOfBounds {
            offset: 1,
            requested: 2,
            size: 3,
        };
        let b = a; // Copy
        assert_eq!(a, b);
        // Verify Clone trait is available (clone returns same value as copy).
        let c = Clone::clone(&a);
        assert_eq!(a, c);
    }

    // -- Constructor tests --

    #[test]
    fn new_creates_empty() {
        let b = MemoryBackend::new();
        assert!(b.as_bytes().is_empty());
    }

    #[test]
    fn with_size_creates_zeroed() {
        let b = MemoryBackend::with_size(4096);
        assert_eq!(b.as_bytes().len(), 4096);
        assert!(b.as_bytes().iter().all(|&x| x == 0));
    }

    #[test]
    fn from_bytes_stores_data() {
        let b = MemoryBackend::from_bytes(vec![1, 2, 3]);
        assert_eq!(b.as_bytes(), &[1, 2, 3]);
    }

    #[test]
    fn into_bytes_returns_data() {
        let b = MemoryBackend::from_bytes(vec![4, 5, 6]);
        assert_eq!(b.into_bytes(), vec![4, 5, 6]);
    }

    #[test]
    fn default_is_new() {
        let a = MemoryBackend::new();
        let b = MemoryBackend::default();
        assert_eq!(a.as_bytes(), b.as_bytes());
    }

    // -- ReadAt tests --

    #[test]
    fn read_at_basic() {
        let b = MemoryBackend::from_bytes(vec![1, 2, 3, 4, 5]);
        let mut buf = [0u8; 3];
        b.read_at(0, &mut buf).unwrap();
        assert_eq!(buf, [1, 2, 3]);

        b.read_at(2, &mut buf).unwrap();
        assert_eq!(buf, [3, 4, 5]);
    }

    #[test]
    fn read_at_all_bytes() {
        let b = MemoryBackend::from_bytes(vec![1, 2, 3, 4, 5]);
        let mut buf = [0u8; 5];
        b.read_at(0, &mut buf).unwrap();
        assert_eq!(buf, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn read_at_empty_buffer() {
        let b = MemoryBackend::new();
        // Empty buffer read succeeds regardless of offset.
        b.read_at(0, &mut []).unwrap();
        b.read_at(9999, &mut []).unwrap();
    }

    #[test]
    fn read_at_out_of_bounds_offset() {
        let b = MemoryBackend::from_bytes(vec![1, 2, 3]);
        let mut buf = [0u8; 1];
        let err = b.read_at(10, &mut buf).unwrap_err();
        assert_eq!(
            err,
            MemoryError::OutOfBounds {
                offset: 10,
                requested: 1,
                size: 3,
            }
        );
    }

    #[test]
    fn read_at_partial_out_of_bounds() {
        let b = MemoryBackend::from_bytes(vec![1, 2, 3]);
        let mut buf = [0u8; 2];
        let err = b.read_at(2, &mut buf).unwrap_err();
        assert_eq!(
            err,
            MemoryError::OutOfBounds {
                offset: 2,
                requested: 2,
                size: 3,
            }
        );
    }

    #[test]
    fn read_at_empty_backend() {
        let b = MemoryBackend::new();
        let mut buf = [0u8; 1];
        assert!(b.read_at(0, &mut buf).is_err());
    }

    #[test]
    fn read_at_overflow() {
        use crate::backend::BackendError as _;
        let b = MemoryBackend::from_bytes(vec![1, 2, 3]);
        let mut buf = [0u8; 1];
        // u64::MAX + 1 would overflow
        let err = b.read_at(u64::MAX, &mut buf).unwrap_err();
        assert_eq!(err.kind(), StorageErrorKind::OutOfBounds);
    }

    #[test]
    fn len_returns_correct_size() {
        let b = MemoryBackend::from_bytes(vec![1, 2, 3]);
        assert_eq!(b.len().unwrap(), 3);
    }

    #[test]
    fn len_empty() {
        let b = MemoryBackend::new();
        assert_eq!(b.len().unwrap(), 0);
    }

    // -- WriteAt tests --

    #[test]
    fn write_at_auto_extend_from_empty() {
        let mut b = MemoryBackend::new();
        b.write_at(0, &[1, 2, 3]).unwrap();
        assert_eq!(b.as_bytes(), &[1, 2, 3]);
    }

    #[test]
    fn write_at_auto_extend_with_gap() {
        let mut b = MemoryBackend::new();
        b.write_at(10, &[0xAA, 0xBB]).unwrap();
        assert_eq!(b.as_bytes().len(), 12);
        assert!(b.as_bytes()[..10].iter().all(|&x| x == 0));
        assert_eq!(b.as_bytes()[10], 0xAA);
        assert_eq!(b.as_bytes()[11], 0xBB);
    }

    #[test]
    fn write_at_overwrite() {
        let mut b = MemoryBackend::from_bytes(vec![1, 2, 3, 4, 5]);
        b.write_at(1, &[0xAA, 0xBB]).unwrap();
        assert_eq!(b.as_bytes(), &[1, 0xAA, 0xBB, 4, 5]);
    }

    #[test]
    fn write_at_extend_beyond() {
        let mut b = MemoryBackend::from_bytes(vec![1, 2, 3]);
        b.write_at(2, &[0xAA, 0xBB, 0xCC]).unwrap();
        assert_eq!(b.as_bytes(), &[1, 2, 0xAA, 0xBB, 0xCC]);
    }

    #[test]
    fn set_len_grow() {
        let mut b = MemoryBackend::new();
        b.set_len(10).unwrap();
        assert_eq!(b.len().unwrap(), 10);
        assert!(b.as_bytes().iter().all(|&x| x == 0));
    }

    #[test]
    fn set_len_truncate() {
        let mut b = MemoryBackend::from_bytes(vec![1, 2, 3, 4, 5]);
        b.set_len(3).unwrap();
        assert_eq!(b.as_bytes(), &[1, 2, 3]);
    }

    #[test]
    fn set_len_same_size() {
        let mut b = MemoryBackend::from_bytes(vec![1, 2, 3]);
        b.set_len(3).unwrap();
        assert_eq!(b.as_bytes(), &[1, 2, 3]);
    }

    #[test]
    fn write_at_empty_buffer() {
        let mut b = MemoryBackend::from_bytes(vec![1, 2, 3]);
        b.write_at(0, &[]).unwrap();
        assert_eq!(b.as_bytes(), &[1, 2, 3]);
    }

    // -- Durability tests --

    #[test]
    fn sync_data_noop() {
        let mut b = MemoryBackend::new();
        b.sync_data().unwrap();
    }

    #[test]
    fn sync_all_noop() {
        let mut b = MemoryBackend::new();
        b.sync_all().unwrap();
    }

    #[test]
    fn sync_does_not_alter_data() {
        let mut b = MemoryBackend::from_bytes(vec![1, 2, 3]);
        b.sync_data().unwrap();
        b.sync_all().unwrap();
        assert_eq!(b.as_bytes(), &[1, 2, 3]);
    }

    // -- StorageBackend compile-time assertion --

    fn _assert_storage_backend<T: crate::backend::StorageBackend>() {}
    fn _check() {
        _assert_storage_backend::<MemoryBackend>();
    }

    // -- Snapshot helper tests --

    #[test]
    fn snapshot_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.bin");

        let original = MemoryBackend::from_bytes(vec![0xDE, 0xAD, 0xBE, 0xEF]);
        original.save_to_file(&path).unwrap();

        let loaded = MemoryBackend::load_from_file(&path).unwrap();
        assert_eq!(loaded.as_bytes(), original.as_bytes());
    }

    #[test]
    fn snapshot_file_contains_exact_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("snapshot.bin");

        let data = vec![0x01, 0x02, 0x03, 0x04];
        let b = MemoryBackend::from_bytes(data.clone());
        b.save_to_file(&path).unwrap();

        let on_disk = std::fs::read(&path).unwrap();
        assert_eq!(on_disk, data);
    }

    #[test]
    fn load_from_nonexistent_path() {
        let result = MemoryBackend::load_from_file(std::path::Path::new("/nonexistent/path.bin"));
        assert!(result.is_err());
    }
}
