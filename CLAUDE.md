# CLAUDE.md — Task 31: Cleanup, Hardening & Edition Upgrade

**Project:** Phonograph — Embedded Graph Database with Extensible Schema & Pluggable Inference  
**Task:** 31  
**Scope:** Edition upgrade, feature flag simplification, crash bug fix, panic-to-Result conversions, dead code removal  
**Status:** Pending  
**Depends on:** Workspace redesign (Tasks 30, migration complete)

---

## Orientation

This is a hardening task that addresses structural and safety issues accumulated during
the initial implementation and workspace migration. The codebase currently compiles, passes
all tests, and is functionally complete — but carries unnecessary complexity from Claude Code
decisions that should not have been made, plus one crash bug that was documented as a "known
limitation" rather than fixed.

**This task does NOT add features.** It removes unnecessary machinery, fixes a crash, and
upgrades the edition. The public API surface does not change. All existing tests must
continue to pass (with adjustments for edition syntax and removed feature gates).

**What this task does:**

1. **Edition upgrade:** `2021` → `2024` across the workspace, remove `rust-version` fields
2. **Feature flag simplification:** Remove the `std`/`alloc` cascade from `phonograph` and
   `phonograph_db` — `alloc` becomes unconditional
3. **Crash bug fix:** Large property values (>~1 KB) cause a panic in leaf page overflow
   dispatch (`leaf.rs:215`)
4. **Panic-to-Result conversions:** Replace remaining `assert!`/`panic!` in public library
   APIs with proper `Result` returns
5. **Dead code cleanup:** Remove orphaned `#[cfg(feature = "alloc")]` gates, spurious
   dev-dependencies, and let-chain→nested-if workarounds from the edition 2021 downgrade

**What this task does NOT do:**

- Add new features (batch insert, property indexes, write lock timeout, streaming iterators)
- Change the public API signatures (except where `assert!` → `Result` improves a return type)
- Modify the on-disk format
- Add or remove crates from the workspace

---

## Required Reading

Before making any changes, read these documents:

1. **`030-workspace-redesign.md`** — The architectural source of truth for the three-crate
   workspace. Sections §5 (phonograph), §6 (phonograph\_db), §7 (phonograph\_std), §12
   (feature flags). Understand the current structure so you modify it correctly.

2. **`archive/audits/2026-03-26-codebase-audit.md`** — The prior codebase audit. Section 3
   (Safety, Error Handling & Panics) lists all panic sites. Some were fixed in Task 30;
   this task addresses the remainder.

3. **`archive/completion-reports/28-integration-testing.md`** — Documents the large property
   value panic (Bug #1) and the extension name persistence gap (Bug #2). Bug #1 is in scope
   for this task. Bug #2 is out of scope.

4. **`archive/completion-reports/29-documentation-publish.md`** — Documents the edition
   2024→2021 downgrade and the 5 let-chain refactoring sites that need to be reverted.

5. **`archive/completion-reports/30-audit-fixes.md`** — Documents which panic sites were
   already fixed. Do not duplicate this work.

6. **`CLAUDE.md` (project root)** — Current project-wide rules. This task will update
   CLAUDE.md itself upon completion (feature flag rules, edition references, MSRV references).

7. **`checklist.md` (this directory)** — The ordered implementation steps. Execute sequentially.

---

## Key Decisions (Settled)

These decisions are made. Do not revisit them.

### D1: Edition 2024, no `rust-version` field

Set `edition = "2024"` in the workspace `Cargo.toml` `[workspace.package]` table and in
each per-crate `Cargo.toml`. Remove all `rust-version` fields entirely — there is no MSRV
policy. Users are expected to use the latest stable Rust toolchain.

### D2: `alloc` is unconditional in `phonograph` and `phonograph_db`

Both `phonograph` and `phonograph_db` are `no_std + alloc` crates. The `alloc` feature
gate was always required for the crates to be useful — without it, `phonograph` exports
nothing meaningful (no types, no schema, no errors), and `phonograph_db` exports nothing
at all.

The new feature flag structure:

**`phonograph`:**
```toml
[features]
default = ["std"]
std = []       # Enables std::error::Error impls only
```

No `alloc` feature. The crate unconditionally uses `extern crate alloc`.

**`phonograph_db`:**
```toml
[features]
default = ["std"]
std = ["phonograph/std"]
```

No `alloc` feature. The crate unconditionally uses `extern crate alloc`.

**`phonograph_std`:** Unchanged (no feature flags — always `std`).

### D3: Revert let-chain refactors

The 5 sites where let-chains were refactored to nested `if` blocks (for edition 2021
compatibility) must be reverted to their original let-chain form. Edition 2024 supports
let-chains natively.

Known sites:
- `crates/phonograph_db/src/storage/btree/cursor.rs` (line ~160)
- `crates/phonograph_db/src/db/database.rs` (line ~405)
- `crates/phonograph_db/src/db/write_txn.rs` (line ~938)
- `crates/phonograph_std/tests/inference_tests.rs` (line ~811)
- `crates/phonograph_std/tests/e2e_integration.rs` (line ~563)

These are approximate line numbers — locate the nested `if` patterns and restore them to
`if let ... && ...` chains.

### D4: The overflow page dispatch bug is a crash bug, not a "limitation"

The panic at `leaf.rs:215` when inserting values >~1 KB is not an acceptable known
limitation. The overflow page infrastructure already exists (overflow pages can be built,
chained, parsed, and read back). The bug is that the B-tree insert path does not dispatch
to overflow when a value exceeds the inline threshold. This must be fixed.

### D5: Public API methods must not panic on user-controllable input

Any `assert!`, `panic!`, or `unwrap()` in a public method that can be triggered by
user-provided data (not internal invariant violations) must be converted to `Result`.
Internal `debug_assert!` calls and test-only `unwrap()` are acceptable.

---

## Definition of Done

All of the following must be true:

1. **`edition = "2024"` in all `Cargo.toml` files.** No `rust-version` field anywhere.

2. **No `alloc` feature in `phonograph` or `phonograph_db`.** Both crates unconditionally
   use `extern crate alloc`. The only feature flag is `std` (default on).

3. **All `#[cfg(feature = "alloc")]` gates removed** from `phonograph` and `phonograph_db`
   source files. Modules, re-exports, and compile-test assertions are unconditional.

4. **Large property values (10 KB+) round-trip without panic.** A test inserts a node with
   `Value::Bytes(vec![0u8; 10_000])`, commits, reads it back, and verifies exact match.

5. **No `assert!`, `panic!`, or `unwrap()` in public library methods** on user-controllable
   input. (Mutex lock unwraps after the spin migration may be acceptable if `spin::Mutex::lock`
   returns the guard directly without `Result`. Verify this.)

6. **Let-chain syntax restored** at all 5 sites.

7. **`tempfile` removed from `phonograph` dev-dependencies.**

8. **All tests pass:**
   ```bash
   cargo test --workspace
   cargo clippy --workspace --all-targets -- -D warnings
   cargo doc --workspace --no-deps
   ```

9. **`no_std` verification passes:**
   ```bash
   cargo check -p phonograph --no-default-features
   cargo check -p phonograph_db --no-default-features
   ```
   Note: these now compile without `--features alloc` since `alloc` is unconditional.

10. **CHANGELOG.md updated:** Remove "Large property values (>~1 KB) may cause panics"
    from Known Limitations. Add a Fixed section if appropriate.

11. **README.md updated:** Remove "Large property values (>~1 KB) are not supported in v0.1"
    from Known Limitations.

12. **Project root `CLAUDE.md` updated:** Feature flag rules reflect the new simplified
    structure. No references to `rust-version` or MSRV.

---

## Out of Scope

- **New features:** Property value indexes, streaming iterators, batch insert, write lock
  timeout. These are real limitations but are future work, not hardening.
- **Extension name persistence gap** (Bug #2 from Task 28). This is a correctness bug but
  requires design decisions about the persistence protocol. Separate task.
- **Splitting large files** (`write_txn.rs`, `serialization.rs`). These are refactoring
  tasks, not hardening.
- **Adding new tests** beyond those needed to verify the fixes in this task.
