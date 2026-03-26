# Checklist: Task 23 — Implement HAL Trait Layer & std Persistent Backend

**Parent:** Task 23 (this checklist)  
**Implements:** `src/hal/` (trait definitions, `no_std + alloc`), `src/hal_std/` (FileBackend, `std` only), and updates to `src/lib.rs` and `Cargo.toml`.  
**Primary spec:** `009-hal-trait-design.md` — all type signatures and implementation code come from this document. When in doubt, it is authoritative.

Execute items in order. After each item, run the verification command(s) listed. Do not proceed until verification passes.

---

## Phase 0: Cargo.toml Updates and Module Scaffolding

### 0.1 — Add platform-specific dependencies to Cargo.toml

Add to `[dependencies]`:

```toml
[target.'cfg(unix)'.dependencies]
libc = { version = "0.2", optional = true }

[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.59", features = ["Win32_Storage_FileSystem", "Win32_Foundation", "Win32_System_IO"], optional = true }
```

Update feature flags so these are pulled in only under `std`:

```toml
[features]
default = ["std"]
std = ["alloc", "libc", "windows-sys"]
alloc = []
```

**⚠ Pitfall:** The `windows-sys` version and feature names may have changed. If the exact features listed above cause resolution errors, check the latest `windows-sys` docs and adjust. The required APIs are `LockFileEx`, `UnlockFileEx`, `OVERLAPPED`, `LOCKFILE_EXCLUSIVE_LOCK`, `LOCKFILE_FAIL_IMMEDIATELY`. Document any deviation in the completion report.

**Verify:** `cargo check` succeeds (existing code still compiles).

### 0.2 — Create HAL module structure

Create the following files with placeholder module-level doc comments (`//!`):

```
src/hal/mod.rs
src/hal/error.rs
src/hal/traits.rs
src/hal/lifecycle.rs
```

In `src/hal/mod.rs`, add:
```rust
//! Hardware Abstraction Layer (HAL) — trait definitions for storage I/O.
//!
//! This module defines the trait hierarchy that all storage backends
//! implement. The traits are `no_std + alloc` compatible and object-safe,
//! enabling both static dispatch (generics) and dynamic dispatch
//! (`dyn StorageBackend`).
//!
//! # Trait hierarchy
//!
//! ```text
//! StorageErrorType (associated Error type)
//!     ├── ReadAt   (&self — concurrent reads)
//!     ├── WriteAt  (&mut self — exclusive writes)
//!     └── Sync     (&mut self — durability control)
//!           └── StorageBackend = ReadAt + WriteAt + Sync (blanket impl)
//! ```
//!
//! Lifecycle traits (`OpenableBackend`, `LockableBackend`) are `std`-only
//! and live in the [`lifecycle`] submodule.

pub mod error;
pub mod traits;
pub mod lifecycle;

// Re-export primary types at the `hal` level
pub use error::{StorageErrorKind, StorageError, StorageErrorType};
pub use traits::{ReadAt, WriteAt, Sync, StorageBackend};
pub use lifecycle::{OpenableBackend, LockableBackend};
```

**⚠ Pitfall — `Sync` re-export:** Re-exporting `hal::Sync` at the `hal` module level means downstream code can write `hal::Sync`. If this causes ambiguity with `core::marker::Sync` in certain contexts, the implementation may choose to not re-export `Sync` and instead require `hal::traits::Sync` or rename the trait. Document any decision.

**Verify:** `cargo check --no-default-features --features alloc` succeeds (empty modules).

### 0.3 — Create hal_std module structure

Create:
```
src/hal_std/mod.rs
src/hal_std/file_backend.rs
```

In `src/hal_std/mod.rs`, add:
```rust
//! `std` persistent file backend.
//!
//! This module provides [`FileBackend`], the primary durable storage
//! backend for the database. It uses `pread()`/`pwrite()` on Unix
//! and `ReadFile()`/`WriteFile()` with explicit offsets on Windows.

pub mod file_backend;

pub use file_backend::{FileBackend, FileBackendConfig, FileError, FileLockGuard};
```

**Verify:** `cargo check` succeeds.

### 0.4 — Register new modules in lib.rs

Add to `src/lib.rs` (after the existing module declarations):

```rust
pub mod hal;

#[cfg(feature = "std")]
pub mod hal_std;
```

Add re-exports at the crate root (after the existing re-exports):

```rust
pub use hal::{
    StorageErrorKind, StorageError, StorageErrorType,
    ReadAt, WriteAt, StorageBackend,
};
// Note: hal::Sync is NOT re-exported at the crate root to avoid
// confusion with core::marker::Sync. Access it as hal::Sync.
```

**⚠ Pitfall — `hal::Sync` re-export:** Do NOT add `Sync` to the crate root re-exports. The name `graph_db::Sync` would shadow `core::marker::Sync` for users who `use graph_db::*`. Users access durability control via `graph_db::hal::Sync` or through the `StorageBackend` supertrait.

**Verify:**
- `cargo check --no-default-features --features alloc`
- `cargo check`

---

## Phase 1: HAL Error Types (`src/hal/error.rs`)

### 1.1 — Implement StorageErrorKind

Define the enum exactly as specified in `009-hal-trait-design.md` §4.1:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StorageErrorKind {
    OutOfBounds,
    Io,
    ReadOnly,
    StorageFull,
    MediaCorruption,
    Interrupted,
    LockContention,
    Other,
}
```

Add `/// ` doc comments on the enum and every variant, matching the spec text.

Implement `core::fmt::Display` for `StorageErrorKind` — each variant should produce a short human-readable label (e.g., `"out of bounds"`, `"I/O error"`, `"read-only"`, etc.).

**Verify:** `cargo check --no-default-features --features alloc`

### 1.2 — Implement StorageError trait

```rust
pub trait StorageError: core::fmt::Debug + core::fmt::Display {
    fn kind(&self) -> StorageErrorKind;
}
```

Add comprehensive doc comments matching `009-hal-trait-design.md` §4.2. Document object safety and the `Display` bound rationale.

**Verify:** `cargo check --no-default-features --features alloc`

### 1.3 — Implement StorageErrorType trait

```rust
pub trait StorageErrorType {
    type Error: StorageError;
}
```

Add doc comments explaining its role as the shared error-type binder for `ReadAt`, `WriteAt`, and `Sync`.

**Verify:** `cargo check --no-default-features --features alloc`

### 1.4 — Unit tests for StorageErrorKind

In `#[cfg(test)] mod tests` within `src/hal/error.rs`:

- Test `Display` for every `StorageErrorKind` variant (non-empty, reasonable text).
- Test `Clone`, `Copy`, `PartialEq`, `Eq`, `Hash` — construct, clone, compare.
- Test that `StorageErrorKind` is `#[non_exhaustive]` — document this in a comment (cannot be tested at compile time from within the crate, but note it for external consumers).

**Verify:** `cargo test -- hal::error` passes.

---

## Phase 2: HAL Core Traits (`src/hal/traits.rs`)

### 2.1 — Implement ReadAt trait

Define exactly as specified in `009-hal-trait-design.md` §5.1:

```rust
pub trait ReadAt: StorageErrorType {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Self::Error>;
    fn len(&self) -> Result<u64, Self::Error>;
}
```

Add comprehensive doc comments on the trait and both methods, including:
- Concurrency note (`&self` enables concurrent reads)
- Error conditions for each method
- Contract: on success, exactly `buf.len()` bytes are read (no partial reads)
- Object safety note

**⚠ Pitfall:** Use `&self` (not `&mut self`) for `read_at` and `len`. This is a deliberate design choice for concurrent read access.

**Verify:** `cargo check --no-default-features --features alloc`

### 2.2 — Implement WriteAt trait

Define exactly as specified in `009-hal-trait-design.md` §5.2:

```rust
pub trait WriteAt: StorageErrorType {
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<(), Self::Error>;
    fn set_len(&mut self, new_size: u64) -> Result<(), Self::Error>;
}
```

Add comprehensive doc comments, including:
- `&mut self` rationale (single-writer invariant)
- Durability note (writes are NOT durable until `sync_data`/`sync_all`)
- `set_len` behavior: extend with zero-fill, truncate discards, same-size is no-op
- fsync note on `set_len`: caller must use `sync_all()` after extension
- Error conditions
- No `append()` method — reference decision D2

**Verify:** `cargo check --no-default-features --features alloc`

### 2.3 — Implement Sync trait

Define exactly as specified in `009-hal-trait-design.md` §5.3:

```rust
pub trait Sync: StorageErrorType {
    fn sync_data(&mut self) -> Result<(), Self::Error>;
    fn sync_all(&mut self) -> Result<(), Self::Error>;
}
```

Add comprehensive doc comments, including:
- Two-level durability explanation
- When to use `sync_data` vs `sync_all` (file extension → `sync_all`)
- Platform mapping table (Linux → fdatasync/fsync, macOS → F_FULLFSYNC, Windows → FlushFileBuffers)
- In-memory backends: no-op
- `&mut self` rationale (serializes with writes, prevents race in commit protocol)

**⚠ Pitfall — naming:** The trait is named `Sync` within the `hal` module. This shadows `core::marker::Sync` only if someone does `use crate::hal::*`. Within the `hal` module itself, use `self::Sync` if ambiguity arises. The `StorageBackend` blanket impl must reference this as `self::Sync` or `super::Sync` as appropriate. Test that the blanket impl compiles without ambiguity.

**Verify:** `cargo check --no-default-features --features alloc`

### 2.4 — Implement StorageBackend trait and blanket impl

```rust
pub trait StorageBackend: ReadAt + WriteAt + Sync {}

impl<T: ReadAt + WriteAt + Sync> StorageBackend for T {}
```

Here, `Sync` refers to `hal::Sync` (the trait defined in step 2.3), NOT `core::marker::Sync`. The compiler resolves this correctly because the `Sync` in scope within `traits.rs` is the one defined in this module (or imported from `super`).

Add doc comments explaining:
- This is a bundle trait, not an extension point
- Blanket impl means users only implement the three sub-traits
- Object safety
- Usage pattern with `Arc<RwLock<B>>` in the storage engine

**Verify:** `cargo check --no-default-features --features alloc`

### 2.5 — Compile-time object-safety assertions

Add to the test module (or a standalone compile-check in `src/hal/traits.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Verify all traits are object-safe by constructing trait object types.
    // These functions are never called — they only need to compile.
    fn _assert_read_at_object_safe(_: &dyn ReadAt<Error = StorageErrorKind>) {}
    fn _assert_write_at_object_safe(_: &mut dyn WriteAt<Error = StorageErrorKind>) {}
    fn _assert_sync_object_safe(_: &mut dyn Sync<Error = StorageErrorKind>) {}
    fn _assert_storage_backend_object_safe(
        _: &dyn StorageBackend<Error = StorageErrorKind>,
    ) {}
}
```

**⚠ Pitfall:** `StorageErrorKind` may not implement `StorageError` — if so, create a minimal test error type that does, or use a concrete backend error type. The point is to prove the trait is object-safe, not to test a real backend. If `StorageErrorKind` doesn't implement `StorageError`, define a `#[cfg(test)] struct DummyError;` that does.

**Verify:** `cargo test -- hal::traits` compiles (these are compile-time checks, not runtime tests).

---

## Phase 3: HAL Lifecycle Traits (`src/hal/lifecycle.rs`)

### 3.1 — Implement OpenableBackend trait

Define exactly as specified in `009-hal-trait-design.md` §7:

```rust
#[cfg(feature = "std")]
pub trait OpenableBackend: StorageBackend + Sized {
    type Config;

    fn open(config: Self::Config) -> Result<Self, Self::Error>;
    fn create(config: Self::Config) -> Result<Self, Self::Error>;
    fn open_or_create(config: Self::Config) -> Result<Self, Self::Error>
    where
        Self::Config: Clone,
    {
        match Self::open(config.clone()) {
            Ok(backend) => Ok(backend),
            Err(_) => Self::create(config),
        }
    }
}
```

Add comprehensive doc comments including:
- `std`-only rationale
- NOT object-safe (has `Sized` bound) — intentional
- The default `open_or_create` has a TOCTOU race; FileBackend overrides it atomically
- Config associated type explanation

**⚠ Pitfall — conditional compilation:** The `OpenableBackend` and `LockableBackend` trait *definitions* live in `src/hal/lifecycle.rs` but are `#[cfg(feature = "std")]`. Even though `src/hal/` is always compiled, individual items within it can be `std`-gated. The module itself is always present, but the traits within it are conditional.

Ensure the re-export in `src/hal/mod.rs` is also conditional:
```rust
#[cfg(feature = "std")]
pub use lifecycle::{OpenableBackend, LockableBackend};
```

**Verify:**
- `cargo check --no-default-features --features alloc` (lifecycle module exists but traits are absent — no errors)
- `cargo check` (traits present under std)

### 3.2 — Implement LockableBackend trait

Define exactly as specified in `009-hal-trait-design.md` §8:

```rust
#[cfg(feature = "std")]
pub trait LockableBackend: StorageErrorType {
    type LockGuard: Send;

    fn try_lock_exclusive(&self) -> Result<Self::LockGuard, Self::Error>;
}
```

Add comprehensive doc comments including:
- Advisory vs mandatory locking (Unix vs Windows)
- RAII guard pattern
- `Send` bound on guard for cross-thread compatibility
- Non-blocking only (decision D7)
- `&self` receiver (the lock state is external to the backend struct)

**Verify:** `cargo check`

---

## Phase 4: FileBackend Error Type (`src/hal_std/file_backend.rs`)

### 4.1 — Implement FileError enum

Define exactly as specified in `009-hal-trait-design.md` §9.2:

```rust
#[derive(Debug)]
pub enum FileError {
    Io(std::io::Error),
    LockContention,
    OutOfBounds { offset: u64, len: usize, file_size: u64 },
    ReadOnly,
}
```

Add doc comments on the enum and every variant.

Implement:
- `core::fmt::Display` for `FileError` — match each variant with a clear message
- `std::error::Error` for `FileError` — `source()` returns `Some(io_error)` for `Io`, `None` for others
- `hal::StorageError` for `FileError` — map each variant to the appropriate `StorageErrorKind`
- `From<std::io::Error>` for `FileError`

For the `StorageError` impl, map `std::io::ErrorKind::Interrupted` to `StorageErrorKind::Interrupted` and `PermissionDenied` to `StorageErrorKind::ReadOnly`, all others to `StorageErrorKind::Io`. Match spec exactly.

**Verify:** `cargo check`

### 4.2 — Implement StorageErrorType for FileBackend

```rust
impl hal::StorageErrorType for FileBackend {
    type Error = FileError;
}
```

(This can be added now even though `FileBackend` struct is defined in the next step — or define together. Either way, ensure it compiles.)

**Verify:** `cargo check`

---

## Phase 5: FileBackend Core Implementation (`src/hal_std/file_backend.rs`)

### 5.1 — Define FileBackendConfig struct

```rust
#[derive(Debug, Clone)]
pub struct FileBackendConfig {
    pub path: std::path::PathBuf,
    pub read_only: bool,
}
```

Add doc comments on the struct and both fields.

**Verify:** `cargo check`

### 5.2 — Define FileBackend struct

```rust
pub struct FileBackend {
    file: std::fs::File,
    read_only: bool,
}
```

Add doc comments explaining:
- Primary persistent backend
- Uses `pread`/`pwrite` (Unix) and `seek_read`/`seek_write` (Windows)
- Thread safety model: `ReadAt` is `&self` (concurrent reads), `WriteAt`/`Sync` are `&mut self` (exclusive writes)

**Verify:** `cargo check`

### 5.3 — Implement ReadAt for FileBackend

Follow `009-hal-trait-design.md` §9.4 exactly:

- `read_at`: empty buf → early return `Ok(())`. Use `#[cfg(unix)]` with `std::os::unix::fs::FileExt::read_exact_at`. Use `#[cfg(windows)]` with `std::os::windows::fs::FileExt::seek_read` in a loop until buf is full. Map `UnexpectedEof` to `FileError::OutOfBounds`. Include `#[cfg(not(any(unix, windows)))]` with `compile_error!`.
- `len`: use `self.file.metadata()?.len()`.

**⚠ Pitfall — Windows `seek_read` loop:** Windows `seek_read` does NOT guarantee reading all bytes in one call. You MUST loop. If `seek_read` returns 0 before buf is full, return `OutOfBounds`.

**⚠ Pitfall — metadata call in error path:** The `file_size` in `OutOfBounds` error uses `self.file.metadata().map(|m| m.len()).unwrap_or(0)`. This is a best-effort diagnostic — if metadata also fails, report 0 rather than propagating a second error.

**Verify:** `cargo check`

### 5.4 — Implement WriteAt for FileBackend

Follow `009-hal-trait-design.md` §9.5 exactly:

- `write_at`: check `read_only` → `FileError::ReadOnly`. Empty buf → early return. Use `#[cfg(unix)]` with `write_all_at`. Use `#[cfg(windows)]` with `seek_write` in a loop. Include compile_error for unsupported platforms.
- `set_len`: check `read_only`. Delegate to `self.file.set_len(new_size)`.

**⚠ Pitfall — Windows `seek_write` loop:** Same as reads — `seek_write` may return fewer bytes than requested. Loop until all bytes are written. If `seek_write` returns 0, return an `Io` error with `WriteZero` kind.

**⚠ Pitfall — FileBackend does NOT auto-extend:** Unlike MemoryBackend, writing beyond the current file size without first calling `set_len` may produce an OS error (behavior varies by platform). The contract says the caller must call `set_len` first. The implementation does not explicitly check for out-of-bounds writes — it lets the OS report the error naturally.

**Verify:** `cargo check`

### 5.5 — Implement hal::Sync for FileBackend

Follow `009-hal-trait-design.md` §9.6 exactly:

- Both methods: if `read_only`, return `Ok(())` (nothing to sync).

**`sync_data`:**
- `#[cfg(target_os = "macos")]`: Use `libc::fcntl(fd, libc::F_FULLFSYNC)`. macOS's `fsync()` and `fdatasync()` do NOT guarantee data reaches the physical medium — only `F_FULLFSYNC` does.
- `#[cfg(all(unix, not(target_os = "macos")))]`: Use `self.file.sync_data()` (maps to `fdatasync`).
- `#[cfg(windows)]`: Use `self.file.sync_all()` (Windows has no data-only sync).

**`sync_all`:**
- `#[cfg(target_os = "macos")]`: Same as `sync_data` — `F_FULLFSYNC` covers both.
- `#[cfg(all(unix, not(target_os = "macos")))]`: Use `self.file.sync_all()` (maps to `fsync`).
- `#[cfg(windows)]`: Use `self.file.sync_all()`.

Both methods include `#[cfg(not(any(unix, windows)))]` with `compile_error!`.

**⚠ Pitfall — macOS F_FULLFSYNC:** You need `use std::os::unix::io::AsRawFd;` to get the raw fd. The `libc::fcntl` call returns -1 on error; use `std::io::Error::last_os_error()` to capture it.

**⚠ Pitfall — `unsafe` block:** The `libc::fcntl` call requires an `unsafe` block. Add a `// SAFETY:` comment explaining that the file descriptor is valid because `self.file` is an open `File`.

**Verify:** `cargo check`

---

## Phase 6: FileBackend Lifecycle Implementation

### 6.1 — Implement OpenableBackend for FileBackend

Follow `009-hal-trait-design.md` §9.7:

- `open`: Use `OpenOptions::new().read(true).write(!config.read_only).open(&config.path)`. Wrap in `FileBackend`.
- `create`: If `read_only`, return `FileError::ReadOnly`. Use `OpenOptions::new().read(true).write(true).create_new(true).open(&config.path)`.
- `open_or_create`: Override the default (which has a TOCTOU race). If `read_only`, delegate to `open`. Otherwise use `OpenOptions::new().read(true).write(true).create(true).open(&config.path)` — this atomically creates-or-opens.

**Verify:** `cargo check`

### 6.2 — Implement LockableBackend for FileBackend

Follow `009-hal-trait-design.md` §9.8:

**Unix (`#[cfg(unix)]`):**
- Use `libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB)`.
- On failure: if `raw_os_error() == Some(libc::EWOULDBLOCK)`, return `FileError::LockContention`; otherwise return `FileError::Io`.
- On success: return `FileLockGuard { fd }`.

**Windows (`#[cfg(windows)]`):**
- Use `windows_sys::Win32::Storage::FileSystem::LockFileEx` with `LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY`.
- On failure: return `FileError::LockContention`.
- On success: return `FileLockGuard { handle }`.

**⚠ Pitfall — `unsafe` blocks:** Both the Unix `flock` and Windows `LockFileEx` calls require `unsafe`. Add `// SAFETY:` comments.

**Verify:** `cargo check`

### 6.3 — Implement FileLockGuard

Define the RAII guard struct:

```rust
pub struct FileLockGuard {
    #[cfg(unix)]
    fd: std::os::unix::io::RawFd,
    #[cfg(windows)]
    handle: windows_sys::Win32::Foundation::HANDLE,
}
```

- `unsafe impl Send for FileLockGuard {}` — with a SAFETY comment explaining that the fd/handle is valid for the guard's lifetime and flock/LockFile are thread-safe.
- `impl Drop for FileLockGuard` — unlock on drop. Unix: `libc::flock(fd, libc::LOCK_UN)`. Windows: `UnlockFileEx`. Ignore errors in `drop()` (best-effort cleanup).

Add doc comments explaining the RAII pattern and that dropping the guard releases the lock.

**⚠ Pitfall — conditional fields:** The struct has different fields depending on the platform. Both `#[cfg(unix)]` and `#[cfg(windows)]` variants must compile on their respective targets. On unsupported platforms, the struct may be empty or the `LockableBackend` impl may be absent.

**Verify:** `cargo check`

---

## Phase 7: Tests — HAL Trait Definitions

### 7.1 — Test StorageErrorKind Display and equality

In `src/hal/error.rs` tests:

- Every variant's `Display` output is non-empty.
- `StorageErrorKind::Io == StorageErrorKind::Io` (PartialEq).
- `StorageErrorKind::Io != StorageErrorKind::ReadOnly`.
- Clone produces equal values.

**Verify:** `cargo test -- hal::error` passes.

### 7.2 — Test object safety with a mock backend

In `src/hal/traits.rs` tests, create a minimal mock backend:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::hal::error::*;

    #[derive(Debug)]
    struct MockError;

    impl core::fmt::Display for MockError {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            write!(f, "mock error")
        }
    }

    impl StorageError for MockError {
        fn kind(&self) -> StorageErrorKind {
            StorageErrorKind::Other
        }
    }

    struct MockBackend {
        data: Vec<u8>,
    }

    // Implement StorageErrorType, ReadAt, WriteAt, Sync for MockBackend
    // ... (complete implementations)
}
```

Tests:
- Construct `MockBackend`, write bytes, read them back, verify equality.
- Call `len()`, verify correct size.
- `set_len` to expand, verify zero-fill. `set_len` to truncate, verify new length.
- `sync_data()` and `sync_all()` succeed (no-op in mock).
- Verify `MockBackend` satisfies `StorageBackend` bound (blanket impl works).
- Verify `&dyn StorageBackend<Error = MockError>` compiles (object safety).

**Verify:** `cargo test -- hal::traits` passes.

---

## Phase 8: Tests — FileBackend

### 8.1 — Test FileError Display and StorageError mapping

In `src/hal_std/file_backend.rs` tests:

- Construct every `FileError` variant.
- Verify `Display` output is non-empty and descriptive.
- Verify `StorageError::kind()` returns the correct `StorageErrorKind` for each variant.
- Verify `From<std::io::Error>` works.
- Verify `std::error::Error::source()` returns `Some` for `Io`, `None` for others.

**Verify:** `cargo test -- hal_std` passes.

### 8.2 — Test read/write round-trip

Use `tempfile::NamedTempFile` (or `tempfile::tempdir` + manual path) to create a temporary file.

- Create a `FileBackend` via `OpenableBackend::create`.
- `set_len` to, e.g., 4096 bytes.
- Write a known byte pattern at offset 0.
- Write a different pattern at offset 2048.
- Read back both offsets, verify equality.
- Verify `len()` returns 4096.

**Verify:** `cargo test -- hal_std` passes.

### 8.3 — Test out-of-bounds read

- Create a `FileBackend` with a known size (e.g., 100 bytes via `set_len`).
- Attempt to read 50 bytes at offset 80 (would need to read past end).
- Verify the result is `Err` and `kind()` is `StorageErrorKind::OutOfBounds`.

**Verify:** `cargo test -- hal_std` passes.

### 8.4 — Test empty buffer edge cases

- `read_at` with an empty buffer (`&mut []`) at any offset → `Ok(())`.
- `write_at` with an empty buffer at any offset → `Ok(())`.

**Verify:** `cargo test -- hal_std` passes.

### 8.5 — Test read-only mode

- Create a file (with a write-capable backend), write some data, drop the backend.
- Open the same file with `read_only: true`.
- Verify `read_at` works normally.
- Verify `write_at` returns `Err` with `kind() == StorageErrorKind::ReadOnly`.
- Verify `set_len` returns `Err` with `kind() == StorageErrorKind::ReadOnly`.
- Verify `sync_data` and `sync_all` return `Ok(())` (no-op in read-only mode).

**Verify:** `cargo test -- hal_std` passes.

### 8.6 — Test sync operations

- Create a `FileBackend`, write data, call `sync_data()`.
- Write more data, call `sync_all()`.
- Both should return `Ok(())`.
- (Durability cannot be verified in a unit test — we trust the OS. The test verifies the call completes without error.)

**Verify:** `cargo test -- hal_std` passes.

### 8.7 — Test open/create/open_or_create lifecycle

- **Create new file:** `create` with a path that doesn't exist → succeeds.
- **Create existing file:** `create` with a path that already exists → fails (Io error, because `create_new` is used).
- **Open existing file:** `open` with the path from step 1 → succeeds.
- **Open non-existent file:** `open` with a path that doesn't exist → fails.
- **Open-or-create, file doesn't exist:** `open_or_create` → creates file, succeeds.
- **Open-or-create, file exists:** `open_or_create` → opens existing, succeeds.

**Verify:** `cargo test -- hal_std` passes.

### 8.8 — Test file locking

- Create a `FileBackend`.
- Call `try_lock_exclusive()` → should succeed, returns a `FileLockGuard`.
- While holding the guard, verify the backend can still read and write (the lock is on the file, not on the backend's methods).
- Drop the guard (lock released).

**Note:** Testing that a *second* lock attempt fails requires either a second `FileBackend` on the same file (same process, different fd) or a spawned child process. Within the same process, opening a second fd and attempting `flock` should fail with `EWOULDBLOCK` on Unix. Implement this test if feasible; if platform behavior makes it unreliable, document and skip with `#[ignore]` and a comment.

**Verify:** `cargo test -- hal_std` passes.

### 8.9 — Test persistence across open/close

- Create a `FileBackend`, write data at a known offset, `sync_data()`, drop the backend.
- Open a new `FileBackend` on the same path.
- Read back the data, verify it matches what was written.

This is the basic durability smoke test.

**Verify:** `cargo test -- hal_std` passes.

---

## Phase 9: Integration with Existing Crate

### 9.1 — Verify no regressions in existing tests

Run the full test suite to ensure the new modules don't break anything from Task 22 (core types):

```
cargo test
```

All existing tests must still pass.

### 9.2 — Verify no_std compilation

```
cargo check --no-default-features --features alloc
```

The `hal/` module (trait definitions) must compile under `no_std + alloc`. The `hal_std/` module must be absent (gated behind `std` feature).

### 9.3 — Add compile-time assertions to lib.rs

Add to the existing `compile_tests` module in `src/lib.rs`:

```rust
#[cfg(test)]
mod compile_tests {
    // ... existing assertions from Task 22 ...

    // HAL trait object safety
    fn _assert_storage_backend_object_safe<E: crate::hal::StorageError>(
        _: &dyn crate::hal::StorageBackend<Error = E>,
    ) {}

    // FileBackend is StorageBackend
    #[cfg(feature = "std")]
    fn _assert_file_backend_is_storage_backend(
        _: &crate::hal_std::FileBackend,
    ) {
        fn _check<T: crate::hal::StorageBackend>() {}
        _check::<crate::hal_std::FileBackend>();
    }

    // FileBackend is OpenableBackend
    #[cfg(feature = "std")]
    fn _assert_file_backend_is_openable(
        _: &crate::hal_std::FileBackend,
    ) {
        fn _check<T: crate::hal::OpenableBackend>() {}
        _check::<crate::hal_std::FileBackend>();
    }

    // FileBackend is LockableBackend
    #[cfg(feature = "std")]
    fn _assert_file_backend_is_lockable(
        _: &crate::hal_std::FileBackend,
    ) {
        fn _check<T: crate::hal::LockableBackend>() {}
        _check::<crate::hal_std::FileBackend>();
    }
}
```

**Verify:** `cargo test` compiles.

---

## Phase 10: Final Verification

### 10.1 — Full no_std verification

```
cargo check --no-default-features --features alloc
```

Must succeed with zero errors.

### 10.2 — Full std verification

```
cargo check
```

Must succeed with zero errors.

### 10.3 — Full test suite

```
cargo test
```

All tests pass, zero failures.

### 10.4 — Clippy

```
cargo clippy --all-targets --all-features -- -D warnings
```

Zero warnings.

### 10.5 — Documentation

```
cargo doc --no-deps
```

Zero warnings. Every `pub` item has a doc comment.

### 10.6 — Review against design documents

Manually verify:
- Every type in the module layout table in `CLAUDE.md` (project root) §Module Layout Reference for `hal/` and `hal_std/` is defined and in the correct module.
- Trait signatures match `009-hal-trait-design.md` §4–8.
- `FileBackend` implementation matches `009-hal-trait-design.md` §9.
- Error types match `009-hal-trait-design.md` §4 and §9.2.
- `StorageErrorKind` variants match the design (all 8: `OutOfBounds`, `Io`, `ReadOnly`, `StorageFull`, `MediaCorruption`, `Interrupted`, `LockContention`, `Other`).
- `#[non_exhaustive]` is present on `StorageErrorKind`.
- `FileBackend` uses `pread`/`pwrite` equivalent (not seek+read) on Unix.
- macOS `sync_data` uses `F_FULLFSYNC` (not `fdatasync`).
- `FileLockGuard` implements `Send`.
- `FileLockGuard` releases lock on drop.

Document any intentional deviations from the spec in the completion report.

---

## Post-Completion

Produce a completion report following the format in the master project prompt's Instance Rules section. Include:

- The verification evidence from Phase 10
- Resolution of the `hal::Sync` naming question (kept or renamed?)
- Resolution of the `windows-sys` API question (if applicable)
- Any `#[ignore]`d tests with justification
- Any deviations from the design spec with rationale
