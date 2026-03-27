# CLAUDE.md — Task 26: Implement Inference Hook Infrastructure

**Project:** Embedded Graph Database with Extensible Schema & Pluggable Inference  
**Implementation Task:** 26 (preparation task: 18)  
**Module:** `src/db/inference_engine.rs` (primary), extensions to `src/db/` transaction types  
**Status:** Pending  
**Depends on:** Task 25 (query & traversal engine), which depends on Task 24 (storage engine)  
**Preparation depends on:** Task 12 (`012-design-document.md`), Task 17 (`tasks/25-query-engine/`)

---

## Orientation

This is Task 26, the implementation of the inference hook infrastructure. Within the project's hierarchy, this is one task in a 4-phase, 29-task project. Sibling implementation tasks are 22 (core types), 23 (HAL + std backend), 24 (storage engine), 25 (query engine), 27 (in-memory backend), 28 (integration testing), 29 (docs/publish). Task 27 depends on this task's output.

The inference hook system is the mechanism by which downstream code registers and invokes pluggable inference rules. This task implements the **infrastructure** — rule registration, triggering dispatch, result management (ephemeral and materialized), caching, and provenance tracking. It does **not** implement any domain-specific inference rules. A minimal test-only example rule is included solely for testing the infrastructure.

By the time this task begins, the following should already be implemented:

- **Core types** (Task 22): `InferenceRule` trait, `InferredFact`, `InferenceResult`, `InferenceMode`, `ProvenanceRecord`, `InferredEntity`, `MaterializedMapping`, `InferenceError` — all in `src/inference/`
- **Query engine** (Task 25): `ReadTransaction` and `WriteTransaction` with full graph query capabilities, `GraphView` implementation, `ChangeSet` production
- **Database** (Task 25): `Database::open()`, `Database::read_txn()`, `Database::write_txn()`, extension registration stubs, inference method stubs returning `Error::Inference(RuleNotFound(...))`

This task replaces those stubs with working inference infrastructure.

---

## Required Reading

Before writing any code, read these documents from the project knowledge. Read them in the order listed — later documents build on earlier ones.

1. **`012-design-document.md`** — The single source of truth. Key sections for this task:
   - §2 — Architecture overview and layer diagram
   - §3 — Crate structure, feature flags, module layout (especially `db/` submodules)
   - §10 — Concurrency Model (single-writer MVCC, snapshot lifecycle, write locking)
   - §11 — Transaction Lifecycle (read/write sequences, commit protocol, read-your-own-writes)
   - §14 — Inference Hook Architecture (the primary reference for this task: InferenceEngine, triggering, modes, materialization, caching, provenance, sequential rule execution)
   - §15 — Public API Surface (Database, ReadTransaction, WriteTransaction — inference-related methods)
   - §16 — Cross-Cutting Concerns (error handling, concurrency guarantees)
   - §17 — Design Decision Log (especially I1–I12, D17–D18, A3, A8)
   - §19 — Consolidated B-Tree Catalog and Schema Store Key Map (provenance key encoding: prefix `0x06`)

2. **`011-inference-hook-design.md`** — The authoritative, detailed specification for everything in this task:
   - §3 — Architecture overview (three-component diagram)
   - §5 — InferenceEngine struct, concurrency patterns, initialization
   - §6 — Triggering flow (full dispatch pseudocode)
   - §7 — Result representation (ephemeral vs. materialized, MaterializedMapping)
   - §8 — Provenance tracking (ProvenanceRegistry, persistence, key encoding)
   - §9 — Caching and invalidation (generation-based, LRU, hit conditions, dirty flag bypass)
   - §10 — Materialization lifecycle (cleanup, insertion, provenance recording)
   - §11 — Fact validation during materialization
   - §12 — Interaction with transactions and concurrency
   - §13 — Interaction with constraint validation
   - §14 — Performance expectations
   - §15 — Scope boundary (core vs. downstream)
   - §§16–18 — Walkthroughs (inverse edge, OWL subclass, re-inference)

3. **`010-api-surface-spec.md`** — Authoritative reference for:
   - §5 — Database lifecycle and extension registration methods
   - §6.1 — ReadTransaction inference methods
   - §6.2 — WriteTransaction inference methods
   - §6.3 — InferenceMode definition

4. **`006-schema-extension-spec.md`** — Authoritative reference for:
   - §10.3 — GraphView trait (the internal read interface passed to rules)
   - §11 — InferenceRule trait, InferredFact, InferenceResult
   - §12 — Extension registration lifecycle

5. **`CLAUDE.md` (project root)** — Project-wide rules: `no_std + alloc` requirements, documentation standards, test expectations, code style, module layout, feature flags.

---

## What This Task Produces

| Component | Location | Description |
|-----------|----------|-------------|
| **InferenceEngine** | `src/db/inference_engine.rs` | Internal struct with rule registry, cache, and provenance registry |
| **InferenceCache** | `src/db/inference_engine.rs` | LRU cache keyed by `(rule_name, data_generation)` |
| **ProvenanceRegistry** | `src/db/inference_engine.rs` | In-memory provenance index with persistence to Schema Store B-tree |
| **Inference dispatch** | `src/db/read_txn.rs`, `src/db/write_txn.rs` | `run_inference()` and `run_all_inference()` implementations replacing stubs |
| **Provenance queries** | `src/db/read_txn.rs`, `src/db/write_txn.rs` | `is_inferred_node()`, `is_inferred_edge()`, `node_provenance()`, `edge_provenance()` |
| **Materialization mapping** | `src/db/write_txn.rs` | `last_materialization_mapping()` |
| **Provenance persistence** | `src/db/inference_engine.rs` | Serialize/deserialize provenance records to/from Schema Store B-tree |
| **Test example rule** | `tests/inference_tests.rs` | Minimal rule for testing the infrastructure (not a public or built-in rule) |

---

## Key Design Decisions to Follow

These are non-negotiable decisions from the design documents. Do not deviate.

1. **Inference runs only on explicit request.** No automatic triggers, no background processes. The caller decides when inference happens. (012 §14.1, 011 Principle #1.)

2. **The caller chooses the mode.** `Ephemeral` vs `Materialized` is per-invocation, not per-rule. (012 §14.5, 011 Principle #2.)

3. **Rule Registry is a `BTreeMap<String, Box<dyn InferenceRule>>`.** Protected by `RwLock` — read lock for invocation, write lock for registration. Registration is on `Database`, not in transactions. (012 §14.2, 011 §5.2.)

4. **Cache is keyed by `(rule_name, data_generation)`.** LRU eviction, default 64 entries, configurable via `DatabaseConfig::inference_cache_size`. Set to 0 to disable. Not persisted. (012 §14.7, 011 §9.)

5. **Cache bypass on dirty write transactions.** If the write transaction has pending mutations (dirty flag is true), the cache is always bypassed because the rule should see uncommitted writes. (011 §9.3.)

6. **Provenance is stored in the Schema Store B-tree** with key prefix `0x06`. Loaded into memory at startup. Written to WriteBuffer during materialization. (012 §19.2, 011 §8.4.)

7. **Cleanup-and-reinsert on re-inference.** When a rule is re-run in materialized mode, all previously materialized facts from that rule are removed before new facts are inserted. No diff-based approach. (011 §10.2, decision I6.)

8. **`run_all_inference` executes rules sequentially in registration order.** This enables rule chaining. (012 §14.9, 011 §6.4, decision I7.)

9. **Materialized facts participate in commit-time constraint validation.** They are regular data in the WriteBuffer. (011 §13.)

10. **No automatic invalidation of materialized facts.** The caller explicitly re-runs inference when the underlying data changes. (011 decision I10.)

---

## Concurrency Model for Inference

The inference engine operates within the existing single-writer MVCC model:

- **Read transactions:** Acquire a read lock on the rule registry. Run rules against the snapshot. Cache is checked/populated. Results are always ephemeral. No provenance writes.
- **Write transactions:** Acquire a read lock on the rule registry. Run rules against the snapshot + WriteBuffer overlay. If materializing: clean up old facts, write new facts to WriteBuffer, record provenance in WriteBuffer.
- **Rule registration/unregistration:** Acquires a write lock on the rule registry. Does not require a write transaction. Callable from any thread at any time (via `Database` methods).

The `RwLock` on the rule registry is separate from the database write lock. Multiple read transactions can invoke inference concurrently. Only one write transaction can exist at a time (enforced by the database write lock, not by the inference engine).

---

## Definition of Done

All of the following must be true before this task is COMPLETE:

1. **InferenceEngine is implemented** with all three sub-components (rule registry, cache, provenance registry) as specified in `011-inference-hook-design.md` §5.

2. **Rule registration and unregistration work correctly** via `Database::register_inference_rule()` and `Database::unregister_inference_rule()`. Registration replaces existing rules with the same name. Unregistration returns `true` if found.

3. **All four inference entry points are functional:**
   - `ReadTransaction::run_inference(rule_name)` — ephemeral only
   - `ReadTransaction::run_all_inference()` — ephemeral only
   - `WriteTransaction::run_inference(rule_name, mode)` — caller chooses mode
   - `WriteTransaction::run_all_inference(mode)` — caller chooses mode

4. **Ephemeral mode** returns `InferenceResult` without modifying the graph. Cache is used when applicable.

5. **Materialized mode** writes inferred facts to the WriteBuffer, assigns real IDs to new nodes/edges, records provenance, and makes the `MaterializedMapping` available via `last_materialization_mapping()`.

6. **Re-inference cleanup works.** Running the same rule in materialized mode twice in the same transaction correctly removes previously materialized facts before inserting new ones.

7. **Provenance queries are implemented:**
   - `is_inferred_node(NodeId) -> bool`
   - `is_inferred_edge(EdgeId) -> bool`
   - `node_provenance(NodeId) -> Option<ProvenanceRecord>`
   - `edge_provenance(EdgeId) -> Option<ProvenanceRecord>`

8. **Provenance persists across database sessions.** Provenance records are written to the Schema Store B-tree with prefix `0x06` and loaded on database open.

9. **Cache behaves correctly:** hits on matching `(rule_name, generation)`, misses on mismatch, bypass when write transaction is dirty, respects `inference_cache_size` configuration (including 0 = disabled).

10. **Inference only runs on explicit request.** No automatic triggers exist anywhere in the codebase.

11. **The `InferenceRule` trait is implementable by external code.** Verified by a test that defines a rule in `tests/` (outside `src/`) and registers it via the public API.

12. **`run_all_inference` executes rules sequentially in registration order.** Verified by a test with two rules where rule B depends on rule A's materialized output.

13. **Materialized facts participate in constraint validation at commit time.** Verified by a test that registers both a rule and a validator, materializes, and commits.

14. **A minimal test-only example rule exists** in `tests/` that exercises all `InferredFact` variants. This rule is NOT part of the public API or the `src/` code.

15. **All tests pass:** `cargo test`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo doc --no-deps` with zero warnings.

---

## Out of Scope

- **Specific inference rules** (OWL, RDFS, SKOS, etc.) — always downstream.
- **Fixpoint inference loop** — downstream crates build this by calling `run_inference` repeatedly.
- **Type-aware cache optimization** (§9.6 in 011) — deferred to post-v1.
- **Incremental inference** (processing only changes) — deferred future optimization.
- **Parallel rule execution** — `run_all_inference` is always sequential.
- **In-memory backend** — Task 27.
