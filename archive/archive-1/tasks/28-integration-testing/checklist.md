# Checklist: Task 28 — Implement Integration Testing & Hardening

**Parent:** Task 20 (this checklist)  
**Implements:** End-to-end integration tests, fuzz harness, concurrency stress tests, and doc-test coverage for all public API items.

Execute items in order. After each item, run the verification command(s) listed. Do not proceed until verification passes.

---

## Phase 0: Test Infrastructure Setup

### 0.1 — Review existing test code and create shared helpers

Before writing any new tests, review all existing tests in `src/` (unit tests) and `tests/` (integration tests) from Tasks 22–27. Identify:
- What test helpers already exist (graph builder utilities, temp file management, mock validators/rules).
- What integration test files already exist and their naming conventions.
- What test-only `ConstraintValidator` and `InferenceRule` implementations already exist.

Create (or extend) a shared test helper module. If `tests/` does not already have a helpers module, create `tests/helpers/mod.rs` with:

```rust
// tests/helpers/mod.rs
// Shared test helpers for integration tests.
```

If the test harness requires a different structure (e.g., each test file is a separate crate), adapt accordingly — the helper code may need to live in a `test_utils` module within `src/` gated behind `#[cfg(test)]`.

**Verify:** `cargo test` still passes (no regressions from file additions).

### 0.2 — Implement test-only ConstraintValidator

If not already implemented by Tasks 25–27, create a test-only constraint validator that enforces "every node of type X must have property key Y set to a non-null value." This is the simplest useful validator for integration testing.

```rust
/// Test-only: requires that nodes of a specific type have a specific
/// property set to a non-null value.
pub(crate) struct RequiredPropertyValidator {
    pub target_type: TypeId,
    pub required_key: PropertyKeyId,
    pub property_name: String,
}

impl ConstraintValidator for RequiredPropertyValidator {
    fn name(&self) -> &str { "test::RequiredProperty" }
    fn applies_to_types(&self) -> Option<Vec<TypeId>> {
        Some(vec![self.target_type])
    }
    fn validate(
        &self,
        changes: &ChangeSet<'_>,
        graph: &dyn GraphView,
        _types: &dyn TypeRegistryView,
        _keys: &dyn PropertyKeyRegistryView,
    ) -> Vec<ConstraintViolation> {
        // Check inserted and modified nodes of target_type
        // Return ConstraintViolation if property is missing or null
        // ...
    }
}
```

If a similar validator already exists from an earlier task, reuse it. The key requirement is that it genuinely validates data — it must be able to both pass and fail.

**Verify:** `cargo check --tests`

### 0.3 — Implement test-only InferenceRule

If not already implemented by Task 26, create a test-only inference rule. The recommended rule: "for every edge of type `knows` from A → B, infer an edge of type `known_by` from B → A." This is simple enough to verify by inspection and exercises all parts of the inference pipeline.

```rust
/// Test-only: infers inverse edges.
/// For every `source_edge_type` edge A→B, infers an `inverse_edge_type` edge B→A.
pub(crate) struct InverseEdgeRule {
    pub source_edge_type: TypeId,
    pub inverse_edge_type: TypeId,
}

impl InferenceRule for InverseEdgeRule {
    fn name(&self) -> &str { "test::InverseEdge" }
    fn applies_to_types(&self) -> Option<Vec<TypeId>> {
        Some(vec![self.source_edge_type])
    }
    fn infer(
        &self,
        graph: &dyn GraphView,
        _types: &dyn TypeRegistryView,
        _keys: &dyn PropertyKeyRegistryView,
    ) -> InferenceResult {
        // Scan edges of source_edge_type, produce NewEdge facts for inverse
        // ...
    }
}
```

If a similar rule already exists from Task 26, reuse it.

**Verify:** `cargo check --tests`

### 0.4 — Create test graph builder utility

Create a helper function that builds a "standard test graph" used across multiple scenarios. The graph should include:
- 4 node types: `"Entity"` (root), `"Person"` (subtype of Entity), `"Organization"` (subtype of Entity), `"Project"` (independent type)
- 3 edge types: `"knows"`, `"works_at"`, `"leads"`
- 4 property keys: `"name"` (String), `"age"` (I64), `"founded"` (I64), `"active"` (Bool)
- 8+ nodes: at least 3 Person, 2 Organization, 3 Project
- 12+ edges: a mix of knows, works_at, and leads

The helper returns a struct containing all the TypeIds, PropertyKeyIds, NodeIds, and EdgeIds for assertions.

```rust
pub(crate) struct TestGraph {
    // Type IDs
    pub entity_type: TypeId,
    pub person_type: TypeId,
    pub org_type: TypeId,
    pub project_type: TypeId,
    pub knows_type: TypeId,
    pub works_at_type: TypeId,
    pub leads_type: TypeId,
    // Property key IDs
    pub name_key: PropertyKeyId,
    pub age_key: PropertyKeyId,
    pub founded_key: PropertyKeyId,
    pub active_key: PropertyKeyId,
    // Node IDs
    pub alice: NodeId,
    pub bob: NodeId,
    pub carol: NodeId,
    pub acme: NodeId,
    pub globex: NodeId,
    pub proj_alpha: NodeId,
    pub proj_beta: NodeId,
    pub proj_gamma: NodeId,
    // Edge IDs (representative subset)
    pub alice_knows_bob: EdgeId,
    pub bob_knows_carol: EdgeId,
    pub alice_works_at_acme: EdgeId,
    pub alice_leads_alpha: EdgeId,
    // ... etc.
}

/// Build the standard test graph in the given database.
/// Returns all IDs for assertion.
pub(crate) fn build_test_graph(db: &Database) -> Result<TestGraph, Error> {
    // Register types, property keys, insert nodes and edges
    // ...
}
```

**Verify:** `cargo check --tests`

---

## Phase 1: End-to-End Integration Tests

All tests in this phase are integration tests living in `tests/`. Each test creates its own database (temp file or in-memory) and is fully self-contained.

### 1.1 — Scenario 1: Basic CRUD round-trip (persistent)

Create a test that exercises the fundamental create-read-update-delete cycle with a persistent database:

1. Open a persistent database at a temp path.
2. Begin a write transaction.
3. Register a node type `"Person"` and a property key `"name"` (String).
4. Insert 3 nodes of type Person with name properties.
5. Insert 2 edges of a `"knows"` edge type between nodes.
6. Commit.
7. Begin a read transaction.
8. Verify: `node_count()` returns 3. `edge_count()` returns 2.
9. Verify: `get_node(id)` returns each node with correct type and properties.
10. Verify: `get_edge(id)` returns each edge with correct source, target, type.
11. Verify: `outgoing_edges(alice, Some(knows_type))` returns expected edges.
12. Verify: `incoming_edges(bob, Some(knows_type))` returns expected edges.
13. Begin another write transaction.
14. Update a node's property (change a name).
15. Delete one edge.
16. Delete one node (verify cascading edge deletion).
17. Commit.
18. Begin a read transaction.
19. Verify: updated property is visible. Deleted node/edges are gone. Counts are correct.

**Verify:** `cargo test --test integration -- e2e_basic_crud` (adapt test name to actual file structure) passes.

### 1.2 — Scenario 2: Type hierarchy and subtype-aware queries

1. Open a persistent database at a temp path.
2. Register a type hierarchy: `"Entity"` → `"Person"`, `"Entity"` → `"Organization"`.
3. Register an edge type `"relates_to"`.
4. Insert 3 Person nodes, 2 Organization nodes.
5. Commit.
6. Read transaction:
   - `nodes_by_type(person_type, include_subtypes: false)` → returns 3 nodes.
   - `nodes_by_type(entity_type, include_subtypes: false)` → returns 0 nodes (no nodes are typed *only* as Entity).
   - `nodes_by_type(entity_type, include_subtypes: true)` → returns 5 nodes (all Person + Organization nodes).
7. Verify `type_definition(person_type)` shows `supertypes: [entity_type]`.
8. Verify schema error on cycle: attempt to make Entity a subtype of Person → `Error::Schema(CycleDetected { .. })`.

**Verify:** `cargo test -- e2e_schema_hierarchy` passes.

### 1.3 — Scenario 3: Persistence close/reopen round-trip

This is the critical durability test.

1. Open a persistent database at a temp path.
2. Build the standard test graph (using the helper from 0.4).
3. Record all node IDs, edge IDs, types, and property values.
4. Drop the `Database` object (closes file handles, flushes).
5. Re-open the same file path.
6. Begin a read transaction.
7. Verify **every** piece of data survives the round-trip:
   - All node types and property keys are present in the schema.
   - Every node is retrievable by ID with correct type labels and properties.
   - Every edge is retrievable by ID with correct source, target, type, properties.
   - `node_count()` and `edge_count()` match pre-close values.
   - Adjacency queries return the same results.
   - Type hierarchy (supertypes) is preserved.
8. Perform a write after reopen: insert a new node, commit. Verify it persists through another close/reopen cycle.

**Verify:** `cargo test -- e2e_persistence` passes.

### 1.4 — Scenario 4: Full extension system round-trip (REQUIRED)

This is the mandatory scenario specified in the task description. It exercises the entire extension lifecycle including persistence.

1. Open a persistent database at a temp path.
2. Register the test-only `RequiredPropertyValidator` (from 0.2) targeting Person nodes and the `"name"` property.
3. Register the test-only `InverseEdgeRule` (from 0.3) with `knows` → `known_by`.
4. Register custom types: `"Person"` (node), `"knows"` (edge), `"known_by"` (edge).
5. Register property key: `"name"` (String).
6. **Test constraint validation — rejection:**
   - Begin write transaction.
   - Insert a Person node **without** the `"name"` property.
   - Attempt to commit → expect `Error::ConstraintViolation`.
7. **Test constraint validation — acceptance:**
   - Begin write transaction.
   - Insert a Person node **with** `"name"` property.
   - Insert two more Person nodes with names.
   - Insert `"knows"` edges: Alice→Bob, Bob→Carol.
   - Commit → expect success.
8. **Test inference — ephemeral mode:**
   - Begin read transaction.
   - Call `run_inference("test::InverseEdge")` (or equivalent).
   - Verify result contains NewEdge facts for Bob→Alice and Carol→Bob.
   - Drop read transaction. Verify ephemeral facts are not persisted.
9. **Test inference — materialized mode:**
   - Begin write transaction.
   - Call `run_inference("test::InverseEdge", InferenceMode::Materialized)`.
   - Verify materialized edges are created.
   - Commit.
   - Begin read transaction. Verify the `known_by` edges exist in the graph.
10. **Test provenance queries:**
    - Verify `is_inferred_edge(known_by_edge_id)` returns true.
    - Verify `edge_provenance(known_by_edge_id)` returns the correct provenance record.
    - Verify `is_inferred_node(alice_id)` returns false (Alice was explicitly inserted).
11. **Persistence round-trip:**
    - Drop the Database.
    - Re-open the same file.
    - Verify `missing_extensions()` reports `"test::RequiredProperty"` and `"test::InverseEdge"` as missing.
    - Re-register both extensions.
    - Verify `missing_extensions()` returns empty.
    - Begin read transaction. Verify all data is intact:
      - Original nodes and edges are present.
      - Materialized `known_by` edges are present.
      - Provenance records are preserved.
12. **Test extension unregistration:**
    - Unregister the inference rule.
    - Verify `inference_rule_names()` no longer contains `"test::InverseEdge"`.
    - Attempt `run_inference("test::InverseEdge")` → expect `Error::Inference(RuleNotFound(..))`.

**Verify:** `cargo test -- e2e_extension_roundtrip` passes.

### 1.5 — Scenario 5: Persistent vs. in-memory equivalence

Verify that the same sequence of operations produces identical results on both backends.

1. Define a deterministic operation sequence:
   - Register types and property keys.
   - Insert nodes and edges.
   - Commit.
   - Read back all data.
2. Execute the sequence on a persistent database (temp file).
3. Execute the same sequence on an in-memory database.
4. Compare results field-by-field:
   - Same node/edge counts.
   - Same node/edge data (types, properties).
   - Same adjacency query results.
   - Same type hierarchy queries.

**⚠ Pitfall:** Node/edge IDs should be identical if the ID allocation algorithm is deterministic from an empty database. If IDs differ, compare by content rather than by ID.

**Verify:** `cargo test -- e2e_cross_backend` passes.

### 1.6 — Scenario 6: Complex multi-hop traversal

Build a rich graph and verify multi-hop traversal correctness.

1. Open a database (in-memory is fine for speed).
2. Build the standard test graph (8+ nodes, 12+ edges, 3+ edge types, type hierarchy).
3. Multi-hop traversal: starting from Alice, follow `works_at` edges to find her Organization, then follow `works_at` edges **incoming** to find all co-workers, then follow `knows` edges from co-workers to find their acquaintances.
   - This is a 3-hop traversal composing single-hop primitives.
   - Verify the result set contains exactly the expected nodes.
4. Multi-hop traversal: starting from a Project, follow `leads` edges **incoming** to find leaders, then follow `knows` edges to find their acquaintances, then follow `works_at` edges to find the organizations those acquaintances belong to.
   - This is a 3-hop traversal across 3 different edge types.
   - Verify the result set.
5. Filtered traversal: repeat traversal (3) but filter edges by type at each hop. Verify filtering produces the correct subset.
6. Subtype-aware query: query `nodes_by_type(entity_type, include_subtypes: true)` and verify all Person + Organization nodes appear.
7. Count verification: `node_count()`, `edge_count()`, `nodes_by_type` counts all consistent.

**Verify:** `cargo test -- e2e_complex_traversal` passes.

### 1.7 — Scenario 7: Edge cases and error paths

Test error conditions and boundary cases that span the full stack.

1. **Empty database queries:** Open database, immediately `read_txn()`, verify `node_count()` is 0, `get_node(NodeId(1))` returns `NotFound`.
2. **Empty transaction commit:** Begin write transaction, commit with no changes. Verify success and no side effects.
3. **Delete nonexistent node:** `delete_node(NodeId(999))` → `Error::NotFound`.
4. **Delete nonexistent edge:** `delete_edge(EdgeId(999))` → `Error::NotFound`.
5. **Duplicate type name:** Register `"Person"` (node type) twice → `Error::Schema(DuplicateTypeName)`.
6. **Type kind mismatch:** Register `"Person"` as a node type, then try to use it as an edge type when inserting an edge → verify appropriate error.
7. **Parallel edges:** Insert two edges from A→B with the same type. Verify both are returned by `outgoing_edges`. Delete one, verify only the other remains.
8. **Cascading delete:** Insert node A with 5 incident edges (mix of outgoing/incoming). Delete node A. Verify all 5 edges are also deleted.
9. **Property update:** Insert node with properties, update one property, verify only the updated property changed.
10. **`validate_all()` after schema change:** Build a graph, then register a new constraint that some existing data violates. Call `validate_all()`. Verify it returns the expected violations.
11. **Large property values:** Insert a node with a `Value::Bytes(vec![0u8; 10_000])` property. Read it back and verify exact match. This tests overflow page handling.
12. **ID recycling (if applicable):** Insert a node, delete it, insert another node. If the database recycles IDs, verify the new node gets the recycled ID. If not, document the behavior.

**Verify:** `cargo test -- e2e_edge_cases` passes.

---

## Phase 2: Concurrency Stress Tests

All tests in this phase spawn multiple threads and assert consistency invariants. They should use `Arc<Database>` for cross-thread sharing and create transactions on each thread (transactions are `!Send`).

### 2.1 — Snapshot isolation under continuous writes

Test that readers always see consistent snapshots even while a writer is actively committing.

1. Open a persistent database with initial data: 100 nodes (numbered 1–100), each with a `"value"` property set to its index.
2. Spawn 1 writer thread that executes 20 write transactions in a loop, each updating a batch of 10 nodes (incrementing their `"value"` by 1).
3. Spawn 4 reader threads that each execute 50 read transactions in a loop. Each read transaction:
   - Reads all 100 nodes.
   - Verifies: every node is present (none missing). The `"value"` properties are internally consistent (if node 5 shows value V, and 5 was in the same batch as node 6, then node 6 should also show value V or V−1 — no partial-batch visibility).
   - Records the snapshot's maximum value seen (for eventual ordering assertion).
4. Join all threads.
5. Verify: no panics, no errors. Reader threads never saw partial transactions.

**⚠ Pitfall — "partial-batch visibility" assertion.** The simplest invariant is: within a single read transaction, the data is self-consistent (every node is present, counts are correct). The specific "same batch" assertion depends on how values are grouped in write transactions. Design the write batches to make the invariant checkable — e.g., all nodes in a batch get the same generation counter.

**Verify:** `cargo test -- concurrency_snapshot_isolation` passes.

### 2.2 — Write serialization under contention

Test that the single-writer lock correctly serializes concurrent write attempts.

1. Open a database.
2. Spawn 8 threads, each attempting to:
   - `write_txn()` (this blocks until the lock is available).
   - Insert a unique node with a thread-specific property value.
   - Commit.
3. Join all threads.
4. Open a read transaction.
5. Verify: exactly 8 nodes exist, each with a unique thread-specific value. No duplicates, no missing values.

This confirms that `write_txn()` correctly queues and serializes concurrent callers.

**Verify:** `cargo test -- concurrency_write_contention` passes.

### 2.3 — High-throughput mixed read/write stress test

A sustained high-contention test to surface subtle races.

1. Open a database with initial data (50 nodes, 100 edges).
2. Spawn 1 writer thread that continuously:
   - Begins a write transaction.
   - Inserts 5 nodes and 10 edges (random but valid connectivity).
   - Commits.
   - Repeats for 50 iterations.
3. Spawn 6 reader threads that continuously:
   - Begin a read transaction.
   - Read `node_count()` and `edge_count()`.
   - Read a random sample of 10 node IDs.
   - Follow outgoing edges for each sampled node.
   - Drop the transaction.
   - Repeat until the writer is done.
4. Run for a bounded duration (5 seconds maximum) or until the writer finishes.
5. Join all threads.
6. Final verification: open a read transaction and verify `node_count()` and `edge_count()` match expected totals (50 + 50×5 = 300 nodes, 100 + 50×10 = 600 edges).

**⚠ Pitfall — test runtime.** This test may run for several seconds. Mark with `#[ignore]` and a doc comment: `// Stress test: ~5 seconds. Run with `cargo test -- --ignored`.`

**Verify:** `cargo test -- concurrency_high_throughput --ignored` passes.

### 2.4 — Concurrent access with in-memory backend

Repeat a simplified version of test 2.1 using `DatabaseConfig::in_memory()` instead of persistent. This verifies the in-memory backend's concurrency correctness.

1. Open an in-memory database.
2. Populate with initial data (50 nodes).
3. Spawn 1 writer + 3 readers (same pattern as 2.1, reduced iterations).
4. Verify: no panics, no corruption, readers see consistent snapshots.

**Verify:** `cargo test -- concurrency_in_memory` passes.

---

## Phase 3: Fuzz Testing

### 3.1 — Set up cargo-fuzz infrastructure

Initialize the fuzz testing framework:

1. Run `cargo fuzz init` (if `fuzz/` directory doesn't exist).
2. Add the crate as a dependency in `fuzz/Cargo.toml`:
   ```toml
   [dependencies]
   graph_db = { path = ".." }
   libfuzzer-sys = "0.4"
   ```
3. Verify the directory structure:
   ```
   fuzz/
   ├── Cargo.toml
   └── fuzz_targets/
   ```

**⚠ Pitfall — network access.** `cargo fuzz init` requires `cargo-fuzz` to be installed. If it is not available in the build environment, create the `fuzz/Cargo.toml` and directory structure manually. The `libfuzzer-sys` crate must be available (may need network access).

**Verify:** `fuzz/Cargo.toml` exists and is valid.

### 3.2 — Fuzz target: record deserialization

Create `fuzz/fuzz_targets/fuzz_record_deser.rs`:

This target feeds arbitrary byte sequences into the record deserialization functions — the functions that parse `NodeRecord` and `EdgeRecord` from raw bytes (as stored in B-tree leaf cells).

```rust
#![no_main]
use libfuzzer_sys::fuzz_target;
use graph_db::...; // import the deserialization function(s)

fuzz_target!(|data: &[u8]| {
    // Attempt to deserialize a NodeRecord from arbitrary bytes.
    // This should never panic — it should return Err for invalid data.
    let _ = NodeRecord::from_bytes(data);

    // Attempt to deserialize an EdgeRecord from arbitrary bytes.
    let _ = EdgeRecord::from_bytes(data);

    // Attempt to parse B-tree keys from arbitrary bytes.
    // (add any other deserialization entry points here)
});
```

The goal: no panics, no buffer overflows, no undefined behavior. `Err` returns for invalid data are expected and correct.

**⚠ Pitfall — identifying the right functions.** The exact deserialization functions depend on the implementation from Tasks 24–25. Examine `src/storage/serialization.rs` (or equivalent) to find the entry points. The fuzz target should exercise every public or `pub(crate)` deserialization function.

**Verify:** `cargo fuzz build` succeeds (if available). Otherwise, `cargo check --manifest-path fuzz/Cargo.toml` succeeds.

### 3.3 — Fuzz target: API operation sequences

Create `fuzz/fuzz_targets/fuzz_api_operations.rs`:

This target interprets arbitrary bytes as a sequence of database operations and executes them on an in-memory database. The operations include: insert node, insert edge, delete node, delete edge, update node, query node, query edges. The goal is to find operation sequences that cause panics or corruption.

```rust
#![no_main]
use libfuzzer_sys::fuzz_target;
use graph_db::*;

fuzz_target!(|data: &[u8]| {
    // Use bytes to drive a sequence of operations.
    // Each byte (or small group of bytes) is interpreted as an operation
    // code + operand.
    //
    // Example encoding:
    //   0x00..0x1F: insert node with type_id = byte & 0x0F
    //   0x20..0x3F: insert edge with source/target from recent nodes
    //   0x40..0x4F: delete most recently inserted node
    //   0x50..0x5F: begin new write transaction + commit
    //   0x60..0x6F: query node_count
    //   ... etc.
    //
    // All operations are wrapped in catch_unwind or simply allowed
    // to return errors (errors are OK, panics are bugs).

    let db = match Database::open(DatabaseConfig::in_memory()) {
        Ok(db) => db,
        Err(_) => return,
    };

    // Interpret `data` as an operation sequence...
    // (implementation depends on exact API surface)
});
```

**Verify:** `cargo fuzz build` succeeds (if available). Otherwise, `cargo check --manifest-path fuzz/Cargo.toml` succeeds.

### 3.4 — Run fuzz tests

Run each fuzz target for a minimum of 60 seconds:

```bash
cargo fuzz run fuzz_record_deser -- -max_total_time=60
cargo fuzz run fuzz_api_operations -- -max_total_time=60
```

**⚠ Pitfall — CI environment.** `cargo fuzz` requires nightly Rust and LLVM sanitizers. If not available, document the limitation and provide the fuzz targets for manual execution. The fuzz targets should still compile under stable Rust (the `libfuzzer-sys` harness is nightly-only, but the target code itself is standard Rust).

**Verify:** Both fuzz targets run for 60 seconds without panics or crashes. Record the output (number of runs, corpus size) in the completion report.

---

## Phase 4: Doc-Test Coverage

This phase adds `/// # Examples` doc-tests to every public API item. Doc-tests must be runnable — they use `DatabaseConfig::in_memory()` for convenience (no temp files needed).

### 4.1 — Doc-tests for identity types

Add doc-tests to all ID types in `src/types/`:
- `NodeId`: construction, `NULL`, `is_null()`, `Display`.
- `EdgeId`: construction, `NULL`, `is_null()`.
- `TypeId`: construction, `NULL`, `is_null()`.
- `PropertyKeyId`: construction, `NULL`, `is_null()`.

Example:
```rust
/// A unique identifier for a node in the graph.
///
/// # Examples
/// ```
/// use graph_db::NodeId;
///
/// let id = NodeId(42);
/// assert_eq!(id.0, 42);
/// assert!(!id.is_null());
/// assert!(NodeId::NULL.is_null());
/// ```
```

**Verify:** `cargo test --doc -- types` passes.

### 4.2 — Doc-tests for Value and ValueTypeDescriptor

Add doc-tests to:
- `Value` enum: construction of each variant, `is_null()`, `as_*()` accessors.
- `ValueTypeDescriptor` enum: construction, `matches_descriptor()`.
- `Value::matches_descriptor()`: show matching and non-matching examples.

**Verify:** `cargo test --doc -- types` passes.

### 4.3 — Doc-tests for Node, Edge, PropertyMap

Add doc-tests to:
- `Node`: construction with type labels and properties.
- `Edge`: construction with source, target, type labels.
- `PropertyMap` type alias: show creating a BTreeMap and inserting properties.

**Verify:** `cargo test --doc -- types` passes.

### 4.4 — Doc-tests for type system types

Add doc-tests to:
- `TypeKind`: show `Node` and `Edge` variants.
- `TypeDefinition`: construction with name, kind, supertypes.
- `PropertyDeclaration`: construction with key, value_type, required flag.

**Verify:** `cargo test --doc -- types` passes.

### 4.5 — Doc-tests for schema traits

Add doc-tests to:
- `GraphView` trait: show a brief example of how a mock implementation would look, or a usage example with `ReadTransaction`.
- `TypeRegistryView` trait: similar.
- `PropertyKeyRegistryView` trait: similar.

**⚠ Pitfall:** Trait doc-tests are trickier — you can't easily instantiate a trait object in a doc-test. Show usage through the concrete `ReadTransaction` or provide a brief mock. If a runnable example is impractical, use `/// ```no_run` or `/// ```ignore` with a comment explaining why.

**Verify:** `cargo test --doc -- schema` passes (or no failures if using `no_run`).

### 4.6 — Doc-tests for constraint types

Add doc-tests to:
- `ConstraintValidator` trait: show a minimal implementation skeleton.
- `ChangeSet`: show how to inspect changes.
- `NodeChange`, `EdgeChange`: show pattern matching on variants.
- `ConstraintViolation`: construction.
- `ViolationSubject`: construction.

**Verify:** `cargo test --doc -- constraint` passes.

### 4.7 — Doc-tests for inference types

Add doc-tests to:
- `InferenceRule` trait: show a minimal implementation skeleton.
- `InferredFact`: show each variant.
- `InferenceResult`: construction.
- `InferenceMode`: show `Ephemeral` and `Materialized` variants.
- `ProvenanceRecord`: construction and field access.
- `InferredEntity`: construction.
- `MaterializedMapping`: construction.

**Verify:** `cargo test --doc -- inference` passes.

### 4.8 — Doc-tests for error types

Add doc-tests to:
- `Error` enum: show each variant.
- `SchemaError`, `NotFoundError`, `TransactionError`, `InferenceError`: construction and Display.
- `StorageError`: construction (with and without source under `std`).

**Verify:** `cargo test --doc -- error` passes.

### 4.9 — Doc-tests for Database and DatabaseConfig

Add doc-tests to:
- `DatabaseConfig::persistent()`: show creation with defaults.
- `DatabaseConfig::in_memory()`: show creation.
- Builder methods: `buffer_pool_frames()`, `page_size()`, `extension_startup_check()`.
- `Database::open()`: show opening an in-memory database.
- `Database::read_txn()`: show opening a read transaction.
- `Database::write_txn()`: show opening a write transaction.
- `Database::register_constraint()`: show registering a validator.
- `Database::register_inference_rule()`: show registering a rule.
- `Database::constraint_names()`: show listing names.
- `Database::inference_rule_names()`: show listing names.
- `Database::missing_extensions()`: show checking for missing extensions.

Example for `Database::open()`:
```rust
/// # Examples
/// ```
/// use graph_db::{Database, DatabaseConfig};
///
/// let db = Database::open(DatabaseConfig::in_memory()).unwrap();
/// let txn = db.read_txn().unwrap();
/// assert_eq!(txn.node_count().unwrap(), 0);
/// ```
```

**Verify:** `cargo test --doc -- db` passes.

### 4.10 — Doc-tests for ReadTransaction

Add doc-tests to every public method on `ReadTransaction`:
- `get_node()`: show reading a node.
- `get_edge()`: show reading an edge.
- `node_count()`, `edge_count()`.
- `nodes_by_type()`: with and without `include_subtypes`.
- `edges_by_type()`.
- `outgoing_edges()`, `incoming_edges()`.
- `nodes_by_property()`.
- `type_definition()`, `all_type_definitions()`.
- `property_key()`, `all_property_keys()`.
- `run_inference()`.
- Provenance queries: `is_inferred_node()`, `is_inferred_edge()`, `node_provenance()`, `edge_provenance()`.

Each doc-test should set up a minimal database (in-memory, insert data in a write txn, commit, then demonstrate the read method).

**⚠ Pitfall — doc-test verbosity.** Each doc-test must be self-contained, which means repeating setup code. Keep it minimal: use the smallest graph that demonstrates the method. Consider a `/// ```` block that uses a helper function if the setup is complex, but ensure the doc-test is still runnable.

**Verify:** `cargo test --doc -- read_txn` passes.

### 4.11 — Doc-tests for WriteTransaction

Add doc-tests to every public method on `WriteTransaction`:
- `register_type()`: show registering a node type and an edge type.
- `register_property_key()`.
- `insert_node()`, `insert_edge()`.
- `update_node()`, `update_edge()`.
- `delete_node()`, `delete_edge()`.
- `commit()`: show a successful commit.
- `rollback()` (if exposed): show discarding a transaction.
- `run_inference()` with `InferenceMode`.
- `validate_all()`.
- Read-your-own-writes: show inserting a node and reading it back within the same write transaction.

**Verify:** `cargo test --doc -- write_txn` passes.

### 4.12 — Doc-tests for builder types

Add doc-tests to:
- `NodeBuilder`: show building a node with type and properties.
- `EdgeBuilder`: show building an edge.
- `TypeDefinitionBuilder`: show building a type with supertypes and property declarations.

**Verify:** `cargo test --doc -- builder` passes.

### 4.13 — Doc-tests for HAL types (if public)

If the HAL module exposes public types (e.g., `StorageBackend`, `ReadAt`, `WriteAt` traits), add doc-tests showing:
- Trait method signatures and expected behavior.
- `FileBackend` construction (under `std`).
- `MemoryBackend` construction.

If HAL traits are `pub(crate)` only, skip this step and note it in the completion report.

**Verify:** `cargo test --doc` passes (or no applicable doc-tests).

### 4.14 — Full doc-test sweep

Run the complete doc-test suite:

```bash
cargo test --doc
```

Fix any failures. Then verify that `cargo doc --no-deps` produces no warnings — every public item should have documentation.

**Verify:**
- `cargo test --doc` passes with zero failures.
- `cargo doc --no-deps` produces zero warnings.

---

## Phase 5: Final Verification

### 5.1 — Full test suite

```bash
cargo test
```

All tests pass — existing unit tests, new integration tests, new doc-tests. Zero failures.

### 5.2 — Ignored tests (stress tests)

```bash
cargo test -- --ignored
```

All stress/fuzz tests pass.

### 5.3 — Clippy

```bash
cargo clippy --all-targets --all-features -- -D warnings
```

Zero warnings.

### 5.4 — Documentation

```bash
cargo doc --no-deps
```

Zero warnings. Every public item has a doc comment with at least one example.

### 5.5 — no_std verification

```bash
cargo check --no-default-features --features alloc
```

Must succeed. Integration tests and doc-tests are `std`-only, but the core types they test remain `no_std`-compatible.

### 5.6 — Test count audit

Count the tests and verify the acceptance criteria are met:
- [ ] 7+ end-to-end integration tests (Phase 1).
- [ ] 3+ concurrency stress tests (Phase 2).
- [ ] 2 fuzz targets built and executed for 60+ seconds each (Phase 3).
- [ ] Doc-tests on all public API items (Phase 4).
- [ ] Zero `#[ignore]` without a documented reason.

Record the counts in the completion report.

### 5.7 — Review for test isolation

Scan all integration tests and verify:
- No test reads from or writes to a hardcoded file path.
- Every persistent test uses `tempfile::TempDir` or equivalent.
- No test depends on execution order.
- No test shares state with another test.

---

## Post-Completion

Produce a completion report following the format in the master project prompt's Instance Rules section. Include:
- Status (COMPLETE / PARTIAL / BLOCKED).
- Verification evidence from Phase 5 (test counts, clippy output, doc-test output).
- Fuzz testing results (number of executions, corpus size, any findings).
- Summary of any bugs found during testing (with reproduction steps).
- List of any `#[ignore]`d tests with documented justifications.
- Context for Task 29 (Documentation & Publish Preparation): what doc-test coverage is now in place, what documentation gaps remain.
- Any residual concerns or deferred items.
