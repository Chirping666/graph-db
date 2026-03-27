# CLAUDE.md — Task 25: Implement Query & Traversal Engine

**Project:** Embedded Graph Database with Extensible Schema & Pluggable Inference  
**Implementation Task:** 25 (preparation task: 17)  
**Module:** `src/db/`  
**Status:** Pending  
**Depends on:** Task 24 (persistent storage engine)  
**Preparation depends on:** Task 12 (`012-design-document.md`), Task 16 (`tasks/24-storage-engine/`)

---

## Orientation

This is Task 25, the implementation of the database engine layer — the `src/db/` module. It sits on top of the storage engine (`src/storage/`) from Task 24 and provides the entire public API: `Database`, `DatabaseConfig`, `ReadTransaction`, `WriteTransaction`, the `GraphReader` trait, the `WriteBuffer`, the in-memory schema cache, constraint validation dispatch at commit time, and all query/traversal operations.

Within the project's hierarchy, this is one task in a 4-phase, 29-task project. Sibling implementation tasks are 22 (core types), 23 (HAL + std backend), 24 (storage engine), 26 (inference hooks), 27 (in-memory backend), 28 (integration testing), 29 (docs/publish). Task 26 (inference hook infrastructure) depends on this task's output.

**Scope boundary with Task 26 (Inference Hooks):** This task implements the `db/` module *except* the inference engine. Specifically, `run_inference()` and `run_all_inference()` methods, `InferenceEngine`, `InferenceCache`, `ProvenanceRegistry`, and all provenance query methods (`is_inferred_node`, `is_inferred_edge`, `node_provenance`, `edge_provenance`) are Task 26's responsibility. This task provides the structural hooks that Task 26 will fill in — the `InferenceEngine` field in `Database`, placeholder `run_inference` signatures that return `Error::Inference(RuleNotFound)`, and the provenance query stubs.

---

## Required Reading

Before writing any code, read these documents in order:

1. **`012-design-document.md`** — The single source of truth. Key sections for this task:
   - §2 — Architecture overview and layer diagram
   - §3 — Crate structure, feature flags, module layout (especially `db/` submodules)
   - §4 — Core data model (Node, Edge, Value, PropertyMap)
   - §5 — Type system and schema (TypeDefinition, PropertyDeclaration, type hierarchy)
   - §6 — Graph Storage Model (B-tree catalog — understanding which B-trees serve which queries)
   - §10 — Concurrency Model (single-writer MVCC, snapshot lifecycle, write locking, thread safety)
   - §11 — Transaction Lifecycle (read/write sequences, commit protocol, read-your-own-writes)
   - §13 — Constraint Validation System (ChangeSet production, validator dispatch, validation modes)
   - §14 — Inference Hook Architecture (understand the interface, but defer implementation to Task 26)
   - §15 — Public API Surface (Database, ReadTransaction, WriteTransaction, GraphReader)
   - §16 — Cross-Cutting Concerns (error handling, naming, concurrency guarantees)
   - §17 — Design Decision Log (especially A1–A12, G7–G8, G17, D12–D13)
   - §19 — Consolidated B-Tree Catalog and Schema Store Key Map

2. **`010-api-surface-spec.md`** — Authoritative reference for:
   - Database lifecycle and configuration (§5)
   - ReadTransaction API (§6.1)
   - WriteTransaction API (§6.2)
   - GraphReader trait (§10.1)
   - Multi-hop traversal patterns (§10.3)
   - Counting methods (§10.4)
   - Node/Edge builders (§7–9)
   - Extension registration (§5.2)
   - Missing extension detection (§5.4)
   - Full usage examples (§§12–17)

3. **`007-graph-storage-model.md`** — Authoritative reference for:
   - B-tree catalog: which B-tree serves which query (§4, §9.3)
   - Schema-to-storage mapping (§9)
   - WriteBuffer design and ChangeSet production (§12, §15)
   - Overlay view for read-your-own-writes (§12.3)
   - ID allocation and recycling (§14)
   - Performance characteristics (§16)

4. **`006-schema-extension-spec.md`** — Authoritative reference for:
   - GraphView trait (§10.3) — the internal read interface for validators and inference rules
   - ChangeSet structure (§10.2)
   - ConstraintValidator trait and dispatch (§10.5–10.6)
   - TypeRegistryView and PropertyKeyRegistryView traits (§8, §9)

5. **`011-inference-hook-design.md`** — Read for interface understanding (Task 26 implements):
   - InferenceEngine architecture (§3)
   - Rule registry (§5)
   - Cache design (§9)
   - Provenance system (§8)

6. **`CLAUDE.md` (project root)** — Project-wide rules, especially:
   - Rule 1: No external database crate dependencies
   - Rule 2: `db/` requires `std` feature
   - Rule 4: Documentation on every public item
   - Rule 5: Test coverage expectations
   - Rule 7: Code style and conventions

7. **Existing code from Tasks 22, 23, and 24:**
   - `src/types/` — NodeId, EdgeId, TypeId, PropertyKeyId, Value, Node, Edge, PropertyMap, etc.
   - `src/schema/` — GraphView, TypeRegistryView, PropertyKeyRegistryView traits
   - `src/constraint/` — ConstraintValidator, ChangeSet, NodeChange, EdgeChange, ConstraintViolation
   - `src/inference/` — InferenceRule, InferredFact, InferenceResult, InferenceMode
   - `src/error/` — Error, SchemaError, StorageError, NotFoundError, TransactionError, InferenceError
   - `src/hal/` — ReadAt, WriteAt, StorageBackend traits
   - `src/hal_std/` — FileBackend implementation
   - `src/storage/` — StorageEngine, BufferPool, B-tree operations, Snapshot, PageAllocator, serialization

---

## Objective

Implement the complete database engine layer in `src/db/`, providing the public API that application code uses to interact with the database.

After this task, the following must be true:
- A caller can open/create a database via `Database::open(config)`
- A caller can begin read-only transactions that see a consistent snapshot
- A caller can begin read-write transactions with read-your-own-writes semantics
- All query operations from the API spec work: `get_node`, `get_edge`, `outgoing_edges`, `incoming_edges`, `neighbors`, `nodes_by_type`, `edges_by_type`, `nodes_by_property`, `node_count`, `edge_count`, counting variants
- All mutation operations work: `insert_node`, `insert_edge`, `update_node`, `update_edge`, `delete_node` (with cascade), `delete_edge`
- Schema operations work: `register_type`, `register_property_key`, type hierarchy queries
- The `GraphReader` trait is implemented by both transaction types
- The `GraphView` trait is implemented for the overlay view (snapshot + WriteBuffer)
- ChangeSet production works correctly at commit time
- Constraint validators are dispatched at commit time; violations reject the commit
- Extension registration/unregistration works on `Database`
- The write lock enforces single-writer semantics
- Concurrent access works: multiple readers + one writer
- Inference-related methods exist as stubs that Task 26 will complete
- Multi-hop traversals compose correctly from single-hop primitives

---

## Module Layout

```
src/db/
├── mod.rs              // Module-level re-exports, db module doc
├── config.rs           // DatabaseConfig, StorageMode, builder
├── database.rs         // Database struct, open/close, extension registration
├── read_txn.rs         // ReadTransaction: snapshot-based reads
├── write_txn.rs        // WriteTransaction: mutations + reads + commit
├── write_buffer.rs     // WriteBuffer: in-memory change tracking
├── schema_cache.rs     // SchemaCache: in-memory TypeRegistry + PropertyKeyRegistry
├── graph_reader.rs     // GraphReader trait impl for both transaction types
├── graph_view.rs       // GraphView impl: overlay of snapshot + WriteBuffer
└── builders.rs         // NodeBuilder, EdgeBuilder, TypeDefinitionBuilder
```

All modules in `src/db/` are gated behind `#[cfg(feature = "std")]`.

---

## Key Design Decisions to Follow

These decisions are settled in the design documents. Do not re-open them during implementation.

| Decision | Choice | Reference |
|----------|--------|-----------|
| Concurrency model | Single-writer MVCC via CoW snapshots | 012 §10.1, G7–G8 |
| Transaction isolation | Snapshot Isolation (effectively Serializable with single writer) | 012 §10.1 |
| Transaction API naming | `read_txn()`, `write_txn()` | 012 §16.2, A11 |
| `commit(self)` consumes | Transaction consumed on commit (success or failure) | 010 A2 |
| Owned returns from queries | `Vec<Node>`, `Vec<Edge>` (not borrowed) | 010 A5 |
| Extension registration | On `Database`, not in transactions | 010 A3 |
| Node deletion cascading | Cascade-delete all incident edges | 010 A9 |
| Edge endpoints immutable | `update_edge` ignores source/target changes | 010 A10 |
| `Database` thread safety | `Send + Sync` (internal Mutex/RwLock) | 012 §10.4, A12 |
| Transaction thread safety | `!Send`, `!Sync` | 012 §10.4, A12 |
| Validation at commit | ChangeSet built before B-tree materialization | 012 §11.2, G17 |
| `nodes_by_property` | Full scan in v1 (no property index) | 007 §7.5 |
| Parallel edges permitted | Multiple edges same src→tgt allowed | 010 §9.3 |
| No built-in multi-hop | Callers compose single-hop primitives | 010 §10.3 |
| `GraphReader` vs `GraphView` | GraphReader (public, owned, fallible) vs GraphView (internal, borrowed, infallible) | 010 §10.2 |
| `validate_all()` semantics | Synthetic full-insert ChangeSet | 010 A8 |

---

## Error Handling

The `db/` module uses the project's unified `Error` type. Specific mapping:

| Situation | Error variant |
|-----------|--------------|
| Storage engine I/O failure | `Error::Storage(StorageError)` |
| Node/edge not found (for update/delete) | `Error::NotFound(NotFoundError)` |
| Schema violation (duplicate type name, cycle) | `Error::Schema(SchemaError)` |
| Constraint violation at commit | `Error::ConstraintViolation(Vec<ConstraintViolation>)` |
| Transaction misuse (write on read-only) | `Error::Transaction(TransactionError)` |
| Inference rule not found (stub) | `Error::Inference(InferenceError)` |

---

## Testing Strategy

This task requires comprehensive testing because it validates the entire stack from public API through storage engine to disk.

**Categories of tests:**

1. **Unit tests** — Per-module tests for WriteBuffer, SchemaCache, ChangeSet production, GraphView overlay logic, builder ergonomics.

2. **Integration tests** — Full round-trip tests: open database → write transaction → insert data → commit → read transaction → verify data. These exercise the complete stack.

3. **Multi-hop traversal tests** — At least one complex scenario: build a graph with 4+ node types and 3+ edge types, then traverse multi-hop paths composing single-hop operations. Verify correct results.

4. **Concurrent access tests** — Multiple reader threads + one writer thread operating simultaneously. Verify snapshot isolation (readers don't see uncommitted writes), writer serialization (second writer blocks until first commits), and no data races.

5. **Crash recovery tests** (if applicable at this layer) — Verify that after opening a database, previously committed data is visible and uncommitted data is not.

6. **Cascading delete tests** — Delete a node with incident edges, verify all edges are removed.

7. **ChangeSet and constraint validation tests** — Register a mock constraint validator, insert violating data, verify commit is rejected with correct violations.

8. **Schema operation tests** — Register types with hierarchies, query subtypes, verify type-hierarchy-aware queries.

9. **Edge cases** — Null IDs, empty transactions (commit with no changes), property updates on nodes/edges, read-your-own-writes within a write transaction, parallel edges between same endpoints.

**Test organization:**
- Unit tests: `#[cfg(test)] mod tests` within each `db/` submodule
- Integration tests: `tests/db_integration.rs` (or multiple files)
- Concurrency tests: `tests/concurrency.rs`

**Verification:** `cargo test` must pass with zero failures after this task.

---

## Key Pitfalls and Edge Cases

1. **Read-your-own-writes overlay.** Within a `WriteTransaction`, reads must see pending changes overlaid on the base snapshot. This means `get_node` must check the WriteBuffer first, then fall back to the base B-tree. This overlay logic is the trickiest part of the module. The WriteBuffer check must handle: inserted nodes (return from buffer), updated nodes (return modified version from buffer), deleted nodes (return `None` even if base snapshot has it). The same applies to adjacency queries — `outgoing_edges` must merge edges from the base snapshot with pending edge inserts, exclude pending edge deletes, and apply pending edge updates.

2. **ChangeSet ordering.** The ChangeSet must accurately reflect all mutations in the order they produce correct before/after snapshots. If a node is inserted and then updated in the same transaction, the ChangeSet should contain only `NodeChange::Inserted(final_version)`, not both an insert and a modify. If a node is inserted and then deleted in the same transaction, it should not appear in the ChangeSet at all.

3. **Cascading edge deletion.** When `delete_node(id)` is called, all edges where `source == id` or `target == id` must also be deleted. These edge deletions must appear in the ChangeSet and must update the WriteBuffer's adjacency tracking.

4. **Type hierarchy resolution.** `nodes_by_type(type_id, include_subtypes=true)` requires resolving all subtypes of `type_id` from the in-memory type hierarchy, then performing a union of Type Index range scans. The SchemaCache must maintain a precomputed subtype map for efficient lookup.

5. **ID allocation.** New nodes and edges need IDs. The ID allocation scheme uses the Schema Store's ID counters (next_node_id, next_edge_id). Within a write transaction, IDs must be allocated monotonically and not reused within the same transaction. The Freelist provides recycled IDs, but in v1, simply incrementing the counter is acceptable.

6. **Schema modifications in a write transaction.** `register_type` and related methods modify the Schema Store B-tree. These changes must be part of the same transaction's commit. The SchemaCache must be updated to reflect pending schema changes for read-your-own-writes.

7. **`commit(self)` consumes the transaction.** After commit (whether success or constraint failure), the transaction is consumed. Per design decision A2, constraint violations consume the transaction — the caller builds a new transaction to retry. Implement `commit` as taking `self` by value.

8. **Drop-based abort.** If a `WriteTransaction` is dropped without calling `commit()`, it must automatically abort — release the write mutex, discard the WriteBuffer, decrement the snapshot reference. Implement via `Drop`.

9. **`validate()` dry-run.** The `validate()` method on `WriteTransaction` runs validators against pending changes without committing. It must build a temporary ChangeSet and call validators, but not trigger any B-tree materialization.

10. **`validate_all()` performance.** This synthesizes a full-insert ChangeSet by treating every node and edge in the database as newly inserted. For large databases this is O(N). Document this clearly.

11. **Extension names persistence.** When a constraint validator or inference rule is registered, its name is persisted in the Schema Store. On database open, names are loaded. `missing_extensions()` compares persisted names against currently registered in-memory extensions.

12. **Inference stubs.** `run_inference` and `run_all_inference` must exist on both transaction types but should return `Error::Inference(InferenceError::RuleNotFound(...))` or similar until Task 26 fills in the implementation. The `Database` struct should have an `inference_engine` field that Task 26 will populate.

---

## Out of Scope

- `hal_mem/` (MemoryBackend) — Task 27
- Inference engine implementation (`InferenceEngine`, `InferenceCache`, `ProvenanceRegistry`) — Task 26
- Provenance query implementation (`is_inferred_node`, `node_provenance`, etc.) — Task 26
- Integration testing across all subsystems — Task 28
- Documentation and publish preparation — Task 29

---

## Definition of Done

All of the following must be true before this task is complete:

- [ ] `src/db/mod.rs` exists with module structure and public re-exports
- [ ] `src/db/config.rs` implements `DatabaseConfig`, `StorageMode`, and builder
- [ ] `src/db/database.rs` implements `Database` struct with `open()`, `read_txn()`, `write_txn()`, extension registration, `missing_extensions()`, and `Drop`
- [ ] `src/db/read_txn.rs` implements `ReadTransaction` with all read methods from `010-api-surface-spec.md` §6.1
- [ ] `src/db/write_txn.rs` implements `WriteTransaction` with all read + mutation + schema + validation + commit/abort methods from `010-api-surface-spec.md` §6.2
- [ ] `src/db/write_buffer.rs` implements `WriteBuffer` with change tracking, overlay logic, and ChangeSet production
- [ ] `src/db/schema_cache.rs` implements in-memory `SchemaCache` with `TypeRegistryView` and `PropertyKeyRegistryView`
- [ ] `src/db/graph_reader.rs` (or inline in transaction modules) implements the `GraphReader` trait for both transaction types
- [ ] `src/db/graph_view.rs` implements `GraphView` as the snapshot + WriteBuffer overlay for validators/inference
- [ ] `src/db/builders.rs` implements `NodeBuilder`, `EdgeBuilder`, `TypeDefinitionBuilder`
- [ ] `Database` is `Send + Sync`; transactions are `!Send`, `!Sync`
- [ ] Single-writer concurrency enforced via write mutex
- [ ] Snapshot isolation: readers see consistent snapshots unaffected by concurrent writes
- [ ] Read-your-own-writes: write transaction reads see pending changes
- [ ] Cascading node deletion removes all incident edges
- [ ] ChangeSet correctly produced from WriteBuffer at commit time
- [ ] Constraint validators dispatched at commit; violations reject commit
- [ ] At least one multi-hop traversal test (4+ hops, 3+ edge types)
- [ ] Concurrent access tests: multiple readers + one writer
- [ ] Inference methods exist as stubs (return `Error::Inference` or placeholder)
- [ ] `cargo check` succeeds
- [ ] `cargo check --no-default-features --features alloc` succeeds (db module not compiled, no breakage)
- [ ] `cargo test` passes — all tests green
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` — zero warnings
- [ ] `cargo doc --no-deps` — zero warnings; every `pub` item in `src/db/` has a doc comment
