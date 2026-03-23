# Checklist: Task 27 — Implement In-Memory HAL Backend

**Parent:** Task 19 (this checklist)  
**Implements:** `src/hal_mem/` (MemoryError, MemoryBackend), integration in `src/db/database.rs` for `StorageMode::InMemory`, snapshot helpers, and full-stack equivalence tests.

Execute items in order. After each item, run the verification command(s) listed. Do not proceed until verification passes.

---

## Phase 0: Pre-Implementation Review

### 0.1 — Review existing HAL traits and FileBackend

Before writing any code, read:
- `src/hal/error.rs` — `StorageErrorKind`, `StorageError` trait, `StorageErrorType`
- `src/hal/traits.rs` — `ReadAt`, `WriteAt`, `hal::Sync`, `StorageBackend`
- `src/hal/lifecycle.rs` — `OpenableBackend`, `LockableBackend` (MemoryBackend does NOT implement these)
- `src/hal_std/file_backend.rs` — reference implementation for how a backend satisfies the HAL traits

Note the exact trait signatures, associated types, method names, and error handling patterns. `MemoryBackend` must satisfy the same trait bounds with the same semantics (minus durability).

Also review:
- `src/db/database.rs` — identify how `Database::open()` currently dispatches on `StorageMode` and where `InMemory` support needs to be wired in
- `src/lib.rs` — check whether `pub mod hal_mem;` already exists with `#[cfg(feature = "alloc")]` gating

**Verify:** No code changes — this is a read-only review step. Confirm you understand the trait contracts and the `Database::open` dispatch structure before proceeding.

---

## Phase 1: Module Scaffolding

### 1.1 — Create `src/hal_mem/mod.rs`

Create `src/hal_mem/mod.rs` with:
- `//!` module doc comment explaining the module's purpose: in-memory storage backend for testing, ephemeral databases, and `no_std + alloc` environments
- `pub mod memory_backend;`
- Public re-exports: `pub use memory_backend::{MemoryBackend, MemoryError};`

If `src/hal_mem/` or `src/hal_mem/mod.rs` already exists (created as a placeholder by an earlier task), update it to match the above.

**Verify:** `cargo check --no-default-features --features alloc` succeeds.

### 1.2 — Ensure `hal_mem` is declared in `lib.rs`

In `src/lib.rs`, verify that the following exists:
```rust
#[cfg(feature = "alloc")]
pub mod hal_mem;
```

If it does not exist, add it. Also add or verify the crate-root re-export:
```rust
#[cfg(feature = "alloc")]
pub use hal_mem::{MemoryBackend, MemoryError};
```

**⚠ Pitfall:** `hal_mem` is gated on `alloc`, not `std`. It must compile in `no_std + alloc` environments.

**Verify:**
- `cargo check` succeeds.
- `cargo check --no-default-features --features alloc` succeeds.

---

## Phase 2: Error Type (`src/hal_mem/memory_backend.rs`)

### 2.1 — Implement MemoryError enum

Create `src/hal_mem/memory_backend.rs`. Add the `//!` module doc comment.

Import from the HAL module:
```rust
use crate::hal::{StorageError, StorageErrorKind, StorageErrorType};
```

Use `alloc` imports where needed (though `MemoryError` itself uses no heap allocation).

Define `MemoryError` exactly as specified in `009-hal-trait-design.md` §10.1:

```rust
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
```

**Verify:** `cargo check --no-default-features --features alloc`

### 2.2 — Implement Display for MemoryError

Implement `core::fmt::Display`:
- `OutOfBounds` → format as `"out of bounds: read at offset {offset} with length {requested} exceeds memory size {size}"`

**⚠ Pitfall:** Use `core::fmt`, not `std::fmt`. This is a `no_std` type.

**Verify:** `cargo check --no-default-features --features alloc`

### 2.3 — Implement StorageError trait for MemoryError

Implement the `StorageError` trait:
- `fn kind(&self) -> StorageErrorKind` — `OutOfBounds` maps to `StorageErrorKind::OutOfBounds`

**Verify:** `cargo check --no-default-features --features alloc`

### 2.4 — Conditional std::error::Error for MemoryError

Under `#[cfg(feature = "std")]`, implement `std::error::Error` for `MemoryError`.

`MemoryError` has no `source()` — return `None`.

**Verify:** `cargo check`

### 2.5 — Unit tests for MemoryError

In `#[cfg(test)] mod tests` within `memory_backend.rs`:
- Test construction of `OutOfBounds` variant with specific values.
- Test `Display` output matches the expected format string.
- Test `StorageError::kind()` returns `StorageErrorKind::OutOfBounds`.
- Test `Clone`, `Copy`, `PartialEq`, `Eq` derive behavior.

**Verify:** `cargo test -- memory_backend` passes.

---

## Phase 3: MemoryBackend Struct and Constructors

### 3.1 — Define MemoryBackend struct

```rust
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

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
```

**Verify:** `cargo check --no-default-features --features alloc`

### 3.2 — Implement constructors

Implement on `MemoryBackend`:

- `pub fn new() -> Self` — creates an empty backend (`data: Vec::new()`)
- `pub fn with_size(size: usize) -> Self` — creates backend with `size` zero-bytes (`alloc::vec![0u8; size]`)
- `pub fn from_bytes(data: Vec<u8>) -> Self` — creates backend from existing data (the "load from snapshot in bytes" entry point)
- `pub fn as_bytes(&self) -> &[u8]` — returns the backing data as a slice (the "snapshot to bytes" entry point)
- `pub fn into_bytes(self) -> Vec<u8>` — consumes backend and returns the backing data

Add `/// ` doc comments on each method explaining its purpose and use case.

**Verify:** `cargo check --no-default-features --features alloc`

### 3.3 — Implement Default for MemoryBackend

```rust
impl Default for MemoryBackend {
    fn default() -> Self {
        Self::new()
    }
}
```

This satisfies clippy's `new_without_default` lint.

**Verify:** `cargo clippy --all-targets --all-features -- -D warnings`

### 3.4 — Unit tests for constructors

Test:
- `MemoryBackend::new()` creates backend with empty data (`as_bytes().is_empty()` is true).
- `MemoryBackend::with_size(4096)` creates backend with 4096 zero-bytes.
- `MemoryBackend::from_bytes(vec![1, 2, 3])` stores the provided data.
- `as_bytes()` returns the correct data for each constructor.
- `into_bytes()` returns the correct data and consumes the backend.
- `Default::default()` produces the same result as `new()`.

**Verify:** `cargo test -- memory_backend` passes.

---

## Phase 4: HAL Trait Implementations

### 4.1 — Implement StorageErrorType for MemoryBackend

```rust
impl StorageErrorType for MemoryBackend {
    type Error = MemoryError;
}
```

**Verify:** `cargo check --no-default-features --features alloc`

### 4.2 — Implement ReadAt for MemoryBackend

Follow `009-hal-trait-design.md` §10.3 exactly:

- `read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), MemoryError>`:
  - Empty `buf` → return `Ok(())` immediately (no bounds check)
  - Convert `offset` to `usize`
  - Compute `end = offset.checked_add(buf.len())` — if overflow, return `OutOfBounds`
  - If `end > self.data.len()`, return `OutOfBounds`
  - Copy `self.data[offset..end]` into `buf`

- `len(&self) -> Result<u64, MemoryError>`:
  - Return `Ok(self.data.len() as u64)`

**⚠ Pitfall — `checked_add` for overflow.** On 32-bit platforms, `offset as usize + buf.len()` can overflow. Use `checked_add` and return `OutOfBounds` on `None`.

**⚠ Pitfall — method name.** The HAL trait may use `len()` or `size()` — match the exact name from `src/hal/traits.rs`. The design spec uses `len()`.

**Verify:** `cargo check --no-default-features --features alloc`

### 4.3 — Implement WriteAt for MemoryBackend

Follow `009-hal-trait-design.md` §10.3 exactly:

- `write_at(&mut self, offset: u64, buf: &[u8]) -> Result<(), MemoryError>`:
  - Empty `buf` → return `Ok(())` immediately
  - Convert `offset` to `usize`, compute `end = offset + buf.len()`
  - If `end > self.data.len()`, resize: `self.data.resize(end, 0)` (auto-extend)
  - Copy `buf` into `self.data[offset..end]`

- `set_len(&mut self, new_size: u64) -> Result<(), MemoryError>`:
  - `self.data.resize(new_size as usize, 0)`

**⚠ Pitfall — auto-extend behavior.** This is a deliberate difference from `FileBackend`. `MemoryBackend` grows automatically on write. Document this in the method's doc comment.

**Verify:** `cargo check --no-default-features --features alloc`

### 4.4 — Implement hal::Sync for MemoryBackend

Both methods are no-ops:

```rust
impl hal::Sync for MemoryBackend {
    fn sync_data(&mut self) -> Result<(), MemoryError> {
        Ok(())
    }

    fn sync_all(&mut self) -> Result<(), MemoryError> {
        Ok(())
    }
}
```

Add `/// ` doc comments noting that sync is a no-op because in-memory writes are immediately visible.

**⚠ Pitfall — `hal::Sync` vs `core::marker::Sync`.** Use the fully qualified path or a module import alias if the naming conflict was resolved during Task 23. Check how `FileBackend` handles this import.

**Verify:** `cargo check --no-default-features --features alloc`

### 4.5 — Verify StorageBackend bound is satisfied

`StorageBackend` is defined as a supertrait alias combining `ReadAt + WriteAt + hal::Sync + StorageErrorType`. After implementing all four traits, verify that `MemoryBackend` satisfies `StorageBackend`.

If `StorageBackend` is an empty trait with supertraits, add:
```rust
impl StorageBackend for MemoryBackend {}
```

If `StorageBackend` is a blanket impl, no explicit impl is needed. Check the definition in `src/hal/traits.rs`.

Add a compile-time assertion in tests:
```rust
fn _assert_storage_backend(_: &dyn StorageBackend<Error = MemoryError>) {}
```

**⚠ Pitfall:** If `StorageBackend` is not object-safe (e.g., due to associated types), use a generic bound assertion instead:
```rust
fn _assert_backend<B: StorageBackend>() {}
fn _check() { _assert_backend::<MemoryBackend>(); }
```

**Verify:** `cargo check --no-default-features --features alloc`

### 4.6 — Unit tests for ReadAt

Test:
- Read from a backend created with `from_bytes(vec![1, 2, 3, 4, 5])`.
  - Read bytes 0..3 → `[1, 2, 3]`
  - Read bytes 2..5 → `[3, 4, 5]`
  - Read all bytes 0..5 → `[1, 2, 3, 4, 5]`
- Empty buffer read returns `Ok(())` regardless of offset.
- Out-of-bounds read (offset beyond data length) returns `MemoryError::OutOfBounds`.
- Out-of-bounds read (offset valid but offset + len exceeds data length) returns `OutOfBounds`.
- Read from empty backend returns `OutOfBounds` for any non-empty buffer.
- `len()` returns correct size.
- `len()` returns 0 for an empty backend.

**Verify:** `cargo test -- memory_backend` passes.

### 4.7 — Unit tests for WriteAt

Test:
- Write to offset 0 of empty backend → backend auto-extends.
- Write to offset 10 of empty backend → backend grows to 10 + write length, bytes 0..10 are zeros.
- Overwrite existing data at offset 0.
- Write beyond current length → auto-extend and zeros fill the gap.
- `set_len` to larger → backend grows, new bytes are zero.
- `set_len` to smaller → backend truncates.
- `set_len` to same size → no-op.
- Empty buffer write returns `Ok(())` and does not change data.

**Verify:** `cargo test -- memory_backend` passes.

### 4.8 — Unit tests for hal::Sync

Test:
- `sync_data()` returns `Ok(())`.
- `sync_all()` returns `Ok(())`.
- Calling sync after writes does not alter the data.

**Verify:** `cargo test -- memory_backend` passes.

---

## Phase 5: Snapshot Helpers

### 5.1 — Implement save_to_file (std-only)

Under `#[cfg(feature = "std")]`:

```rust
impl MemoryBackend {
    /// Save the current contents to a file.
    ///
    /// This writes the raw byte contents of the in-memory storage
    /// to the specified path. The resulting file is a valid database
    /// file and can be opened with `FileBackend`.
    ///
    /// # Errors
    /// Returns `std::io::Error` if the file cannot be written.
    ///
    /// # Note
    /// This operation is NOT atomic. If the process crashes during
    /// the write, the file may be incomplete or corrupt.
    pub fn save_to_file(&self, path: &std::path::Path) -> std::io::Result<()> {
        std::fs::write(path, &self.data)
    }
}
```

**Verify:** `cargo check`

### 5.2 — Implement load_from_file (std-only)

Under the same `#[cfg(feature = "std")]` impl block:

```rust
    /// Load contents from a file into a new in-memory backend.
    ///
    /// This reads the entire file into memory. The file should be
    /// a valid database file (e.g., one previously saved with
    /// `save_to_file` or created by `FileBackend`).
    ///
    /// # Errors
    /// Returns `std::io::Error` if the file cannot be read.
    pub fn load_from_file(path: &std::path::Path) -> std::io::Result<Self> {
        let data = std::fs::read(path)?;
        Ok(MemoryBackend { data })
    }
```

**Verify:** `cargo check`

### 5.3 — Unit tests for snapshot helpers

Test (requires `std`, use `tempfile` crate or `std::env::temp_dir()`):
- Create a `MemoryBackend` with known data (e.g., `from_bytes(vec![0xDE, 0xAD, 0xBE, 0xEF])`).
- Save to a temp file with `save_to_file`.
- Load from the same file with `load_from_file`.
- Verify loaded data equals original data (`as_bytes()` comparison).
- Verify the file on disk contains the exact bytes.
- Test `load_from_file` with a nonexistent path returns an error.
- Test `save_to_file` to a read-only directory returns an error (if easily testable on the platform).

**Verify:** `cargo test -- memory_backend` passes.

---

## Phase 6: Database Integration

### 6.1 — Wire MemoryBackend into Database::open()

In `src/db/database.rs` (or wherever the `Database::open` method dispatches on `StorageMode`):

When `StorageMode::InMemory` is selected:
1. Create a new `MemoryBackend` (empty via `MemoryBackend::new()` or pre-sized with `MemoryBackend::with_size()`)
2. Initialize it as a fresh database — the same initialization logic used to create a new persistent database file (write superblock, create initial B-tree root pages, etc.)
3. Skip file locking — do NOT call `LockableBackend::try_lock()` or equivalent
4. Pass the `MemoryBackend` to the storage engine constructor just like `FileBackend`

**⚠ Pitfall — type erasure.** If the storage engine is generic over `B: StorageBackend`, this may already work. If it uses `dyn StorageBackend`, verify that `MemoryBackend` can be boxed into the same dynamic type. Check how `FileBackend` is passed to the storage engine and follow the same pattern.

**⚠ Pitfall — initialization path.** The fresh-database initialization must be exactly the same as for `FileBackend` when creating a new database file. If `Database::open` currently detects "new file" by checking if the file is empty / doesn't exist, the in-memory path needs an equivalent: "backend is empty (len == 0), so initialize."

**⚠ Pitfall — config fields.** `DatabaseConfig::in_memory()` sets `extension_startup_check: false` by default (the API spec says so — there's no previously persisted extension list to check against for a fresh in-memory DB). Respect this default.

**Verify:**
- `cargo check` succeeds.
- The following minimal test compiles and runs:
  ```rust
  let db = Database::open(DatabaseConfig::in_memory())?;
  let _txn = db.read_txn()?;
  ```

### 6.2 — Basic in-memory database smoke test

Write a test (in `src/db/` tests or an integration test in `tests/`) that:
1. Opens an in-memory database.
2. Begins a write transaction.
3. Registers a type (e.g., a node type "Person").
4. Registers a property key (e.g., "name").
5. Inserts a node with the type and a property.
6. Commits the transaction.
7. Begins a read transaction.
8. Reads the node back and verifies its type and property.

This is the minimum "it works" test.

**Verify:** `cargo test -- in_memory` or appropriate test name passes.

---

## Phase 7: Full-Stack Equivalence Tests

These tests verify that the complete database functionality works identically through the `MemoryBackend`. Each test performs a sequence of operations on an in-memory database and verifies the results. Where feasible, the test also performs the same operations on a persistent database (using `FileBackend` with a temp file) and asserts identical results.

### 7.1 — Schema operations equivalence test

Test with in-memory database:
- Register multiple node types with a type hierarchy (e.g., "Entity" → "Person", "Entity" → "Organization").
- Register multiple edge types (e.g., "knows", "works_at").
- Register multiple property keys with different `ValueTypeDescriptor`s.
- Query types back: `type_definition()`, `all_type_definitions()`.
- Query property keys back: `property_key()`, `all_property_keys()`.
- Verify subtype relationships resolve correctly.

**Verify:** `cargo test -- equivalence` or appropriate test name passes.

### 7.2 — Node and edge CRUD equivalence test

Test with in-memory database:
- Insert multiple nodes with different types and properties.
- Insert edges between nodes.
- Read nodes and edges back by ID.
- Update node properties.
- Update edge properties.
- Delete an edge.
- Delete a node (verify cascading edge deletion).
- Verify counts (`node_count`, `edge_count`).

**Verify:** `cargo test` passes.

### 7.3 — Query and traversal equivalence test

Test with in-memory database:
- Build a small graph (8+ nodes, 10+ edges, 3+ edge types).
- Query nodes by type (with and without subtype inclusion).
- Query edges by type.
- Query outgoing and incoming edges for a specific node.
- Perform a multi-hop traversal (4+ hops, 3+ edge types).
- Verify results match expected values.

**Verify:** `cargo test` passes.

### 7.4 — Constraint validation equivalence test

Test with in-memory database:
- Register a custom `ConstraintValidator` (reuse or duplicate the test validator from Task 25/26 tests).
- Attempt a write transaction that violates the constraint.
- Verify that `commit()` fails with `Error::ConstraintViolation`.
- Attempt a write transaction that satisfies the constraint.
- Verify that `commit()` succeeds.

**Verify:** `cargo test` passes.

### 7.5 — Inference equivalence test

Test with in-memory database:
- Register a custom `InferenceRule` (reuse or duplicate the minimal test rule from Task 26 tests).
- Insert base data.
- Invoke `run_inference` with the rule.
- Verify inferred results are returned correctly.
- If materialization is supported: verify materialized facts appear in subsequent reads.

**Verify:** `cargo test` passes.

### 7.6 — Concurrent access test

Test with in-memory database:
- Spawn multiple threads (or use scoped threads).
- Each thread opens a read transaction and performs queries.
- One thread opens a write transaction, inserts data, commits.
- Verify no panics, no data corruption, readers see consistent snapshots.

**⚠ Pitfall:** Use `Arc<Database>` to share across threads. Transactions are `!Send`, so each thread must create its own.

**Verify:** `cargo test` passes.

---

## Phase 8: Snapshot Round-Trip Tests

### 8.1 — In-memory snapshot round-trip

Test:
1. Create an in-memory database.
2. Insert a variety of data (types, property keys, nodes, edges).
3. Obtain a reference to the `MemoryBackend` (this may require adding a method to `Database`, or use `save_to_file` as the test interface).
4. Save snapshot to a temp file via `save_to_file`.
5. Create a new `MemoryBackend` via `load_from_file`.
6. Open a new `Database` with this loaded backend (if the API supports passing a pre-loaded `MemoryBackend`; alternatively, open the temp file with `DatabaseConfig::persistent(path)`).
7. Read all data back and verify it matches what was inserted.

**⚠ Pitfall — accessing the backend.** The `Database` may not expose its internal backend directly. If `Database` does not provide a `snapshot_to_file` or similar method, the test may need to:
  - Use `save_to_file` through whatever API is available, or
  - Create a `MemoryBackend` directly, initialize it, write data through transactions, then call `save_to_file` on the backend before wrapping it in `Database`.

If no clean API path exists for snapshot, document the limitation in the completion report and test via file interop (see 8.2).

**Verify:** `cargo test` passes.

### 8.2 — Snapshot interoperability: in-memory → persistent

Test:
1. Create an in-memory database and insert data.
2. Snapshot to a temp file.
3. Open the temp file as a persistent database (`DatabaseConfig::persistent(path)`).
4. Read all data back and verify it matches.

This verifies that the raw bytes produced by `MemoryBackend` are a valid database file.

**Verify:** `cargo test` passes.

### 8.3 — Snapshot interoperability: persistent → in-memory

Test:
1. Create a persistent database at a temp path and insert data.
2. Close the database (drop it).
3. Load the database file into a `MemoryBackend` via `load_from_file`.
4. Open a new database with the loaded backend.
5. Read all data back and verify it matches.

This verifies that a `FileBackend`-written file can be loaded into `MemoryBackend`.

**Verify:** `cargo test` passes.

---

## Phase 9: Final Verification

### 9.1 — Full no_std verification

```
cargo check --no-default-features --features alloc
```

Must succeed with zero errors. The `hal_mem` module must compile without `std`.

### 9.2 — Full std verification

```
cargo check
```

Must succeed with zero errors.

### 9.3 — Full test suite

```
cargo test
```

All tests pass, zero failures.

### 9.4 — Clippy

```
cargo clippy --all-targets --all-features -- -D warnings
```

Zero warnings.

### 9.5 — Documentation

```
cargo doc --no-deps
```

Zero warnings. Every `pub` item in `src/hal_mem/` has a doc comment.

### 9.6 — Review against design documents

Manually verify:
- `MemoryError` matches `009-hal-trait-design.md` §10.1 (variants, derives, Display format).
- `MemoryBackend` struct matches §10.2 (field, constructors, doc comments).
- `ReadAt` impl matches §10.3 (overflow check, empty-buffer short-circuit, error cases).
- `WriteAt` impl matches §10.3 (auto-extend, empty-buffer short-circuit).
- `hal::Sync` impl matches §10.3 (no-op).
- Snapshot helpers match §10.4 (`save_to_file`, `load_from_file`, std-only gating).
- `MemoryBackend` does NOT implement `OpenableBackend` or `LockableBackend`.
- `Database::open(DatabaseConfig::in_memory())` works and produces a fully functional database.
- The full database stack (schema, CRUD, queries, traversals, constraints, inference) works through the in-memory backend.

Document any intentional deviations from the spec in the completion report.

---

## Post-Completion

Produce a completion report following the format in the master project prompt's Instance Rules section. Include the verification evidence from Phase 9.
