# 011 — Inference Hook Architecture Design Specification

**Project:** Embedded Graph Database with Extensible Schema & Pluggable Inference  
**Task:** 11 — Design: Inference Hook Architecture  
**Status:** Complete  
**Depends on:** Task 6 (`006-schema-extension-spec.md`), Task 10 (`010-api-surface-spec.md`)  
**Intended audience:** All downstream design and implementation tasks (especially Tasks 12, 18, 20). A reader familiar with Rust and the upstream specifications should be able to implement the inference engine described here without reference to external sources.

---

## Table of Contents

1. [Purpose and Scope](#1-purpose-and-scope)
2. [Design Principles](#2-design-principles)
3. [Architecture Overview](#3-architecture-overview)
4. [The InferenceRule Trait (Recap)](#4-the-inferencerule-trait-recap)
5. [Inference Engine Internal Structure](#5-inference-engine-internal-structure)
6. [Triggering: How Inference Is Invoked](#6-triggering-how-inference-is-invoked)
7. [Result Representation Strategy](#7-result-representation-strategy)
8. [Provenance Tracking](#8-provenance-tracking)
9. [Caching and Invalidation](#9-caching-and-invalidation)
10. [Materialization Lifecycle](#10-materialization-lifecycle)
11. [Fact Validation During Materialization](#11-fact-validation-during-materialization)
12. [Interaction with Transactions and Concurrency](#12-interaction-with-transactions-and-concurrency)
13. [Interaction with Constraint Validation](#13-interaction-with-constraint-validation)
14. [Performance Expectations](#14-performance-expectations)
15. [Scope Boundary: Core vs. Downstream](#15-scope-boundary-core-vs-downstream)
16. [Walkthrough: Inverse Edge Rule](#16-walkthrough-inverse-edge-rule)
17. [Walkthrough: OWL Subclass Propagation](#17-walkthrough-owl-subclass-propagation)
18. [Walkthrough: Re-Inference After Data Change](#18-walkthrough-re-inference-after-data-change)
19. [Design Decision Log](#19-design-decision-log)

---

## 1. Purpose and Scope

This document is the authoritative specification for the **inference hook infrastructure** of the embedded graph database crate. It defines the internal architecture that sits between the public API (Task 10) and the `InferenceRule` trait (Task 6): how rules are invoked, how inferred facts are represented and tracked, how caching works, when cached results are invalidated, and how inference interacts with transactions and concurrency.

### What this document defines

- The `InferenceEngine` internal component: structure, state, and responsibilities
- The triggering flow: what happens when `run_inference` or `run_all_inference` is called
- Result representation: how the engine handles `InferenceResult` for both ephemeral and materialized modes
- Provenance tracking: how the database distinguishes inferred facts from asserted facts
- Caching: when inference results are reused vs. recomputed
- Invalidation: how the database knows that cached or materialized facts may be stale
- The materialization lifecycle: writing inferred facts, cleanup, re-inference
- Interaction with the single-writer MVCC concurrency model
- Interaction with constraint validators at commit time
- Performance expectations and complexity bounds

### What this document does NOT define

- The `InferenceRule` trait itself — defined in `006-schema-extension-spec.md` Section 11
- The `InferredFact` and `InferenceResult` types — defined in `006-schema-extension-spec.md` Section 11
- The `InferenceMode` enum — defined in `010-api-surface-spec.md` Section 6.3
- The public API signatures for `run_inference` / `run_all_inference` — defined in `010-api-surface-spec.md` Sections 6.1–6.2
- Any specific inference rules — these are always downstream
- The on-disk storage format for provenance metadata — that is a storage-layer concern for Task 12/16

### Relationship to upstream documents

- **Task 6** defines the `InferenceRule` trait, `InferredFact`, `InferenceResult`, and `InferenceMode`. This document builds the infrastructure that orchestrates those types.
- **Task 10** defines the public API through which callers invoke inference. This document specifies the internal behavior behind those API methods.
- **Task 7** (Graph Storage Model) defines the concurrency model (single-writer MVCC, CoW B-trees), the `WriteBuffer`, and the snapshot mechanism. This document describes how inference operates within that concurrency model.

---

## 2. Design Principles

These principles guide every design decision in this document. When two concerns conflict, lower-numbered principles take precedence.

1. **Inference runs only on explicit request.** No background processes, no automatic triggers, no surprise recomputation. The caller always decides when inference happens. (From Task 6, Principle #5.)

2. **The caller chooses the mode.** Ephemeral vs. materialized is selected per invocation, not per rule. The same rule can be used both ways. (From Task 6, Decision D17.)

3. **Inferred facts are first-class data when materialized.** Once materialized, inferred nodes and edges are stored identically to user-asserted data. They participate in queries, traversals, and constraint validation. The only difference is that provenance metadata records their origin.

4. **No automatic invalidation or cleanup.** When base data changes, previously materialized inferred facts are not automatically removed or recalculated. The caller explicitly requests re-inference (which handles cleanup). This avoids hidden performance costs and unpredictable behavior.

5. **Caching is optional and conservative.** The cache is a performance optimization, not a correctness mechanism. If the cache is empty or stale, inference is simply recomputed. The database never returns stale cached results as if they were current.

6. **`no_std + alloc` for the inference engine core.** The `InferenceEngine`'s data structures, provenance tracking, and cache types use only `alloc` types. The `std`-dependent transaction glue lives in the `db` module.

---

## 3. Architecture Overview

The inference subsystem consists of three internal components:

```
┌─────────────────────────────────────────────────────────────┐
│                      Public API Layer                        │
│  ReadTransaction::run_inference()                            │
│  WriteTransaction::run_inference(mode)                       │
│  WriteTransaction::run_all_inference(mode)                   │
│  ReadTransaction::run_all_inference()                        │
└───────────────────────┬─────────────────────────────────────┘
                        │
                        ▼
┌─────────────────────────────────────────────────────────────┐
│                    InferenceEngine                            │
│                                                              │
│  ┌──────────────────┐  ┌───────────────┐  ┌──────────────┐  │
│  │  Rule Registry    │  │ Result Cache  │  │  Provenance  │  │
│  │                   │  │               │  │   Registry   │  │
│  │ name → Box<dyn    │  │ (rule, gen)   │  │              │  │
│  │   InferenceRule>  │  │  → results    │  │ EntityId →   │  │
│  │                   │  │               │  │  RuleOrigin  │  │
│  └──────────────────┘  └───────────────┘  └──────────────┘  │
└───────────────────────┬─────────────────────────────────────┘
                        │
                        ▼
┌─────────────────────────────────────────────────────────────┐
│                   Storage / Transaction Layer                 │
│  GraphView (snapshot or snapshot+WriteBuffer overlay)         │
│  TypeRegistryView, PropertyKeyRegistryView                   │
│  WriteBuffer (for materialization)                            │
└─────────────────────────────────────────────────────────────┘
```

**Rule Registry:** Stores registered `Box<dyn InferenceRule>` instances keyed by name. Managed by the `Database` via `register_inference_rule` / `unregister_inference_rule`. Protected by an internal `RwLock` (read access for inference invocation, write access for registration changes).

**Result Cache:** An in-memory cache of recent inference results, keyed by `(rule_name, data_generation)`. Used to avoid redundant recomputation within a session. Not persisted to disk.

**Provenance Registry:** Tracks which materialized entities (nodes, edges, property updates, type assignments) were produced by which inference rule. Persisted to disk as part of the database's metadata. Used for cleanup during re-inference and for querying the provenance of any entity.

---

## 4. The InferenceRule Trait (Recap)

For convenience, the full trait as defined in `006-schema-extension-spec.md` Section 11.4:

```rust
pub trait InferenceRule: Send + Sync {
    fn name(&self) -> &str;
    fn applies_to_types(&self) -> Option<Vec<TypeId>>;
    fn infer(
        &self,
        graph: &dyn GraphView,
        types: &dyn TypeRegistryView,
        keys: &dyn PropertyKeyRegistryView,
    ) -> InferenceResult;
}
```

The `InferenceEngine` calls `infer()` and is responsible for everything that happens before and after that call: preparing the `GraphView`, caching the result, materializing facts if requested, and recording provenance.

---

## 5. Inference Engine Internal Structure

### 5.1 InferenceEngine struct

```rust
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::boxed::Box;

/// The internal inference engine. Owned by the Database.
///
/// Manages rule registration, caching, and provenance tracking.
/// All methods that mutate state require appropriate synchronization
/// (provided by the Database's internal locking).
pub(crate) struct InferenceEngine {
    /// Registered inference rules, keyed by name.
    rules: BTreeMap<String, Box<dyn InferenceRule>>,

    /// In-memory cache of inference results.
    cache: InferenceCache,

    /// Provenance registry: tracks which entities were inferred by which rule.
    /// Loaded from disk at database open, persisted on commit.
    provenance: ProvenanceRegistry,
}
```

### 5.2 Concurrency and access patterns

The `InferenceEngine` is owned by the `Database` struct. Access patterns:

- **Rule registration / unregistration:** Requires a write lock on the rule registry. Does not require a database write transaction — registration is a `Database`-level operation (per Task 10 decision A3).
- **Inference invocation (read or write transaction):** Requires a read lock on the rule registry (to look up the named rule) and read access to the graph. Does not modify the rule registry.
- **Cache reads/writes:** The cache is accessed under the same lock as the rule registry. Cache misses trigger rule evaluation; cache hits return cloned results.
- **Provenance reads:** Available to any transaction (read or write). Provenance is part of the database state.
- **Provenance writes:** Occur only during materialization in a write transaction. Written to the `WriteBuffer` alongside the materialized facts.

### 5.3 Initialization

When the `Database` opens:

1. The `InferenceEngine` is created with an empty rule registry and empty cache.
2. The provenance registry is loaded from the Schema Store B-tree (where provenance records are persisted — see Section 8.3).
3. The application registers its inference rules via `Database::register_inference_rule()`.
4. `Database::missing_extensions()` reports any rule names persisted in the provenance registry that have no corresponding registered rule.

---

## 6. Triggering: How Inference Is Invoked

### 6.1 Entry points

There are four public entry points (defined in `010-api-surface-spec.md`):

| Method | Context | Mode |
|--------|---------|------|
| `ReadTransaction::run_inference(rule_name)` | Read-only snapshot | Always ephemeral |
| `ReadTransaction::run_all_inference()` | Read-only snapshot | Always ephemeral |
| `WriteTransaction::run_inference(rule_name, mode)` | Snapshot + WriteBuffer | Caller chooses |
| `WriteTransaction::run_all_inference(mode)` | Snapshot + WriteBuffer | Caller chooses |

### 6.2 Internal dispatch flow

When any `run_inference` variant is called:

```
procedure run_inference(rule_name, mode, graph_context):
    1. Look up the rule by name in the rule registry.
       → If not found: return Error::Inference(RuleNotFound(rule_name))

    2. Check the cache for (rule_name, current_data_generation).
       → If cache hit AND mode is Ephemeral:
           return cached InferenceResult (clone)
       → If cache hit AND mode is Materialized:
           skip to step 4 using cached result
       → If cache miss: proceed to step 3

    3. Invoke rule.infer(graph_view, type_registry, key_registry).
       Store the result in the cache keyed by
       (rule_name, current_data_generation).

    4. If mode is Ephemeral:
       → Return the InferenceResult to the caller. Done.

    5. If mode is Materialized:
       a. Validate each InferredFact (Section 11).
       b. Clean up previously materialized facts from this rule
          (Section 10.2).
       c. Write new facts to the WriteBuffer (Section 10.3).
       d. Record provenance for each new entity (Section 8).
       e. Return the InferenceResult (with assigned IDs for
          new nodes/edges) to the caller.
```

### 6.3 The GraphView provided to rules

The `GraphView` that the inference engine provides to `rule.infer()` depends on the transaction context:

- **Read transaction:** The `GraphView` wraps the transaction's immutable snapshot. The rule sees exactly the committed state as of when the read transaction began. No pending writes are visible (there are none in a read transaction).

- **Write transaction:** The `GraphView` wraps the snapshot overlaid with the `WriteBuffer`. The rule sees committed data plus any pending (uncommitted) mutations from the current write transaction. This is the "read-your-own-writes" semantics established by Task 10.

**Rationale:** Inference in a write transaction should see the effects of mutations made earlier in that transaction. For example, if the caller inserts some nodes, then runs inference, the rule should be able to see those nodes. This matches the read-your-own-writes contract of `WriteTransaction`.

### 6.4 The `run_all_inference` flow

`run_all_inference` iterates over all registered rules in registration order and calls `run_inference` for each:

```
procedure run_all_inference(mode, graph_context):
    results = Vec::new()
    for rule_name in rule_registry.keys() (in registration order):
        result = run_inference(rule_name, mode, graph_context)?
        results.push(result)
    return results
```

**Important:** When running multiple rules in materialized mode, each rule's `infer()` call sees the graph state *including facts materialized by previously-run rules in this same call*. This enables rule chaining — rule B can see the facts that rule A materialized. The order is registration order, which is deterministic and under the caller's control.

**Design decision — sequential execution, not parallel:** Rules execute sequentially. Parallel execution would prevent rule chaining and would require complex synchronization around the WriteBuffer. For v1, sequential execution is both simpler and more predictable.

---

## 7. Result Representation Strategy

### 7.1 Ephemeral mode

In ephemeral mode, the `InferenceResult` is returned directly to the caller as an in-memory data structure. No state changes occur in the database. The result contains `InferredFact` values with placeholder IDs:

- `InferredFact::NewNode` — the caller does not receive a `NodeId` (none has been assigned).
- `InferredFact::NewEdge` — same for `EdgeId`.
- `InferredFact::NodePropertyUpdate`, `EdgePropertyUpdate`, `NodeTypeAssignment`, `EdgeTypeAssignment` — reference existing entity IDs, so these are fully specified.

The caller can inspect the result, count facts, filter them, or use them for display purposes. Ephemeral results have no effect on the database and leave no trace.

### 7.2 Materialized mode

In materialized mode, the inference engine writes the inferred facts into the `WriteBuffer` as part of the current write transaction. The facts become indistinguishable from user-asserted data in terms of storage and queryability. The differences are:

1. **Provenance is recorded.** Each materialized entity is tracked in the provenance registry (Section 8).
2. **IDs are assigned.** New nodes and edges receive real `NodeId` / `EdgeId` values from the ID allocator.
3. **The InferenceResult returned to the caller contains the assigned IDs.** For `NewNode` facts, the returned result includes an `assigned_id` field so the caller knows the actual ID of the materialized node.

### 7.3 MaterializedInferenceResult

The `InferenceResult` type from Task 6 does not include assigned IDs (since it was designed to also serve ephemeral mode). For materialized mode, the engine produces an enriched result:

```rust
/// The result of a materialized inference run.
///
/// Extends `InferenceResult` with the IDs assigned to newly created
/// entities during materialization.
#[derive(Clone, Debug)]
pub struct MaterializedMapping {
    /// For each InferredFact::NewNode in the original result (by index),
    /// the NodeId that was assigned when it was written to the graph.
    pub new_node_ids: Vec<(usize, NodeId)>,

    /// For each InferredFact::NewEdge in the original result (by index),
    /// the EdgeId that was assigned when it was written to the graph.
    pub new_edge_ids: Vec<(usize, EdgeId)>,
}
```

The `run_inference` method in a `WriteTransaction` with `Materialized` mode returns `InferenceResult` (as defined by Task 6). The `MaterializedMapping` is accessible via a separate method on the `WriteTransaction`:

```rust
impl<'db> WriteTransaction<'db> {
    /// After a materialized inference run, returns the ID mapping
    /// from the most recent materialization.
    ///
    /// Returns `None` if no materialized inference has been run
    /// in this transaction, or if the most recent run was ephemeral.
    pub fn last_materialization_mapping(&self) -> Option<&MaterializedMapping> { ... }
}
```

**Rationale:** Adding a new return type to `run_inference` would break the symmetry between read and write transaction signatures. Instead, the mapping is available as a side-channel that only applies in materialized mode. Callers who don't need the mapping can ignore it.

---

## 8. Provenance Tracking

### 8.1 Purpose

Provenance tracking answers two questions:

1. **"Was this entity inferred or asserted?"** — Allows callers to distinguish user-created data from inference-generated data.
2. **"Which rule produced this entity?"** — Enables targeted cleanup during re-inference: when rule R is re-run, only the entities that rule R previously produced are removed.

### 8.2 Provenance record

```rust
/// Provenance information for a single materialized inference artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenanceRecord {
    /// The name of the inference rule that produced this entity.
    pub rule_name: String,

    /// The data generation (transaction ID) at which this entity
    /// was materialized.
    pub materialized_at: u64,
}

/// Identifies a materialized inference artifact.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InferredEntity {
    /// A node created by inference.
    Node(NodeId),

    /// An edge created by inference.
    Edge(EdgeId),

    /// A property set on an existing node by inference.
    NodeProperty { node: NodeId, key: PropertyKeyId },

    /// A property set on an existing edge by inference.
    EdgeProperty { edge: EdgeId, key: PropertyKeyId },

    /// A type label added to an existing node by inference.
    NodeType { node: NodeId, type_id: TypeId },

    /// A type label added to an existing edge by inference.
    EdgeType { edge: EdgeId, type_id: TypeId },
}
```

### 8.3 ProvenanceRegistry

```rust
/// Tracks which entities in the graph were produced by inference rules.
///
/// Persisted as records in the Schema Store B-tree (alongside type
/// definitions and property key registrations). Loaded into memory
/// at database open time.
pub(crate) struct ProvenanceRegistry {
    /// Map from inferred entity to its provenance.
    by_entity: BTreeMap<InferredEntity, ProvenanceRecord>,

    /// Reverse index: rule_name → set of entities produced by that rule.
    /// Enables efficient cleanup during re-inference.
    by_rule: BTreeMap<String, Vec<InferredEntity>>,
}
```

The `ProvenanceRegistry` provides the following operations:

```rust
impl ProvenanceRegistry {
    /// Record that `entity` was produced by `rule_name` at `txn_id`.
    pub(crate) fn record(
        &mut self,
        entity: InferredEntity,
        rule_name: &str,
        txn_id: u64,
    ) { ... }

    /// Remove all provenance records for entities produced by `rule_name`.
    /// Returns the list of entities that were removed (for cleanup).
    pub(crate) fn remove_by_rule(
        &mut self,
        rule_name: &str,
    ) -> Vec<InferredEntity> { ... }

    /// Look up the provenance of a specific entity.
    /// Returns None if the entity was user-asserted (not inferred).
    pub(crate) fn get(&self, entity: &InferredEntity) -> Option<&ProvenanceRecord> { ... }

    /// Check whether a specific entity was produced by inference.
    pub(crate) fn is_inferred(&self, entity: &InferredEntity) -> bool { ... }

    /// Return all entities produced by a specific rule.
    pub(crate) fn entities_by_rule(&self, rule_name: &str) -> &[InferredEntity] { ... }
}
```

### 8.4 Persistence

Provenance records are stored in the **Schema Store B-tree** alongside type definitions and property key registrations. The key encoding is:

```
Provenance Key Encoding:
  [prefix: 1 byte = 0x03]  // Distinguishes from type (0x01) and
                            // property key (0x02) records
  [entity_kind: 1 byte]    // 0x01=Node, 0x02=Edge, 0x03=NodeProp,
                            // 0x04=EdgeProp, 0x05=NodeType, 0x06=EdgeType
  [entity_id: 8 bytes]     // NodeId or EdgeId (big-endian u64)
  [sub_id: 4 bytes]        // PropertyKeyId or TypeId (big-endian u32)
                            // Zero for Node/Edge entities
Total key: 14 bytes (fixed)

Provenance Value Encoding:
  [txn_id: 8 bytes, LE u64]
  [rule_name_len: 2 bytes, LE u16]
  [rule_name: variable bytes, UTF-8]
Total value: 10 + rule_name_len bytes (variable)
```

**Rationale for storing provenance in the Schema Store B-tree:** Provenance metadata is structurally similar to schema metadata — it is administrative information about the database's contents, not the graph data itself. It is small in volume (one record per inferred entity), and co-locating it with schema data avoids adding a new B-tree to the file format (which would require a superblock change). The prefix byte (`0x03`) cleanly separates provenance records from type and property key records in the same B-tree.

### 8.5 Public provenance query API

The public API exposes provenance as read-only queries on transactions:

```rust
impl<'db> ReadTransaction<'db> {
    /// Check whether a node was produced by inference.
    pub fn is_inferred_node(&self, id: NodeId) -> Result<bool, Error> { ... }

    /// Check whether an edge was produced by inference.
    pub fn is_inferred_edge(&self, id: EdgeId) -> Result<bool, Error> { ... }

    /// Get the provenance record for a node, if it was inferred.
    pub fn node_provenance(&self, id: NodeId) -> Result<Option<ProvenanceRecord>, Error> { ... }

    /// Get the provenance record for an edge, if it was inferred.
    pub fn edge_provenance(&self, id: EdgeId) -> Result<Option<ProvenanceRecord>, Error> { ... }
}

// WriteTransaction has the same methods (delegates to the overlay view).
```

---

## 9. Caching and Invalidation

### 9.1 Cache structure

The inference cache is an in-memory structure that stores recent inference results to avoid redundant computation:

```rust
/// A cache key: identifies a specific inference computation.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct CacheKey {
    rule_name: String,
    data_generation: u64,
}

/// The in-memory inference result cache.
pub(crate) struct InferenceCache {
    /// Cached results, keyed by (rule_name, data_generation).
    entries: BTreeMap<CacheKey, InferenceResult>,

    /// Maximum number of cached entries. When exceeded, the oldest
    /// entries are evicted. Default: 64.
    max_entries: usize,
}
```

### 9.2 Data generation tracking

The **data generation** is the `transaction_id` from the most recent committed write transaction. It is a monotonically increasing counter maintained by the storage engine (already present in the superblock — see `008-file-format-spec.md` Section 4).

- When a write transaction commits, the `transaction_id` increments. This is the new data generation.
- A read transaction's data generation is the `transaction_id` of the snapshot it observes.
- A write transaction's data generation is the `transaction_id` of the snapshot it started from, plus a flag indicating "has pending writes." If the write transaction has pending mutations, the data generation is effectively "uncommitted" and no cache entry from a previous generation is valid.

### 9.3 Cache hit conditions

A cache entry for `(rule_name, generation)` is valid if and only if:

1. The current transaction's snapshot generation matches `generation`.
2. The current transaction has no pending writes (i.e., no mutations have been made in this write transaction since the snapshot was taken, or the transaction is read-only).

**Why condition 2?** If the caller has inserted nodes and then calls `run_inference`, the rule should see those new nodes. A cached result from the pre-mutation snapshot would be stale. Therefore, any mutation in the write transaction invalidates the cache for the current transaction's purposes.

**Implementation:** The write transaction maintains a `dirty` flag that is set on the first mutation. When `dirty` is true, the cache is bypassed for all rules.

### 9.4 Cache invalidation

The cache does not need an explicit invalidation mechanism. Entries become naturally unreachable as the data generation advances:

- When transaction N commits, the generation advances from N-1 to N.
- Cache entries from generation N-1 will never match a transaction in generation N.
- The cache's LRU eviction (bounded by `max_entries`) eventually removes stale entries.

**No-persist guarantee:** The cache is purely in-memory and not persisted to disk. On database close and reopen, the cache starts empty. This eliminates an entire class of cache coherence bugs.

### 9.5 Cache configuration

```rust
impl DatabaseConfig {
    /// Set the maximum number of cached inference results.
    /// Default: 64. Set to 0 to disable caching entirely.
    pub fn inference_cache_size(mut self, max_entries: usize) -> Self {
        self.inference_cache_size = max_entries;
        self
    }
}
```

### 9.6 Type-aware cache optimization (deferred)

A future optimization could use `applies_to_types()` to preserve cache entries when a commit only affects types that the rule doesn't care about. For example, if rule R applies to types {A, B} and a commit only modifies types {C, D}, then R's cached result is still valid.

This optimization is deferred to a post-v1 release because:

1. It requires tracking which types were affected by each commit, which adds bookkeeping.
2. The correctness argument is subtle (a rule's `applies_to_types` is advisory, not exhaustive).
3. The simple generation-based cache is already effective for the common case (run inference, read result, repeat without intervening writes).

---

## 10. Materialization Lifecycle

### 10.1 Overview

Materialization is the process of converting ephemeral `InferredFact` values into durable graph data. It has three phases: cleanup, insertion, and provenance recording.

### 10.2 Cleanup: removing previously materialized facts

When a rule is re-run in materialized mode, the engine first removes any facts that were materialized by a previous run of the same rule. This prevents duplicate or stale inferred data from accumulating:

```
procedure cleanup_previous_materialization(rule_name, write_buffer):
    entities = provenance.remove_by_rule(rule_name)
    
    for entity in entities:
        match entity:
            InferredEntity::Node(id):
                write_buffer.delete_node(id)  // cascading edge delete
            InferredEntity::Edge(id):
                write_buffer.delete_edge(id)
            InferredEntity::NodeProperty { node, key }:
                write_buffer.remove_node_property(node, key)
            InferredEntity::EdgeProperty { edge, key }:
                write_buffer.remove_edge_property(edge, key)
            InferredEntity::NodeType { node, type_id }:
                write_buffer.remove_node_type(node, type_id)
            InferredEntity::EdgeType { edge, type_id }:
                write_buffer.remove_edge_type(edge, type_id)
```

**Design decision — cleanup before re-inference, not diff-based:** An alternative would be to run the rule first, diff the new results against old materialized facts, and apply only the delta. This would be more efficient when most facts are unchanged, but it requires a reliable equality comparison between old and new facts (which is complicated by ID assignment for new entities and by `Value::PartialEq` limitations with `f64`). The cleanup-and-reinsert approach is simpler, always correct, and acceptable in performance for v1 because inference is an explicit, non-hot-path operation.

### 10.3 Insertion: writing inferred facts to the WriteBuffer

After cleanup, the engine iterates over the `InferenceResult.facts` and writes each to the `WriteBuffer`:

```
procedure materialize_facts(result, write_buffer) -> MaterializedMapping:
    mapping = MaterializedMapping::new()
    
    for (index, fact) in result.facts.enumerate():
        match fact:
            InferredFact::NewNode { type_labels, properties, is_anonymous }:
                node = Node { id: NodeId(0), type_labels, properties, is_anonymous }
                assigned_id = write_buffer.insert_node(node)
                mapping.new_node_ids.push((index, assigned_id))
                provenance.record(InferredEntity::Node(assigned_id),
                                  &result.rule_name, current_txn_id)

            InferredFact::NewEdge { type_labels, source, target, properties }:
                edge = Edge { id: EdgeId(0), type_labels, source, target, properties }
                assigned_id = write_buffer.insert_edge(edge)
                mapping.new_edge_ids.push((index, assigned_id))
                provenance.record(InferredEntity::Edge(assigned_id),
                                  &result.rule_name, current_txn_id)

            InferredFact::NodePropertyUpdate { node, key, value }:
                write_buffer.set_node_property(node, key, value)
                provenance.record(
                    InferredEntity::NodeProperty { node, key },
                    &result.rule_name, current_txn_id)

            InferredFact::EdgePropertyUpdate { edge, key, value }:
                write_buffer.set_edge_property(edge, key, value)
                provenance.record(
                    InferredEntity::EdgeProperty { edge, key },
                    &result.rule_name, current_txn_id)

            InferredFact::NodeTypeAssignment { node, type_id }:
                write_buffer.add_node_type(node, type_id)
                provenance.record(
                    InferredEntity::NodeType { node, type_id },
                    &result.rule_name, current_txn_id)

            InferredFact::EdgeTypeAssignment { edge, type_id }:
                write_buffer.add_edge_type(edge, type_id)
                provenance.record(
                    InferredEntity::EdgeType { edge, type_id },
                    &result.rule_name, current_txn_id)

    return mapping
```

### 10.4 Provenance recording

Provenance records are written to the `WriteBuffer` alongside the data they describe. They are part of the same atomic transaction: if the transaction aborts, both the materialized facts and their provenance records are discarded.

### 10.5 Re-inference pattern

The typical pattern for re-inference after data changes:

```rust
// Session 1: initial inference
let mut txn = db.write_txn()?;
// ... insert data ...
txn.run_inference("MyRule", InferenceMode::Materialized)?;
txn.commit()?;

// Session 2: data has changed, re-infer
let mut txn = db.write_txn()?;
// ... modify data ...
txn.run_inference("MyRule", InferenceMode::Materialized)?;
// ^ This automatically cleans up old materialized facts
//   from "MyRule" before writing new ones.
txn.commit()?;
```

The caller does not need to manually delete old inferred facts. The cleanup is automatic when materializing.

---

## 11. Fact Validation During Materialization

### 11.1 Pre-materialization validation

Before writing each `InferredFact` to the `WriteBuffer`, the engine performs basic structural validation:

```rust
/// Errors that can occur during fact materialization.
/// Returned as Error::Inference(InvalidFact { ... }).
```

| Fact type | Validation checks |
|-----------|-------------------|
| `NewNode` | Type labels reference valid TypeIds (exist in the type registry). Type labels are for node types (not edge types). |
| `NewEdge` | Source and target NodeIds reference existing nodes (in the snapshot + WriteBuffer overlay). Type labels reference valid edge TypeIds. |
| `NodePropertyUpdate` | The NodeId references an existing node. The PropertyKeyId is registered. |
| `EdgePropertyUpdate` | The EdgeId references an existing edge. The PropertyKeyId is registered. |
| `NodeTypeAssignment` | The NodeId references an existing node. The TypeId is a valid node type. |
| `EdgeTypeAssignment` | The EdgeId references an existing edge. The TypeId is a valid edge type. |

If any fact fails validation, the entire materialization is aborted (no partial writes) and `Error::Inference(InvalidFact { rule_name, message })` is returned. The cleanup of previously materialized facts is also rolled back (since the entire transaction can be aborted by the caller).

**Design decision — fail-fast on first invalid fact:** An alternative would be to collect all invalid facts and return them all. Fail-fast is chosen because an invalid fact usually indicates a bug in the rule, and the remaining facts may depend on the invalid one (e.g., a `NewEdge` referencing a `NewNode` that failed validation). Reporting only the first error simplifies both the engine and the rule author's debugging experience.

### 11.2 Constraint validation at commit time

Materialized inferred facts are part of the transaction's `ChangeSet`. When `commit()` is called, all registered `ConstraintValidator`s run against the full `ChangeSet`, which includes both user-asserted mutations and materialized inferred mutations. If a validator rejects an inferred fact, the commit fails.

This is the correct behavior: inferred facts are first-class data and must satisfy all constraints. A rule that produces constraint-violating facts is buggy, and the constraint system catches this at commit time.

---

## 12. Interaction with Transactions and Concurrency

### 12.1 Read transactions

Inference in a read transaction:

- Operates on a frozen, consistent snapshot. No contention with writers.
- Always ephemeral (no mutations possible in a read transaction).
- Multiple read transactions can run inference concurrently without coordination (each operates on its own snapshot).
- The `GraphView` provided to the rule wraps the read-only snapshot. No `WriteBuffer` overlay is present.

### 12.2 Write transactions

Inference in a write transaction:

- Holds the exclusive write lock (as established by the single-writer model).
- The `GraphView` provided to the rule wraps the snapshot overlaid with the `WriteBuffer`, providing read-your-own-writes semantics.
- Materialized facts are written to the `WriteBuffer` and committed atomically with all other mutations.
- Only one write transaction exists at a time, so there are no concurrent materialization conflicts.

### 12.3 Snapshot isolation for inference

A key property: **the `GraphView` that a rule receives is a consistent snapshot**. In a read transaction, it is the committed state. In a write transaction, it is the committed state plus the transaction's pending writes. The rule cannot see concurrent uncommitted changes (there are none — single-writer model).

This means inference results are deterministic for a given graph state: running the same rule twice on the same snapshot produces the same facts. This property is important for caching correctness.

### 12.4 Inference engine access pattern

The inference engine's internal state is accessed as follows:

| Component | Read transaction | Write transaction |
|-----------|-----------------|-------------------|
| Rule registry | Read lock | Read lock |
| Cache | Read/write (via internal lock) | Read/write (via internal lock) |
| Provenance registry | Read (from snapshot) | Read/write (via WriteBuffer) |

The rule registry is protected by an `RwLock` internal to the `Database`. The cache is protected by the same `RwLock` (or a separate `Mutex` — implementation choice). Provenance reads go through the snapshot/overlay like any other data read.

---

## 13. Interaction with Constraint Validation

### 13.1 Ordering: inference before validation

If the caller runs inference (materialized) and then commits, the commit-time validation sees the inferred facts in the `ChangeSet`. The ordering is:

1. Caller makes mutations (insert nodes, edges, etc.).
2. Caller calls `run_inference("MyRule", Materialized)`.
3. Engine cleans up old materialized facts, runs the rule, writes new facts.
4. Caller calls `commit()`.
5. Engine builds the `ChangeSet` from all WriteBuffer mutations (including materialized inferred facts).
6. All registered `ConstraintValidator`s run against the `ChangeSet` + graph overlay.
7. If all pass → commit succeeds. If any fail → commit is rejected.

### 13.2 Inferred facts in `validate_all()`

The `validate_all()` method (Task 10, Section 6.2) synthesizes a full-insert `ChangeSet` treating every node and edge as newly inserted. **Materialized inferred facts are included** in this synthetic `ChangeSet` — they are regular data and must satisfy all constraints.

### 13.3 Constraint validation does not see ephemeral inference

Ephemeral inference results are not written to the graph and do not appear in any `ChangeSet`. Constraint validators never see ephemeral facts. This is by design: ephemeral results are a "what-if" mechanism, not a data mutation.

---

## 14. Performance Expectations

### 14.1 Inference invocation cost

The cost of `run_inference` is:

```
T(run_inference) = T(cache_lookup) + T(rule.infer) + T(materialization)
```

- **Cache lookup:** O(log N) where N is the number of cache entries. Negligible.
- **`rule.infer()`:** Entirely rule-dependent. The core cannot bound this. The rule author is responsible for the complexity of their inference logic.
- **Materialization:** O(F) where F is the number of inferred facts. Each fact requires one B-tree insertion (amortized O(log N) per insertion). For materialization with cleanup, add O(C) for cleaning up C previously materialized facts (each requires one B-tree deletion).

### 14.2 Provenance overhead

- **Storage:** One provenance record per materialized inferred entity. Each record is 14 bytes (key) + ~20–50 bytes (value) = ~30–60 bytes per entity. For a database with 10,000 inferred entities, this is ~300–600 KB — negligible compared to graph data.
- **Lookup:** Provenance lookup is a single B-tree query. O(log N) where N is the total number of schema + provenance records in the Schema Store B-tree.
- **Memory:** The provenance registry is loaded into memory at startup. For 10,000 inferred entities, this is ~500 KB of heap memory.

### 14.3 Cache effectiveness

The cache is most effective when:

- The same rule is invoked multiple times within the same data generation (e.g., multiple read transactions each running the same rule before any write commits).
- Rules are expensive to compute.

The cache is not effective when:

- Every inference call is preceded by a write transaction commit (each commit advances the generation).
- The write transaction has pending mutations (cache is bypassed).

For the common pattern of "write data → run inference → commit," the cache provides no benefit because the rule runs once per transaction anyway. The cache primarily benefits the "read-only inference query" pattern.

### 14.4 Scalability considerations

- **Rule execution scales with graph size.** Rules that scan the entire graph (e.g., transitive closure) are O(N) or worse. This is inherent to the inference model — the core cannot optimize rule logic. Documentation should recommend that rule authors use `applies_to_types()` to narrow their scan scope.
- **Materialization scales with fact count.** Each fact is one B-tree operation. For 10,000 facts, this is 10,000 B-tree inserts — significant but bounded and predictable.
- **Cleanup scales with previous fact count.** Re-inference deletes all previous facts before inserting new ones. For a rule that produces 10,000 facts, cleanup is 10,000 B-tree deletes. This is acceptable for explicit, caller-initiated operations.

---

## 15. Scope Boundary: Core vs. Downstream

### 15.1 What the core provides (this specification)

| Capability | Description |
|------------|-------------|
| Rule registration | `Database::register_inference_rule()`, `unregister_inference_rule()` |
| Rule invocation | `run_inference()`, `run_all_inference()` on both transaction types |
| Ephemeral results | Return `InferenceResult` without side effects |
| Materialization | Write inferred facts to the graph, with ID assignment |
| Provenance tracking | Record and query which entities were inferred by which rule |
| Cleanup on re-inference | Remove previously materialized facts before re-materializing |
| Fact validation | Structural validation of each `InferredFact` before write |
| Result caching | In-memory cache keyed by (rule_name, data_generation) |
| Constraint integration | Materialized facts are subject to commit-time validation |
| Concurrency safety | Inference operates within the MVCC transaction model |

### 15.2 What downstream crates provide

| Capability | Description |
|------------|-------------|
| Actual inference rules | Implementations of `InferenceRule` (e.g., OWL subsumption, SKOS transitive broader, custom domain rules) |
| Multi-pass inference | Running rules in a specific order to achieve fixpoint (the core runs rules once per invocation; multi-pass is caller logic) |
| Incremental inference | Inspecting the `ChangeSet` to determine which rules need re-running (the core provides full-graph inference; incremental optimization is downstream) |
| Semantic invalidation | Knowing that "if data of type X changed, rule Y's results are stale" (the core provides generation-based staleness; semantic dependency tracking is downstream) |
| Rule dependency graphs | Ordering rules based on dependencies (rule A produces data that rule B consumes). The core runs rules in registration order; explicit dependency management is downstream. |
| Inference strategies | Forward chaining, backward chaining, etc. The core provides a simple "run all applicable rules once" mechanism. Complex reasoning strategies are downstream. |

### 15.3 Fixpoint inference (downstream pattern)

A common pattern for ontology systems is to run inference to a fixpoint — repeatedly running rules until no new facts are produced:

```rust
// Downstream crate pattern — NOT in the core
fn run_to_fixpoint(txn: &mut WriteTransaction, rule_names: &[&str]) -> Result<(), Error> {
    loop {
        let mut any_new_facts = false;
        for &rule in rule_names {
            let result = txn.run_inference(rule, InferenceMode::Materialized)?;
            if !result.facts.is_empty() {
                any_new_facts = true;
            }
        }
        if !any_new_facts {
            break;
        }
    }
    Ok(())
}
```

The core enables this pattern (each `run_inference` call sees previously materialized facts from the same transaction) but does not implement it. Fixpoint detection, termination guarantees, and cycle detection are the downstream crate's responsibility.

---

## 16. Walkthrough: Inverse Edge Rule

This walkthrough traces the full lifecycle of the `InverseEdgeRule` from `010-api-surface-spec.md` Section 12.4.

### Setup

```rust
// Application registers the rule
db.register_inference_rule(Box::new(InverseEdgeRule {
    source_edge_type: knows_type,
    inverse_edge_type: known_by_type,
}))?;
```

The engine stores the rule in the rule registry under the name `"InverseEdgeRule"`.

### Ephemeral invocation

```rust
let txn = db.read_txn()?;
let result = txn.run_inference("InverseEdgeRule")?;
```

1. Engine looks up `"InverseEdgeRule"` in the registry. Found.
2. Engine checks cache for `("InverseEdgeRule", snapshot_generation)`. Cache miss (first invocation).
3. Engine constructs a `GraphView` wrapping the read transaction's snapshot.
4. Engine calls `rule.infer(graph_view, types, keys)`.
5. The rule scans all `knows` edges, checks for missing inverses, returns `InferenceResult` with `NewEdge` facts.
6. Engine stores the result in the cache.
7. Engine returns the result to the caller. No state changes.

### Materialized invocation

```rust
let mut txn = db.write_txn()?;
let result = txn.run_inference("InverseEdgeRule", InferenceMode::Materialized)?;
```

1–5. Same as above, but the `GraphView` includes the WriteBuffer overlay.

6. Mode is `Materialized`. Engine checks for previous materializations by `"InverseEdgeRule"`:
   - Queries `provenance.entities_by_rule("InverseEdgeRule")`.
   - If any entities exist: deletes them from the WriteBuffer and removes their provenance records.
7. Engine validates each `NewEdge` fact:
   - Source node exists? ✓
   - Target node exists? ✓
   - Edge type is a valid edge TypeId? ✓
8. Engine writes each `NewEdge` to the WriteBuffer. Each receives a new `EdgeId`.
9. Engine records provenance: `InferredEntity::Edge(new_id)` → `("InverseEdgeRule", current_txn_id)`.
10. Engine stores the `MaterializedMapping` on the transaction.
11. Engine returns the `InferenceResult` to the caller.
12. Caller calls `txn.commit()`. The inferred edges are in the `ChangeSet`. Constraint validators run. If all pass, the edges (and their provenance records) are durably committed.

---

## 17. Walkthrough: OWL Subclass Propagation

This walkthrough shows how an OWL-lite subclass propagation rule would interact with the inference engine.

### Rule behavior (downstream)

The rule: "For every node N with type labels [A], if A is a subtype of B in the type hierarchy, infer that N also has type label B."

```rust
struct SubclassPropagationRule;

impl InferenceRule for SubclassPropagationRule {
    fn name(&self) -> &str { "SubclassPropagation" }
    fn applies_to_types(&self) -> Option<Vec<TypeId>> { None } // applies to all types

    fn infer(
        &self,
        graph: &dyn GraphView,
        types: &dyn TypeRegistryView,
        _keys: &dyn PropertyKeyRegistryView,
    ) -> InferenceResult {
        let mut facts = Vec::new();
        // For each node type in the registry...
        for type_def in types.all_types() {
            if type_def.kind != TypeKind::Node { continue; }
            let supertypes = types.all_supertypes(type_def.id);
            if supertypes.is_empty() { continue; }
            // For each node of this type...
            for node in graph.nodes_by_type(type_def.id, false) {
                for &super_id in &supertypes {
                    if !node.type_labels.contains(&super_id) {
                        facts.push(InferredFact::NodeTypeAssignment {
                            node: node.id,
                            type_id: super_id,
                        });
                    }
                }
            }
        }
        InferenceResult { facts, rule_name: "SubclassPropagation".into() }
    }
}
```

### Materialization flow

1. Caller runs `txn.run_inference("SubclassPropagation", Materialized)`.
2. Engine cleans up any previous `SubclassPropagation` materializations (removes previously added type labels).
3. Engine runs the rule. The rule produces `NodeTypeAssignment` facts.
4. Engine validates: each referenced node exists, each TypeId is a valid node type.
5. Engine writes each type assignment to the WriteBuffer via `add_node_type(node_id, type_id)`.
6. Engine records provenance: `InferredEntity::NodeType { node, type_id }` → `("SubclassPropagation", txn_id)`.
7. On commit, constraint validators see the updated type labels.

### Re-inference after a schema change

If the user adds a new supertype relationship (e.g., `Student` is now a subtype of `Researcher`), the previously materialized type labels are stale. The caller re-runs:

```rust
let mut txn = db.write_txn()?;
txn.add_supertype(student_type, researcher_type)?;
txn.run_inference("SubclassPropagation", InferenceMode::Materialized)?;
txn.commit()?;
```

The engine cleans up old type assignments, re-runs the rule (which now sees the new hierarchy), and materializes the updated results.

---

## 18. Walkthrough: Re-Inference After Data Change

This walkthrough shows the cleanup behavior when base data changes and inference is re-run.

### Initial state

- Nodes: A, B, C
- Edges: A→knows→B, B→knows→C
- Rule: `TransitiveClosureRule` (infers A→knows→C)
- Materialized: edge A→knows→C with provenance `("TransitiveClosureRule", txn_5)`

### Data change

The caller deletes the edge B→knows→C:

```rust
let mut txn = db.write_txn()?;
txn.delete_edge(b_knows_c_edge_id)?;
```

At this point, the materialized A→knows→C edge is stale (it was inferred from a path that no longer exists). The core does **not** automatically detect or remove it — principle #4 (no automatic invalidation).

### Re-inference

The caller re-runs inference:

```rust
txn.run_inference("TransitiveClosureRule", InferenceMode::Materialized)?;
```

1. **Cleanup:** Engine queries `provenance.entities_by_rule("TransitiveClosureRule")`. Finds `InferredEntity::Edge(a_knows_c_id)`. Deletes this edge from the WriteBuffer.
2. **Rule execution:** The rule scans edges. It sees A→knows→B but B has no outgoing `knows` edges (B→C was deleted). It produces zero facts.
3. **Materialization:** Zero facts to write. The provenance for `"TransitiveClosureRule"` is now empty.
4. **Result:** The stale inferred edge has been removed. The graph is consistent.

```rust
txn.commit()?; // Committed: stale edge deleted, no new inferred edges.
```

---

## 19. Design Decision Log

| # | Decision | Alternatives Considered | Rationale |
|---|----------|------------------------|-----------|
| I1 | Cleanup-and-reinsert on re-inference (not diff-based) | Diff old vs. new, apply delta | Simpler, always correct. Diff requires reliable equality comparison across ID reassignment. Acceptable performance for an explicit, caller-initiated operation. |
| I2 | In-memory cache, not persisted | Persist cache to disk | Eliminates cache coherence bugs. Cache is a pure optimization; cold cache just means one recomputation. Database close/reopen starts fresh. |
| I3 | Generation-based cache keying (transaction_id) | Timestamp-based; hash-of-graph-state | Transaction ID is already maintained by the storage engine. Monotonic, cheap to compare. Hash-of-state would be expensive to compute. |
| I4 | Cache bypassed when write transaction has pending mutations | Always bypass in write transactions; never bypass | Pending mutations change the effective graph state, so a cached result from the pre-mutation snapshot is stale. Never-bypass wastes the cache for read-only inference in write transactions before any mutations. |
| I5 | Provenance stored in Schema Store B-tree (not a new B-tree) | Dedicated Provenance B-tree; in-memory only; per-record flag | Avoids adding a new root pointer to the superblock. Schema Store is lightly loaded and well-suited for metadata. Per-record flag would pollute the core data model (Task 6 Decision D6 explicitly rejected `is_inferred` on Node/Edge). In-memory only loses provenance on restart. |
| I6 | Provenance loaded into memory at startup | On-demand provenance queries (always go to B-tree) | Provenance is small (one record per inferred entity) and frequently accessed during re-inference cleanup. In-memory lookup is O(log N) in a BTreeMap vs. O(log N) in a B-tree with I/O. Memory cost is acceptable. |
| I7 | Sequential rule execution in `run_all_inference` | Parallel execution | Sequential enables rule chaining (rule B sees rule A's materialized facts). Parallel would require complex synchronization and prevent chaining. |
| I8 | Fail-fast on first invalid fact during materialization | Collect all invalid facts; skip invalid facts | First invalid fact usually indicates a rule bug. Remaining facts may depend on it. Simpler error handling for both the engine and the user. |
| I9 | `MaterializedMapping` as a separate accessor, not embedded in `InferenceResult` | New `MaterializedInferenceResult` return type; add optional fields to `InferenceResult` | Preserves API symmetry between read and write transactions. `InferenceResult` is defined in the `no_std` types layer; adding optional `NodeId`/`EdgeId` fields would make it mode-aware, breaking separation of concerns. |
| I10 | No automatic invalidation of materialized facts | Auto-invalidate on commit; lazy invalidation on read | Auto-invalidation violates principle #1 (inference only on explicit request). Lazy invalidation adds unpredictable latency to reads. Explicit re-inference is predictable and under caller control. |
| I11 | Fixed `max_entries` LRU eviction for cache | Unbounded cache; time-based expiry | Unbounded cache leaks memory in long-running processes. Time-based expiry is arbitrary and doesn't correlate with data freshness (generation does). Fixed-size LRU is simple and predictable. |
| I12 | Public provenance query API (is_inferred_node, node_provenance) | Provenance is internal-only; provenance as a property on Node/Edge | Callers need to distinguish inferred from asserted data (e.g., for display, export, debugging). Internal-only would force workarounds. Per-entity property would pollute the data model. A dedicated query API is clean and optional. |

---

## Completion Report: Task 11 — Inference Hook Architecture

### Status: COMPLETE

### Done Criterion:

The criterion requires:

1. **Inference rule trait** — ✓ Recapped in Section 4 (defined by Task 6; this document builds infrastructure on top).
2. **Registration mechanism** — ✓ Section 5 (rule registry within `InferenceEngine`), Section 3 (architecture overview). Registration is via `Database::register_inference_rule()` as specified by Task 10.
3. **Triggering API** — ✓ Section 6 defines the full dispatch flow for all four entry points (`run_inference` and `run_all_inference` on both transaction types).
4. **Result representation strategy** — ✓ Section 7 defines ephemeral results (returned as-is), materialized results (written to WriteBuffer with ID assignment), and `MaterializedMapping` for assigned IDs.
5. **Caching/invalidation approach** — ✓ Section 9 defines the generation-based in-memory cache, hit conditions, natural invalidation, and cache configuration.
6. **Interaction with transactions/concurrency** — ✓ Section 12 details behavior in read vs. write transactions, snapshot isolation, and the engine's access patterns under the single-writer MVCC model.
7. **Performance expectations** — ✓ Section 14 provides complexity analysis for invocation, provenance overhead, and cache effectiveness.
8. **Scope boundary (core vs. downstream)** — ✓ Section 15 explicitly enumerates what the core provides and what downstream crates provide, including the fixpoint inference pattern.

All criteria met.

### Deliverables:
- `011-inference-hook-design.md` — this document

### Summary:

Designed the complete inference hook architecture for the embedded graph database. The architecture consists of three internal components: a rule registry (holding registered `Box<dyn InferenceRule>` instances), an in-memory result cache (keyed by rule name and data generation), and a provenance registry (tracking which entities were produced by which rule, persisted in the Schema Store B-tree).

Key design decisions: (1) cleanup-and-reinsert on re-inference rather than diff-based updates, for simplicity and correctness; (2) generation-based cache invalidation using the existing transaction ID counter; (3) provenance stored in the Schema Store B-tree to avoid superblock changes; (4) sequential rule execution in `run_all_inference` to enable rule chaining; (5) no automatic invalidation of materialized facts — the caller explicitly re-runs inference when needed.

The design integrates cleanly with the MVCC concurrency model (single writer, snapshot isolation), the constraint validation system (materialized facts participate in commit-time validation), and the public API (ephemeral in read transactions, caller-chosen mode in write transactions).

### Context for Next Task:

**Task 12 (Design Synthesis)** should read `011-inference-hook-design.md` (this deliverable) alongside all other design documents (006–010). Key items for Task 12:

- The `InferenceEngine` is an internal component of the `Database` struct. Task 12 should describe its position in the crate's module hierarchy (likely `db/inference_engine.rs` or `inference/engine.rs`).
- Provenance records are stored in the Schema Store B-tree with a `0x03` prefix byte. Task 12 should ensure the B-tree key encoding for provenance is consistent with the existing Schema Store key encodings defined in `007-graph-storage-model.md`.
- The `MaterializedMapping` and public provenance query methods (`is_inferred_node`, `node_provenance`, etc.) are new additions to the API surface. Task 12 should incorporate them into the consolidated API specification.
- The inference cache configuration (`inference_cache_size`) should be added to `DatabaseConfig` in Task 12's consolidated configuration section.
- The sequential rule execution order in `run_all_inference` is a documented behavioral contract. Task 12 should note this in the cross-cutting concerns section.

### Residual Concerns:

1. **Provenance for property updates and type assignments on user-asserted entities.** If an inference rule sets a property on a user-created node, the provenance registry records that property assignment as inferred. During re-inference cleanup, that property is removed. This is correct behavior — but it means the user cannot manually set the same property and expect it to survive re-inference. Documentation should warn about this: if both user code and inference rules write to the same property key on the same node, the last writer wins and provenance tracks only the inference rule's write.

2. **Provenance registry memory footprint.** The provenance registry is loaded entirely into memory at startup. For databases with millions of inferred entities, this could consume significant memory (estimated ~50 bytes per entity × 1M = ~50 MB). A future optimization could use lazy loading or a B-tree scan instead of full materialization. For v1, the in-memory approach is acceptable because databases with millions of inferred entities are an advanced use case.

3. **Cache is per-Database instance, not per-transaction.** In a highly concurrent scenario with many read transactions all running the same rule, the cache prevents redundant computation. However, the cache is shared state protected by a lock. In extreme concurrency (hundreds of concurrent readers all cache-missing simultaneously), the lock could become a bottleneck. A per-transaction cache or lock-free structure could be considered post-v1.

4. **Type-aware cache optimization (Section 9.6) is explicitly deferred.** This is noted as a future improvement that could significantly reduce recomputation when commits don't affect the types a rule cares about. It requires additional bookkeeping (tracking which types were modified per commit) and careful correctness analysis.

### Upstream Flags:

1. **New public API methods not in Task 10 — ADVISORY.**
   - What was discovered: This document introduces `is_inferred_node()`, `is_inferred_edge()`, `node_provenance()`, `edge_provenance()` on `ReadTransaction` and `WriteTransaction`, plus `last_materialization_mapping()` on `WriteTransaction` and `inference_cache_size()` on `DatabaseConfig`. These were not specified in `010-api-surface-spec.md`.
   - Which task(s) it affects: Task 12 (must incorporate these into the consolidated API)
   - Severity: ADVISORY (non-breaking additions to the API; no existing signatures change)
   - Suggested action: Task 12 should add these methods to the consolidated API surface and ensure they appear in the crate's public documentation.

2. **Provenance B-tree key prefix `0x03` must be reserved in the Schema Store key encoding — ADVISORY.**
   - What was discovered: `007-graph-storage-model.md` defines the Schema Store B-tree key encoding. This document adds a new key prefix (`0x03`) for provenance records. Task 12 should verify that `0x03` is not already claimed and formally reserve it.
   - Which task(s) it affects: Task 12, Task 16 (implementation)
   - Severity: ADVISORY (the Schema Store key encoding in `007` uses `0x01` for types and `0x02` for property keys; `0x03` appears to be available)
   - Suggested action: Task 12 should add provenance key encoding to the Schema Store key format specification and confirm no collision.
