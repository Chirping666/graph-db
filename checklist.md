# checklist.md — Workspace Redesign Migration

**Design document:** `030-workspace-redesign.md`  
**Governs:** `CLAUDE.md` (session workflow)  
**Status:** Pending

Execute items in order. Each step has a verification command — do not proceed until
it passes. Steps marked **⚠ WORKSPACE BROKEN** indicate that `cargo check --workspace`
will fail until a later gate restores it; use per-crate checks in those intervals.

---

## Phase 0: Pre-Migration Snapshot

- [ ] **0.1 — Baseline verification.** Confirm the workspace compiles and all tests pass
  before making any changes.
  ```bash
  cargo test --workspace
  cargo clippy --workspace --all-targets --all-features -- -D warnings
  ```
  Record the test count (expected: 473+ tests, 60+ doc-tests). This is the regression
  baseline — every test that passes here must still pass at the end.
  **Verify:** Zero failures. Note the exact counts.

- [ ] **0.2 — Workspace Cargo.toml: switch to explicit resolver.** Add `resolver = "2"`
  to the workspace `[workspace]` table if not already present. This is required for the
  new three-crate layout.
  **Verify:** `cargo check --workspace` still passes.

---

## Phase 1: Rename `graph_db_core` → `phonograph` + Strip Non-Vocabulary Modules

> **Reference:** `030-workspace-redesign.md` §5, §8, §14 Phase 1

**⚠ WORKSPACE BROKEN after this phase.** The `graph_db` crate depends on modules that
will be removed from `phonograph`. The workspace will not compile again until Phase 2
restores those modules in `phonograph_db`. Use per-crate checks during this phase.

- [ ] **1.1 — Rename the directory and update crate metadata.**
  - Rename `crates/graph_db_core/` → `crates/phonograph/`.
  - Update `crates/phonograph/Cargo.toml`: set `name = "phonograph"`, update `description`,
    `keywords`, and `categories` per `030` §5.
  - Update workspace root `Cargo.toml`: change `members` to include `crates/phonograph`
    instead of `crates/graph_db_core`.
  - Update `graph_db`'s `Cargo.toml`: change the dependency from `graph_db_core` to
    `phonograph = { path = "crates/phonograph" }`.
  - Update all `use graph_db_core::` imports in `graph_db/src/` to `use phonograph::`.
  - **Verify:** `cargo check -p phonograph` passes.

- [ ] **1.2 — Remove backend modules from `phonograph`.**
  - Delete `crates/phonograph/src/backend/` directory.
  - Delete `crates/phonograph/src/backend_mem/` directory.
  - Remove `pub mod backend;` and `pub mod backend_mem;` from `crates/phonograph/src/lib.rs`.
  - Remove all backend-related re-exports from `lib.rs` (`ReadAt`, `WriteAt`, `Durability`,
    `StorageBackend`, `StorageErrorKind`, `StorageErrorType`, `MemoryBackend`, `MemoryError`).
  - **Do not delete the source files permanently yet** — copy them to a temporary staging
    location or recover from git history in Phase 2.
  - **Verify:** `cargo check -p phonograph --no-default-features --features alloc` passes.

- [ ] **1.3 — Strip database-specific error types from `phonograph`.**
  In `crates/phonograph/src/error/mod.rs`:
  - Remove the `StorageError` struct.
  - Remove the `TransactionError` enum.
  - Remove the unified `Error` enum.
  - Remove all `From` impls that convert into the unified `Error`.
  - Keep `SchemaError`, `NotFoundError`, `InferenceError` and their `Display` impls.
  - Update `lib.rs` re-exports to only export the kept error types.
  - **Verify:** `cargo check -p phonograph --no-default-features --features alloc` passes.

- [ ] **1.4 — Verify `phonograph` is a clean vocabulary crate.**
  - Confirm zero non-dev dependencies in `crates/phonograph/Cargo.toml`.
  - Confirm no storage/backend/transaction concepts leaked through:
    ```bash
    grep -r "ReadAt\|WriteAt\|StorageBackend\|MemoryBackend\|StorageError\|TransactionError" crates/phonograph/src/
    ```
    Must return empty.
  - Confirm feature flags match `030` §12:
    ```toml
    [features]
    default = ["std"]
    std = ["alloc"]
    alloc = []
    ```
  - **Verify:** All three checks pass. `phonograph` is self-contained.

### ▸ Phase 1 Gate

- [ ] **Phase 1 gate — all must pass:**
  ```bash
  cargo check -p phonograph
  cargo check -p phonograph --no-default-features --features alloc
  cargo test -p phonograph
  cargo doc -p phonograph --no-deps
  ```
  The workspace as a whole will NOT compile yet — `graph_db` has broken imports.
  This is expected and will be resolved in Phase 2.

---

## Phase 2: Create `phonograph_db` Crate

> **Reference:** `030-workspace-redesign.md` §6, §8, §9, §10, §11, §14 Phase 2

This is the largest phase. It creates the database engine crate, moves code into it,
swaps sync primitives, and generifies `Database<B>`.

- [ ] **2.1 — Scaffold `phonograph_db` crate.**
  - Create `crates/phonograph_db/` with `Cargo.toml` and `src/lib.rs`.
  - `Cargo.toml` per `030` §6: depend on `phonograph`, `spin`, `hashbrown`, `crc32fast`,
    `xxhash-rust`. Feature flags per `030` §12.
  - `src/lib.rs`: add `#![cfg_attr(not(feature = "std"), no_std)]` and `extern crate alloc`.
  - Add `phonograph_db` to workspace `members` in root `Cargo.toml`.
  - **Verify:** `cargo check -p phonograph_db` passes (empty crate).

- [ ] **2.2 — Move backend traits and in-memory backend into `phonograph_db`.**
  - Restore `backend/` and `backend_mem/` from the pre-Phase-1 state (via `git` or staging)
    into `crates/phonograph_db/src/`.
  - Rename the `StorageError` *trait* (in `backend/`) to `BackendError` per decision R17.
  - Update all references to the old trait name.
  - Update imports: `use graph_db_core::` → `use crate::` or `use phonograph::` as appropriate.
  - Wire up `pub mod backend;` and `pub mod backend_mem;` in `phonograph_db`'s `lib.rs`.
  - **Verify:** `cargo check -p phonograph_db` passes.

- [ ] **2.3 — Move storage engine into `phonograph_db`.**
  - Move `graph_db/src/storage/` → `crates/phonograph_db/src/storage/`.
  - Update all imports in the moved files: `use crate::` paths, `use phonograph::` for
    core types, remove any `use graph_db_core::` or `use graph_db::` references.
  - Wire up `pub mod storage;` in `phonograph_db`'s `lib.rs`.
  - **Verify:** `cargo check -p phonograph_db` passes.

- [ ] **2.4 — Move database engine into `phonograph_db`.**
  - Move `graph_db/src/db/` → `crates/phonograph_db/src/db/`.
  - Update all imports in the moved files.
  - Wire up `pub mod db;` in `phonograph_db`'s `lib.rs`.
  - This will NOT compile yet — `db/` depends on `std::sync`, `std::collections::HashMap`,
    `AnyBackend`, and `PathBuf`/`StorageMode`. Those are fixed in the next steps.
  - **Verify:** Deferred to 2.8.

- [ ] **2.5 — Create the `phonograph_db` error module.**
  - Create `crates/phonograph_db/src/error/mod.rs`.
  - Define `StorageError` struct (moved from old `graph_db_core`).
  - Define `TransactionError` enum (moved from old `graph_db_core`).
  - Define the unified `Error` enum, referencing `phonograph::SchemaError`,
    `phonograph::NotFoundError`, `phonograph::InferenceError` for the vocabulary variants.
  - Implement `Display`, `From` conversions, and conditional `std::error::Error`.
  - Wire up `pub mod error;` in `phonograph_db`'s `lib.rs`.
  - **Verify:** Deferred to 2.8.

- [ ] **2.6 — Create `sync.rs` and swap sync primitives.**
  - Create `crates/phonograph_db/src/sync.rs` per `030` §10:
    ```rust
    pub(crate) use spin::Mutex;
    pub(crate) use spin::MutexGuard;
    pub(crate) use spin::RwLock;
    pub(crate) use spin::RwLockReadGuard;
    pub(crate) use spin::RwLockWriteGuard;
    pub(crate) use alloc::sync::Arc;
    ```
  - Replace all `use std::sync::{Mutex, RwLock, Arc, ...}` in `db/` and `storage/`
    with `use crate::sync::*`.
  - Replace all `use std::collections::HashMap` with `use hashbrown::HashMap`.
  - **Verify:** Deferred to 2.8.

- [ ] **2.7 — Generify `Database<B>` and split `DatabaseConfig`.**
  - Make `Database`, `DatabaseInner`, `ReadTransaction`, `WriteTransaction` generic
    over `B: StorageBackend` per `030` §9.
  - Remove `AnyBackend` from `phonograph_db` (it moves to `phonograph_std` in Phase 3).
  - Split `DatabaseConfig`: keep only engine-level fields (`page_size`,
    `buffer_pool_frames`, `inference_cache_size`, `application_id`). Remove `PathBuf`,
    `StorageMode`, and any `std::path` references.
  - Rewrite `Database::open` / `Database::create` to accept a backend `B` directly
    instead of constructing one internally from config.
  - **Verify:** Deferred to 2.8.

- [ ] **2.8 — Add re-exports and compile `phonograph_db`.**
  - Add `pub use phonograph::*;` re-export in `phonograph_db`'s `lib.rs` (decision R14).
  - Add convenience re-exports for the database facade types.
  - Resolve any remaining compile errors from the moves and refactors.
  - **Verify:**
    ```bash
    cargo check -p phonograph_db
    cargo check -p phonograph_db --no-default-features --features alloc
    ```
    Both commands pass. This is the critical gate — the database engine compiles
    under `no_std + alloc`.

### ▸ Phase 2 Gate

- [ ] **Phase 2 gate — all must pass:**
  ```bash
  cargo check -p phonograph
  cargo check -p phonograph --no-default-features --features alloc
  cargo check -p phonograph_db
  cargo check -p phonograph_db --no-default-features --features alloc
  ```
  The workspace still won't fully compile because `graph_db` (the old top-level
  crate) has had its `src/storage/`, `src/db/`, and `src/backend_std/` gutted.
  Phase 3 addresses this.

---

## Phase 3: Create `phonograph_std` Crate

> **Reference:** `030-workspace-redesign.md` §7, §14 Phase 3

- [ ] **3.1 — Scaffold `phonograph_std` crate.**
  - Create `crates/phonograph_std/` with `Cargo.toml` and `src/lib.rs`.
  - `Cargo.toml` per `030` §7: depend on `phonograph` and `phonograph_db`.
    Add `libc` under `[target.'cfg(unix)'.dependencies]`.
  - No feature flags — this crate is always `std`.
  - Add `phonograph_std` to workspace `members`.
  - **Verify:** `cargo check -p phonograph_std` passes (empty crate).

- [ ] **3.2 — Move `FileBackend` and create `AnyBackend`.**
  - Move `graph_db/src/backend_std/` → `crates/phonograph_std/src/backend_std/`.
  - Update imports to reference `phonograph_db::backend::*` for the backend traits.
  - Create `crates/phonograph_std/src/any_backend.rs` — move the `AnyBackend` enum
    from the old `db/database.rs`. It now wraps `FileBackend` and
    `phonograph_db::backend_mem::MemoryBackend`.
  - **Verify:** `cargo check -p phonograph_std` passes.

- [ ] **3.3 — Add convenience API and re-exports.**
  In `phonograph_std`'s `lib.rs`:
  - `pub use phonograph::*;` and `pub use phonograph_db::*;` (decision R14).
  - `pub type Database = phonograph_db::Database<AnyBackend>;`
  - `pub fn open(path: impl AsRef<std::path::Path>) -> Result<Database, Error>`
  - `pub fn open_in_memory() -> Result<Database, Error>`
  - Create `FileConfig` struct per `030` §9 (`path`, `read_only`, `engine: DatabaseConfig`).
  - **Verify:** `cargo check -p phonograph_std` passes.

### ▸ Phase 3 Gate

- [ ] **Phase 3 gate — all must pass:**
  ```bash
  cargo check -p phonograph
  cargo check -p phonograph_db
  cargo check -p phonograph_std
  ```
  All three crates compile. The workspace may still not build because `graph_db`
  (the old root crate) is now a gutted shell. Phase 4 fixes that.

---

## Phase 4: Retire or Facade `graph_db`

> **Reference:** `030-workspace-redesign.md` §14 Phase 4

- [ ] **4.1 — Decide: facade or delete.** Confirm choice with the user before proceeding.
  - **Option A (facade):** Replace `graph_db`'s `src/lib.rs` with a single re-export:
    `pub use phonograph_std::*;`. Update `graph_db`'s `Cargo.toml` to depend only on
    `phonograph_std`. Remove `crc32fast`, `xxhash-rust`, `libc` — those now belong to
    their respective crates.
  - **Option B (delete):** Remove `graph_db` from the workspace entirely. Move `tests/`,
    `examples/`, `fuzz/`, `README.md`, `CHANGELOG.md`, and licenses to the workspace root
    (or into `phonograph_std`). Update the workspace `Cargo.toml`.

- [ ] **4.2 — Migrate tests and examples.**
  - Update all `use graph_db::` imports in `tests/` and `examples/` to use the new
    crate paths (`use phonograph_std::*` if using the facade, or direct crate imports).
  - Ensure doc-tests in all three crates compile.
  - Move integration tests to the appropriate crate or to the workspace `tests/` root.
  - **Verify:** `cargo test --workspace` passes.

- [ ] **4.3 — Update workspace-level metadata.**
  - Update root `Cargo.toml` workspace members list.
  - Update `exclude` patterns (now just `"archive/"`, `"fuzz/"`, etc.).
  - Update `repository` URL if still placeholder.
  - **Verify:** `cargo check --workspace` passes.

### ▸ Phase 4 Gate

- [ ] **Phase 4 gate — all must pass:**
  ```bash
  cargo check --workspace
  cargo test --workspace
  ```
  The workspace compiles and all tests pass. This is the first time since Phase 1
  that the full workspace is healthy.

---

## Phase 5: Final Verification

> **Reference:** `030-workspace-redesign.md` §17

Run the complete verification checklist. Every item must pass.

- [ ] **5.1 — `phonograph` isolation checks.**
  ```bash
  cargo check -p phonograph --no-default-features --features alloc
  grep -r "ReadAt\|WriteAt\|StorageBackend\|StorageError\|TransactionError" crates/phonograph/src/
  ```
  First command passes. Second command returns empty.
  Inspect `crates/phonograph/Cargo.toml` — zero non-dev dependencies.

- [ ] **5.2 — `phonograph_db` no\_std checks.**
  ```bash
  cargo check -p phonograph_db --no-default-features --features alloc
  grep -r "use std::" crates/phonograph_db/src/
  ```
  First command passes. Second returns empty or only `#[cfg(feature = "std")]`-gated lines.

- [ ] **5.3 — `phonograph_std` compilation.**
  ```bash
  cargo check -p phonograph_std
  ```
  Passes.

- [ ] **5.4 — Full workspace build, test, lint, docs.**
  ```bash
  cargo build --workspace
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  cargo doc --workspace --no-deps
  ```
  All pass with zero warnings.

- [ ] **5.5 — Regression test count.** Compare `cargo test --workspace` output against
  the Phase 0 baseline. All 311+ (or 473+, depending on baseline) tests must still pass.
  No test should have been silently dropped.

- [ ] **5.6 — Functional smoke tests.**
  - `Database<MemoryBackend>` round-trips nodes and edges (via doc-test or integration test).
  - `Database<AnyBackend>` (through `phonograph_std::open_in_memory()`) round-trips
    nodes and edges.
  - If persistent backend is available: `phonograph_std::open(path)` creates a file,
    writes data, reopens, and reads it back.
  - **Verify:** All smoke tests pass.

### ▸ Phase 5 Gate — MIGRATION COMPLETE

- [ ] **All 13 checks from `030-workspace-redesign.md` §17 pass.** The workspace is a
  clean three-crate layout. Commit with:
  ```
  chore(workspace): complete three-crate migration to phonograph
  ```

---

## Pitfalls & Reminders

These are recurring gotchas from the design doc and the existing codebase. Keep them
in mind throughout the migration.

1. **`StorageError` name collision (R17).** The backend *trait* is now `BackendError`.
   The error *struct* is `StorageError`. Both live in `phonograph_db` but in different
   modules (`backend::BackendError` vs `error::StorageError`). Do not confuse them.

2. **`spin::Mutex` is not re-entrant.** The current code has careful lock ordering
   (`storage` → `write_mutex` → `current_snapshot`). Preserve this ordering when
   moving to `spin`.

3. **`alloc::sync::Arc` requires the `alloc` feature.** Ensure `Arc` comes from
   `alloc::sync`, not `std::sync`, in `phonograph_db`.

4. **`DatabaseInner` currently uses `unsafe impl Send/Sync`.** After generifying
   over `B: StorageBackend`, verify these impls are still sound (or remove them
   if the compiler can derive them).

5. **`MemoryBackend` snapshot helpers use `std::fs`.** The `save_to_file` and
   `load_from_file` methods must remain gated behind `#[cfg(feature = "std")]`
   in `phonograph_db`, or move to `phonograph_std`.

6. **Tests reference `graph_db::` paths.** A bulk find-and-replace will be needed
   in Phase 4. Be careful not to break doc-tests in the core crate whose paths
   changed from `graph_db_core::` to `phonograph::`.

7. **The `Cargo.lock` will change significantly.** New dependencies (`spin`,
   `hashbrown`) and renamed crates will cause a large diff. This is expected.

8. **Keep the workspace compiling per-crate between gates.** Full workspace
   compilation breaks after Phase 1 and is not restored until Phase 4. During
   that interval, always verify using per-crate `cargo check -p <crate>`.
