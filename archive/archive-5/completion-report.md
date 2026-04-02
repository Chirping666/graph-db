# Completion Report — Static Dispatch & Consistency Hardening

**Status:** Complete
**Date:** 2026-04-02

---

## Summary

This checklist brought the codebase into conformance with architectural principles A7 (static
dispatch), A8 (float comparisons), and A9 (overlay contract) defined in `CLAUDE.md`. All
findings from the March 2026 consistency and security review have been addressed.

---

## What Changed

### Phase 1: Static Dispatch & Schema Reference
- `OverlayGraphView` now holds a `&'s SchemaCache` reference (lifetime parameter added).
- `OverlayGraphView::build()` takes `&impl SnapshotReader` (static dispatch) instead of
  `&dyn SnapshotReader` (dynamic dispatch).
- All call sites in `write_txn.rs` and test code updated.

### Phase 2: Subtype Resolution in OverlayGraphView
- `nodes_by_type` and `edges_by_type` on `OverlayGraphView` now correctly resolve subtypes
  using `SchemaCache::all_subtypes()` when `include_subtypes` is `true`.
- Previously, both branches of the `include_subtypes` conditional were identical (subtype
  parameter was ignored).
- New tests verify subtype resolution for both nodes and edges.

### Phase 3: Changeset-Scoped Preloading
- `SnapshotReader` trait gained `nodes_by_type_ids` and `edges_by_type_ids` methods with
  default implementations.
- `BaseSnapshotReader` overrides these to use the type index B-tree scan for performance.
- `OverlayGraphView::build()` accepts an optional `affected_types` parameter. When provided,
  only base entities with matching types (plus adjacency neighbors of changed nodes) are
  loaded, avoiding full database scans on commit.
- New tests verify scoped preloading excludes unrelated types and includes adjacency neighbors.

### Phase 4: `total_eq` Consistency
- `nodes_by_property` in both `OverlayGraphView` and `WriteTransaction` now uses
  `Value::total_eq()` instead of `PartialEq` (`==`), fixing NaN property lookups.
- New test verifies NaN-valued properties are matchable.

### Phase 5: Counter Deserialization Bounds Check
- `Database::load_schema()` now validates that persisted `u64` counter values fit in `u32`
  before casting, using `u32::try_from()`. Returns `StorageError` on overflow instead of
  silently truncating.
- New test verifies overflow detection.

### Phase 6: Inference Cache Allocation Reduction
- `InferenceCache` restructured from `BTreeMap<(String, u64), CacheEntry>` to a two-level
  `BTreeMap<String, BTreeMap<u64, CacheEntry>>`, eliminating a `String` allocation on every
  `get()` call.

### Phase 7: Documentation & Metadata
- `CHANGELOG.md` updated with all changes.
- Doc comments on modified methods updated to reflect new behavior, including `total_eq`
  semantics notes on `nodes_by_property`.

---

## Architectural Principles Addressed

| Principle | Description | Status |
|-----------|-------------|--------|
| A7 | Prefer static dispatch over dynamic dispatch | Enforced — `OverlayGraphView::build()` uses `&impl SnapshotReader` |
| A8 | Semantic correctness for float comparisons | Enforced — all engine property lookups use `total_eq` |
| A9 | `OverlayGraphView` respects full `GraphView` contract | Enforced — subtype resolution implemented, schema reference held |

---

## Review Findings Addressed

1. `OverlayGraphView::build()` used `&dyn SnapshotReader` → changed to `&impl SnapshotReader`
2. `OverlayGraphView` lacked schema reference → now holds `&'s SchemaCache`
3. `nodes_by_type` ignored `include_subtypes` → resolves via `all_subtypes()`
4. `edges_by_type` ignored `include_subtypes` → resolves via `all_subtypes()`
5. `OverlayGraphView::nodes_by_property` used `PartialEq` → uses `total_eq`
6. `WriteTransaction::nodes_by_property` used `PartialEq` → uses `total_eq`
7. Full database scan on every commit → changeset-scoped preloading with `affected_types`
8. `InferenceCache::get()` allocated `String` on every call → two-level map eliminates allocation
9. Counter deserialization silently truncated `u64` to `u32` → bounds-checked with `u32::try_from`

---

## Test Count

| Metric | Before (Phase 0) | After (Phase 8) | Delta |
|--------|-------------------|------------------|-------|
| Passed | 490 | 496 | +6 |
| Ignored | 3 | 3 | 0 |
| Failed | 0 | 0 | 0 |

---

## Files Modified

- `checklist.md`
- `CHANGELOG.md`
- `crates/phonograph_db/src/db/database.rs`
- `crates/phonograph_db/src/db/graph_view.rs`
- `crates/phonograph_db/src/db/inference_engine.rs`
- `crates/phonograph_db/src/db/read_txn.rs`
- `crates/phonograph_db/src/db/write_txn.rs`

---

## Residual Concerns

All pre-existing residual concerns from `CLAUDE.md` remain unchanged. No new residual
concerns were introduced. The changeset-scoped preloading trade-off (residual concern #7)
is now implemented and documented.
