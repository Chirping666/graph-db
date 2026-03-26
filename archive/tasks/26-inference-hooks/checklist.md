# Checklist: Task 26 — Implement Inference Hook Infrastructure

**Parent:** Task 18 (this checklist)  
**Implements:** `InferenceEngine`, `InferenceCache`, `ProvenanceRegistry` in `src/db/inference_engine.rs`; inference dispatch in `src/db/read_txn.rs` and `src/db/write_txn.rs`; provenance persistence via Schema Store B-tree; test example rule in `tests/`.

Execute items in order. After each item, run the verification command(s) listed. Do not proceed until verification passes.

---

## Phase 0: Review Existing Stubs and Infrastructure

### 0.1 — Audit existing inference stubs

Before writing any code, examine the current state of the codebase:

- `src/db/` — Locate the existing `Database` struct, `ReadTransaction`, `WriteTransaction`.
- Identify the existing stubs for `run_inference`, `run_all_inference`, `register_inference_rule`, `unregister_inference_rule`, and the `inference_engine` field on `Database` / `DatabaseInner`.
- Locate the `GraphView` implementation that transactions provide to validators — the same implementation will be passed to inference rules.
- Check how `DatabaseConfig` currently handles `inference_cache_size`.
- Examine the Schema Store B-tree's key encoding to confirm prefix `0x06` is available for provenance records.

Document any deviations from the design documents in your session plan. Do not modify code in this step.

**Verify:** You can describe the current inference stub locations and the `GraphView` implementation that will be reused.

---

## Phase 1: InferenceCache

### 1.1 — Implement InferenceCache struct

Create `src/db/inference_engine.rs` (or add to the existing file if one was stubbed). Define:

```rust
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// In-memory LRU cache for inference results.
///
/// Keyed by `(rule_name, data_generation)`. Bounded by `max_entries`.
/// Not persisted to disk. When the cache is full, the least recently
/// used entry is evicted.
pub(crate) struct InferenceCache {
    /// Cache entries: key → (result, access_order_counter).
    entries: BTreeMap<(String, u64), CacheEntry>,
    /// Maximum number of entries. 0 = caching disabled.
    max_entries: usize,
    /// Monotonically increasing counter for LRU ordering.
    access_counter: u64,
}

struct CacheEntry {
    result: InferenceResult,
    last_accessed: u64,
}
```

Implement:

```rust
impl InferenceCache {
    pub(crate) fn new(max_entries: usize) -> Self { ... }

    /// Look up a cached result. Returns `Some(result.clone())` on hit,
    /// `None` on miss. Updates the LRU access counter on hit.
    pub(crate) fn get(
        &mut self,
        rule_name: &str,
        generation: u64,
    ) -> Option<InferenceResult> { ... }

    /// Insert a result into the cache. If full, evicts the LRU entry.
    /// No-op if max_entries == 0.
    pub(crate) fn insert(
        &mut self,
        rule_name: String,
        generation: u64,
        result: InferenceResult,
    ) { ... }

    /// Clear all entries. Called when the cache should be fully invalidated.
    pub(crate) fn clear(&mut self) { ... }
}
```

**⚠ Pitfall — LRU eviction with `BTreeMap`:** The cache uses a `BTreeMap` keyed by `(rule_name, generation)` for O(log N) lookup. For LRU eviction, you need to find the entry with the smallest `last_accessed`. One approach: maintain a separate `BTreeMap<u64, (String, u64)>` mapping `access_counter → cache_key` for efficient eviction. Alternatively, scan the entries on eviction (acceptable since `max_entries` defaults to 64). Choose the simpler approach for v1.

**⚠ Pitfall — `InferenceResult` must be `Clone`.** The cache returns cloned results. Verify that `InferenceResult` (from Task 22) derives `Clone`. If it doesn't, this is a bug in the types task.

**Verify:** `cargo check`

### 1.2 — Unit tests for InferenceCache

In a `#[cfg(test)] mod tests` block within the inference engine module:

- Test `new(0)` creates a disabled cache; `get()` always returns `None`, `insert()` is a no-op.
- Test `new(2)` with two inserts, then two gets return the correct results.
- Test LRU eviction: insert 3 entries into a cache with `max_entries = 2`, verify the least recently used entry is evicted.
- Test that accessing an entry updates its LRU priority (prevents eviction).
- Test `clear()` empties the cache.
- Test cache miss on wrong generation: insert at generation 5, get at generation 6 returns `None`.
- Test cache miss on wrong rule name.

**Verify:** `cargo test -- inference_engine` passes.

---

## Phase 2: ProvenanceRegistry

### 2.1 — Implement ProvenanceRegistry struct

In `src/db/inference_engine.rs`:

```rust
/// Tracks which entities in the graph were produced by inference rules.
///
/// Maintained in memory with persistence to the Schema Store B-tree.
/// Loaded from disk at database open; updated during materialization.
pub(crate) struct ProvenanceRegistry {
    /// Forward index: entity → provenance record.
    by_entity: BTreeMap<InferredEntity, ProvenanceRecord>,
    /// Reverse index: rule_name → entities produced by that rule.
    by_rule: BTreeMap<String, Vec<InferredEntity>>,
}
```

Implement:

```rust
impl ProvenanceRegistry {
    pub(crate) fn new() -> Self { ... }

    /// Record that `entity` was produced by `rule_name` at `txn_id`.
    pub(crate) fn record(
        &mut self,
        entity: InferredEntity,
        rule_name: &str,
        txn_id: u64,
    ) { ... }

    /// Remove all provenance records for entities produced by `rule_name`.
    /// Returns the removed entities (for cleanup from the WriteBuffer).
    pub(crate) fn remove_by_rule(
        &mut self,
        rule_name: &str,
    ) -> Vec<InferredEntity> { ... }

    /// Look up provenance of a specific entity. None if user-asserted.
    pub(crate) fn get(
        &self,
        entity: &InferredEntity,
    ) -> Option<&ProvenanceRecord> { ... }

    /// Check whether a specific entity was produced by inference.
    pub(crate) fn is_inferred(
        &self,
        entity: &InferredEntity,
    ) -> bool { ... }

    /// Return all entities produced by a specific rule.
    pub(crate) fn entities_by_rule(
        &self,
        rule_name: &str,
    ) -> &[InferredEntity] { ... }
}
```

**⚠ Pitfall — `entities_by_rule` empty case.** If the rule has no entities, return an empty slice. Use a static empty `Vec` or return `&[]` from a default. Do not unwrap or panic.

**⚠ Pitfall — Reverse index consistency.** Every `record()` must insert into both `by_entity` and `by_rule`. Every `remove_by_rule()` must clean up both maps. If the entity already exists in `by_entity` (e.g., re-materialization without cleanup), the old record should be replaced and the reverse index updated. This scenario should not occur if the caller follows the cleanup-before-insert protocol, but defensive coding is warranted.

**Verify:** `cargo check`

### 2.2 — Unit tests for ProvenanceRegistry

- Test `record()` followed by `get()` returns the correct record.
- Test `is_inferred()` returns `true` for recorded entities, `false` for others.
- Test `remove_by_rule()` removes all entities for a rule and returns them.
- Test `remove_by_rule()` on a non-existent rule returns an empty vec.
- Test `entities_by_rule()` returns correct entities, and empty slice for unknown rules.
- Test multiple rules: entities from rule A are not returned by `remove_by_rule("B")`.
- Test recording an entity twice for the same rule replaces the record.

**Verify:** `cargo test -- inference_engine` passes.

---

## Phase 3: Provenance Persistence

### 3.1 — Implement provenance serialization

Add methods to `ProvenanceRegistry` for converting to/from the Schema Store B-tree key/value format.

**Key encoding** (from `012-design-document.md` §19.2, `011-inference-hook-design.md` §8.4):

```
Key: [0x06: 1B] [entity_kind: 1B] [entity_id: 8B BE u64] [sub_id: 4B BE u32]
  entity_kind: 0x01=Node, 0x02=Edge, 0x03=NodeProperty,
               0x04=EdgeProperty, 0x05=NodeType, 0x06=EdgeType
  entity_id: NodeId or EdgeId as big-endian u64
  sub_id: PropertyKeyId or TypeId as big-endian u32; zero for Node/Edge entities
Total key: 14 bytes (fixed)

Value: [txn_id: 8B LE u64] [rule_name_len: 2B LE u16] [rule_name: UTF-8 bytes]
Total value: 10 + rule_name_len bytes (variable)
```

Implement:

```rust
impl ProvenanceRegistry {
    /// Encode a single provenance entry as (key_bytes, value_bytes)
    /// for writing to the Schema Store B-tree.
    pub(crate) fn encode_entry(
        entity: &InferredEntity,
        record: &ProvenanceRecord,
    ) -> (Vec<u8>, Vec<u8>) { ... }

    /// Decode a provenance entry from Schema Store key/value bytes.
    /// Returns None if the key does not start with prefix 0x06 or is malformed.
    pub(crate) fn decode_entry(
        key: &[u8],
        value: &[u8],
    ) -> Option<(InferredEntity, ProvenanceRecord)> { ... }

    /// Populate this registry from an iterator of raw (key, value) pairs
    /// read from the Schema Store B-tree during database open.
    pub(crate) fn load_from_entries(
        &mut self,
        entries: impl Iterator<Item = (Vec<u8>, Vec<u8>)>,
    ) { ... }

    /// Return all entries as encoded (key, value) pairs for writing
    /// to the Schema Store B-tree. Used during full provenance flush.
    pub(crate) fn to_entries(&self) -> Vec<(Vec<u8>, Vec<u8>)> { ... }
}
```

**⚠ Pitfall — Endianness.** Keys use big-endian for IDs (to maintain sort order in the B-tree). Values use little-endian for txn_id (matching the design doc's convention for values). Do not mix them up.

**⚠ Pitfall — `sub_id` for `InferredEntity::Node` and `InferredEntity::Edge`.** These have no sub-ID; encode `sub_id` as `0u32`. For `NodeProperty` / `EdgeProperty`, encode the `PropertyKeyId` (which is a `u32` internally). For `NodeType` / `EdgeType`, encode the `TypeId`. Check whether `TypeId` and `PropertyKeyId` fit in 4 bytes — the design uses `u32` for these but the newtype wraps a `u64`. The Schema Store key encoding explicitly uses 4 bytes (`u32`) for the sub_id field. If the inner ID type is `u64`, you must truncate or the design has a mismatch to note. Consult the design document's key encoding spec carefully.

**Verify:** `cargo check`

### 3.2 — Round-trip tests for provenance serialization

- Test encode → decode round-trip for each `InferredEntity` variant.
- Test that keys are 14 bytes and start with `0x06`.
- Test that keys sort correctly in B-tree order (Node < Edge < NodeProperty, etc.).
- Test `load_from_entries` populates both forward and reverse indexes.
- Test `to_entries` → `load_from_entries` round-trip preserves all data.
- Test `decode_entry` returns `None` for keys with wrong prefix.
- Test `decode_entry` returns `None` for truncated keys.

**Verify:** `cargo test -- inference_engine` passes.

---

## Phase 4: InferenceEngine Assembly

### 4.1 — Implement InferenceEngine struct

Assemble the three sub-components:

```rust
/// The internal inference engine. Owned by the Database.
///
/// Manages rule registration, caching, and provenance tracking.
pub(crate) struct InferenceEngine {
    rules: BTreeMap<String, Box<dyn InferenceRule>>,
    cache: InferenceCache,
    provenance: ProvenanceRegistry,
}
```

Implement:

```rust
impl InferenceEngine {
    pub(crate) fn new(cache_size: usize) -> Self { ... }

    // --- Rule registry ---

    /// Register an inference rule. Replaces any existing rule with the same name.
    pub(crate) fn register_rule(&mut self, rule: Box<dyn InferenceRule>) {
        let name = rule.name().to_string();
        self.rules.insert(name, rule);
    }

    /// Unregister an inference rule. Returns true if found and removed.
    pub(crate) fn unregister_rule(&mut self, name: &str) -> bool {
        self.rules.remove(name).is_some()
    }

    /// Return the names of all registered rules.
    pub(crate) fn rule_names(&self) -> Vec<String> { ... }

    /// Look up a rule by name. Returns None if not registered.
    pub(crate) fn get_rule(&self, name: &str) -> Option<&dyn InferenceRule> { ... }

    // --- Cache delegation ---

    pub(crate) fn cache_get(
        &mut self, rule_name: &str, generation: u64,
    ) -> Option<InferenceResult> { ... }

    pub(crate) fn cache_insert(
        &mut self, rule_name: String, generation: u64, result: InferenceResult,
    ) { ... }

    // --- Provenance delegation ---

    pub(crate) fn provenance(&self) -> &ProvenanceRegistry { ... }
    pub(crate) fn provenance_mut(&mut self) -> &mut ProvenanceRegistry { ... }
}
```

**⚠ Pitfall — Locking.** The `InferenceEngine` itself does not hold a `RwLock`. The `Database` struct wraps it in a `RwLock<InferenceEngine>` (or equivalent synchronization). The engine's methods assume the caller has already acquired the appropriate lock. This mirrors the pattern used for constraint validators.

**Verify:** `cargo check`

### 4.2 — Wire InferenceEngine into Database

Modify the `Database` / `DatabaseInner` struct to hold the `InferenceEngine`:

- Replace any existing inference stub field with `RwLock<InferenceEngine>`.
- In `Database::open()`, create the `InferenceEngine` with `config.inference_cache_size`.
- In `Database::open()`, after loading the Schema Store, iterate provenance records (key prefix `0x06`) and call `provenance.load_from_entries()`.
- Update `Database::register_inference_rule()` to acquire a write lock on the engine and call `engine.register_rule()`.
- Update `Database::unregister_inference_rule()` similarly.
- Update `Database::inference_rule_names()` to acquire a read lock and delegate.
- Update `Database::missing_extensions()` to compare persisted rule names against `engine.rule_names()`.

**⚠ Pitfall — Persisting rule names.** Rule names are written to the Schema Store during the next write transaction commit. The `Database` must track which rule names need to be persisted. This may already be implemented from Task 25's stub. If not, implement the tracking mechanism: maintain a set of registered rule names that should be persisted, and during commit, ensure the Schema Store records them.

**Verify:** `cargo test` — existing tests still pass. `cargo check`.

---

## Phase 5: Inference Dispatch — Ephemeral Mode

### 5.1 — Implement ephemeral inference on ReadTransaction

Replace the `run_inference` stub on `ReadTransaction`:

```rust
impl<'db> ReadTransaction<'db> {
    pub fn run_inference(&self, rule_name: &str) -> Result<InferenceResult, Error> {
        // 1. Acquire read lock on InferenceEngine.
        // 2. Look up rule by name → Error::Inference(RuleNotFound) if missing.
        // 3. Check cache for (rule_name, self.snapshot_generation()).
        //    → On hit: return cached result (clone).
        // 4. On cache miss: construct GraphView from snapshot.
        //    Invoke rule.infer(graph_view, type_registry, key_registry).
        // 5. Store result in cache.
        // 6. Return result.
    }
}
```

**⚠ Pitfall — GraphView construction.** The `GraphView` for a read transaction is the snapshot alone (no WriteBuffer overlay). Reuse the same `GraphView` construction used for constraint validators or graph queries. Ensure the `TypeRegistryView` and `PropertyKeyRegistryView` are constructed from the same snapshot.

**⚠ Pitfall — Lock scope.** The read lock on the `InferenceEngine` must be held for the duration of steps 2–5 (including rule invocation), because the cache write in step 5 requires mutable access. If using `RwLock<InferenceEngine>`, you'll need a write lock (or interior mutability for the cache). Design decision: upgrade to write lock for cache miss path, or use a `Mutex` instead of `RwLock`. The simplest correct approach is `Mutex<InferenceEngine>` since inference invocation is not a hot path. If you choose `RwLock`, the cache must use interior mutability (`RefCell` or `Mutex<InferenceCache>` inside the engine).

**Verify:** `cargo check`

### 5.2 — Implement `run_all_inference` on ReadTransaction

```rust
impl<'db> ReadTransaction<'db> {
    pub fn run_all_inference(&self) -> Result<Vec<InferenceResult>, Error> {
        // 1. Acquire lock on InferenceEngine.
        // 2. Collect rule names in registration order.
        // 3. For each rule, call run_inference logic (steps 2-6 above).
        //    Collect results into a Vec.
        // 4. Return Vec<InferenceResult>.
    }
}
```

**⚠ Pitfall — Registration order.** `BTreeMap` iterates in alphabetical order, not insertion order. If registration order must be preserved (for deterministic chaining in `run_all_inference`), the `InferenceEngine` needs a secondary `Vec<String>` tracking insertion order, or use an `IndexMap`-like structure. The design says "registration order" (011 §6.4, decision I7). Implement this by maintaining a `Vec<String>` of rule names in insertion order alongside the `BTreeMap`. When a rule is replaced, its position in the order vector does not change.

**Verify:** `cargo check`

### 5.3 — Tests for ephemeral inference

Write tests in `tests/inference_tests.rs` (integration test outside `src/`):

**Test: `ephemeral_inference_returns_facts`**
- Open a database, insert some nodes and edges, commit.
- Register a test rule (see Phase 8 for the example rule definition).
- Open a read transaction, call `run_inference("test_rule")`.
- Verify the result contains expected inferred facts.
- Verify no new nodes or edges appear in the database (ephemeral = no side effects).

**Test: `ephemeral_inference_unknown_rule_returns_error`**
- Open a database.
- Open a read transaction, call `run_inference("nonexistent")`.
- Verify `Error::Inference(RuleNotFound("nonexistent"))` is returned.

**Test: `run_all_inference_ephemeral`**
- Register two rules.
- Open a read transaction, call `run_all_inference()`.
- Verify results from both rules are returned in registration order.

**Verify:** `cargo test -- inference_tests` passes.

---

## Phase 6: Inference Dispatch — Materialized Mode

### 6.1 — Implement materialized inference on WriteTransaction

Replace the `run_inference` stub on `WriteTransaction`:

```rust
impl<'db> WriteTransaction<'db> {
    pub fn run_inference(
        &mut self,
        rule_name: &str,
        mode: InferenceMode,
    ) -> Result<InferenceResult, Error> {
        // 1. Acquire lock on InferenceEngine.
        // 2. Look up rule by name → Error::Inference(RuleNotFound) if missing.
        // 3. If NOT dirty AND cache has (rule_name, generation):
        //    → Ephemeral: return cached result.
        //    → Materialized: use cached result, skip to step 5.
        //    If dirty: always bypass cache (proceed to step 4).
        // 4. Construct GraphView from snapshot + WriteBuffer overlay.
        //    Invoke rule.infer(graph_view, type_registry, key_registry).
        //    If NOT dirty: store result in cache.
        // 5. If mode == Ephemeral: return result.
        // 6. If mode == Materialized:
        //    a. Validate each InferredFact (step 6a below).
        //    b. Clean up previously materialized facts:
        //       provenance.remove_by_rule(rule_name) → list of entities
        //       For each entity: delete from WriteBuffer.
        //    c. Write new facts to WriteBuffer, assign IDs:
        //       For each InferredFact: insert into WriteBuffer.
        //       Build MaterializedMapping with assigned IDs.
        //    d. Record provenance for each new entity.
        //    e. Store the MaterializedMapping (for last_materialization_mapping).
        //    f. Mark transaction as dirty.
        //    g. Return result.
    }
}
```

**⚠ Pitfall — Fact validation (step 6a).** Before materializing, validate each `InferredFact`:
- `NewNode`: Verify that all `type_labels` are registered node types.
- `NewEdge`: Verify that `source` and `target` nodes exist (in snapshot or WriteBuffer), and all `type_labels` are registered edge types.
- `NodePropertyUpdate`: Verify the node exists and the property key is registered.
- `EdgePropertyUpdate`: Verify the edge exists and the property key is registered.
- `NodeTypeAssignment`: Verify the node exists and the type is a registered node type.
- `EdgeTypeAssignment`: Verify the edge exists and the type is a registered edge type.
Return `Error::Inference(InvalidFact { rule_name, message })` on failure. Stop on the first invalid fact (do not partially materialize).

**⚠ Pitfall — Cleanup entity deletion.** When cleaning up `InferredEntity::Node(id)`, the deletion must cascade: remove the node and all its edges. Use the same cascading delete logic used by `WriteTransaction::delete_node()`. For `InferredEntity::Edge(id)`, delete only the edge. For property/type entities, remove just the specific property or type label.

**⚠ Pitfall — ID assignment for new nodes/edges.** Use the same ID allocation mechanism used by `WriteTransaction::insert_node()` and `WriteTransaction::insert_edge()`. The `InferredFact::NewNode` specifies a node with `id: NodeId(0)` — the real ID is assigned during materialization.

**⚠ Pitfall — Dirty flag.** After materialization modifies the WriteBuffer, the transaction becomes dirty. Any subsequent inference calls in the same transaction must bypass the cache.

**Verify:** `cargo check`

### 6.2 — Implement `run_all_inference` on WriteTransaction

```rust
impl<'db> WriteTransaction<'db> {
    pub fn run_all_inference(
        &mut self,
        mode: InferenceMode,
    ) -> Result<Vec<InferenceResult>, Error> {
        // Execute rules sequentially in registration order.
        // Each rule sees the results of prior rules (if materialized).
        // Collect and return all results.
    }
}
```

**⚠ Pitfall — Sequential chaining.** When `mode` is `Materialized`, each rule's output is written to the WriteBuffer before the next rule runs. This means rule B's `GraphView` includes rule A's materialized facts. This is the documented behavior for rule chaining (012 §14.9).

**Verify:** `cargo check`

### 6.3 — Implement `last_materialization_mapping`

```rust
impl<'db> WriteTransaction<'db> {
    pub fn last_materialization_mapping(&self) -> Option<&MaterializedMapping> {
        // Return the mapping from the most recent materialized run,
        // or None if no materialized run has occurred.
    }
}
```

Store the `MaterializedMapping` as a field on `WriteTransaction`. Reset it at the start of each `run_inference` call (so it reflects only the most recent run).

**Verify:** `cargo check`

### 6.4 — Tests for materialized inference

**Test: `materialized_inference_writes_to_graph`**
- Open a database, register types, insert seed data, commit.
- Register a test rule that infers new nodes and edges.
- Open a write transaction, call `run_inference("test_rule", Materialized)`.
- Verify the inferred nodes/edges are visible in the write transaction (read-your-own-writes).
- Commit. Open a new read transaction and verify the inferred data persists.

**Test: `materialized_mapping_has_assigned_ids`**
- After a materialized inference run, call `last_materialization_mapping()`.
- Verify it contains the expected `(index, NodeId)` and `(index, EdgeId)` pairs.
- Verify the assigned IDs are non-null and different from seed data IDs.

**Test: `re_inference_cleans_up_old_facts`**
- Run a rule in materialized mode. Note the inferred node IDs.
- Modify the seed data (change a property).
- Run the same rule in materialized mode again.
- Verify the old inferred nodes are gone and new ones are present.
- Verify provenance reflects only the new facts.

**Test: `ephemeral_mode_in_write_transaction`**
- In a write transaction, call `run_inference("test_rule", Ephemeral)`.
- Verify facts are returned but no new nodes/edges appear in the transaction.

**Test: `invalid_fact_returns_error`**
- Register a rule that produces a `NewEdge` referencing a non-existent source node.
- Call `run_inference` in `Materialized` mode.
- Verify `Error::Inference(InvalidFact { ... })` is returned.
- Verify no partial materialization occurred (no orphaned data).

**Test: `dirty_transaction_bypasses_cache`**
- Open a write transaction. Run inference (ephemeral) — result is cached.
- Insert a new node (makes the transaction dirty).
- Run inference (ephemeral) again.
- Verify the second run re-invokes the rule (seeing the new node), not the cached result.

**Verify:** `cargo test -- inference_tests` passes.

---

## Phase 7: Provenance Query API

### 7.1 — Implement provenance queries on ReadTransaction

```rust
impl<'db> ReadTransaction<'db> {
    /// Returns true if the given node was created by an inference rule.
    pub fn is_inferred_node(&self, id: NodeId) -> bool { ... }

    /// Returns true if the given edge was created by an inference rule.
    pub fn is_inferred_edge(&self, id: EdgeId) -> bool { ... }

    /// Returns the provenance record for an inferred node, or None
    /// if the node was user-asserted.
    pub fn node_provenance(&self, id: NodeId) -> Option<ProvenanceRecord> { ... }

    /// Returns the provenance record for an inferred edge, or None
    /// if the edge was user-asserted.
    pub fn edge_provenance(&self, id: EdgeId) -> Option<ProvenanceRecord> { ... }
}
```

These delegate to the `ProvenanceRegistry` held by the `InferenceEngine`. Acquire a read lock on the engine.

**⚠ Pitfall — Provenance and transaction isolation.** The provenance registry is an in-memory structure that reflects the committed state plus any pending materializations in the current write transaction. For read transactions, provenance reflects the committed state only. If a write transaction has materialized facts but not yet committed, those provenance records are visible in that write transaction but not in concurrent read transactions. Ensure the provenance queries on `WriteTransaction` also check the pending provenance updates (stored in the WriteBuffer or a transaction-local provenance diff).

**Verify:** `cargo check`

### 7.2 — Implement provenance queries on WriteTransaction

Same four methods as on `ReadTransaction`, but also check pending (uncommitted) provenance records from materialization in this transaction.

**Verify:** `cargo check`

### 7.3 — Tests for provenance queries

**Test: `provenance_for_inferred_node`**
- Materialize inference, commit. Open a read transaction.
- `is_inferred_node(inferred_id)` → `true`.
- `is_inferred_node(user_node_id)` → `false`.
- `node_provenance(inferred_id)` → `Some(ProvenanceRecord { rule_name: "test_rule", ... })`.

**Test: `provenance_for_inferred_edge`**
- Same pattern for edges.

**Test: `provenance_persists_across_sessions`**
- Materialize, commit, close database.
- Reopen database (without re-registering the rule).
- Open a read transaction.
- Provenance queries still return correct results.

**Test: `provenance_after_re_inference`**
- Materialize rule A. Commit.
- Open new write transaction. Re-run rule A materialized.
- Verify old inferred entities no longer have provenance; new ones do.

**Verify:** `cargo test -- inference_tests` passes.

---

## Phase 8: Test Example Rule

### 8.1 — Define a minimal test example rule

In `tests/inference_tests.rs` (or a `tests/common/` helper module), define:

```rust
/// A minimal inference rule for testing the infrastructure.
///
/// This rule is NOT part of the public API. It exists solely in test code.
///
/// Behavior: For every node with a property "source" = true,
/// infer a new edge from that node to every node with property
/// "target" = true, with edge type "inferred_link" (if registered).
/// Also infers a property "inferred" = true on the source node.
struct TestInferenceRule {
    edge_type_id: TypeId,
    source_key: PropertyKeyId,
    target_key: PropertyKeyId,
    inferred_key: PropertyKeyId,
}

impl InferenceRule for TestInferenceRule {
    fn name(&self) -> &str { "test_inference_rule" }

    fn applies_to_types(&self) -> Option<Vec<TypeId>> { None }

    fn infer(
        &self,
        graph: &dyn GraphView,
        _types: &dyn TypeRegistryView,
        _keys: &dyn PropertyKeyRegistryView,
    ) -> InferenceResult {
        let mut facts = Vec::new();
        // Find source nodes and target nodes
        // For each (source, target) pair, emit InferredFact::NewEdge
        // For each source, emit InferredFact::NodePropertyUpdate
        // Return InferenceResult { facts, rule_name: self.name().to_string() }
        // ...
    }
}
```

**⚠ Important — External implementability.** This rule is defined in `tests/`, outside `src/`. This proves that the `InferenceRule` trait is implementable by external code. The rule must compile using only the crate's public API — no `pub(crate)` internal access.

**⚠ Important — All InferredFact variants.** The test rule should exercise as many `InferredFact` variants as practical. At minimum: `NewNode`, `NewEdge`, `NodePropertyUpdate`. If feasible, also exercise `EdgePropertyUpdate`, `NodeTypeAssignment`, `EdgeTypeAssignment` (e.g., by defining additional simple rules or extending the test rule).

**Verify:** `cargo test -- inference_tests` passes.

### 8.2 — Define a second test rule for chaining

Define a second rule that depends on the first rule's output:

```rust
/// A rule that reads the "inferred" property set by TestInferenceRule
/// and infers additional facts. Used to verify sequential chaining
/// in run_all_inference.
struct ChainingTestRule { ... }

impl InferenceRule for ChainingTestRule {
    fn name(&self) -> &str { "chaining_test_rule" }
    // ...
}
```

**Verify:** `cargo check`

---

## Phase 9: Inference-Only-On-Request Verification

### 9.1 — Test: no automatic inference triggers

**Test: `no_automatic_inference_on_insert`**
- Register a test rule. Insert nodes and edges. Commit.
- Verify no inferred facts exist (provenance is empty).
- Only after explicitly calling `run_inference` should inferred facts appear.

**Test: `no_automatic_inference_on_commit`**
- Register a test rule. Open write transaction. Insert data. Commit.
- Open a read transaction. Query the graph.
- Verify no inferred nodes/edges exist.

**Test: `no_automatic_inference_on_reopen`**
- Create a database with registered rule and data. Close and reopen.
- Open a read transaction. Query the graph.
- Verify no inferred facts exist (previously materialized facts from the first session should still be present if they were committed, but no NEW inference runs automatically).

**Verify:** `cargo test -- inference_tests` passes.

---

## Phase 10: Constraint Validation Interaction

### 10.1 — Test: materialized facts are validated at commit time

**Test: `materialized_facts_pass_constraint_validation`**
- Register a constraint validator that checks all edges have source and target of specific types.
- Register an inference rule that creates properly typed edges.
- Materialize inference. Commit.
- Verify commit succeeds.

**Test: `invalid_materialized_facts_fail_validation`**
- Register a strict constraint validator.
- Register an inference rule that creates edges violating the constraint.
- Materialize inference. Attempt to commit.
- Verify commit is rejected with constraint violations.

**Verify:** `cargo test -- inference_tests` passes.

---

## Phase 11: Sequential Rule Execution Order

### 11.1 — Test: `run_all_inference` respects registration order

**Test: `rule_chaining_in_run_all_inference`**
- Register `TestInferenceRule` first, then `ChainingTestRule`.
- Open a write transaction, call `run_all_inference(Materialized)`.
- Verify `ChainingTestRule` sees the facts materialized by `TestInferenceRule`.
- Verify results are returned in registration order.

**Test: `run_all_inference_registration_order_not_alphabetical`**
- Register rule "beta" first, then rule "alpha".
- Call `run_all_inference`. Verify "beta" runs first.

**Verify:** `cargo test -- inference_tests` passes.

---

## Phase 12: Provenance Persistence Integration

### 12.1 — Wire provenance into the commit path

During `WriteTransaction::commit()`:

- Serialize the current transaction's provenance changes (new records and deletions) as Schema Store B-tree entries.
- Write them to the Schema Store B-tree alongside other schema metadata.
- The provenance entries use key prefix `0x06`.

**⚠ Pitfall — Incremental vs. full flush.** The simplest approach is to write only the changed provenance records (new records from materialization, removed records from cleanup). This requires tracking which provenance records were added/removed during this transaction. An alternative is to flush the entire provenance registry on every commit, but this is wasteful if provenance hasn't changed.

**Verify:** `cargo test` — all existing tests still pass.

### 12.2 — Wire provenance into the database open path

During `Database::open()`:

- After opening the file and loading the Schema Store, scan for entries with key prefix `0x06`.
- Call `ProvenanceRegistry::load_from_entries()` with the decoded entries.

**Verify:** `cargo test` — provenance persistence tests from Phase 7.3 pass.

---

## Phase 13: Final Verification

### 13.1 — Full test suite

```
cargo test
```

All tests pass, zero failures.

### 13.2 — Clippy

```
cargo clippy --all-targets --all-features -- -D warnings
```

Zero warnings.

### 13.3 — Documentation

```
cargo doc --no-deps
```

Zero warnings. Every `pub` item has a doc comment. Internal (`pub(crate)`) items should also have doc comments for maintainability.

### 13.4 — Verify no_std boundary is preserved

```
cargo check --no-default-features --features alloc
```

The `InferenceEngine`, `InferenceCache`, and `ProvenanceRegistry` live in `src/db/` which is `std`-gated. The inference *types* (`InferenceRule`, `InferredFact`, etc.) in `src/inference/` remain `no_std + alloc`. Verify no `no_std` module was accidentally polluted with `std` imports.

### 13.5 — Review against design documents

Manually verify:

- `InferenceEngine` struct matches `011-inference-hook-design.md` §5.
- Dispatch flow matches `011` §6.2 pseudocode.
- Cache behavior matches `011` §9 (generation keying, LRU, dirty bypass, max_entries config).
- Provenance key encoding matches `012` §19.2 (prefix `0x06`, 14-byte key, LE value).
- Materialization lifecycle matches `011` §10 (cleanup → validate → insert → provenance).
- Fact validation matches `011` §11.
- All four entry points are implemented per `010` §6.
- Sequential rule execution in `run_all_inference` per `011` §6.4.
- No automatic inference triggers anywhere in the codebase.

Document any intentional deviations from the spec in the completion report.

---

## Post-Completion

Produce a completion report following the format in the master project prompt's Instance Rules section. Include the verification evidence from Phase 13.
