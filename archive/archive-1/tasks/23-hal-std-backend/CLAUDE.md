# CLAUDE.md — Task 23: Implement HAL Trait Layer & std Persistent Backend

**Project:** Embedded Graph Database with Extensible Schema & Pluggable Inference
**Implementation Task:** 23 (preparation task: 15)
**Module:** `src/hal/`, `src/hal_std/`, and additions to `src/lib.rs` and `Cargo.toml`
**Status:** Pending
**Depends on:** Task 22 (core types)

---

## Objective

Implement the complete Hardware Abstraction Layer (HAL) for the embedded graph database:

1. **HAL trait definitions** (`src/hal/`) — the `no_std + alloc` trait hierarchy that all storage backends implement: `StorageErrorKind`, `StorageError` trait, `StorageErrorType`, `ReadAt`, `WriteAt`, `hal::Sync`, `StorageBackend`, `OpenableBackend`, and `LockableBackend`.
2. **std persistent backend** (`src/hal_std/`) — `FileBackend`, the primary durable storage backend using `pread`/`pwrite` on Unix and `ReadFile`/`WriteFile` with explicit offsets on Windows, plus platform-correct `fsync` behavior and advisory file locking.

The in-memory backend (`hal_mem/`) is **out of scope** — that is Task 19/27.

---

## Required Reading

Before writing any code, read these documents from the project knowledge:

| Document | Why |
|----------|-----|
| `012-design-document.md` §8 (HAL), §16 (Cross-cutting) | Authoritative design reference — trait signatures, error types, architecture decisions |
| `009-hal-trait-design.md` (entire document) | **Primary spec** for this task — complete trait code, FileBackend implementation, error propagation chain, fsync discipline, platform mapping, all design decisions (D1–D12) |
| `CLAUDE.md` (project root) | Session workflow, project-wide rules (no_std, naming, testing, docs) |
| `checklist.md` (this directory) | Ordered implementation steps — execute sequentially |

Also skim for context (not primary reading):
- `008-file-format-spec.md` §13 (commit protocol), §15 (fsync discipline) — understand what the storage engine will demand from the HAL
- `005-no-std-hal-patterns.md` §6–7 — background on the design principles

---

## Key Design Decisions to Honor

These decisions are settled. Do not re-litigate them during implementation:

| ID | Decision | Reference |
|----|----------|-----------|
| D1 | `StorageError` requires `Display` (not just `Debug`) | 009 §4.2 |
| D2 | No `append()` method — use `set_len()` + `write_at()` | 009 §5.2 |
| D3 | Trait named `hal::Sync` (not `Flush`); use module qualification to avoid `core::marker::Sync` conflict | 009 §5.3 |
| D4 | `&mut self` for sync methods (serializes with writes) | 009 §5.3 |
| D5 | Blanket impl for `StorageBackend` (users impl 3 sub-traits) | 009 §6 |
| D6 | Lock guard as associated type (RAII) | 009 §8 |
| D7 | Non-blocking lock only (`try_lock_exclusive`) | 009 §8 |
| D8 | `flock()` on Unix (not `fcntl`) | 009 §9.8 |
| D9 | `libc` (Unix) and `windows-sys` (Windows) as thin FFI deps | 009 §9.8 |
| D11 | `MemoryBackend` auto-extends; `FileBackend` does NOT (requires `set_len` first) | 009 §5.2 |
| D12 | Two sync methods: `sync_data` and `sync_all` | 009 §5.3 |

---

## Known Residual Concerns (from design phase)

These must be resolved during this implementation:

1. **`hal::Sync` naming conflict:** If the module-qualified name proves awkward in practice, rename to `DurabilityControl` or `StorageSync`. Document the decision either way.

2. **`windows-sys` API surface:** The design spec sketches the Windows locking path. Verify the exact feature flags and types at implementation time. If `windows-sys` proves too heavyweight or has API drift, a raw FFI binding via `extern "system"` is acceptable.

3. **`crc32fast` in `no_std + alloc`:** Not directly needed by the HAL, but verify compatibility if adding it to `Cargo.toml` in this task (it may be deferred to Task 24).

---

## Definition of Done

All of the following must be true:

- [ ] `src/hal/mod.rs`, `src/hal/error.rs`, `src/hal/traits.rs`, `src/hal/lifecycle.rs` exist and contain all trait definitions from `009-hal-trait-design.md` §4–8
- [ ] `src/hal_std/mod.rs` and `src/hal_std/file_backend.rs` exist with the complete `FileBackend` implementation including `ReadAt`, `WriteAt`, `hal::Sync`, `OpenableBackend`, `LockableBackend`, and `FileLockGuard`
- [ ] `Cargo.toml` includes `libc` (Unix target) and optionally `windows-sys` (Windows target) as `std`-feature-only dependencies
- [ ] `cargo check --no-default-features --features alloc` succeeds (HAL traits compile without std)
- [ ] `cargo check` succeeds (full build with std and FileBackend)
- [ ] `cargo test` passes — all HAL and FileBackend tests green
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` — zero warnings
- [ ] `cargo doc --no-deps` — zero warnings; every `pub` item has a doc comment
- [ ] Tests cover: read/write round-trip, out-of-bounds errors, read-only mode, sync operations, file locking (same process), open/create/open_or_create lifecycle, `StorageErrorKind` mapping
- [ ] Platform-correct fsync: macOS uses `F_FULLFSYNC`, Linux uses `fdatasync`/`fsync`, Windows uses `FlushFileBuffers`
- [ ] All HAL traits are object-safe (compile-time assertion test)
- [ ] `ConstraintValidator` and `InferenceRule` Send+Sync assertions still pass (no regressions from Task 22)

---

## Out of Scope

- `hal_mem/` (MemoryBackend) — Task 19/27
- Buffer pool — Task 16/24
- Storage engine — Task 16/24
- B-tree operations — Task 16/24
- Database struct, transactions — Task 16+/24+
