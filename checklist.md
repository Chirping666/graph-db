# checklist.md — Architectural Hardening

**Status:** Pending

This checklist brings the codebase into conformance with architectural principles A1–A6
defined in the project root `CLAUDE.md`. It also adds defense-in-depth hardening, new API
surface (`Value::total_eq`, `try_write_txn`), and fuzz targets.

Execute items in order. Each step has a verification command — do not proceed until
it passes. After completing each step and passing its verification, mark it done by
changing `- [ ]` to `- [x]` in this file.

---

## Required Reading

Before writing any code, read these files:

1. **Project root `CLAUDE.md`** — All project-wide rules and architectural principles.
2. **`030-workspace-redesign.md`** (in `archive/archive-2/`) — Architectural source of
   truth for the workspace structure and design decisions R1–R17.
3. **`crates/phonograph_db/src/sync.rs`** — Current sync primitive re-exports.
4. **`crates/phonograph_db/src/lib.rs`** — Current re-exports and module structure.
5. **`crates/phonograph_std/src/lib.rs`** — Current re-exports, `AnyBackend` usage,
   convenience functions.
6. **`crates/phonograph_std/src/any_backend.rs`** — The `AnyBackend` enum to be removed.
7. **`crates/phonograph_std/src/backend_std/file_backend.rs`** — Current `LockableBackend`
   implementation, `FileLockGuard`, platform-specific `unsafe` code.
8. **`crates/phonograph_db/src/backend/`** — Backend trait definitions (`ReadAt`, `WriteAt`,
   `Durability`, `StorageBackend`, `BackendError`).
9. **`crates/phonograph_db/src/storage/buffer_pool.rs`** — Buffer pool implementation.
10. **`crates/phonograph_db/src/storage/page/overflow.rs`** — Overflow page chain reading.
11. **`crates/phonograph/src/types/mod.rs`** — `Value` enum definition.
12. **`crates/phonograph_db/src/db/database.rs`** — `Database<B>` struct, `write_txn()`.
13. **`audits/2026-03-26-codebase-audit.md`** (in `archive/archive-1/audits/`) — Prior
    audit findings, especially §3 (safety, error handling, panics).

## Done When

1. `sync.rs` uses `std::sync` when `std` is active, `spin` only on `no_std`.
2. `AnyBackend` is removed. Convenience functions return concrete types.
3. All `pub use` re-exports between crates are removed. All imports are explicit.
4. `LockableBackend` trait is unconditional (not `std`-gated), fully abstract.
5. `compile_error!` on unsupported platforms in `phonograph_std`.
6. `MAX_OVERFLOW_CHAIN_LENGTH` is defined and enforced in overflow reading.
7. `Value::total_eq()` exists using `f64::total_cmp` semantics.
8. `try_write_txn(timeout)` exists on `Database<B>` (behind `std` feature).
9. Fuzz targets exist for page parsing and superblock validation.
10. All tests pass. Baseline count maintained or increased.
11. All 13 verification checks from `CLAUDE.md` pass.

## Key Pitfalls

1. **`std::sync::Mutex::lock()` returns `Result`; `spin::Mutex::lock()` returns the
   guard directly.** The sync wrapper uses `.expect("mutex poisoned")` on the `std`
   path — a poisoned mutex means a prior panic, which is unrecoverable.

2. **Removing `AnyBackend` changes return types of `open()` and `open_in_memory()`.**
   All test/example code using `phonograph_std::Database` must be updated. The
   `DatabaseExt::save_to_file` method needs redesign — make it an inherent method on
   `Database<MemoryBackend>` instead.

3. **Removing re-exports breaks all downstream imports.** Every import that resolved
   through the re-export chain must be rewritten. Use `grep -rn` to find all affected
   sites before starting.

4. **`LockableBackend` is currently `#[cfg(feature = "std")]`.** Making it unconditional
   means it must not reference any `std` types. `OpenableBackend` (which uses
   `std::path::Path`) stays `std`-gated; only `LockableBackend` is ungated.

5. **`try_write_txn` needs `std::time::Duration` and `Instant`.** Must be
   `#[cfg(feature = "std")]`-gated.

6. **Fuzz targets require `cargo-fuzz` and nightly.** The fuzz crate is NOT a workspace
   member. Just create the harness and verify it compiles.

7. **`Value::total_eq` must handle all `Value` variants**, not just `Float`. Delegate
   to `PartialEq` for non-float variants.

---

## Phase 0: Setup

- [x] **0.1 — Install the new project root `CLAUDE.md`.**
  Replace the existing `CLAUDE.md` at the project root with the new version that
  contains architectural principles A1–A6 and the updated verification checklist.

  **Verify:** `CLAUDE.md` at the project root contains the `## Architectural Principles`
  section with A1–A6.

- [x] **0.2 — Record the current test baseline.**
  ```bash
  cargo test --workspace 2>&1 | tail -5
  cargo clippy --workspace --all-targets -- -D warnings
  ```
  Record the exact test count (expected: 474 pass, 3 ignored, 0 failures).
  This is the regression baseline.
  **Verify:** Zero failures, zero clippy warnings.

- [x] **0.3 — Record the current import structure.**
  ```bash
  grep -rn 'pub use phonograph' crates/phonograph_db/src/lib.rs crates/phonograph_std/src/lib.rs
  grep -rn 'use phonograph_std::' crates/phonograph_std/tests/ crates/phonograph_std/examples/
  grep -rn 'use phonograph_db::' crates/phonograph_std/tests/ crates/phonograph_std/examples/
  grep -rn 'use phonograph::' crates/phonograph_std/tests/ crates/phonograph_std/examples/
  ```
  Save the output for reference when updating imports in Phases 3 and 4.
  **Verify:** Output saved.

---

## Phase 1: Sync Primitive Abstraction

> **Implements:** CLAUDE.md principle A4

- [x] **1.1 — Rewrite `crates/phonograph_db/src/sync.rs`.**
  Replace the current unconditional spin re-exports with a conditional module:

  ```rust
  //! Synchronization primitives — uses OS-native primitives under `std`,
  //! falls back to `spin` on `no_std`.

  pub(crate) use alloc::sync::Arc;

  #[cfg(feature = "std")]
  mod impl_std {
      pub(crate) use std::sync::MutexGuard;
      pub(crate) use std::sync::RwLockReadGuard;
      pub(crate) use std::sync::RwLockWriteGuard;

      pub(crate) struct Mutex<T>(std::sync::Mutex<T>);

      impl<T> Mutex<T> {
          pub const fn new(value: T) -> Self {
              Self(std::sync::Mutex::new(value))
          }

          pub fn lock(&self) -> MutexGuard<'_, T> {
              self.0.lock().expect("mutex poisoned")
          }

          pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
              self.0.try_lock().ok()
          }
      }

      pub(crate) struct RwLock<T>(std::sync::RwLock<T>);

      impl<T> RwLock<T> {
          pub const fn new(value: T) -> Self {
              Self(std::sync::RwLock::new(value))
          }

          pub fn read(&self) -> RwLockReadGuard<'_, T> {
              self.0.read().expect("rwlock poisoned")
          }

          pub fn write(&self) -> RwLockWriteGuard<'_, T> {
              self.0.write().expect("rwlock poisoned")
          }
      }
  }

  #[cfg(not(feature = "std"))]
  mod impl_spin {
      pub(crate) use spin::Mutex;
      pub(crate) use spin::MutexGuard;
      pub(crate) use spin::RwLock;
      pub(crate) use spin::RwLockReadGuard;
      pub(crate) use spin::RwLockWriteGuard;
  }

  #[cfg(feature = "std")]
  pub(crate) use impl_std::*;

  #[cfg(not(feature = "std"))]
  pub(crate) use impl_spin::*;
  ```

  **Verify:**
  ```bash
  cargo check -p phonograph_db
  cargo check -p phonograph_db --no-default-features
  ```

- [x] **1.2 — Verify all existing sync usage compiles and tests pass.**
  ```bash
  cargo test --workspace
  ```
  **Verify:** All tests pass, same count as baseline.

### ▸ Phase 1 Gate

- [ ] **Phase 1 gate:**
  ```bash
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  cargo check -p phonograph_db --no-default-features
  ```
  All pass.

---

## Phase 2: Remove Dynamic Dispatch (`AnyBackend`)

> **Implements:** CLAUDE.md principle A2

- [ ] **2.1 — Inventory all `AnyBackend` usage.**
  ```bash
  grep -rn 'AnyBackend' crates/phonograph_std/src/
  grep -rn 'AnyBackend' crates/phonograph_std/tests/
  grep -rn 'AnyBackend' crates/phonograph_std/examples/
  ```
  List and categorize every site: type alias, enum construction, enum matching, re-export.

  **Verify:** List is complete.

- [ ] **2.2 — Replace convenience functions with concrete return types.**
  In `crates/phonograph_std/src/lib.rs`, replace:
  ```rust
  pub type Database = phonograph_db::db::Database<AnyBackend>;
  ```
  With:
  ```rust
  /// A database backed by a persistent file.
  pub type FileDatabase = phonograph_db::db::Database<backend_std::FileBackend>;

  /// A database backed by in-memory storage.
  pub type MemoryDatabase = phonograph_db::db::Database<phonograph_db::backend_mem::MemoryBackend>;
  ```

  Update `open()` / `open_with_config()` → return `FileDatabase`.
  Update `open_in_memory()` → return `MemoryDatabase`.
  Remove the `ReadTransaction` / `WriteTransaction` type aliases bound to `AnyBackend`.

  **⚠ `DatabaseExt::save_to_file`:** Make it an inherent method on `MemoryDatabase`
  instead of a trait method that matched on enum variants.

  **Verify:** `cargo check -p phonograph_std` passes.

- [ ] **2.3 — Delete `any_backend.rs` and clean up.**
  Delete `crates/phonograph_std/src/any_backend.rs`. Remove `mod any_backend;` and
  `pub use any_backend::*;` from `lib.rs`. Remove `AnyBackendError` and all impls.

  **Verify:** `cargo check -p phonograph_std` passes.

- [ ] **2.4 — Update all tests and examples.**
  Update every test/example in `crates/phonograph_std/tests/` and `examples/` to use
  concrete types. Replace `phonograph_std::Database` with `FileDatabase` or
  `MemoryDatabase`. Update helper functions like `open_temp_db()`.

  **Verify:** `cargo test --workspace` — all tests pass.

### ▸ Phase 2 Gate

- [ ] **Phase 2 gate:**
  ```bash
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  grep -rn 'AnyBackend' crates/
  ```
  All pass. Grep returns empty.

---

## Phase 3: Remove Re-exports

> **Implements:** CLAUDE.md principle A3

- [ ] **3.1 — Remove `pub use phonograph::*` from `phonograph_db/src/lib.rs`.**
  Delete the line. Fix all broken internal imports — change `use crate::types::*` to
  `use phonograph::types::*` etc.

  **⚠ Doc-tests** that used `use phonograph_db::types::Value` must change to
  `use phonograph::types::Value`.

  **Verify:** `cargo check -p phonograph_db` passes.

- [ ] **3.2 — Remove `pub use phonograph_db::*` from `phonograph_std/src/lib.rs`.**
  Delete the line and the R14 comment. Fix all broken internal imports.

  **Verify:** `cargo check -p phonograph_std` passes.

- [ ] **3.3 — Update all tests to use explicit imports.**
  For every test file in `crates/phonograph_std/tests/`, rewrite imports:
  - Core vocabulary → `use phonograph::`
  - Database engine → `use phonograph_db::`
  - Platform types → `use phonograph_std::`

  **Verify:** `cargo test --workspace` — all tests pass.

- [ ] **3.4 — Update all examples to use explicit imports.**
  Same approach for `crates/phonograph_std/examples/`.

  **Verify:**
  ```bash
  cargo run -p phonograph_std --example basic_usage
  cargo run -p phonograph_std --example owl_lite_ontology
  ```

- [ ] **3.5 — Update doc-tests across all three crates.**
  ```bash
  cargo test --workspace --doc
  ```
  Fix any failures by updating import paths in `///` doc comments.

  **Verify:** `cargo test --workspace --doc` — zero failures.

### ▸ Phase 3 Gate

- [ ] **Phase 3 gate:**
  ```bash
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  cargo doc --workspace --no-deps
  grep -rn 'pub use phonograph::' crates/phonograph_db/src/lib.rs
  grep -rn 'pub use phonograph_db::' crates/phonograph_std/src/lib.rs
  ```
  All pass. Both greps return empty.

---

## Phase 4: Locking Trait Generalization

> **Implements:** CLAUDE.md principle A1

- [ ] **4.1 — Audit current `LockableBackend` trait definition.**
  Read `crates/phonograph_db/src/backend/` and determine: is `LockableBackend`
  `#[cfg(feature = "std")]`-gated? Does it reference `std` types? What about
  `OpenableBackend`? Document findings.

  **Verify:** Audit complete.

- [ ] **4.2 — Make `LockableBackend` unconditional.**
  Remove `#[cfg(feature = "std")]` from the trait. Ensure it contains NO references
  to `std::path`, `std::fs`, `libc`, or OS-specific types. `OpenableBackend` stays
  `std`-gated.

  **Verify:** `cargo check -p phonograph_db --no-default-features` passes.

- [ ] **4.3 — Implement `LockableBackend` for `MemoryBackend`.**
  ```rust
  pub struct MemoryLockGuard;

  impl LockableBackend for MemoryBackend {
      type LockGuard = MemoryLockGuard;
      fn try_lock_exclusive(&self) -> Result<MemoryLockGuard, Self::Error> {
          Ok(MemoryLockGuard)
      }
  }
  ```

  **Verify:** `cargo check -p phonograph_db --no-default-features` passes.

- [ ] **4.4 — Verify `FileBackend`'s impl still compiles.**
  **Verify:** `cargo check -p phonograph_std` passes.

- [ ] **4.5 — Add `compile_error!` for unsupported platforms.**
  In `crates/phonograph_std/src/backend_std/file_backend.rs` at module level:
  ```rust
  #[cfg(not(any(unix, windows)))]
  compile_error!(
      "phonograph_std requires Unix or Windows. \
       For other platforms, implement StorageBackend and LockableBackend \
       for your platform's I/O primitives in a separate crate."
  );
  ```

  **Verify:** `cargo check -p phonograph_std` passes.

### ▸ Phase 4 Gate

- [ ] **Phase 4 gate:**
  ```bash
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  cargo check -p phonograph_db --no-default-features
  grep -r "flock\|LockFileEx\|F_FULLFSYNC\|libc::" crates/phonograph_db/src/
  ```
  All pass. Grep returns empty.

---

## Phase 5: Defense-in-Depth Hardening

> **Implements:** CLAUDE.md principle A5

- [ ] **5.1 — Add `MAX_OVERFLOW_CHAIN_LENGTH` constant.**
  In `crates/phonograph_db/src/storage/page/overflow.rs`:
  ```rust
  /// Maximum overflow pages in a chain. Prevents infinite loops on corrupt data.
  pub const MAX_OVERFLOW_CHAIN_LENGTH: usize = 16_384;
  ```

  **Verify:** `cargo check -p phonograph_db` passes.

- [ ] **5.2 — Enforce the limit in `read_chain`.**
  Add a counter. If it exceeds `MAX_OVERFLOW_CHAIN_LENGTH`, return `StorageError`.

  **Verify:** `cargo test --workspace` — all existing tests pass.

- [ ] **5.3 — Add a unit test for overflow chain length enforcement.**
  Simulate a cyclic or excessively long chain. Verify `read_chain` returns an error.

  **Verify:** `cargo test -p phonograph_db -- overflow` passes.

### ▸ Phase 5 Gate

- [ ] **Phase 5 gate:**
  ```bash
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  ```

---

## Phase 6: API Additions

- [ ] **6.1 — Add `Value::total_eq(&self, &other) -> bool`.**
  In `crates/phonograph/src/types/mod.rs`. Uses `f64::total_cmp` for floats,
  delegates to `PartialEq` for other variants. Also add `property_map_total_eq()`.

  **Verify:** `cargo check -p phonograph` and `cargo doc -p phonograph --no-deps` pass.

- [ ] **6.2 — Add unit tests for `Value::total_eq`.**
  Test: `NaN == NaN` → true, `0.0 != -0.0`, `Float != Integer` (different variants),
  property map comparison.

  **Verify:** `cargo test -p phonograph -- total_eq` passes.

- [ ] **6.3 — Add `WriteLockTimeout` variant to `TransactionError`.**
  In `crates/phonograph_db/src/error/mod.rs`. Update `Display`.

  **Verify:** `cargo check -p phonograph_db` passes.

- [ ] **6.4 — Add `try_write_txn` to `Database<B>`.**
  `#[cfg(feature = "std")]` method. Polling loop with `try_lock()` and
  `thread::sleep(Duration::from_micros(100))`, checking `Instant::elapsed()`.

  **Verify:** `cargo check -p phonograph_db` and `cargo check -p phonograph_db --no-default-features` both pass.

- [ ] **6.5 — Add tests for `try_write_txn`.**
  1. No contention → succeeds.
  2. Timeout under contention → `WriteLockTimeout`.
  3. Lock released in time → succeeds.

  **Verify:** `cargo test --workspace -- try_write_txn` passes.

### ▸ Phase 6 Gate

- [ ] **Phase 6 gate:**
  ```bash
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  cargo doc --workspace --no-deps
  ```

---

## Phase 7: Fuzz Targets

> **Implements:** CLAUDE.md principle A5

- [ ] **7.1 — Create fuzz directory structure.**
  Create `fuzz/Cargo.toml` and `fuzz/fuzz_targets/`. The fuzz crate must NOT be in the
  workspace `members` list — add `"fuzz"` to `exclude` if needed.

  **Verify:** Directory structure exists.

- [ ] **7.2 — Create fuzz targets.**
  One file per target in `fuzz/fuzz_targets/`:
  - `leaf_page_parse.rs`
  - `interior_page_parse.rs`
  - `overflow_page_parse.rs`
  - `superblock_validation.rs`

  Each: `#![no_main]`, `fuzz_target!(|data: &[u8]| { let _ = parse(data); })`.

  **⚠ Visibility:** If parse functions are `pub(crate)`, make them `pub`.

  **Verify:** All four files exist.

- [ ] **7.3 — Verify fuzz targets compile.**
  ```bash
  cd fuzz && cargo check
  ```
  If nightly is unavailable, document it — do not block on this.

  **Verify:** Compiles (or documented as needing nightly).

### ▸ Phase 7 Gate

- [ ] **Phase 7 gate:** `cargo test --workspace` still passes.

---

## Phase 8: Documentation & Metadata Updates

- [ ] **8.1 — Update `README.md`.**
  Update Quick Start to use explicit imports. Remove `AnyBackend` mentions.
  Document `try_write_txn`.

  **Verify:** README.md is accurate.

- [ ] **8.2 — Update `CHANGELOG.md`.**
  Add:
  ```markdown
  ### Changed
  - Sync primitives use `std::sync` when `std` feature is active (fixes priority
    inversion). `spin` is used only on `no_std`.
  - `AnyBackend` removed — convenience functions return concrete `FileDatabase` and
    `MemoryDatabase` types.
  - Re-exports removed — import types from the crate that defines them.
  - `LockableBackend` trait is now unconditional (not `std`-gated).

  ### Added
  - `Value::total_eq()` for deterministic float comparison.
  - `property_map_total_eq()` helper for comparing property maps.
  - `Database::try_write_txn(timeout)` for non-blocking write lock acquisition.
  - `MAX_OVERFLOW_CHAIN_LENGTH` and enforcement in overflow page reading.
  - Fuzz targets for page parsing and superblock validation.
  - `compile_error!` on unsupported platforms in `phonograph_std`.

  ### Fixed
  - Priority inversion under `std` due to unconditional `spin` mutex usage.
  - Potential infinite loop on corrupt overflow page chains.
  ```

  **Verify:** CHANGELOG.md is well-formatted.

### ▸ Phase 8 Gate

- [ ] **Phase 8 gate:** `cargo doc --workspace --no-deps` — zero warnings.

---

## Phase 9: Final Verification

- [ ] **9.1 — Full workspace build, test, lint, docs.**
  ```bash
  cargo build --workspace
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  cargo doc --workspace --no-deps
  ```
  All pass with zero warnings.

- [ ] **9.2 — `no_std` verification.**
  ```bash
  cargo check -p phonograph --no-default-features
  cargo check -p phonograph_db --no-default-features
  ```

- [ ] **9.3 — Regression test count.**
  Compare against Phase 0 baseline. All previously passing tests still pass.
  New tests appear in the count.

- [ ] **9.4 — Run all 13 verification checks from `CLAUDE.md`.**
  Execute every command from the Verification Checklist table. All 13 must pass.

  **Verify:** All 13 pass.

- [ ] **9.5 — Examples still run.**
  ```bash
  cargo run -p phonograph_std --example basic_usage
  cargo run -p phonograph_std --example owl_lite_ontology
  ```

### ▸ Phase 9 Gate — COMPLETE

- [ ] **All verification checks pass.**
  Write a completion report to `completion-report.md` at the project root, documenting:
  - Status
  - What was changed (summary of each phase)
  - Superseded design decisions (R9, R13, R14)
  - New architectural principles installed (A1–A6)
  - Test count before and after
  - Files modified
  - Residual concerns (if any)
