# Completion Report: Task 26 — Inference Hook Infrastructure

**Status:** COMPLETE
**Date:** 2026-03-26
**Sessions:** 1

---

## Done Criterion Assessment

All items from the Definition of Done are satisfied:

| Criterion | Status | Evidence |
|-----------|--------|----------|
| InferenceEngine implemented (rule registry, cache, provenance) | DONE | `src/db/inference_engine.rs` — InferenceEngine, InferenceCache, ProvenanceRegistry |
| Rule registration/unregistration via Database | DONE | Delegates to InferenceEngine, preserves insertion order |
| ReadTransaction::run_inference (ephemeral) | DONE | Cache-aware, builds GraphView from snapshot |
| ReadTransaction::run_all_inference (ephemeral) | DONE | Iterates rules in registration order |
| WriteTransaction::run_inference (ephemeral + materialized) | DONE | Full dispatch with validation, cleanup, materialization |
| WriteTransaction::run_all_inference (both modes) | DONE | Sequential chaining — each rule sees prior materializations |
| Ephemeral mode returns facts without side effects | DONE | `ephemeral_mode_in_write_transaction` test |
| Materialized mode writes to WriteBuffer, assigns IDs | DONE | `materialized_inference_writes_to_graph` test |
| Re-inference cleanup works | DONE | `re_inference_cleans_up_old_facts` test |
| Provenance queries (is_inferred_node/edge, node/edge_provenance) | DONE | Both ReadTransaction and WriteTransaction |
| Provenance persists across sessions | DONE | `provenance_persists_across_sessions` test |
| Cache: hits on (rule_name, generation), misses on mismatch | DONE | 7 unit tests |
| Cache: bypass when write transaction is dirty | DONE | `dirty_transaction_bypasses_cache` test |
| Cache: respects inference_cache_size (0 = disabled) | DONE | `cache_disabled_always_misses` test |
| Inference only on explicit request | DONE | 3 no-auto-trigger tests |
| InferenceRule implementable by external code | DONE | TestInferenceRule defined in `tests/` |
| run_all_inference executes in registration order | DONE | `run_all_inference_registration_order_not_alphabetical` test |
| Materialized facts participate in constraint validation | DONE | `materialized_facts_pass_constraint_validation` test |
| MaterializedMapping available via last_materialization_mapping | DONE | `materialized_mapping_has_assigned_ids` test |
| cargo test | DONE | 359 pass, 0 fail, 2 ignored |
| cargo clippy --all-targets --all-features -- -D warnings | DONE | Zero warnings |
| cargo doc --no-deps | DONE | Zero warnings |
| cargo check --no-default-features --features alloc | DONE | Zero errors |

---

## Deliverables

### Source Files (new)
- `src/db/inference_engine.rs` — InferenceCache (LRU, 7 tests), ProvenanceRegistry (7 tests, 10 encode/decode tests), InferenceEngine (4 tests)

### Source Files (modified)
- `src/db/mod.rs` — Added `pub mod inference_engine;`, updated module doc
- `src/db/database.rs` — Replaced `inference_registry: RwLock<Vec<...>>` with `Mutex<InferenceEngine>`, loads provenance at open, delegates rule registration
- `src/db/read_txn.rs` — Replaced inference/provenance stubs with working dispatch + ReadTxnSnapshotReader
- `src/db/write_txn.rs` — Replaced stubs with full materialized inference, added dirty flag, pending_provenance, provenance_removals, last_materialization, provenance commit path

### Test Files (new)
- `tests/inference_tests.rs` — 18 integration tests covering all Definition of Done criteria

### Test Counts
- Unit tests (lib): 320 (28 new in inference_engine, minus 1 from method removal)
- Integration tests: 34 (9 db + 4 concurrency + 18 inference + 3 storage)
- Doc tests: 5
- **Total: 359 pass, 0 fail, 2 ignored**

---

## Notable Decisions

1. **`Mutex<InferenceEngine>` over `RwLock`** — Cache mutation on reads requires exclusive access. Since inference is not a hot path, the simpler Mutex is preferable. The engine lock is separate from the storage engine Mutex, so no deadlock risk.

2. **`Vec<String> rule_order`** alongside `BTreeMap<String, Box<dyn InferenceRule>>` — BTreeMap provides O(log N) lookup by name, but iterates alphabetically. `rule_order` preserves insertion order for deterministic chaining in `run_all_inference`.

3. **`dirty: bool` on WriteTransaction** — Set by all mutation methods (insert, update, delete, property set, type label changes). When dirty, inference always bypasses cache and re-invokes the rule to see uncommitted changes.

4. **Pending provenance stored transaction-locally** — `pending_provenance: Vec<(InferredEntity, ProvenanceRecord)>` and `provenance_removals: Vec<InferredEntity>` track uncommitted provenance changes. Visible within the write transaction's provenance queries. Written to Schema Store B-tree during commit.

5. **Provenance persistence uses incremental writes** — Only changed provenance records (removals + new records from this transaction) are written at commit, not a full flush. This avoids O(N) writes when provenance is large but unchanged.

6. **Provenance key encoding uses dummy record for deletion** — When deleting provenance entries during commit, `encode_entry` is called with a dummy record since only the key (which depends on entity, not record) is needed for B-tree deletion.

7. **Fact validation before materialization** — All inferred facts are validated before any changes are made. This ensures no partial materialization on error: either all facts are valid and materialized, or none are.

---

## Context for Next Task (Task 27: In-Memory Backend)

Task 27 should build on:
- The `InferenceEngine` is fully functional and integrated into `Database`/transactions
- `Database::open()` with `StorageMode::InMemory` still returns an error (unchanged from Task 25)
- The `SnapshotReader` trait in `graph_view.rs` provides the abstraction needed for different backends
- All core types remain `no_std + alloc` compatible
- The `hal_mem/` module with `MemoryBackend` exists from Task 22 but is not wired into `Database`

---

## Residual Concerns

1. **Extension name persistence** — Rule names are persisted via provenance records (prefix 0x06) and extension name records (prefix 0x05). The extension name registration/unregistration through WriteBuffer's SchemaChange works for explicit buffer operations, but `Database::register_inference_rule()` doesn't create a WriteBuffer entry since it operates outside a transaction. Rule names are still discoverable via provenance entries.

2. **Full flush on first provenance write** — The incremental approach works well after the initial materialization, but the first materialization for a rule writes all new provenance entries. This is inherent to the design.

3. **LRU eviction is O(N) scan** — With default max_entries=64, this is negligible. If the cache size were increased significantly, a secondary index (BTreeMap<u64, key>) would be needed.
