# Completion Report: Audit-Based Fixes (Task 30)

**Status:** COMPLETE
**Date:** 2026-03-26
**Audit reference:** `audits/2026-03-26-codebase-audit.md`

---

## Summary

Applied fixes for all actionable findings from the 2026-03-26 codebase audit, spanning
P0 (critical), P1 (high), P2 (medium), and P3 (low) priorities. All changes maintain
full backward compatibility — no public API breakage. The crate passes all 474 tests,
clippy with `-D warnings`, and `cargo doc --no-deps` without warnings.

---

## Findings Addressed

### P0 — `no_std` Feature Gating (FIXED)

**Problem:** `types`, `schema`, `constraint`, `inference`, `error` modules were exported
unconditionally in `lib.rs` despite requiring the `alloc` feature. Building with
`--no-default-features` failed.

**Fix:** Gated all five modules and their re-exports behind `#[cfg(feature = "alloc")]`.
The `backend` module remains ungated (uses only `core::fmt`). Compile-test assertions
for alloc-dependent traits are also gated.

**File:** `src/lib.rs`

**Verification:**
- `cargo check --no-default-features` — passes (bare `no_std`, only `backend` available)
- `cargo check --no-default-features --features alloc` — passes

### P1 — Page-Parsing Unwraps Replaced with Error Returns (FIXED)

**Problem:** 12 `try_into().unwrap()` calls in page-parsing code would panic on corrupted
database files instead of returning recoverable errors. All target functions already
returned `Result`.

**Fix:** Replaced every production-code unwrap with `.try_into().map_err(|_| StorageError { ... })?`:

| File | Unwraps Fixed | Function |
|------|--------------|----------|
| `src/storage/page/header.rs` | 4 | `deserialize()`, `validate_checksum()` |
| `src/storage/page/overflow.rs` | 2 | `parse()` |
| `src/storage/page/interior.rs` | 2 | `parse()` |
| `src/db/database.rs` | 4 | `load_schema()` |
| **Total** | **12** | |

Test-only unwraps (interior.rs `byte_level_layout` test) were intentionally left as-is.

### P1 — `unreachable!()` and `panic!()` Replaced (FIXED)

| Location | Was | Now |
|----------|-----|-----|
| `src/storage/btree/insert.rs:511` | `unreachable!()` after propagate_split loop | `Err(StorageError { ... })` |
| `src/storage/snapshot.rs:69` | `panic!()` in `root_for_tree()` | Returns `Option<PageId>`, `None` for out-of-range |

The `root_for_tree` change required updating the test from `#[should_panic]` to
`assert_eq!(snap.root_for_tree(8), None)`. No production callers existed (roots are
accessed directly via `self.roots.node_store` etc.).

### P2 — Re-export Database Types at Crate Root (FIXED)

**Problem:** Users had to write `use graph_db::db::{Database, DatabaseConfig, ...}`.

**Fix:** Added `#[cfg(feature = "std")]` re-exports in `lib.rs`:

```rust
pub use db::{Database, DatabaseConfig, ReadTransaction, WriteTransaction,
             NodeBuilder, EdgeBuilder, TypeDefinitionBuilder, ...};
```

Users can now write `use graph_db::Database;`.

### P2 — Extract `commit()` into Phases (FIXED)

**Problem:** `WriteTransaction::commit()` was 391 lines with duplicated edge index operations.

**Fix (three-stage extraction):**

1. **`apply_cow`** converted from `&self` method to free function (eliminates borrow conflicts).

2. **Edge index helpers** extracted — `insert_edge_indexes()` and `delete_edge_indexes()`
   consolidate the outgoing-adj + incoming-adj + type-index pattern that was repeated
   4 times (~130 lines of duplication removed).

3. **Phase helpers** extracted — `commit_node_changes()` and `commit_edge_changes()` as
   generic free functions taking `&WriteBuffer` + `&mut StorageEngine<B>` + `&mut SnapshotRoots`.

**Result:** `commit()` reduced from 391 to ~140 lines. `write_txn.rs` total: 1,994 -> 1,925 lines.

### P2 — Extract B-tree Traversal Helper (FIXED)

**Problem:** `insert.rs:92-116` and `delete.rs:64-87` contained identical 25-line
traversal-to-leaf loops.

**Fix:**
- Added `PathEntry` struct to `btree/mod.rs` (was duplicated in both files).
- Added `BTree::traverse_to_leaf()` method returning `(Vec<PathEntry>, PageId)`.
- Both `insert()` and `delete()` now call the shared helper, then process the leaf.
- Flattened both functions by one nesting level (eliminated the `loop { match { ... } }` pattern).

**Line count reduction:**

| File | Before | After | Savings |
|------|--------|-------|---------|
| `btree/insert.rs` | 514 | 451 | -63 |
| `btree/delete.rs` | 206 | 160 | -46 |
| `btree/mod.rs` | 467 | 533 | +66 (shared code) |
| **Net** | **1,187** | **1,144** | **-43** |

### P2 — `DatabaseConfig::validate()` (FIXED)

**Problem:** `DatabaseConfig::page_size()` panicked via `assert!()` on invalid input.

**Fix:** Added `DatabaseConfig::validate() -> Result<(), Error>` that checks page_size
constraints. Called automatically from `Database::open()`. Builder asserts are kept for
immediate feedback; `validate()` catches configs constructed by setting `pub` fields directly.

Three new tests added: `validate_catches_invalid_page_size`,
`validate_catches_small_page_size`, `validate_accepts_valid_config`.

### P3 — Variable Renames in B-tree Code (FIXED)

Renamed cryptic single-letter frame handles in `insert.rs` and `delete.rs`:

| Old | New | Context |
|-----|-----|---------|
| `f` | `frame` | Generic page frame |
| `lf` | `left_frame` | Left page after split |
| `rf` | `right_frame` | Right page after split |
| `ff` | `new_frame` | Interior CoW frame |
| `pf` | `parent_frame` | Parent interior frame |
| `pd` | `parent_data` | Parent page data |
| `int` | `parent_page` | Parsed interior page |
| `c` | `parent_cells` | Cell vector |
| `rc` | `parent_right_child` | Right child pointer |
| `nid` | `new_parent_id` | Allocated page ID |
| `pb` | `parent_bytes` | Serialized page bytes |
| `pe` | `parent_entry` | Path entry |

### P3 — SAFETY Comments on Windows Unsafe Blocks (FIXED)

Added explicit `// SAFETY:` comments to two `core::mem::zeroed()` calls for Windows
`OVERLAPPED` struct initialization in `backend_std/file_backend.rs` (lines 401, 438).

---

## Audit Items Not Addressed (by design)

| Audit Item | Reason |
|------------|--------|
| P1: Unit tests for B-tree ops, transactions | Separate testing task; existing 474 integration/unit/doc tests provide indirect coverage |
| P2: Split `serialization.rs` (1,267 lines) | Cohesive module per audit assessment; no functional issue |
| P2: Doc tests for `commit()`, `abort()`, `insert_node()` | Documentation task, not a code defect |
| P3: Integration tests for corruption recovery | New test development, separate scope |
| Low: Make `DatabaseConfig` fields private | Would break existing usage patterns |
| Low: Convenience wrapper for constraint registration | Ergonomic improvement, not a defect |

---

## Verification

| Check | Result |
|-------|--------|
| `cargo check --no-default-features` | Pass |
| `cargo check --no-default-features --features alloc` | Pass |
| `cargo test` | 474 passed, 0 failed, 1 ignored |
| `cargo clippy --all-targets --all-features -- -D warnings` | 0 warnings |
| `cargo doc --no-deps` | 0 warnings |

---

## Files Modified

| File | Changes |
|------|---------|
| `src/lib.rs` | Feature gates on modules + re-exports; crate-root Database re-exports |
| `src/storage/page/header.rs` | 4 unwraps -> error returns |
| `src/storage/page/overflow.rs` | 2 unwraps -> error returns |
| `src/storage/page/interior.rs` | 2 unwraps -> error returns |
| `src/db/database.rs` | 4 unwraps -> error returns; call config.validate() |
| `src/storage/btree/insert.rs` | unreachable -> error; use traverse_to_leaf; rename vars |
| `src/storage/btree/delete.rs` | Use traverse_to_leaf; rename vars |
| `src/storage/btree/mod.rs` | Add PathEntry + traverse_to_leaf |
| `src/storage/snapshot.rs` | panic -> Option return |
| `src/db/write_txn.rs` | Extract commit phases + edge index helpers |
| `src/db/config.rs` | Add validate() method + tests |
| `src/backend_std/file_backend.rs` | SAFETY comments on Windows unsafe blocks |
