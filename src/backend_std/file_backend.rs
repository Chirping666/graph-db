//! [`FileBackend`] implementation — the primary persistent storage backend.

use std::fs::{File, OpenOptions};
use std::path::PathBuf;

use crate::backend;
use crate::backend::error::{StorageError, StorageErrorKind, StorageErrorType};
use crate::backend::lifecycle::{LockableBackend, OpenableBackend};
use crate::backend::traits::{ReadAt, WriteAt};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Configuration for opening or creating a [`FileBackend`].
#[derive(Debug, Clone)]
pub struct FileBackendConfig {
    /// Path to the database file.
    pub path: PathBuf,
    /// If `true`, the file is opened in read-only mode. Write operations
    /// will return [`FileError::ReadOnly`].
    pub read_only: bool,
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Error type for the file-backed storage backend.
#[derive(Debug)]
pub enum FileError {
    /// An I/O error from the operating system.
    Io(std::io::Error),
    /// The database file is locked by another process.
    LockContention,
    /// A read or write attempted to access beyond the file size.
    OutOfBounds {
        /// The byte offset of the attempted operation.
        offset: u64,
        /// The number of bytes the operation tried to read or write.
        len: usize,
        /// The file size at the time of the error (best-effort).
        file_size: u64,
    },
    /// The backend is in read-only mode.
    ReadOnly,
}

impl core::fmt::Display for FileError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FileError::Io(e) => write!(f, "I/O error: {e}"),
            FileError::LockContention => {
                write!(f, "database file is locked by another process")
            }
            FileError::OutOfBounds {
                offset,
                len,
                file_size,
            } => {
                write!(
                    f,
                    "out of bounds: offset {offset} with length {len} exceeds file size {file_size}"
                )
            }
            FileError::ReadOnly => write!(f, "database is opened in read-only mode"),
        }
    }
}

impl std::error::Error for FileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FileError::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl StorageError for FileError {
    fn kind(&self) -> StorageErrorKind {
        match self {
            FileError::Io(e) => match e.kind() {
                std::io::ErrorKind::Interrupted => StorageErrorKind::Interrupted,
                std::io::ErrorKind::PermissionDenied => StorageErrorKind::ReadOnly,
                _ => StorageErrorKind::Io,
            },
            FileError::LockContention => StorageErrorKind::LockContention,
            FileError::OutOfBounds { .. } => StorageErrorKind::OutOfBounds,
            FileError::ReadOnly => StorageErrorKind::ReadOnly,
        }
    }
}

impl From<std::io::Error> for FileError {
    fn from(e: std::io::Error) -> Self {
        FileError::Io(e)
    }
}

// ---------------------------------------------------------------------------
// FileBackend
// ---------------------------------------------------------------------------

/// File-backed storage backend — the primary persistent backend.
///
/// Uses `pread()`/`pwrite()` on Unix and `ReadFile()`/`WriteFile()` with
/// explicit offsets on Windows for thread-safe random I/O without shared
/// seek position state.
///
/// # Thread safety
///
/// [`ReadAt::read_at`] takes `&self`, enabling concurrent reads via the
/// underlying `pread` call. [`WriteAt`] and [`Durability`](backend::Durability)
/// take `&mut self`, ensuring exclusive write access at the Rust type level.
/// The storage engine manages concurrency via `RwLock`.
pub struct FileBackend {
    file: File,
    read_only: bool,
}

impl StorageErrorType for FileBackend {
    type Error = FileError;
}

impl ReadAt for FileBackend {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), FileError> {
        if buf.is_empty() {
            return Ok(());
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt;
            self.file.read_exact_at(buf, offset).map_err(|e| {
                if e.kind() == std::io::ErrorKind::UnexpectedEof {
                    FileError::OutOfBounds {
                        offset,
                        len: buf.len(),
                        file_size: self.file.metadata().map(|m| m.len()).unwrap_or(0),
                    }
                } else {
                    FileError::Io(e)
                }
            })
        }

        #[cfg(windows)]
        {
            use std::os::windows::fs::FileExt;
            let mut total_read = 0;
            while total_read < buf.len() {
                let n = self
                    .file
                    .seek_read(&mut buf[total_read..], offset + total_read as u64)
                    .map_err(FileError::Io)?;
                if n == 0 {
                    return Err(FileError::OutOfBounds {
                        offset,
                        len: buf.len(),
                        file_size: self.file.metadata().map(|m| m.len()).unwrap_or(0),
                    });
                }
                total_read += n;
            }
            Ok(())
        }

        #[cfg(not(any(unix, windows)))]
        {
            compile_error!("FileBackend requires Unix (pread) or Windows (seek_read) support");
        }
    }

    fn len(&self) -> Result<u64, FileError> {
        Ok(self.file.metadata()?.len())
    }
}

impl WriteAt for FileBackend {
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<(), FileError> {
        if self.read_only {
            return Err(FileError::ReadOnly);
        }
        if buf.is_empty() {
            return Ok(());
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::FileExt;
            self.file.write_all_at(buf, offset)?;
            Ok(())
        }

        #[cfg(windows)]
        {
            use std::os::windows::fs::FileExt;
            let mut total_written = 0;
            while total_written < buf.len() {
                let n = self
                    .file
                    .seek_write(&buf[total_written..], offset + total_written as u64)
                    .map_err(FileError::Io)?;
                if n == 0 {
                    return Err(FileError::Io(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "seek_write returned 0",
                    )));
                }
                total_written += n;
            }
            Ok(())
        }

        #[cfg(not(any(unix, windows)))]
        {
            compile_error!("FileBackend requires Unix (pwrite) or Windows (seek_write) support");
        }
    }

    fn set_len(&mut self, new_size: u64) -> Result<(), FileError> {
        if self.read_only {
            return Err(FileError::ReadOnly);
        }
        self.file.set_len(new_size)?;
        Ok(())
    }
}

impl backend::Durability for FileBackend {
    fn sync_data(&mut self) -> Result<(), FileError> {
        if self.read_only {
            return Ok(());
        }

        #[cfg(target_os = "macos")]
        {
            use std::os::unix::io::AsRawFd;
            // SAFETY: The file descriptor is valid because `self.file` is an
            // open `File`. `F_FULLFSYNC` is the only way to guarantee data
            // reaches the physical medium on macOS — `fsync`/`fdatasync` only
            // flush to the drive's write cache.
            let ret = unsafe { libc::fcntl(self.file.as_raw_fd(), libc::F_FULLFSYNC) };
            if ret == -1 {
                return Err(FileError::Io(std::io::Error::last_os_error()));
            }
            Ok(())
        }

        #[cfg(all(unix, not(target_os = "macos")))]
        {
            self.file.sync_data()?;
            Ok(())
        }

        #[cfg(windows)]
        {
            self.file.sync_all()?;
            Ok(())
        }

        #[cfg(not(any(unix, windows)))]
        {
            compile_error!("FileBackend requires Unix or Windows for fsync");
        }
    }

    fn sync_all(&mut self) -> Result<(), FileError> {
        if self.read_only {
            return Ok(());
        }

        #[cfg(target_os = "macos")]
        {
            use std::os::unix::io::AsRawFd;
            // SAFETY: Same as sync_data — F_FULLFSYNC covers both data and
            // metadata on macOS.
            let ret = unsafe { libc::fcntl(self.file.as_raw_fd(), libc::F_FULLFSYNC) };
            if ret == -1 {
                return Err(FileError::Io(std::io::Error::last_os_error()));
            }
            Ok(())
        }

        #[cfg(all(unix, not(target_os = "macos")))]
        {
            self.file.sync_all()?;
            Ok(())
        }

        #[cfg(windows)]
        {
            self.file.sync_all()?;
            Ok(())
        }

        #[cfg(not(any(unix, windows)))]
        {
            compile_error!("FileBackend requires Unix or Windows for fsync");
        }
    }
}

// ---------------------------------------------------------------------------
// OpenableBackend
// ---------------------------------------------------------------------------

impl OpenableBackend for FileBackend {
    type Config = FileBackendConfig;

    fn open(config: FileBackendConfig) -> Result<Self, FileError> {
        let file = OpenOptions::new()
            .read(true)
            .write(!config.read_only)
            .open(&config.path)?;
        Ok(FileBackend {
            file,
            read_only: config.read_only,
        })
    }

    fn create(config: FileBackendConfig) -> Result<Self, FileError> {
        if config.read_only {
            return Err(FileError::ReadOnly);
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&config.path)?;
        Ok(FileBackend {
            file,
            read_only: false,
        })
    }

    /// Opens an existing file or creates it atomically.
    ///
    /// Overrides the default TOCTOU-racy implementation with an atomic
    /// `open` + `create(true)` call.
    fn open_or_create(config: FileBackendConfig) -> Result<Self, FileError>
    where
        Self::Config: Clone,
    {
        if config.read_only {
            return Self::open(config);
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&config.path)?;
        Ok(FileBackend {
            file,
            read_only: false,
        })
    }
}

// ---------------------------------------------------------------------------
// File locking
// ---------------------------------------------------------------------------

/// RAII guard for an exclusive file lock. Releasing the guard (via `Drop`)
/// releases the lock.
pub struct FileLockGuard {
    #[cfg(unix)]
    fd: std::os::unix::io::RawFd,
    #[cfg(windows)]
    handle: std::os::windows::io::RawHandle,
}

impl core::fmt::Debug for FileLockGuard {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FileLockGuard").finish_non_exhaustive()
    }
}

// SAFETY: The file descriptor (Unix) or handle (Windows) is valid for the
// lifetime of the guard. `flock`/`LockFileEx` are safe to use from any thread.
unsafe impl Send for FileLockGuard {}

impl Drop for FileLockGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            // Best-effort unlock — ignore errors on drop.
            // SAFETY: `self.fd` is a valid file descriptor obtained from an
            // open `File`. `LOCK_UN` releases the lock.
            unsafe {
                libc::flock(self.fd, libc::LOCK_UN);
            }
        }

        #[cfg(windows)]
        {
            use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;
            // SAFETY: `OVERLAPPED` is valid when zero-initialized.
            let mut overlapped = unsafe { core::mem::zeroed() };
            // SAFETY: `self.handle` is a valid file handle. Best-effort unlock.
            unsafe {
                UnlockFileEx(self.handle, 0, u32::MAX, u32::MAX, &mut overlapped);
            }
        }
    }
}

impl LockableBackend for FileBackend {
    type LockGuard = FileLockGuard;

    fn try_lock_exclusive(&self) -> Result<FileLockGuard, FileError> {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = self.file.as_raw_fd();
            // SAFETY: `fd` is a valid file descriptor from `self.file`.
            // `LOCK_EX | LOCK_NB` requests a non-blocking exclusive lock.
            let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
            if ret == -1 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                    return Err(FileError::LockContention);
                }
                return Err(FileError::Io(err));
            }
            Ok(FileLockGuard { fd })
        }

        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Storage::FileSystem::{
                LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
            };

            let handle = self.file.as_raw_handle();
            // SAFETY: `OVERLAPPED` is a plain-old-data struct that is valid
            // when zero-initialized (all fields are integers or pointers).
            let mut overlapped = unsafe { core::mem::zeroed() };
            // SAFETY: `handle` is a valid file handle from `self.file`.
            let result = unsafe {
                LockFileEx(
                    handle as _,
                    LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                    0,
                    u32::MAX,
                    u32::MAX,
                    &mut overlapped,
                )
            };
            if result == 0 {
                return Err(FileError::LockContention);
            }
            Ok(FileLockGuard { handle })
        }

        #[cfg(not(any(unix, windows)))]
        {
            compile_error!("FileBackend requires Unix or Windows for file locking");
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Durability;

    // === FileError tests ===

    #[test]
    fn file_error_display() {
        let io_err = FileError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "not found",
        ));
        assert!(format!("{io_err}").contains("not found"));

        let lock_err = FileError::LockContention;
        assert!(format!("{lock_err}").contains("locked"));

        let oob = FileError::OutOfBounds {
            offset: 100,
            len: 50,
            file_size: 120,
        };
        let s = format!("{oob}");
        assert!(s.contains("100"));
        assert!(s.contains("50"));
        assert!(s.contains("120"));

        let ro = FileError::ReadOnly;
        assert!(format!("{ro}").contains("read-only"));
    }

    #[test]
    fn file_error_kind_mapping() {
        assert_eq!(
            FileError::Io(std::io::Error::other("x")).kind(),
            StorageErrorKind::Io
        );
        assert_eq!(
            FileError::Io(std::io::Error::new(std::io::ErrorKind::Interrupted, "x")).kind(),
            StorageErrorKind::Interrupted
        );
        assert_eq!(
            FileError::Io(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "x")).kind(),
            StorageErrorKind::ReadOnly
        );
        assert_eq!(FileError::LockContention.kind(), StorageErrorKind::LockContention);
        assert_eq!(
            FileError::OutOfBounds { offset: 0, len: 0, file_size: 0 }.kind(),
            StorageErrorKind::OutOfBounds
        );
        assert_eq!(FileError::ReadOnly.kind(), StorageErrorKind::ReadOnly);
    }

    #[test]
    fn file_error_from_io() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        let fe: FileError = io.into();
        assert!(matches!(fe, FileError::Io(_)));
    }

    #[test]
    fn file_error_std_source() {
        use std::error::Error;
        let io_err = FileError::Io(std::io::Error::other("x"));
        assert!(io_err.source().is_some());
        assert!(FileError::LockContention.source().is_none());
        assert!(FileError::ReadOnly.source().is_none());
    }

    // === FileBackend round-trip ===

    fn temp_config(dir: &tempfile::TempDir) -> FileBackendConfig {
        FileBackendConfig {
            path: dir.path().join("test.db"),
            read_only: false,
        }
    }

    #[test]
    fn read_write_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let mut fb = FileBackend::create(temp_config(&dir)).unwrap();
        fb.set_len(4096).unwrap();
        fb.write_at(0, b"hello").unwrap();
        fb.write_at(2048, b"world").unwrap();

        let mut buf1 = [0u8; 5];
        fb.read_at(0, &mut buf1).unwrap();
        assert_eq!(&buf1, b"hello");

        let mut buf2 = [0u8; 5];
        fb.read_at(2048, &mut buf2).unwrap();
        assert_eq!(&buf2, b"world");

        assert_eq!(fb.len().unwrap(), 4096);
    }

    #[test]
    fn out_of_bounds_read() {
        let dir = tempfile::tempdir().unwrap();
        let mut fb = FileBackend::create(temp_config(&dir)).unwrap();
        fb.set_len(100).unwrap();

        let mut buf = [0u8; 50];
        let err = fb.read_at(80, &mut buf).unwrap_err();
        assert_eq!(err.kind(), StorageErrorKind::OutOfBounds);
    }

    #[test]
    fn empty_buffer_edge_cases() {
        let dir = tempfile::tempdir().unwrap();
        let mut fb = FileBackend::create(temp_config(&dir)).unwrap();
        fb.read_at(0, &mut []).unwrap();
        fb.write_at(0, &[]).unwrap();
    }

    #[test]
    fn read_only_mode() {
        let dir = tempfile::tempdir().unwrap();
        let config = temp_config(&dir);

        // Create file with some data
        {
            let mut fb = FileBackend::create(config.clone()).unwrap();
            fb.set_len(100).unwrap();
            fb.write_at(0, b"data").unwrap();
            Durability::sync_data(&mut fb).unwrap();
        }

        // Open read-only
        let ro_config = FileBackendConfig {
            path: config.path.clone(),
            read_only: true,
        };
        let mut fb = FileBackend::open(ro_config).unwrap();

        // Read works
        let mut buf = [0u8; 4];
        fb.read_at(0, &mut buf).unwrap();
        assert_eq!(&buf, b"data");

        // Write rejected
        let err = fb.write_at(0, b"x").unwrap_err();
        assert_eq!(err.kind(), StorageErrorKind::ReadOnly);

        // set_len rejected
        let err = fb.set_len(200).unwrap_err();
        assert_eq!(err.kind(), StorageErrorKind::ReadOnly);

        // Sync is no-op in read-only mode
        Durability::sync_data(&mut fb).unwrap();
        Durability::sync_all(&mut fb).unwrap();
    }

    #[test]
    fn sync_operations() {
        let dir = tempfile::tempdir().unwrap();
        let mut fb = FileBackend::create(temp_config(&dir)).unwrap();
        fb.set_len(100).unwrap();
        fb.write_at(0, b"test").unwrap();
        Durability::sync_data(&mut fb).unwrap();
        fb.write_at(50, b"more").unwrap();
        Durability::sync_all(&mut fb).unwrap();
    }

    // === Open/create/open_or_create lifecycle ===

    #[test]
    fn create_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let _fb = FileBackend::create(temp_config(&dir)).unwrap();
    }

    #[test]
    fn create_existing_file_fails() {
        let dir = tempfile::tempdir().unwrap();
        let config = temp_config(&dir);
        let _fb = FileBackend::create(config.clone()).unwrap();
        let err = FileBackend::create(config);
        assert!(err.is_err());
    }

    #[test]
    fn open_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let config = temp_config(&dir);
        drop(FileBackend::create(config.clone()).unwrap());
        let _fb = FileBackend::open(config).unwrap();
    }

    #[test]
    fn open_nonexistent_file_fails() {
        let dir = tempfile::tempdir().unwrap();
        let err = FileBackend::open(temp_config(&dir));
        assert!(err.is_err());
    }

    #[test]
    fn open_or_create_new() {
        let dir = tempfile::tempdir().unwrap();
        let _fb = FileBackend::open_or_create(temp_config(&dir)).unwrap();
    }

    #[test]
    fn open_or_create_existing() {
        let dir = tempfile::tempdir().unwrap();
        let config = temp_config(&dir);
        drop(FileBackend::create(config.clone()).unwrap());
        let _fb = FileBackend::open_or_create(config).unwrap();
    }

    // === File locking ===

    #[test]
    fn lock_and_release() {
        let dir = tempfile::tempdir().unwrap();
        let fb = FileBackend::create(temp_config(&dir)).unwrap();
        let guard = fb.try_lock_exclusive().unwrap();
        // Lock is held — we can still read/write through the same backend
        drop(guard);
        // Lock released — can re-acquire
        let _guard2 = fb.try_lock_exclusive().unwrap();
    }

    #[test]
    fn lock_contention_same_process() {
        let dir = tempfile::tempdir().unwrap();
        let config = temp_config(&dir);
        let fb1 = FileBackend::create(config.clone()).unwrap();
        let _guard = fb1.try_lock_exclusive().unwrap();

        // Open a second fd to the same file
        let fb2 = FileBackend::open(config).unwrap();
        let err = fb2.try_lock_exclusive().unwrap_err();
        assert_eq!(err.kind(), StorageErrorKind::LockContention);
    }

    // === Persistence across open/close ===

    #[test]
    fn persistence_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let config = temp_config(&dir);

        // Write and sync
        {
            let mut fb = FileBackend::create(config.clone()).unwrap();
            fb.set_len(100).unwrap();
            fb.write_at(10, b"persistent").unwrap();
            Durability::sync_data(&mut fb).unwrap();
        }

        // Reopen and verify
        {
            let fb = FileBackend::open(config).unwrap();
            let mut buf = [0u8; 10];
            fb.read_at(10, &mut buf).unwrap();
            assert_eq!(&buf, b"persistent");
        }
    }
}
