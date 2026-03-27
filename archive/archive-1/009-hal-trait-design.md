# 009 — HAL Trait Layer Design

**Project:** Embedded Graph Database with Extensible Schema & Pluggable Inference  
**Task:** 9 — Design: HAL Trait Layer  
**Status:** Complete  
**Depends on:** Task 5 (`005-no-std-hal-patterns.md`), Task 8 (`008-file-format-spec.md`)  
**Intended audience:** All downstream design and implementation tasks (especially Tasks 12, 15, 19). A Rust developer familiar with the file format spec (Task 8) should be able to implement all traits and backends described here.

---

## Table of Contents

1. [Purpose and Scope](#1-purpose-and-scope)
2. [Design Goals and Constraints](#2-design-goals-and-constraints)
3. [Crate Structure and Feature Flags](#3-crate-structure-and-feature-flags)
4. [Error Types](#4-error-types)
5. [Core Traits — `ReadAt`, `WriteAt`, `Sync`](#5-core-traits--readat-writeat-sync)
6. [Combined Trait — `StorageBackend`](#6-combined-trait--storagebackend)
7. [Lifecycle Traits — `OpenableBackend`](#7-lifecycle-traits--openablebackend)
8. [File Locking — `LockableBackend`](#8-file-locking--lockablebackend)
9. [Default `std` Persistent Backend](#9-default-std-persistent-backend)
10. [In-Memory Backend](#10-in-memory-backend)
11. [Hypothetical `no_std` NOR Flash Backend Walkthrough](#11-hypothetical-no_std-nor-flash-backend-walkthrough)
12. [Error Propagation Chain](#12-error-propagation-chain)
13. [fsync Discipline and Platform Mapping](#13-fsync-discipline-and-platform-mapping)
14. [Durability Warnings](#14-durability-warnings)
15. [Design Decision Log](#15-design-decision-log)

---

## 1. Purpose and Scope

This document is the authoritative specification for the **Hardware Abstraction Layer (HAL)** of the embedded graph database. The HAL defines the Rust trait hierarchy that abstracts all storage I/O, enabling the database engine to operate identically across different storage backends: a persistent file on disk, a `Vec<u8>` in RAM, or a bare-metal NOR flash chip.

### What this document defines

- The complete Rust trait hierarchy for storage I/O (`ReadAt`, `WriteAt`, `Sync`, `StorageBackend`)
- The error type system (`StorageErrorKind`, the `StorageError` trait)
- The lifecycle trait for backends that require open/create semantics (`OpenableBackend`)
- The file locking trait for single-process exclusivity (`LockableBackend`)
- The full `std` persistent file backend design (ready for implementation)
- The full in-memory backend design (ready for implementation)
- A walkthrough of a hypothetical `no_std` NOR flash backend
- The fsync discipline (mapping abstract sync methods to platform-specific calls)
- The error propagation chain from HAL through the storage engine to the public API

### What this document does NOT define

- The buffer pool implementation — uses these traits but is defined in `007-graph-storage-model.md` Section 10
- The commit protocol — uses these traits but is defined in `008-file-format-spec.md` Section 13
- The public API for creating/opening a database — Task 10
- The actual B-tree, page format, or record layout — Tasks 7 and 8

### Relationship to upstream documents

- **Task 5** (`005-no-std-hal-patterns.md`): Provides the preliminary trait sketch (Section 6), design principles (Section 7), and the `no_std`/`alloc` architecture that this document refines into a final design.
- **Task 8** (`008-file-format-spec.md`): Provides the commit protocol (Section 13) and fsync discipline (Section 15) that determine the exact I/O operations the HAL must expose.

### Key refinements from the Task 5 preliminary sketch

The Task 5 sketch proposed a single `Flush` trait with one `flush()` method. Task 8 (Section 15, upstream flag #2) established that two distinct sync operations are required: `sync_data()` and `sync_all()`. This document replaces `Flush` with a `Sync` trait that provides both methods. Other refinements include adding file locking, refining error handling, and tightening method contracts to match the page-aligned I/O model specified by Task 8.

---

## 2. Design Goals and Constraints

These goals are inherited from Task 5 (Section 6.1 and Section 7), refined by the file format requirements from Task 8.

| # | Goal | Source |
|---|------|--------|
| G1 | All core traits are `no_std + alloc` compatible — no `std` dependency in trait definitions | Task 5 §7 |
| G2 | All core traits are object-safe — `dyn StorageBackend` must work for runtime backend selection | Task 5 §5.3, §7 |
| G3 | Synchronous (blocking) I/O only — no async in v1 | Task 5 §7 |
| G4 | Minimal trait surface — implementable on constrained hardware | Task 5 §2.2 (embedded-hal principle) |
| G5 | Single associated `Error` type per trait family — no type proliferation | Task 5 §5.3 |
| G6 | `ReadAt` takes `&self` — enables concurrent reads without `&mut` | Task 5 §7 |
| G7 | Explicit, two-level durability control — `sync_data()` and `sync_all()` | Task 8 §15.2 (upstream flag #2) |
| G8 | File locking for single-process exclusivity | Task 8 §14.3, Task 4 §8.9 |
| G9 | `Infallible` is a valid error type — simplifies testing and in-memory use | Task 5 §2.2 |
| G10 | Feature flags: `std` (default) and `alloc` (implied by `std`) — no proliferation | Task 5 §5.4, §7 |

---

## 3. Crate Structure and Feature Flags

Following Task 5's recommendation (Section 1.4, Approach A), the HAL is part of the main database crate, not a separate crate. Feature flags control which modules are compiled.

```toml
# Cargo.toml (relevant excerpt)
[features]
default = ["std"]
std = ["alloc"]
alloc = []
```

```rust
// src/lib.rs
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

// HAL trait definitions — always available (no_std + alloc)
pub mod hal;

// std persistent backend — only with std feature
#[cfg(feature = "std")]
pub mod hal_std;

// In-memory backend — available with alloc (no std required)
#[cfg(feature = "alloc")]
pub mod hal_mem;
```

**Module layout:**

```
src/
├── hal/
│   ├── mod.rs          // Re-exports
│   ├── error.rs        // StorageErrorKind, StorageError trait
│   ├── traits.rs       // ReadAt, WriteAt, Sync, StorageBackend
│   └── lifecycle.rs    // OpenableBackend, LockableBackend (trait defs only)
├── hal_std/
│   ├── mod.rs          // Re-exports
│   └── file_backend.rs // FileBackend implementation
├── hal_mem/
│   ├── mod.rs          // Re-exports
│   └── memory_backend.rs // MemoryBackend implementation
```

**Rationale:** Trait definitions live in `hal/` (always compiled, no `std` dependency). Implementations live in feature-gated modules. This follows the `embedded-hal` pattern: the interface is universal, the implementations are platform-specific.

---

## 4. Error Types

### 4.1 `StorageErrorKind` enum

A `no_std`-compatible error category enum. Backends map their concrete errors to one of these kinds, enabling generic error handling without knowing the concrete type.

```rust
// src/hal/error.rs

/// Categorizes storage errors for generic error handling.
///
/// This enum allows code that is generic over `StorageBackend` to make
/// decisions based on error category without knowing the concrete error
/// type. It follows the `embedded-hal` `ErrorKind` pattern (Task 5 §2.2).
///
/// # Extensibility
///
/// The `#[non_exhaustive]` attribute ensures that adding new variants
/// in future versions does not break downstream match arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StorageErrorKind {
    /// A read or write referenced an offset or range beyond the
    /// current extent of the storage medium.
    OutOfBounds,

    /// An underlying I/O operation failed. This covers OS-level errors
    /// (permission denied, disk full, hardware fault) and any backend-
    /// specific transient failures.
    Io,

    /// The storage medium is in read-only mode and a write was attempted.
    ReadOnly,

    /// The storage medium cannot grow to accommodate a requested
    /// `set_len()` or `write_at()` operation.
    StorageFull,

    /// A checksum mismatch or structural inconsistency was detected
    /// in the storage medium itself. Distinct from application-level
    /// data corruption — this means the medium's own integrity check
    /// failed.
    MediaCorruption,

    /// An I/O operation was interrupted (e.g., `EINTR` on Unix).
    /// The caller may retry.
    Interrupted,

    /// The file is locked by another process and exclusive access
    /// cannot be obtained.
    LockContention,

    /// An error not covered by any of the above categories.
    Other,
}
```

### 4.2 `StorageError` trait

The `no_std`-compatible error trait. This replaces `std::error::Error` in the HAL layer.

```rust
// src/hal/error.rs (continued)

/// Trait for HAL storage errors.
///
/// Every concrete error type produced by a backend must implement this
/// trait. The `kind()` method enables generic error handling; the
/// `Debug` bound enables logging and diagnostic output.
///
/// # Object safety
///
/// This trait is object-safe. `dyn StorageError` is valid.
///
/// # Relationship to `std::error::Error`
///
/// This trait does NOT extend `std::error::Error` because that type
/// is not available in `no_std`. Backend implementations that have
/// `std` available should additionally implement `std::error::Error`
/// on their concrete error types for interoperability, but this is
/// not required by the HAL.
pub trait StorageError: core::fmt::Debug + core::fmt::Display {
    /// Returns the category of this error.
    fn kind(&self) -> StorageErrorKind;
}
```

**Design decision D1 — `Display` bound on `StorageError`:** The Task 5 sketch required only `Debug`. We add `Display` because error messages need to be user-facing in the public API layer, and `core::fmt::Display` is available in `no_std`. This avoids a lossy conversion from `Debug` output to user-readable messages higher in the stack.

### 4.3 `StorageErrorType` trait

Groups the associated error type for a storage implementation, following the `embedded-hal` `ErrorType` pattern.

```rust
// src/hal/error.rs (continued)

/// Associates a concrete error type with a storage implementation.
///
/// This trait exists to be a supertrait of `ReadAt`, `WriteAt`, and
/// `Sync`, avoiding the need to repeat the `Error` associated type
/// in each. All three traits share a single error type per backend.
///
/// # Object safety
///
/// Object-safe. The associated type is constrained to `StorageError`,
/// which is itself object-safe.
pub trait StorageErrorType {
    /// The error type produced by this backend's I/O operations.
    type Error: StorageError;
}
```

---

## 5. Core Traits — `ReadAt`, `WriteAt`, `Sync`

These are the three primitive I/O traits. Together they compose into `StorageBackend`.

### 5.1 `ReadAt`

```rust
// src/hal/traits.rs

/// Random-access read from a storage medium.
///
/// # Concurrency
///
/// `ReadAt` takes `&self` (not `&mut self`). This is a deliberate
/// design choice (Task 5 §7) that enables multiple concurrent readers
/// without requiring `&mut` access. On file-backed backends, this
/// maps to `pread()` (Unix) or `ReadFile()` with an explicit offset
/// (Windows), both of which are thread-safe.
///
/// The storage engine wraps the backend in `Arc<RwLock<B>>` and uses
/// the read lock for read operations. Because `ReadAt` takes `&self`,
/// multiple read transactions can execute concurrently.
///
/// # Object safety
///
/// This trait is object-safe.
pub trait ReadAt: StorageErrorType {
    /// Read exactly `buf.len()` bytes starting at byte offset `offset`.
    ///
    /// # Errors
    ///
    /// - `StorageErrorKind::OutOfBounds` if `offset + buf.len()` exceeds
    ///   the current storage size.
    /// - `StorageErrorKind::Io` on underlying I/O failure.
    /// - `StorageErrorKind::MediaCorruption` if the medium detects an
    ///   integrity error during the read.
    ///
    /// # Contract
    ///
    /// On success, exactly `buf.len()` bytes have been read into `buf`.
    /// Partial reads are not exposed — the implementation must retry
    /// or return an error.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Self::Error>;

    /// Returns the current size of the storage in bytes.
    ///
    /// For a file-backed backend, this is the file's length. For an
    /// in-memory backend, this is the length of the backing `Vec<u8>`.
    ///
    /// # Errors
    ///
    /// - `StorageErrorKind::Io` if the size cannot be determined.
    fn len(&self) -> Result<u64, Self::Error>;
}
```

### 5.2 `WriteAt`

```rust
// src/hal/traits.rs (continued)

/// Random-access write to a storage medium.
///
/// # Concurrency
///
/// `WriteAt` takes `&mut self`. This enforces the single-writer
/// invariant at the Rust type level — only one mutable reference
/// exists, so only one writer can execute at a time. The storage
/// engine holds `&mut` via the `RwLock` write guard during commit.
///
/// # Durability
///
/// Writes through this trait are NOT guaranteed to be durable until
/// `Sync::sync_data()` or `Sync::sync_all()` is called. The
/// implementation may buffer writes in userspace or OS buffers.
///
/// # Object safety
///
/// This trait is object-safe.
pub trait WriteAt: StorageErrorType {
    /// Write exactly `buf.len()` bytes at byte offset `offset`.
    ///
    /// If `offset + buf.len()` exceeds the current storage size, the
    /// behavior is backend-defined:
    /// - File backends: may fail with `OutOfBounds` (caller must use
    ///   `set_len()` first to extend the file).
    /// - Memory backends: may auto-extend.
    ///
    /// # Errors
    ///
    /// - `StorageErrorKind::OutOfBounds` if the write extends beyond
    ///   the storage and the backend does not auto-extend.
    /// - `StorageErrorKind::Io` on underlying I/O failure.
    /// - `StorageErrorKind::ReadOnly` if the backend is read-only.
    /// - `StorageErrorKind::StorageFull` if the medium cannot
    ///   accommodate the write.
    ///
    /// # Contract
    ///
    /// On success, exactly `buf.len()` bytes have been written.
    /// Partial writes are not exposed.
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<(), Self::Error>;

    /// Set the size of the storage to `new_size` bytes.
    ///
    /// - If `new_size > current_size`: the storage is extended. New
    ///   bytes are zero-filled.
    /// - If `new_size < current_size`: the storage is truncated.
    ///   Data beyond `new_size` is lost.
    /// - If `new_size == current_size`: no-op.
    ///
    /// # fsync note
    ///
    /// After extending a file, the caller MUST call `sync_all()`
    /// (not `sync_data()`) to ensure the new file size metadata is
    /// durable. See `008-file-format-spec.md` Section 15.2.
    ///
    /// # Errors
    ///
    /// - `StorageErrorKind::StorageFull` if the medium cannot grow
    ///   to the requested size.
    /// - `StorageErrorKind::Io` on underlying I/O failure.
    fn set_len(&mut self, new_size: u64) -> Result<(), Self::Error>;
}
```

**Design decision D2 — No `append()` method:** The Task 5 sketch included `append()`. We remove it because the file format (Task 8, Section 12) always extends the file with `set_len()` before writing new pages. There is no append-to-end pattern — all writes are to known offsets (page ID × page size). Removing `append()` simplifies the trait and avoids the need for the backend to track the current end-of-file position atomically.

### 5.3 `Sync`

```rust
// src/hal/traits.rs (continued)

/// Durability control: flush buffered writes to stable storage.
///
/// This trait provides two levels of sync, as required by the commit
/// protocol (Task 8, Section 15):
///
/// - `sync_data()`: Flushes file data only. Faster but does not
///   guarantee that file metadata (size, timestamps) is durable.
/// - `sync_all()`: Flushes file data AND metadata. Required after
///   file extension (`set_len()`) to ensure the new size is durable.
///
/// # Object safety
///
/// This trait is object-safe.
pub trait Sync: StorageErrorType {
    /// Flush all buffered data writes to stable storage.
    ///
    /// After this call returns `Ok(())`, all bytes written via
    /// `write_at()` since the last `sync_data()` or `sync_all()`
    /// call are guaranteed to survive a process crash (assuming the
    /// underlying storage correctly implements the sync primitive).
    ///
    /// # Platform mapping
    ///
    /// | Platform | System call |
    /// |----------|-------------|
    /// | Linux | `fdatasync()` |
    /// | macOS | `fcntl(F_FULLFSYNC)` |
    /// | Windows | `FlushFileBuffers()` |
    ///
    /// # In-memory backends
    ///
    /// No-op. In-memory writes are immediately visible.
    ///
    /// # Errors
    ///
    /// - `StorageErrorKind::Io` if the sync fails.
    fn sync_data(&mut self) -> Result<(), Self::Error>;

    /// Flush all buffered data AND metadata to stable storage.
    ///
    /// This is a superset of `sync_data()` — it additionally ensures
    /// that file metadata (size, modification time, directory entry)
    /// is durable.
    ///
    /// # When to use
    ///
    /// Call `sync_all()` instead of `sync_data()` when the file was
    /// extended via `set_len()` in the current transaction. This
    /// ensures the new file size is durable before the superblock
    /// references pages in the extended region.
    ///
    /// See `008-file-format-spec.md` Section 15.2 for the precise
    /// rule.
    ///
    /// # Platform mapping
    ///
    /// | Platform | System call |
    /// |----------|-------------|
    /// | Linux | `fsync()` |
    /// | macOS | `fcntl(F_FULLFSYNC)` |
    /// | Windows | `FlushFileBuffers()` |
    ///
    /// Note: On macOS, both `sync_data` and `sync_all` map to
    /// `F_FULLFSYNC` because macOS's `fsync()` does not guarantee
    /// data reaches the physical medium — only `F_FULLFSYNC` does.
    ///
    /// # In-memory backends
    ///
    /// No-op.
    ///
    /// # Errors
    ///
    /// - `StorageErrorKind::Io` if the sync fails.
    fn sync_all(&mut self) -> Result<(), Self::Error>;
}
```

**Design decision D3 — Trait name `Sync` vs `Flush`:** The name `Sync` is more precise: this trait controls durability, not buffer flushing. However, `Sync` conflicts with `core::marker::Sync`. To resolve this, the trait is defined in the `hal` module and always referenced as `hal::Sync` or imported with a rename. In practice, the storage engine uses `StorageBackend` (the combined trait), not `hal::Sync` directly, so the conflict rarely surfaces. If the naming proves awkward during implementation (Task 15), renaming to `DurabilityControl` is acceptable without changing the design.

**Design decision D4 — `&mut self` for `sync_data()` and `sync_all()`:** The Task 5 sketch used `&mut self` for `Flush::flush()`, and we retain this. While `fsync()` is logically an operation on a file descriptor (not mutable data), requiring `&mut self` ensures that sync calls are serialized with write operations. This prevents a subtle race: calling `sync_data()` concurrently with `write_at()` could sync a partial set of writes, violating the commit protocol's ordering requirements. The storage engine already holds `&mut` during the commit path, so this is not an additional constraint.

---

## 6. Combined Trait — `StorageBackend`

```rust
// src/hal/traits.rs (continued)

/// Full storage backend: readable, writable, and syncable.
///
/// This is the primary trait bound used throughout the storage engine.
/// A type that implements `ReadAt + WriteAt + hal::Sync` automatically
/// implements `StorageBackend` via the blanket impl.
///
/// # Usage in the storage engine
///
/// The storage engine is generic over `B: StorageBackend`:
///
/// ```rust,ignore
/// pub struct Engine<B: StorageBackend> {
///     backend: Arc<RwLock<B>>,
///     // ...
/// }
/// ```
///
/// For runtime backend selection (e.g., choosing between file and
/// memory at startup), use `dyn StorageBackend<Error = E>`:
///
/// ```rust,ignore
/// type AnyBackend = Box<dyn StorageBackend<Error = BoxedError>>;
/// ```
///
/// # Object safety
///
/// This trait is object-safe because all its supertraits are
/// object-safe.
pub trait StorageBackend: ReadAt + WriteAt + hal::Sync {}

/// Blanket implementation: any type implementing all three sub-traits
/// is automatically a `StorageBackend`.
impl<T: ReadAt + WriteAt + hal::Sync> StorageBackend for T {}
```

**Design decision D5 — Blanket impl vs. explicit impl:** The blanket impl means users never need to write `impl StorageBackend for MyBackend` — they implement the three sub-traits and get `StorageBackend` for free. This reduces boilerplate and ensures consistency. The downside is that `StorageBackend` cannot have its own methods (only inherited ones). This is intentional: the trait is a bundle, not an extension point.

---

## 7. Lifecycle Traits — `OpenableBackend`

```rust
// src/hal/lifecycle.rs

/// Open/create semantics for storage backends that manage external
/// resources (files, device handles).
///
/// This trait is `std`-only because filesystem open/create has no
/// analogue on bare metal. The core trait definitions (`ReadAt`,
/// `WriteAt`, `Sync`, `StorageBackend`) do NOT require this trait —
/// a `no_std` backend can be constructed by its own means and then
/// used through `StorageBackend`.
///
/// # Object safety
///
/// NOT object-safe (has `Self: Sized` bound). This is intentional —
/// construction is inherently type-specific. After construction, the
/// backend is used through `dyn StorageBackend`.
#[cfg(feature = "std")]
pub trait OpenableBackend: StorageBackend + Sized {
    /// Configuration for opening or creating a backend.
    ///
    /// For the file backend, this is `FileBackendConfig` (path, mode,
    /// page size hint, etc.). Other backends define their own config.
    type Config;

    /// Open an existing storage medium.
    ///
    /// Returns an error if the medium does not exist or cannot be opened.
    fn open(config: Self::Config) -> Result<Self, Self::Error>;

    /// Create a new storage medium, overwriting any existing content.
    ///
    /// Returns an error if the medium cannot be created.
    fn create(config: Self::Config) -> Result<Self, Self::Error>;

    /// Open an existing medium or create it if it does not exist.
    ///
    /// This is a convenience method. Backends that support it should
    /// implement it atomically (e.g., using `O_CREAT` on Unix).
    /// The default implementation tries `open()` first, then `create()`.
    fn open_or_create(config: Self::Config) -> Result<Self, Self::Error>
    where
        Self::Config: Clone,
    {
        // Note: This default implementation has a TOCTOU race.
        // The FileBackend overrides this with an atomic implementation.
        match Self::open(config.clone()) {
            Ok(backend) => Ok(backend),
            Err(_) => Self::create(config),
        }
    }
}
```

---

## 8. File Locking — `LockableBackend`

```rust
// src/hal/lifecycle.rs (continued)

/// Advisory file locking for single-process exclusivity.
///
/// The database requires exclusive access to its file to prevent
/// corruption from concurrent processes (Task 8, Section 14.3).
/// This trait provides the mechanism; the database engine calls it
/// at open time and releases the lock at close time.
///
/// This trait is `std`-only because file locking is an OS concept.
/// In-memory backends do not need it. `no_std` backends should
/// manage exclusivity through their own mechanisms (e.g., a hardware
/// mutex on a shared flash chip).
///
/// # Advisory vs. mandatory
///
/// On most Unix systems, `flock()` is advisory — it prevents
/// cooperative processes from acquiring the same lock but cannot
/// prevent a non-cooperating process from reading/writing the file.
/// On Windows, `LockFile()` is mandatory. The database documents
/// this as advisory-level protection (Task 8, Section 14.3).
///
/// # Object safety
///
/// Object-safe.
#[cfg(feature = "std")]
pub trait LockableBackend: StorageErrorType {
    /// A guard value that represents the held lock. When dropped,
    /// the lock is released.
    ///
    /// The guard must be `Send` so it can be held across thread
    /// boundaries (the database's `Engine` may be `Send + Sync`).
    type LockGuard: Send;

    /// Attempt to acquire an exclusive lock on the storage medium.
    ///
    /// This is a non-blocking operation. It returns immediately with
    /// either the lock guard or an error.
    ///
    /// # Errors
    ///
    /// - `StorageErrorKind::LockContention` if another process holds
    ///   the lock.
    /// - `StorageErrorKind::Io` on other locking failures.
    fn try_lock_exclusive(&self) -> Result<Self::LockGuard, Self::Error>;
}
```

**Design decision D6 — Lock guard as associated type:** The lock is represented as a guard value (RAII pattern). When the guard is dropped, the lock is released. This prevents forgetting to unlock and ensures the lock lifetime is tied to the database's lifetime. The `Send` bound on the guard allows the database engine to be `Send + Sync`.

**Design decision D7 — Non-blocking lock only:** We provide only `try_lock_exclusive()` (non-blocking), not `lock_exclusive()` (blocking). A database that finds its file locked by another process should fail immediately with a clear error, not hang indefinitely. The caller can retry if desired.

---

## 9. Default `std` Persistent Backend

### 9.1 Configuration

```rust
// src/hal_std/file_backend.rs

use std::path::PathBuf;

/// Configuration for opening or creating a file-backed database.
#[derive(Debug, Clone)]
pub struct FileBackendConfig {
    /// Path to the database file.
    pub path: PathBuf,

    /// If true, open the file in read-only mode. Write operations
    /// will return `StorageErrorKind::ReadOnly`.
    pub read_only: bool,
}
```

### 9.2 Error type

```rust
// src/hal_std/file_backend.rs (continued)

/// Error type for the `std` file-backed storage backend.
///
/// Wraps `std::io::Error` with additional context.
#[derive(Debug)]
pub enum FileError {
    /// An I/O error from the operating system.
    Io(std::io::Error),

    /// The file is locked by another process.
    LockContention,

    /// A read or write was out of bounds.
    OutOfBounds {
        offset: u64,
        len: usize,
        file_size: u64,
    },

    /// The backend is in read-only mode.
    ReadOnly,
}

impl core::fmt::Display for FileError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FileError::Io(e) => write!(f, "I/O error: {}", e),
            FileError::LockContention => {
                write!(f, "database file is locked by another process")
            }
            FileError::OutOfBounds { offset, len, file_size } => {
                write!(
                    f,
                    "out of bounds: read/write at offset {} with length {} \
                     exceeds file size {}",
                    offset, len, file_size
                )
            }
            FileError::ReadOnly => write!(f, "database is opened in read-only mode"),
        }
    }
}

#[cfg(feature = "std")]
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
```

### 9.3 Backend structure

```rust
// src/hal_std/file_backend.rs (continued)

use std::fs::{File, OpenOptions};

/// File-backed storage backend. The primary persistent backend.
///
/// Uses `pread()`/`pwrite()` on Unix and `ReadFile()`/`WriteFile()`
/// with explicit offsets on Windows for thread-safe random I/O
/// without shared seek position state.
///
/// # Thread safety
///
/// `ReadAt::read_at()` takes `&self`, enabling concurrent reads.
/// The underlying `pread` is thread-safe. `WriteAt` and `Sync` take
/// `&mut self`, ensuring exclusive write access at the Rust type
/// level. The storage engine manages concurrency via `RwLock`.
pub struct FileBackend {
    /// The open file handle.
    file: File,

    /// Whether the file was opened in read-only mode.
    read_only: bool,
}
```

### 9.4 `ReadAt` implementation

```rust
impl StorageErrorType for FileBackend {
    type Error = FileError;
}

impl ReadAt for FileBackend {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), FileError> {
        use std::io::Read;

        if buf.is_empty() {
            return Ok(());
        }

        // Use platform-specific positional read (no seek state).
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
            // Windows seek_read does not guarantee reading all bytes.
            // We must loop until buf is full.
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
            compile_error!(
                "FileBackend requires either Unix (pread) or Windows (seek_read) support"
            );
        }
    }

    fn len(&self) -> Result<u64, FileError> {
        Ok(self.file.metadata()?.len())
    }
}
```

### 9.5 `WriteAt` implementation

```rust
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
            compile_error!(
                "FileBackend requires either Unix (pwrite) or Windows (seek_write) support"
            );
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
```

### 9.6 `Sync` implementation

```rust
impl hal::Sync for FileBackend {
    fn sync_data(&mut self) -> Result<(), FileError> {
        if self.read_only {
            return Ok(()); // Nothing to sync in read-only mode.
        }

        #[cfg(target_os = "macos")]
        {
            // macOS's fsync() does not guarantee data reaches the
            // physical medium. Only F_FULLFSYNC does.
            // File::sync_data() on macOS calls fdatasync() which is
            // also insufficient. We must use fcntl(F_FULLFSYNC).
            use std::os::unix::io::AsRawFd;
            let ret = unsafe { libc::fcntl(self.file.as_raw_fd(), libc::F_FULLFSYNC) };
            if ret == -1 {
                return Err(FileError::Io(std::io::Error::last_os_error()));
            }
            Ok(())
        }

        #[cfg(all(unix, not(target_os = "macos")))]
        {
            // Linux: fdatasync() — syncs data, not metadata.
            self.file.sync_data()?;
            Ok(())
        }

        #[cfg(windows)]
        {
            // Windows: FlushFileBuffers() syncs both data and metadata.
            // There is no "data only" variant, so sync_data and
            // sync_all behave identically on Windows.
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
            // Same as sync_data on macOS — F_FULLFSYNC covers both.
            use std::os::unix::io::AsRawFd;
            let ret = unsafe { libc::fcntl(self.file.as_raw_fd(), libc::F_FULLFSYNC) };
            if ret == -1 {
                return Err(FileError::Io(std::io::Error::last_os_error()));
            }
            Ok(())
        }

        #[cfg(all(unix, not(target_os = "macos")))]
        {
            // Linux: fsync() — syncs data AND metadata.
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
```

### 9.7 `OpenableBackend` implementation

```rust
#[cfg(feature = "std")]
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

    fn open_or_create(config: FileBackendConfig) -> Result<Self, FileError> {
        if config.read_only {
            return Self::open(config);
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&config.path)?;
        Ok(FileBackend {
            file,
            read_only: false,
        })
    }
}
```

### 9.8 `LockableBackend` implementation

```rust
#[cfg(feature = "std")]
impl LockableBackend for FileBackend {
    type LockGuard = FileLockGuard;

    fn try_lock_exclusive(&self) -> Result<FileLockGuard, FileError> {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let fd = self.file.as_raw_fd();
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
            use windows_sys::Win32::Foundation::HANDLE;

            let handle = self.file.as_raw_handle() as HANDLE;
            let mut overlapped = unsafe { core::mem::zeroed() };
            let result = unsafe {
                LockFileEx(
                    handle,
                    LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                    0,
                    u32::MAX,
                    u32::MAX,
                    &mut overlapped,
                )
            };
            if result == 0 {
                let err = std::io::Error::last_os_error();
                return Err(FileError::LockContention);
            }
            Ok(FileLockGuard { handle })
        }
    }
}

/// RAII guard for a file lock. Releases the lock when dropped.
#[cfg(feature = "std")]
pub struct FileLockGuard {
    #[cfg(unix)]
    fd: std::os::unix::io::RawFd,
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}

// Safety: The file descriptor / handle is valid for the lifetime of the guard,
// and flock/LockFile are safe to use from any thread.
#[cfg(feature = "std")]
unsafe impl Send for FileLockGuard {}

#[cfg(feature = "std")]
impl Drop for FileLockGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            unsafe { libc::flock(self.fd, libc::LOCK_UN) };
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::Storage::FileSystem::UnlockFileEx;
            let mut overlapped = unsafe { core::mem::zeroed() };
            unsafe {
                UnlockFileEx(self.handle, 0, u32::MAX, u32::MAX, &mut overlapped);
            }
        }
    }
}
```

**Design decision D8 — `flock()` vs `fcntl()` locking on Unix:** We use `flock()` rather than `fcntl()` POSIX locks. `flock()` is simpler: it locks the entire file and is released when the file descriptor is closed. `fcntl()` locks are per-process (shared across all file descriptors) and have surprising semantics — closing any descriptor to the file releases all locks. For an embedded database where one process holds one file handle, `flock()` is the correct choice. This matches SQLite's approach on platforms where `flock()` is available.

**Design decision D9 — `libc` and `windows-sys` as dependencies:** The file locking and macOS `F_FULLFSYNC` implementations require platform-specific FFI. We depend on `libc` (for Unix) and `windows-sys` (for Windows) in the `std` feature only. These are thin FFI bindings maintained by the Rust project (or Microsoft), not database crates, so they are permitted under the project constraint "no external database crate dependencies." Both are behind `#[cfg(feature = "std")]` and do not affect the `no_std` core.

---

## 10. In-Memory Backend

The in-memory backend provides RAM-based storage without durability. It is available in `no_std + alloc` environments.

### 10.1 Error type

```rust
// src/hal_mem/memory_backend.rs

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

/// Error type for the in-memory storage backend.
///
/// In-memory writes to a growable `Vec` cannot fail in the I/O sense.
/// The only failure mode is an out-of-bounds read (attempting to read
/// beyond the current `Vec` length without first extending it).
///
/// In practice, most operations on `MemoryBackend` are infallible.
/// This error type exists to satisfy the trait's associated type
/// requirement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryError {
    /// A read was attempted beyond the current storage size.
    OutOfBounds {
        offset: u64,
        requested: usize,
        size: u64,
    },
}

impl core::fmt::Display for MemoryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MemoryError::OutOfBounds { offset, requested, size } => {
                write!(
                    f,
                    "out of bounds: read at offset {} with length {} \
                     exceeds memory size {}",
                    offset, requested, size
                )
            }
        }
    }
}

impl StorageError for MemoryError {
    fn kind(&self) -> StorageErrorKind {
        match self {
            MemoryError::OutOfBounds { .. } => StorageErrorKind::OutOfBounds,
        }
    }
}
```

### 10.2 Backend structure

```rust
/// In-memory storage backend backed by a `Vec<u8>`.
///
/// # Use cases
///
/// - Testing: fast, deterministic, no filesystem interaction.
/// - Ephemeral databases: short-lived data that does not need
///   persistence.
/// - `no_std + alloc` environments without a filesystem.
///
/// # Optional snapshot support
///
/// The in-memory backend supports saving its contents to a byte
/// slice (or file, with `std`) and loading from one. This enables
/// a "snapshot-to-disk" workflow where the database operates in
/// memory for speed and periodically persists a snapshot.
///
/// Snapshot is a secondary capability — the primary persistent
/// backend is `FileBackend`.
///
/// # Thread safety
///
/// `ReadAt::read_at()` takes `&self`. Because `MemoryBackend` stores
/// data in a `Vec<u8>`, immutable access is safe for concurrent
/// reads. The storage engine wraps it in `RwLock` as with any
/// backend.
pub struct MemoryBackend {
    data: Vec<u8>,
}

impl MemoryBackend {
    /// Create a new empty in-memory backend.
    pub fn new() -> Self {
        MemoryBackend { data: Vec::new() }
    }

    /// Create an in-memory backend pre-sized to `capacity` bytes.
    ///
    /// The initial size (as reported by `len()`) is `capacity`, and
    /// all bytes are zero. This is useful for creating a database
    /// with a known initial size without repeated resizing.
    pub fn with_size(size: usize) -> Self {
        MemoryBackend {
            data: alloc::vec![0u8; size],
        }
    }

    /// Create an in-memory backend from existing data.
    ///
    /// This is the "load from snapshot" entry point.
    pub fn from_bytes(data: Vec<u8>) -> Self {
        MemoryBackend { data }
    }

    /// Return the backing data as a byte slice.
    ///
    /// This is the "snapshot to bytes" entry point. The caller can
    /// write this to a file for persistence.
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Consume the backend and return the backing data.
    pub fn into_bytes(self) -> Vec<u8> {
        self.data
    }
}
```

### 10.3 Trait implementations

```rust
impl StorageErrorType for MemoryBackend {
    type Error = MemoryError;
}

impl ReadAt for MemoryBackend {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), MemoryError> {
        if buf.is_empty() {
            return Ok(());
        }
        let offset = offset as usize;
        let end = offset.checked_add(buf.len()).ok_or(MemoryError::OutOfBounds {
            offset: offset as u64,
            requested: buf.len(),
            size: self.data.len() as u64,
        })?;
        if end > self.data.len() {
            return Err(MemoryError::OutOfBounds {
                offset: offset as u64,
                requested: buf.len(),
                size: self.data.len() as u64,
            });
        }
        buf.copy_from_slice(&self.data[offset..end]);
        Ok(())
    }

    fn len(&self) -> Result<u64, MemoryError> {
        Ok(self.data.len() as u64)
    }
}

impl WriteAt for MemoryBackend {
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<(), MemoryError> {
        if buf.is_empty() {
            return Ok(());
        }
        let offset = offset as usize;
        let end = offset + buf.len();
        // Auto-extend if necessary.
        if end > self.data.len() {
            self.data.resize(end, 0);
        }
        self.data[offset..end].copy_from_slice(buf);
        Ok(())
    }

    fn set_len(&mut self, new_size: u64) -> Result<(), MemoryError> {
        self.data.resize(new_size as usize, 0);
        Ok(())
    }
}

impl hal::Sync for MemoryBackend {
    /// No-op: in-memory writes are immediately visible.
    fn sync_data(&mut self) -> Result<(), MemoryError> {
        Ok(())
    }

    /// No-op: in-memory writes are immediately visible.
    fn sync_all(&mut self) -> Result<(), MemoryError> {
        Ok(())
    }
}
```

### 10.4 Snapshot helpers (std-only)

```rust
#[cfg(feature = "std")]
impl MemoryBackend {
    /// Save the current contents to a file.
    ///
    /// This writes the raw byte contents of the in-memory storage
    /// to the specified path. The resulting file is a valid database
    /// file and can be opened with `FileBackend`.
    pub fn save_to_file(&self, path: &std::path::Path) -> std::io::Result<()> {
        std::fs::write(path, &self.data)
    }

    /// Load contents from a file into a new in-memory backend.
    ///
    /// This reads the entire file into memory. The file should be
    /// a valid database file (e.g., one previously saved with
    /// `save_to_file` or created by `FileBackend`).
    pub fn load_from_file(path: &std::path::Path) -> std::io::Result<Self> {
        let data = std::fs::read(path)?;
        Ok(MemoryBackend { data })
    }
}
```

---

## 11. Hypothetical `no_std` NOR Flash Backend Walkthrough

This section demonstrates that the trait surface is implementable on constrained hardware. The backend wraps an `embedded-storage` NOR flash device.

```rust
//! Hypothetical backend for a NOR flash chip.
//!
//! This would live in a downstream crate (e.g., `my-flash-db`), not
//! in the core database crate. It demonstrates that the HAL traits
//! are implementable without `std`.

#![no_std]
extern crate alloc;

use embedded_storage::nor_flash::NorFlash;
use my_graph_db::hal::{
    self, ReadAt, WriteAt, StorageError, StorageErrorKind, StorageErrorType,
};

const SECTOR_SIZE: usize = 4096; // Typical NOR flash sector size.

/// Error type for the NOR flash adapter.
#[derive(Debug)]
pub enum FlashError<E: core::fmt::Debug> {
    /// The underlying flash driver reported an error.
    Flash(E),
    /// An operation was out of bounds for the flash capacity.
    OutOfBounds { offset: u64, capacity: u64 },
    /// NOR flash requires erase before write; the sector-buffer logic
    /// detected an inconsistency.
    EraseFailed,
}

impl<E: core::fmt::Debug> core::fmt::Display for FlashError<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FlashError::Flash(e) => write!(f, "flash driver error: {:?}", e),
            FlashError::OutOfBounds { offset, capacity } => {
                write!(f, "offset {} exceeds flash capacity {}", offset, capacity)
            }
            FlashError::EraseFailed => write!(f, "sector erase failed"),
        }
    }
}

impl<E: core::fmt::Debug> StorageError for FlashError<E> {
    fn kind(&self) -> StorageErrorKind {
        match self {
            FlashError::Flash(_) => StorageErrorKind::Io,
            FlashError::OutOfBounds { .. } => StorageErrorKind::OutOfBounds,
            FlashError::EraseFailed => StorageErrorKind::Io,
        }
    }
}

/// Adapts an `embedded-storage` NOR flash device to the database HAL.
///
/// NOR flash has sector-erase semantics: before writing to a sector,
/// the entire sector must be erased (set to 0xFF). This adapter
/// buffers writes per-sector and performs erase-then-write on flush.
pub struct NorFlashAdapter<F: NorFlash> {
    flash: F,
    /// Sector write buffer. Holds the contents of the currently
    /// "dirty" sector so that partial writes within a sector can
    /// be accumulated before the erase-write cycle.
    write_buffer: [u8; SECTOR_SIZE],
    /// The sector index of the buffered sector, or `None` if no
    /// sector is currently buffered.
    dirty_sector: Option<u32>,
    /// Total capacity in bytes.
    capacity: u64,
}

impl<F: NorFlash> StorageErrorType for NorFlashAdapter<F>
where
    F::Error: core::fmt::Debug,
{
    type Error = FlashError<F::Error>;
}

impl<F: NorFlash> ReadAt for NorFlashAdapter<F>
where
    F::Error: core::fmt::Debug,
{
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        if offset + buf.len() as u64 > self.capacity {
            return Err(FlashError::OutOfBounds {
                offset,
                capacity: self.capacity,
            });
        }
        // Check if the read overlaps the dirty sector buffer.
        if let Some(dirty) = self.dirty_sector {
            let sector_start = dirty as u64 * SECTOR_SIZE as u64;
            let sector_end = sector_start + SECTOR_SIZE as u64;
            let read_end = offset + buf.len() as u64;
            if offset < sector_end && read_end > sector_start {
                // Partial or full overlap — serve from buffer.
                // (Production code would handle partial overlaps
                //  by splitting the read. Simplified here.)
                let buf_offset = (offset - sector_start) as usize;
                buf.copy_from_slice(
                    &self.write_buffer[buf_offset..buf_offset + buf.len()],
                );
                return Ok(());
            }
        }
        // No overlap — read directly from flash.
        embedded_storage::nor_flash::ReadNorFlash::read(
            &self.flash,
            offset as u32,
            buf,
        )
        .map_err(FlashError::Flash)
    }

    fn len(&self) -> Result<u64, Self::Error> {
        Ok(self.capacity)
    }
}

impl<F: NorFlash> WriteAt for NorFlashAdapter<F>
where
    F::Error: core::fmt::Debug,
{
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<(), Self::Error> {
        // Simplified: assumes writes are sector-aligned and
        // sector-sized (which is true for page-aligned database I/O
        // when page_size == SECTOR_SIZE).
        let sector = (offset / SECTOR_SIZE as u64) as u32;

        // If a different sector is dirty, flush it first.
        if let Some(dirty) = self.dirty_sector {
            if dirty != sector {
                self.flush_dirty_sector()?;
            }
        }

        // Load sector into buffer if not already there.
        if self.dirty_sector.is_none() {
            let sector_start = sector as u64 * SECTOR_SIZE as u64;
            embedded_storage::nor_flash::ReadNorFlash::read(
                &self.flash,
                sector_start as u32,
                &mut self.write_buffer,
            )
            .map_err(FlashError::Flash)?;
        }

        // Write into the buffer.
        let buf_offset = (offset % SECTOR_SIZE as u64) as usize;
        self.write_buffer[buf_offset..buf_offset + buf.len()]
            .copy_from_slice(buf);
        self.dirty_sector = Some(sector);
        Ok(())
    }

    fn set_len(&mut self, _new_size: u64) -> Result<(), Self::Error> {
        // NOR flash has fixed capacity — set_len is a no-op if the
        // size is within capacity, error otherwise.
        if _new_size > self.capacity {
            return Err(FlashError::OutOfBounds {
                offset: _new_size,
                capacity: self.capacity,
            });
        }
        Ok(())
    }
}

impl<F: NorFlash> hal::Sync for NorFlashAdapter<F>
where
    F::Error: core::fmt::Debug,
{
    fn sync_data(&mut self) -> Result<(), Self::Error> {
        self.flush_dirty_sector()
    }

    fn sync_all(&mut self) -> Result<(), Self::Error> {
        // No metadata distinction on bare metal — same as sync_data.
        self.flush_dirty_sector()
    }
}

impl<F: NorFlash> NorFlashAdapter<F>
where
    F::Error: core::fmt::Debug,
{
    fn flush_dirty_sector(&mut self) -> Result<(), FlashError<F::Error>> {
        if let Some(sector) = self.dirty_sector.take() {
            let sector_start = sector as u64 * SECTOR_SIZE as u64;
            // Erase the sector, then write the buffer.
            self.flash
                .erase(sector_start as u32, (sector_start + SECTOR_SIZE as u64) as u32)
                .map_err(FlashError::Flash)?;
            self.flash
                .write(sector_start as u32, &self.write_buffer)
                .map_err(FlashError::Flash)?;
        }
        Ok(())
    }
}
```

This walkthrough validates that:

1. The trait surface is implementable without `std` — only `core` and `alloc` are needed.
2. The associated error type accommodates backend-specific errors (flash driver errors wrapped in `FlashError<E>`).
3. The two-level sync (`sync_data`, `sync_all`) maps naturally to bare-metal semantics (both perform the same erase-write flush).
4. The `&self` receiver on `ReadAt` works even with a write buffer — the buffer is read through shared reference (in production, this would need an interior mutability mechanism like `RefCell` for single-threaded `no_std`, or be excluded from the read path if reads always go to flash directly).
5. The page-aligned I/O assumption (Task 8) simplifies the adapter — full-sector writes avoid complex partial-sector merging.

---

## 12. Error Propagation Chain

The HAL error type is the bottom of a three-layer error chain:

```
┌──────────────────────────────────┐
│  Public API Error                │
│  (e.g., DatabaseError)           │
│  ┌──────────────────────────────┐│
│  │  Storage Engine Error        ││
│  │  (e.g., EngineError)        ││
│  │  ┌──────────────────────────┐││
│  │  │  HAL Error               │││
│  │  │  (e.g., FileError,      │││
│  │  │   MemoryError)           │││
│  │  └──────────────────────────┘││
│  └──────────────────────────────┘│
└──────────────────────────────────┘
```

### Conversion strategy

Each layer wraps the layer below via `From` impls:

```rust
// Sketch — the actual types are defined in Tasks 10 (API) and 16 (engine).

/// Storage engine error. Generic over the backend's error type.
pub enum EngineError<E: StorageError> {
    /// An error from the HAL backend.
    Storage(E),
    /// A page checksum mismatch (detected by the engine, not the HAL).
    ChecksumMismatch { page_id: u64 },
    /// The database file's format version is not supported.
    UnsupportedFormat { major: u16, minor: u16 },
    // ... other engine-level errors
}

impl<E: StorageError> From<E> for EngineError<E> {
    fn from(e: E) -> Self {
        EngineError::Storage(e)
    }
}

/// Public API error. Erases the backend type for ergonomics.
pub enum DatabaseError {
    /// A storage or engine error, with the kind preserved.
    Storage {
        kind: StorageErrorKind,
        message: alloc::string::String,
    },
    /// A schema validation error.
    SchemaViolation(alloc::string::String),
    // ... other API-level errors
}

impl<E: StorageError> From<EngineError<E>> for DatabaseError {
    fn from(e: EngineError<E>) -> Self {
        match e {
            EngineError::Storage(se) => DatabaseError::Storage {
                kind: se.kind(),
                message: alloc::format!("{}", se),
            },
            // ... other variants
        }
    }
}
```

**Design decision D10 — Type erasure at the API boundary:** The public API (`DatabaseError`) does not carry the generic `E: StorageError` parameter. It captures the error kind and a formatted message string. This prevents the backend type from leaking into every public API return type, keeping the API ergonomic. Users who need the concrete error can use the engine layer directly.

---

## 13. fsync Discipline and Platform Mapping

This section codifies the platform-specific mapping from the abstract `sync_data()` / `sync_all()` methods to OS primitives. It is the authoritative reference for the `FileBackend` implementation (Task 15).

### 13.1 Platform mapping table

| Method | Linux | macOS | Windows | `no_std` / Memory |
|--------|-------|-------|---------|-------------------|
| `sync_data()` | `fdatasync()` | `fcntl(F_FULLFSYNC)` | `FlushFileBuffers()` | No-op |
| `sync_all()` | `fsync()` | `fcntl(F_FULLFSYNC)` | `FlushFileBuffers()` | No-op |

### 13.2 macOS note

On macOS, `fsync()` only guarantees data reaches the OS kernel's buffer cache, not the physical disk. `fcntl(F_FULLFSYNC)` is the only reliable way to force data to the physical medium. Both `sync_data()` and `sync_all()` map to `F_FULLFSYNC` on macOS.

This is a well-known issue documented by SQLite, LMDB, and other databases. Apple's documentation for `F_FULLFSYNC` explicitly states it is for "applications that require a strict ordering of writes to stable storage."

### 13.3 Windows note

Windows does not distinguish between "data only" and "data + metadata" sync. `FlushFileBuffers()` flushes both. This means `sync_data()` and `sync_all()` behave identically on Windows, which is correct (slightly over-syncing, never under-syncing).

### 13.4 Commit protocol fsync usage

From `008-file-format-spec.md` Section 15:

1. After writing all new data pages: call `sync_all()` if the file was extended in this transaction (i.e., `set_len()` was called), otherwise call `sync_data()`.
2. After writing the new superblock: call `sync_data()` (the superblock write does not change the file size).

The storage engine (Task 16) is responsible for tracking whether `set_len()` was called and selecting the appropriate sync method. The HAL provides both; the engine decides which to use.

---

## 14. Durability Warnings

The following warnings should be documented in the `StorageBackend` trait's module-level documentation and in the crate's README.

### 14.1 Filesystem trust

The crash-safety guarantees of the database (Task 8, Section 14.1) depend on the filesystem correctly implementing `fsync`. Filesystems that silently discard `fsync` (such as certain configurations of ext3/ext4 in older Linux kernels, or some network filesystems) can violate durability guarantees. The database cannot detect or work around this.

**Recommendation:** Use a filesystem known to implement `fsync` correctly: ext4 with `data=ordered` or `data=journal`, XFS, ZFS, APFS, or NTFS.

### 14.2 Disk write caching

Some storage devices (particularly consumer-grade SSDs and USB drives) have volatile write caches that may report `fsync` completion before data reaches non-volatile storage. This can cause data loss on power failure.

**Recommendation:** For critical deployments, use enterprise-grade storage with power-loss protection or disable the device's write cache.

### 14.3 Advisory locking

On Unix, `flock()` is advisory — it prevents cooperating processes from acquiring the lock but cannot prevent a non-cooperating process from reading or modifying the database file. On Windows, file locks are mandatory.

**Recommendation:** Do not open the same database file from multiple processes. If multi-process access is required, use a process-level coordinator (out of scope for this crate).

---

## 15. Design Decision Log

| ID | Decision | Alternatives considered | Rationale |
|----|----------|------------------------|-----------|
| D1 | `Display` bound on `StorageError` | `Debug`-only (Task 5 sketch) | Error messages need to be user-facing in the public API. `Display` is available in `core`. |
| D2 | No `append()` method | Include `append()` (Task 5 sketch) | File format uses `set_len()` + `write_at()`. No append-to-end pattern exists. Removing it simplifies the trait. |
| D3 | Trait named `Sync` (in `hal` module) | `Flush` (Task 5 sketch); `DurabilityControl`; `Fsync` | `Sync` is precise. The `core::marker::Sync` conflict is resolved by module qualification. Can rename if problematic during implementation. |
| D4 | `&mut self` for sync methods | `&self` for sync | Prevents race between sync and concurrent writes. The engine already holds `&mut` during commit. |
| D5 | Blanket impl for `StorageBackend` | Explicit impl required per backend | Reduces boilerplate. Users implement three sub-traits and get `StorageBackend` for free. |
| D6 | Lock guard as associated type (RAII) | `lock()`/`unlock()` methods | RAII prevents forgetting to unlock. `Send` bound ensures cross-thread compatibility. |
| D7 | Non-blocking lock only (`try_lock_exclusive`) | Also provide blocking `lock_exclusive` | Database should fail immediately on contention, not hang. Caller can retry if desired. |
| D8 | `flock()` on Unix | `fcntl()` POSIX locks | `flock()` is simpler and avoids `fcntl`'s surprising per-process (not per-fd) semantics. Matches SQLite. |
| D9 | `libc` and `windows-sys` dependencies | Pure Rust reimplementation; `nix` crate | Thin FFI bindings, not database crates. Minimal and well-maintained. `nix` adds unnecessary abstraction. |
| D10 | Type erasure at API boundary | Propagate generic `E` to public API | Prevents backend type from leaking. Users get `StorageErrorKind` + message. Power users use engine directly. |
| D11 | `MemoryBackend` auto-extends on `write_at` | Require `set_len` before write | Auto-extension is the natural behavior for a `Vec<u8>`. Simplifies testing. `FileBackend` does not auto-extend (requires `set_len` first). |
| D12 | Two sync methods (`sync_data`, `sync_all`) | Single `flush()` (Task 5 sketch) | Required by Task 8 fsync discipline (Section 15.2). `sync_data` avoids unnecessary metadata sync on non-extension commits. |

---

## Completion Report: Task 9 — HAL Trait Layer

### Status: COMPLETE

### Done Criterion:

The criterion requires:

1. Full trait definitions as Rust code — ✓ Sections 4–8 provide complete trait definitions for `StorageErrorKind`, `StorageError`, `StorageErrorType`, `ReadAt`, `WriteAt`, `hal::Sync`, `StorageBackend`, `OpenableBackend`, and `LockableBackend`, all as compilable Rust code with documentation on every method.

2. Documentation for each method — ✓ Every method includes a doc comment describing its contract, errors, platform behavior, and relationship to the file format spec.

3. The `std` persistent backend sketch — ✓ Section 9 provides the complete `FileBackend` implementation including configuration, error type, all trait implementations (`ReadAt`, `WriteAt`, `hal::Sync`, `OpenableBackend`, `LockableBackend`), platform-specific code for Unix and Windows, and the `FileLockGuard` RAII type.

4. The in-memory backend sketch — ✓ Section 10 provides the complete `MemoryBackend` implementation including error type, all trait implementations, snapshot save/load helpers, and construction methods.

5. A walkthrough showing how a hypothetical `no_std` backend would implement the traits — ✓ Section 11 provides a full NOR flash adapter implementation using `embedded-storage`, validating that the trait surface is implementable on constrained hardware without `std`.

All criteria met.

### Deliverables:

- `009-hal-trait-design.md` — this document

### Summary:

Designed the complete HAL trait layer for the embedded graph database, refining the preliminary sketch from Task 5 based on the file format requirements established in Task 8. The major refinement from the Task 5 sketch is replacing the single `flush()` method with two-level durability control (`sync_data()` and `sync_all()`), as required by the fsync discipline in Task 8 Section 15. Other additions include file locking for single-process exclusivity (`LockableBackend`), removal of the `append()` method (not needed by the page-aligned I/O model), and a `Display` bound on the error trait for user-facing error messages.

The design provides three backend implementations: `FileBackend` (std, persistent, primary), `MemoryBackend` (alloc, in-memory, secondary), and a walkthrough `NorFlashAdapter` (no_std, bare-metal). All core traits are `no_std + alloc` compatible and object-safe, enabling both static dispatch (generics) and dynamic dispatch (`dyn StorageBackend`).

### Context for Next Task:

**Task 10 (API Surface)** depends on Tasks 6 and 7, not directly on this document, but should reference it for the `OpenableBackend` configuration types and the `DatabaseError` type-erasure pattern described in Section 12.

**Task 12 (Design Synthesis)** should read `009-hal-trait-design.md` (this deliverable) and incorporate the trait definitions, the error propagation chain, the fsync discipline, and the platform mapping table. Key items to synthesize: the three-trait decomposition (`ReadAt` + `WriteAt` + `Sync` = `StorageBackend`), the `std`-only lifecycle/locking traits, and the design decisions (especially D2, D3, D10, D12).

**Task 15 (HAL & std Backend Implementation)** is the direct implementation task for this design. It should read this document as its primary spec. Key implementation notes: the macOS `F_FULLFSYNC` requirement (Section 13.2), the Windows `seek_read`/`seek_write` looping requirement (Section 9.4), and the `libc`/`windows-sys` dependency decision (D9).

### Residual Concerns:

1. **`hal::Sync` naming conflict with `core::marker::Sync`:** The name is correct semantically but may cause confusion or require explicit disambiguation in `use` statements. If this proves friction-heavy during implementation, renaming to `DurabilityControl` or `StorageSync` is a low-risk change that does not affect the design. The implementation task (Task 15) should decide based on ergonomic experience.

2. **`ReadAt` takes `&self` but `NorFlashAdapter` has a mutable write buffer:** The walkthrough (Section 11) notes that the dirty-sector buffer needs `&self` access for reads that overlap the buffer. In production, this requires `RefCell` (single-threaded) or a similar interior mutability mechanism. This is a complexity specific to flash backends and does not affect the file or memory backends. The walkthrough is simplified; a real implementation would need to handle this carefully.

3. **`windows-sys` dependency specifics:** The file locking implementation (Section 9.8) sketches the Windows path using `windows-sys` types. The exact `windows-sys` feature flags and API may differ at implementation time. Task 15 should verify the API surface.

4. **Error type for `dyn StorageBackend`:** Using `dyn StorageBackend` requires a concrete error type (trait objects need a single type). The pattern `Box<dyn StorageError>` works but introduces allocation on the error path. An alternative is a `BoxedStorageError` wrapper. Task 12 or 15 should finalize this choice.

### Upstream Flags:

None. All findings are consistent with the project's stated constraints and the upstream specifications from Tasks 5 and 8.

---

*Document produced for Task 9. Feeds Tasks 12 (design synthesis), 15 (HAL implementation), and 19 (in-memory backend implementation).*
