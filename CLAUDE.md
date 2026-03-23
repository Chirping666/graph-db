# CLAUDE.md — Project Root

**Project:** Embedded Graph Database with Extensible Schema & Pluggable Inference  
**Crate name:** `graph_db`  
**This file governs Claude Code's behavior for every implementation session (Tasks 22–29).**

---

## Session Workflow

Every Claude Code session follows these steps in order. Do not skip steps.

### 1. Read the design document and relevant project knowledge

Read `012-design-document.md` — the single source of truth for all design decisions. If the task involves a specific subsystem (e.g., file format, HAL, inference), also read the upstream sub-document listed in `012`'s cross-reference index (Section 20). When in doubt about a design question, the design document takes precedence over all other sources.

### 2. Read the scoped CLAUDE.md for the current task

Each implementation task has its own `tasks/<task-dir>/CLAUDE.md`. Read it before doing any work. It specifies the task's objective, required reading, and definition of done.

### 3. Read the checklist.md referenced by the scoped CLAUDE.md

The `checklist.md` is the ordered list of atomic implementation steps. Each item has a verification criterion. This is your work plan — execute it sequentially.

### 4. Review existing code to understand current state

Before writing any code, examine the current state of the repository:
- `src/` — existing module structure
- `Cargo.toml` — current dependencies and feature flags
- Any tests in `tests/` or inline `#[cfg(test)]` modules

Understand what already exists so you build on it correctly, not beside it.

### 5. Create a session plan and confirm with the user

Before implementing, produce a brief plan:
- Which checklist items you will tackle in this session
- Any ambiguities or questions about the checklist
- Any deviations from the design document you anticipate (with justification)

Wait for the user to confirm before proceeding.

### 6. Implement checklist items one at a time

Work through the checklist sequentially. For each item:
1. Implement the code change
2. Ensure it compiles
3. Run the relevant tests (see step 7)
4. Only move to the next item after the current one passes

Do not batch multiple checklist items into a single uncommitted change unless they are tightly coupled (e.g., a type definition and its constructor).

### 7. Run tests after each checklist item

After each checklist item:
- `cargo test` — all tests pass
- `cargo clippy --all-targets --all-features -- -D warnings` — no warnings
- `cargo doc --no-deps` — no documentation warnings
- If the checklist item specifies a `no_std` verification: `cargo check --no-default-features --features alloc`

If a test fails, fix it before moving on. If a clippy warning appears, resolve it. Do not accumulate technical debt within a session.

### 8. Produce a completion report when all items pass

When all checklist items are done and all checks pass, produce a completion report following the format in the master project prompt's Instance Rules section. Include:
- Status (COMPLETE / PARTIAL / BLOCKED)
- Done criterion assessment with evidence
- Deliverables list
- Summary of notable decisions
- Context for the next task
- Residual concerns
- Upstream flags (if any)

---

## Project-Wide Rules

These rules apply to every session, every module, every line of code. Violations must be corrected before a session is considered complete.

### Rule 1: No external database crate dependencies

Do not add dependencies on external database engines, storage libraries, or embedded DB crates (e.g., no `sled`, `redb`, `rocksdb`, `sqlite`, `lmdb`). The entire storage engine is implemented from scratch.

**Allowed dependencies:**
- **Core (`no_std + alloc`):** `crc32fast` only.
- **`std` feature:** `libc` (Unix) / `windows-sys` (Windows) for thin FFI bindings (`pread`, `pwrite`, `flock`, `fdatasync`, `F_FULLFSYNC`).
- **Dev dependencies:** Testing utilities (e.g., `tempfile`) are acceptable.
- **Optional:** `serde` may be offered as an optional feature for user-facing types, but it must not appear in the core serialization path.

General-purpose utility crates are acceptable if they are small, well-maintained, and do not pull in heavy transitive dependency trees. When in doubt, prefer hand-written code.

### Rule 2: `no_std + alloc` for all core code

The following modules must compile under `#![no_std]` with only the `alloc` crate available:
- `types/`
- `schema/`
- `constraint/`
- `inference/`
- `error/` (core error types)
- `hal/` (trait definitions)
- `hal_mem/`

Use `alloc::` imports (`alloc::string::String`, `alloc::vec::Vec`, `alloc::collections::BTreeMap`, `alloc::boxed::Box`) instead of `std::` equivalents in these modules.

The following modules require `std` and are gated behind `#[cfg(feature = "std")]`:
- `db/` (Database, transactions, inference engine)
- `hal_std/` (FileBackend)
- `storage/` (buffer pool, B-tree operations, page management)

**Feature flag structure:**
```toml
[features]
default = ["std"]
std = ["alloc"]
alloc = []
```

**Verification:** Every session must confirm that `cargo check --no-default-features --features alloc` succeeds for the `no_std` modules.

### Rule 3: No baked-in ontology model

The core crate must not hardcode OWL, RDF, RDFS, SKOS, or any specific ontology vocabulary.

- Do not define built-in type names like `"owl:Class"` or `"rdfs:subClassOf"`.
- Do not ship built-in constraint validators or inference rules (except for a minimal test-only example rule if needed for integration testing).
- The type system, constraint system, and inference system provide **mechanism**, not **policy**. All domain-specific semantics come from users or downstream crates via the extension traits (`ConstraintValidator`, `InferenceRule`).

### Rule 4: Documentation on every public item

Every `pub` item — struct, enum, trait, method, function, constant, type alias — must have a doc comment (`///`).

**Method doc comments must include:**
- A one-line summary
- Description of parameters (if not obvious from names/types)
- `# Errors` section listing error conditions
- `# Panics` section (only if the method can panic, and only for programmer-error conditions)
- Performance characteristics when relevant (e.g., "O(log n) B-tree lookup" or "requires one page read")

**Module-level doc comments (`//!`):**
- Explain the module's purpose and its relationship to other modules
- Provide a brief example if the module is a primary entry point

**Crate root documentation:**
- Quick-start example
- Architecture overview
- Feature flag documentation

**Verification:** `cargo doc --no-deps` must produce no warnings.

### Rule 5: Test coverage expectations

**Unit tests:**
- Every public method must have at least one test for the success path.
- Every method that returns `Result` must have at least one test for each documented error condition.
- Edge cases called out in the checklist must have dedicated tests.
- Constructor/validation tests for all core types.

**Integration tests (in `tests/`):**
- End-to-end scenarios: create database → insert data → query → close → reopen → verify data persists.
- Crash recovery scenarios (where applicable).
- Concurrent access scenarios: multiple readers + one writer.
- Extension system round-trip: register validator/inference rule → insert data → validate/infer → verify results.

**Test organization:**
- Unit tests live in `#[cfg(test)] mod tests` within each module.
- Integration tests live in `tests/`.
- Test helpers shared across modules go in a `test_utils` module gated behind `#[cfg(test)]`.

**Verification:** `cargo test` must pass with zero failures. No `#[ignore]` without a documented reason.

### Rule 6: Commit message conventions

```
<type>(<scope>): <short summary>

<optional body>
```

**Types:** `feat`, `fix`, `refactor`, `test`, `docs`, `chore`  
**Scopes:** `types`, `schema`, `constraint`, `inference`, `hal`, `hal-std`, `hal-mem`, `storage`, `db`, `api`, `error`

Examples:
- `feat(types): implement NodeId, EdgeId, TypeId, PropertyKeyId newtypes`
- `feat(hal): define ReadAt, WriteAt, and hal::Sync traits`
- `test(storage): add crash recovery test for dual-superblock commit`
- `fix(db): prevent deadlock in concurrent read transaction creation`
- `docs(api): add quick-start example to crate root`

Each commit should represent a single logical change — typically one checklist item. Do not bundle unrelated changes.

### Rule 7: Code style and conventions

**Naming:**
- Types: `PascalCase`, domain-specific prefixes avoided (`Node` not `GraphNode`).
- Methods: `snake_case`, verb-first for mutations (`insert_node`, `delete_edge`), noun-first for accessors (`type_registry`, `node_count`).
- Transaction constructors: `read_txn()`, `write_txn()`.
- Feature flags: `std`, `alloc` — no proliferation.
- Modules: `lowercase`, underscore-separated for multi-word (`hal_std`, `hal_mem`, `write_buffer`).

**Error handling:**
- All recoverable errors return `Result`. Never panic for recoverable conditions.
- Panics are reserved for programmer errors only (e.g., using a transaction after `commit()`).
- HAL errors use `StorageErrorKind` for generic handling; are type-erased at the public API boundary.
- All public methods return `Result<T, Error>` where `Error` is the crate's unified error enum.

**Serialization:**
- All on-disk serialization is custom binary — no serde, bincode, or CBOR in the storage path.
- Keys: big-endian integers, concatenated (lexicographic order = semantic order).
- Values: little-endian integers. Variable-length fields use 2-byte or 4-byte length prefixes.

**Concurrency:**
- `Database` is `Send + Sync` (internal `Mutex` / `RwLock`).
- `ReadTransaction` and `WriteTransaction` are `!Send`, `!Sync` (hold buffer pool references).
- `ConstraintValidator` and `InferenceRule` require `Send + Sync`.

**Unsafe code:**
- Minimize. Each `unsafe` block must have a `// SAFETY:` comment explaining why it is sound.
- Prefer safe abstractions. If `unsafe` is needed for FFI (e.g., in `hal_std`), isolate it behind safe wrappers.

---

## Module Layout Reference

```
graph_db/
├── lib.rs                  // #![cfg_attr(not(feature = "std"), no_std)]
├── types/                  // Core data types (no_std + alloc)
│   └── mod.rs              // NodeId, EdgeId, TypeId, PropertyKeyId,
│                           // Value, ValueTypeDescriptor, PropertyMap,
│                           // Node, Edge, TypeKind, TypeDefinition,
│                           // PropertyDeclaration
├── schema/                 // Schema traits (no_std + alloc)
│   └── mod.rs              // TypeRegistryView, PropertyKeyRegistryView
├── constraint/             // Constraint traits and types (no_std + alloc)
│   └── mod.rs              // ConstraintValidator, ChangeSet, NodeChange,
│                           // EdgeChange, ConstraintViolation, ViolationSubject
├── inference/              // Inference traits and types (no_std + alloc)
│   └── mod.rs              // InferenceRule, InferredFact, InferenceResult,
│                           // InferenceMode, ProvenanceRecord,
│                           // InferredEntity, MaterializedMapping
├── error/                  // Error types (no_std + alloc core; std extensions)
│   └── mod.rs              // Error, SchemaError, StorageError,
│                           // NotFoundError, TransactionError, InferenceError
├── hal/                    // HAL trait definitions (no_std + alloc)
│   ├── mod.rs
│   ├── error.rs            // StorageErrorKind, StorageError trait,
│   │                       // StorageErrorType
│   ├── traits.rs           // ReadAt, WriteAt, hal::Sync, StorageBackend
│   └── lifecycle.rs        // OpenableBackend, LockableBackend (trait defs)
├── hal_std/                // std persistent backend (std feature only)
│   ├── mod.rs
│   └── file_backend.rs     // FileBackend, FileBackendConfig, FileLockGuard
├── hal_mem/                // In-memory backend (alloc)
│   ├── mod.rs
│   └── memory_backend.rs   // MemoryBackend
├── storage/                // Storage engine internals (std feature)
│   ├── btree/              // B+ tree operations
│   ├── page/               // Page types, headers, serialization
│   ├── buffer_pool.rs      // Buffer pool, clock eviction
│   ├── allocator.rs        // Page allocator, file growth
│   └── serialization.rs    // Property/record serialization
├── db/                     // Database engine (std feature only)
│   ├── config.rs           // DatabaseConfig, StorageMode
│   ├── database.rs         // Database struct
│   ├── read_txn.rs         // ReadTransaction
│   ├── write_txn.rs        // WriteTransaction
│   ├── write_buffer.rs     // WriteBuffer, change tracking
│   ├── schema_cache.rs     // In-memory TypeRegistry, PropertyKeyRegistry
│   └── inference_engine.rs // InferenceEngine, InferenceCache,
│                           // ProvenanceRegistry
```

---

## Key Design References

When implementing, consult these sections of `012-design-document.md`:

| Topic | Section |
|-------|---------|
| Architecture overview & layer diagram | §2 |
| Crate structure & feature flags | §3 |
| Core data model (IDs, Value, Node, Edge) | §4 |
| Type system & schema | §5 |
| Graph storage (B-trees, records, keys) | §6 |
| Single-file format (pages, superblock) | §7 |
| HAL traits | §8 |
| Buffer pool | §9 |
| Concurrency model | §10 |
| Transaction lifecycle | §11 |
| Crash safety & recovery | §12 |
| Constraint validation | §13 |
| Inference hook architecture | §14 |
| Public API surface | §15 |
| Cross-cutting concerns | §16 |
| Design decision log (50+ entries) | §17 |
| Known limitations & deferred work | §18 |
| Authoritative B-tree catalog & Schema Store key map | §19 |

For byte-level format details, also consult:
- `008-file-format-spec.md` — page headers, cell formats, superblock layout, commit protocol
- `007-graph-storage-model.md` — record formats, key encodings, property serialization

---

## Known Residual Concerns

Implementation tasks should be aware of these open items from the design phase:

1. **`hal::Sync` naming conflict with `core::marker::Sync`.** Use module-qualified `hal::Sync` or rename to `DurabilityControl` / `StorageSync` if implementation proves awkward. Resolve during Task 23 (HAL implementation).

2. **Error type for `dyn StorageBackend`.** `Box<dyn StorageError>` works but allocates on the error path. A `BoxedStorageError` wrapper is an alternative. Resolve during Task 23.

3. **`Value` does not implement `Eq`** (due to `f64`). `PartialEq` works correctly for all types except NaN. Documented behavior — no action required for v1.

4. **Deferred secondary freed pages.** 1–3 pages may be temporarily leaked per transaction that triggers freelist B-tree splits. Recovered by `compact()`. Document this in the public API.

5. **`crc32fast` in `no_std + alloc` mode.** Must be verified during Task 23 or 24. If incompatible, a manual CRC32C implementation is acceptable.
