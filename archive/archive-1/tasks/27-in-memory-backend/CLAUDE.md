# CLAUDE.md — Task 27: Implement In-Memory HAL Backend

**Project:** Embedded Graph Database with Extensible Schema & Pluggable Inference  
**Implementation Task:** 27 (preparation task: 19)  
**Module:** `src/hal_mem/` (primary), integration points in `src/db/database.rs`  
**Status:** Pending  
**Depends on:** Task 26 (inference hook infrastructure), which depends on Task 25 (query & traversal engine)  
**Preparation depends on:** Task 12 (`012-design-document.md`), Task 18 (`tasks/26-inference-hooks/`)

---

## Orientation

This is Task 27, the implementation of the in-memory HAL backend. Within the project's hierarchy, this is one task in a 4-phase, 29-task project. Sibling implementation tasks are 22 (core types), 23 (HAL + std backend), 24 (storage engine), 25 (query engine), 26 (inference hooks), 28 (integration testing), 29 (docs/publish). Task 28 depends on this task's output.

The in-memory backend is a **secondary operating mode** where the entire database lives in RAM, backed by a `Vec<u8>`. It implements the same HAL traits (`ReadAt`, `WriteAt`, `hal::Sync`, `StorageBackend`) as the `FileBackend`, so the entire storage engine above the HAL runs identically without any special-casing. The backend includes optional snapshot-to-disk and load-from-disk capabilities for convenience, but durability is not guaranteed — data is lost when the `Database` is dropped unless explicitly snapshotted.

By the time this task begins, the following should already be implemented:

- **Core types** (Task 22): All types in `src/types/`, `src/schema/`, `src/constraint/`, `src/inference/`, `src/error/`
- **HAL trait definitions** (Task 23): `ReadAt`, `WriteAt`, `hal::Sync`, `StorageBackend`, `StorageErrorType`, `StorageErrorKind` in `src/hal/`
- **std persistent backend** (Task 23): `FileBackend` in `src/hal_std/`
- **Storage engine** (Task 24): Page management, buffer pool, B-trees, WAL, allocator in `src/storage/`
- **Query engine** (Task 25): `Database`, `ReadTransaction`, `WriteTransaction`, all query operations in `src/db/`
- **Inference hooks** (Task 26): `InferenceEngine`, `InferenceCache`, `ProvenanceRegistry` in `src/db/inference_engine.rs`

This task implements the `MemoryBackend` in `src/hal_mem/` and wires `Database::open()` to use it when `StorageMode::InMemory` is specified. It then verifies that the full database stack — schema registration, node/edge CRUD, queries, traversals, constraint validation, inference — works identically through the in-memory backend.

---

## Required Reading

Before writing any code, read these documents from the project knowledge. Read them in the order listed — later documents build on earlier ones.

1. **`012-design-document.md`** — The single source of truth. Focus on:
   - §2 (Architecture overview) — understand the layer diagram and where `hal_mem` sits
   - §3 (Crate structure & feature flags) — `hal_mem/` is `alloc`-gated, not `std`-gated
   - §8 (HAL traits) — the trait contracts `MemoryBackend` must satisfy
   - §15 (Public API surface) — `DatabaseConfig::in_memory()` and `StorageMode::InMemory`

2. **`009-hal-trait-design.md`** — The authoritative HAL specification. **Section 10 is the primary reference for this task** — it contains the complete `MemoryBackend` design including:
   - §10.1: `MemoryError` error type
   - §10.2: `MemoryBackend` struct and constructors (`new()`, `with_size()`, `from_bytes()`, `as_bytes()`, `into_bytes()`)
   - §10.3: Trait implementations (`StorageErrorType`, `ReadAt`, `WriteAt`, `hal::Sync`)
   - §10.4: Snapshot helpers (`save_to_file`, `load_from_file`) — `std`-only

3. **`010-api-surface-spec.md`** — The public API. Relevant sections:
   - §5.1: `DatabaseConfig::in_memory()` constructor and `StorageMode::InMemory`
   - §5.2: `Database::open()` — must handle `InMemory` mode

4. **Existing code in `src/hal/`** — Read the implemented HAL trait definitions to understand the exact trait signatures, associated types, and method contracts that `MemoryBackend` must satisfy.

5. **Existing code in `src/hal_std/file_backend.rs`** — Study the `FileBackend` implementation as a reference for how a backend implements the HAL traits. `MemoryBackend` follows the same patterns but is much simpler (no OS I/O, no locking).

6. **Existing code in `src/db/database.rs`** — Understand how `Database::open()` currently creates a `FileBackend` for `StorageMode::Persistent`, and identify where the `InMemory` path needs to be wired in.

---

## Key Design Decisions (from upstream specs)

These decisions are already made. Do not revisit them — implement as specified.

1. **`MemoryBackend` is `no_std + alloc`.** It lives in `src/hal_mem/`, gated behind `#[cfg(feature = "alloc")]`, not `#[cfg(feature = "std")]`. The core `MemoryBackend` type works without `std`. Only the `save_to_file`/`load_from_file` snapshot helpers require `std`.

2. **Backed by `Vec<u8>`.** The entire database image is a contiguous byte vector. This is the simplest approach and sufficient for v1. A sparse `BTreeMap<PageId, Box<[u8]>>` is a potential future optimization but is out of scope.

3. **Auto-extend on write.** `WriteAt::write_at()` automatically resizes the `Vec` if the write extends beyond the current length. This differs from `FileBackend`, which requires an explicit `set_len()` before writing beyond the file size.

4. **`hal::Sync` is a no-op.** In-memory writes are immediately visible. Both `sync_data()` and `sync_all()` return `Ok(())`.

5. **`MemoryBackend` does NOT implement `LockableBackend`.** File locking is meaningless for in-memory storage. The `Database` must skip the locking step when using `MemoryBackend`.

6. **Snapshot is a convenience, not a durability guarantee.** `save_to_file` writes the raw `Vec<u8>` to a file. `load_from_file` reads it back. The resulting file is a valid database file that can also be opened with `FileBackend`. Snapshot is not atomic — if the process crashes during `save_to_file`, the file may be corrupt or incomplete.

7. **No code above the HAL should special-case in-memory vs. persistent.** The entire storage engine, buffer pool, B-tree layer, and database engine operate identically regardless of backend. The HAL abstraction is complete.

---

## Implementation Scope

### In Scope

- `src/hal_mem/mod.rs` — module declaration and re-exports
- `src/hal_mem/memory_backend.rs` — `MemoryError`, `MemoryBackend`, all trait implementations, snapshot helpers
- Wiring in `src/db/database.rs` (or wherever `Database::open` dispatches on `StorageMode`) to use `MemoryBackend` when `StorageMode::InMemory` is selected
- Unit tests for `MemoryBackend` (all trait operations, edge cases, error paths)
- Snapshot round-trip tests (save → load → verify data intact)
- Full-stack equivalence tests verifying that the complete database functionality works through the in-memory backend (schema, CRUD, queries, traversals, constraints, inference)
- Re-exports of `MemoryBackend` from the crate root

### Out of Scope

- Modifying HAL trait definitions — those are locked in Task 23
- Modifying the storage engine — it operates identically regardless of backend
- Modifying the query engine or inference engine — they are backend-agnostic
- Integration testing across all subsystems — Task 28
- Documentation and publish preparation — Task 29

---

## Implementation Notes and Pitfalls

1. **Feature gating.** `hal_mem` is gated on `alloc`, not `std`. The `mod hal_mem` declaration in `lib.rs` should already exist with `#[cfg(feature = "alloc")]`. If it doesn't, add it. The snapshot helpers (`save_to_file`, `load_from_file`) use `std::fs` and `std::path` and must be gated behind `#[cfg(feature = "std")]` within the module.

2. **`ReadAt::read_at(&self, ...)` takes `&self`.** This is important for concurrent reads. Since `MemoryBackend` stores data in a `Vec<u8>`, immutable reads are safe. The storage engine wraps backends in `RwLock` for write exclusivity, so `MemoryBackend` itself does not need internal synchronization.

3. **Integer overflow in `read_at`.** Use `checked_add` for `offset + buf.len()` to prevent wrapping on 32-bit platforms. Return `MemoryError::OutOfBounds` on overflow.

4. **Empty buffer edge case.** Both `read_at` and `write_at` should return `Ok(())` immediately for empty buffers without any bounds checks.

5. **`StorageBackend` supertrait bound.** `StorageBackend` is defined as `ReadAt + WriteAt + hal::Sync + StorageErrorType`. The `MemoryBackend` must implement all four. There is no blanket impl — you must explicitly implement `StorageBackend` for `MemoryBackend` (which may be a marker impl if `StorageBackend` has no additional methods, or may be auto-provided if defined as a supertrait alias).

6. **Database initialization for in-memory mode.** When `Database::open()` receives `StorageMode::InMemory`, it must:
   - Create a new `MemoryBackend` (empty or pre-sized)
   - Initialize it as a fresh database (write the superblock, create initial B-tree roots, etc.) — using the same initialization path as creating a new persistent database file
   - Skip file locking (no `LockableBackend`)
   - Pass the backend to the storage engine just like `FileBackend`

7. **Snapshot interoperability.** A snapshot saved from `MemoryBackend` must be openable by `FileBackend` and vice versa. This is guaranteed by the architecture (the raw bytes are identical), but a test should verify it.

8. **`no_std` compilation check.** After implementing `MemoryBackend`, verify that `cargo check --no-default-features --features alloc` still succeeds. The `hal_mem` module must not pull in any `std` dependencies in its core path.

9. **`MemoryBackend` does NOT implement `OpenableBackend`.** `OpenableBackend` is for backends that need configuration and can fail on open (like file I/O). `MemoryBackend` is constructed directly via `new()`, `with_size()`, or `from_bytes()`.

10. **Full-stack test scope.** The equivalence tests must cover at minimum: type registration, property key registration, node insert/read/update/delete, edge insert/read/update/delete, multi-hop traversal, constraint validation, inference rule execution, and (if the API supports it) snapshot → load → verify data survives the round-trip.

---

## Definition of Done

All of the following must be true before this task is complete:

- [ ] `src/hal_mem/mod.rs` exists with module declaration and public re-exports
- [ ] `src/hal_mem/memory_backend.rs` implements `MemoryError` with `StorageError` trait impl
- [ ] `MemoryBackend` struct implemented with `new()`, `with_size()`, `from_bytes()`, `as_bytes()`, `into_bytes()`
- [ ] `MemoryBackend` implements `StorageErrorType`, `ReadAt`, `WriteAt`, `hal::Sync`
- [ ] `MemoryBackend` satisfies `StorageBackend` (all supertrait bounds met)
- [ ] `save_to_file` and `load_from_file` implemented behind `#[cfg(feature = "std")]`
- [ ] `Database::open()` creates and uses `MemoryBackend` when `StorageMode::InMemory` is specified
- [ ] `MemoryBackend` re-exported from crate root (under `alloc` feature gate)
- [ ] Unit tests pass for all `ReadAt` / `WriteAt` / `hal::Sync` operations including edge cases (empty buffer, out-of-bounds read, auto-extend write, overflow check)
- [ ] Snapshot round-trip test passes: create in-memory DB → insert data → snapshot to file → load from file → verify data intact
- [ ] Snapshot interoperability test: in-memory snapshot → open with `FileBackend` (or vice versa) → verify data readable
- [ ] Full-stack equivalence test passes: same sequence of operations (type registration, CRUD, queries, traversals, constraint validation, inference) produces identical results through both `MemoryBackend` and `FileBackend`
- [ ] `cargo check` succeeds
- [ ] `cargo check --no-default-features --features alloc` succeeds (hal_mem compiles without std)
- [ ] `cargo test` passes — all tests green
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` — zero warnings
- [ ] `cargo doc --no-deps` — zero warnings; every `pub` item in `src/hal_mem/` has a doc comment
