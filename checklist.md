# checklist.md — Static Dispatch & Consistency Hardening

**Status:** Not started

This checklist brings the codebase into conformance with architectural principles A7–A9
defined in the project root `CLAUDE.md`. It addresses all findings from the March 2026
consistency and security review: replacing dynamic dispatch with static dispatch where
possible, fixing the `OverlayGraphView` subtype gap, correcting `PartialEq`→`total_eq`
inconsistencies, and hardening counter deserialization.

Execute items in order. Each step has a verification command — do not proceed until
it passes. After completing each step and passing its verification, mark it done by
changing `- [ ]` to `- [x]` in this file.

---

## Required Reading

Before writing any code, read these files:

1. **Project root `CLAUDE.md`** — All project-wide rules and architectural principles,
   especially A7 (static dispatch), A8 (float comparisons), A9 (overlay contract).
2. **`crates/phonograph_db/src/db/graph_view.rs`** — `OverlayGraphView` struct, its
   `build()` method (currently takes `&dyn SnapshotReader`), `SnapshotReader` trait,
   and `GraphView` impl. This file is the primary target of Phases 1–3.
3. **`crates/phonograph_db/src/db/write_txn.rs`** — `WriteTransaction`, the
   `BaseSnapshotReader` adapter (implements `SnapshotReader`), `run_validators()`,
   `run_inference()`, `commit()`, and `nodes_by_property()`. Contains the call sites
   for `OverlayGraphView::build` and the `PartialEq` property lookup that must
   change to `total_eq`. Phase 3 adds `nodes_by_type_ids`/`edges_by_type_ids`
   overrides to `BaseSnapshotReader`.
4. **`crates/phonograph_db/src/storage/serialization.rs`** — Contains
   `encode_type_index_key` / `decode_type_index_key` used by Phase 3's
   `BaseSnapshotReader` type-index scan overrides.
5. **`crates/phonograph_db/src/db/schema_cache.rs`** — `SchemaCache`, its
   `TypeRegistryView` impl (specifically `all_subtypes()`), and the
   `next_type_id`/`next_property_key_id` counter fields.
6. **`crates/phonograph_db/src/db/database.rs`** — `load_schema()` method where
   counters are deserialized from `u64` and cast to `u32` (finding #12).
7. **`crates/phonograph_db/src/db/inference_engine.rs`** — `InferenceCache::get()`
   where the cache key allocates a `String` on every lookup (finding #11).
8. **`crates/phonograph/src/types/mod.rs`** — `Value::total_eq()` method.
9. **`crates/phonograph_std/tests/inference_tests.rs`** — Integration tests for the
   inference hook infrastructure. Must still pass after all changes.

## Done When

1. `OverlayGraphView::build()` takes `base: &impl SnapshotReader` (static dispatch).
2. `OverlayGraphView` holds a `&'s SchemaCache` reference.
3. `nodes_by_type` and `edges_by_type` on `OverlayGraphView` correctly resolve
   subtypes using the schema cache when `include_subtypes` is `true`.
4. `OverlayGraphView::build()` accepts an optional `affected_types` hint and only
   loads base entities of matching types (plus adjacency neighbors of changed nodes)
   when the hint is provided. Full-database load is preserved when the hint is `None`.
5. `nodes_by_property` on `OverlayGraphView` uses `Value::total_eq()` instead of
   `PartialEq` (`==`).
6. `nodes_by_property` in `WriteTransaction` uses `Value::total_eq()`.
7. Counter deserialization in `Database::load_schema` validates that persisted
   `u64` values fit in `u32` before casting for `next_type_id` and
   `next_property_key_id`.
8. `InferenceCache::get()` avoids allocating a `String` for the lookup key.
9. All existing tests pass. New tests cover subtype resolution in overlay,
   `total_eq` property lookup, changeset-scoped preloading, and counter overflow
   detection.
10. All 17 verification checks from `CLAUDE.md` pass.

## Key Pitfalls

1. **`OverlayGraphView` gains a lifetime parameter.** Changing the struct from
   `OverlayGraphView` to `OverlayGraphView<'s>` propagates to the `GraphView`
   impl (`impl GraphView for OverlayGraphView<'_>`) and all call sites. The
   `build()` signature change also means `write_txn.rs` call sites must pass
   `&self.schema_cache` as the third argument instead of `&SchemaCache`.

2. **`build()` changes from `&dyn SnapshotReader` to `&impl SnapshotReader`.**
   This is a straightforward change but `impl Trait` in function parameters
   makes the function generic, so the compiler monomorphizes it for each caller.
   Verify that both `BaseSnapshotReader` (production) and `MockSnapshot` (tests)
   still work.

3. **Calling `self.schema.all_subtypes()` inside `GraphView` methods requires
   importing `TypeRegistryView`.** Add `use phonograph::schema::TypeRegistryView;`
   inside the method or at the module level. `SchemaCache` implements
   `TypeRegistryView`, so this is just a trait import.

4. **`InferenceCache` uses `BTreeMap<(String, u64), CacheEntry>`.** To avoid
   allocation in `get()`, the idiomatic approach is to keep the `BTreeMap` but
   construct a temporary key for lookup. Since `BTreeMap::get` takes `&Q` where
   `K: Borrow<Q>`, and `String: Borrow<str>`, a tuple `(&str, u64)` cannot
   directly borrow from `(String, u64)`. The simplest correct fix is to change
   the map to `HashMap<(String, u64), CacheEntry>` (from `hashbrown`) and use
   a helper, or restructure to a two-level map: `BTreeMap<String, BTreeMap<u64, CacheEntry>>`.
   The two-level approach avoids both the allocation and the Borrow complexity.

5. **Counter bounds checking must produce a `StorageError`, not a panic.** The
   current code does `counter_val as u32` which silently truncates. Replace with
   `u32::try_from(counter_val).map_err(...)` and return an appropriate
   `StorageError` if the value overflows.

6. **`write_txn.rs` `nodes_by_property` uses `n.properties.get(&key) == Some(value)`.**
   Replace with `n.properties.get(&key).map_or(false, |v| v.total_eq(value))`.
   Note: `Value` derives `PartialEq`, so the old code compiles but produces
   wrong results for `f64::NAN`.

7. **Changeset-scoped preloading is an intentional trade-off.** When
   `affected_types` is provided to `OverlayGraphView::build()`, the overlay only
   contains base entities whose types overlap with the hint plus adjacency
   neighbors of changed nodes. If a constraint validator calls
   `nodes_by_property` for a property on a type NOT in the affected set, the
   result may be incomplete. In practice this is safe because:
   (a) the `applies_to_types()` hint already scopes which validators are invoked,
   (b) validators inspect the changeset's neighborhood, not unrelated types.
   This trade-off is documented in the `build()` doc comment and in `CLAUDE.md`
   residual concern #7.

8. **`SnapshotReader` gains new methods with default impls.** Adding
   `nodes_by_type_ids` and `edges_by_type_ids` with default implementations
   (that filter `all_nodes()`/`all_edges()`) means `MockSnapshot` in tests
   doesn't need to override them — the defaults work correctly. But
   `BaseSnapshotReader` in `write_txn.rs` SHOULD override them to use the type
   index B-tree scan for real performance gains. If only the default impl is
   used in production, the optimization is a no-op.

---

## Phase 0: Setup

- [x] **0.1 — Record the current test baseline.**
  ```bash
  cargo test --workspace 2>&1 | tail -5
  cargo clippy --workspace --all-targets -- -D warnings
  ```
  Record the exact test count. This is the regression baseline.

  **Verify:** Zero failures, zero clippy warnings.

- [x] **0.2 — Verify the starting state of targeted files.**
  ```bash
  grep -n 'dyn SnapshotReader' crates/phonograph_db/src/db/graph_view.rs
  grep -n 'dyn SnapshotReader' crates/phonograph_db/src/db/write_txn.rs
  grep -n 'include_subtypes' crates/phonograph_db/src/db/graph_view.rs
  grep -n 'properties.get.*== Some' crates/phonograph_db/src/db/write_txn.rs
  grep -n 'properties.get.*== Some' crates/phonograph_db/src/db/graph_view.rs
  grep -n 'counter_val as u32' crates/phonograph_db/src/db/database.rs
  ```
  Save this output as a "before" record.

  **Verify:** Output captured and understood.

---

## Phase 1: `OverlayGraphView` — Static Dispatch & Schema Reference

> **Implements:** CLAUDE.md principles A7, A9

- [x] **1.1 — Add lifetime and schema reference to `OverlayGraphView`.**
  In `crates/phonograph_db/src/db/graph_view.rs`, change the struct definition:

  Before:
  ```rust
  pub(crate) struct OverlayGraphView {
      nodes: BTreeMap<NodeId, Node>,
      edges: BTreeMap<EdgeId, Edge>,
      outgoing_index: BTreeMap<NodeId, Vec<EdgeId>>,
      incoming_index: BTreeMap<NodeId, Vec<EdgeId>>,
  }
  ```

  After:
  ```rust
  pub(crate) struct OverlayGraphView<'s> {
      nodes: BTreeMap<NodeId, Node>,
      edges: BTreeMap<EdgeId, Edge>,
      outgoing_index: BTreeMap<NodeId, Vec<EdgeId>>,
      incoming_index: BTreeMap<NodeId, Vec<EdgeId>>,
      /// Schema cache for subtype resolution in `nodes_by_type`/`edges_by_type`.
      schema: &'s SchemaCache,
  }
  ```

  **Verify:** `cargo check -p phonograph_db` — expect errors from mismatched types (not yet
  propagated). That is fine; this step just establishes the struct.

- [x] **1.2 — Change `build()` from `&dyn SnapshotReader` to `&impl SnapshotReader`.**
  In the same file, change the `impl` block and `build` signature:

  Before:
  ```rust
  impl OverlayGraphView {
      pub fn build(
          base: &dyn SnapshotReader,
          buffer: &WriteBuffer,
          _schema: &SchemaCache,
      ) -> Self {
  ```

  After:
  ```rust
  impl<'s> OverlayGraphView<'s> {
      pub fn build(
          base: &impl SnapshotReader,
          buffer: &WriteBuffer,
          schema: &'s SchemaCache,
      ) -> Self {
  ```

  Also update the `Self { ... }` construction at the end of `build()` to include
  `schema,` as a field.

  **Verify:** `cargo check -p phonograph_db` — may still have errors from the `GraphView`
  impl. Proceed to 1.3.

- [x] **1.3 — Update `GraphView` impl to use `OverlayGraphView<'_>`.**
  Change:
  ```rust
  impl GraphView for OverlayGraphView {
  ```
  To:
  ```rust
  impl GraphView for OverlayGraphView<'_> {
  ```

  **Verify:** `cargo check -p phonograph_db` — may still have errors from call sites in
  `write_txn.rs`.

- [x] **1.4 — Update all `OverlayGraphView::build` call sites in `write_txn.rs`.**
  Search `write_txn.rs` for `OverlayGraphView::build`. Each call currently passes
  `&self.base_reader()`, `&self.buffer`, and `&self.schema_cache`. The signature change
  is source-compatible (the third argument is already `&self.schema_cache`), but the
  `base_reader()` return type is `BaseSnapshotReader` which implements `SnapshotReader`
  — this should just work with `&impl SnapshotReader`.

  Verify that `BaseSnapshotReader` still implements `SnapshotReader`. If the compiler
  requires an intermediate binding (due to temporary lifetimes), store the reader in a
  `let` binding:
  ```rust
  let reader = self.base_reader();
  let view = OverlayGraphView::build(&reader, &self.buffer, &self.schema_cache);
  ```

  **Verify:** `cargo check -p phonograph_db` — clean compile.

- [ ] **1.5 — Update the test `MockSnapshot` call sites in `graph_view.rs` tests.**
  The test code constructs `OverlayGraphView::build(&snap, &buf, &schema)`. This should
  continue to work since `MockSnapshot` implements `SnapshotReader`. Just verify the
  tests compile and pass.

  **Verify:**
  ```bash
  cargo test -p phonograph_db -- graph_view
  ```

### ▸ Phase 1 Gate

- [ ] **Phase 1 gate:**
  ```bash
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  cargo check -p phonograph_db --no-default-features
  grep -n 'dyn SnapshotReader' crates/phonograph_db/src/db/graph_view.rs
  ```
  All pass. The grep for `dyn SnapshotReader` returns only test code (the
  `_assert_...object_safe` function) or nothing.

---

## Phase 2: Fix `include_subtypes` in `OverlayGraphView`

> **Implements:** CLAUDE.md principle A9

- [ ] **2.1 — Implement subtype resolution in `nodes_by_type`.**
  In `graph_view.rs`, replace the `nodes_by_type` method body:

  Before (both branches are identical, `include_subtypes` is ignored):
  ```rust
  fn nodes_by_type(&self, type_id: TypeId, include_subtypes: bool) -> Vec<&Node> {
      if include_subtypes {
          self.nodes.values().filter(|n| n.type_labels.contains(&type_id)).collect()
      } else {
          self.nodes.values().filter(|n| n.type_labels.contains(&type_id)).collect()
      }
  }
  ```

  After:
  ```rust
  fn nodes_by_type(&self, type_id: TypeId, include_subtypes: bool) -> Vec<&Node> {
      use phonograph::schema::TypeRegistryView;

      let mut type_ids = alloc::vec![type_id];
      if include_subtypes {
          type_ids.extend(self.schema.all_subtypes(type_id));
      }
      self.nodes
          .values()
          .filter(|n| type_ids.iter().any(|t| n.type_labels.contains(t)))
          .collect()
  }
  ```

  **Verify:** `cargo check -p phonograph_db` — clean compile.

- [ ] **2.2 — Implement subtype resolution in `edges_by_type`.**
  Same pattern. Replace:
  ```rust
  fn edges_by_type(&self, type_id: TypeId, include_subtypes: bool) -> Vec<&Edge> {
      let _ = include_subtypes;
      self.edges.values().filter(|e| e.type_labels.contains(&type_id)).collect()
  }
  ```
  With:
  ```rust
  fn edges_by_type(&self, type_id: TypeId, include_subtypes: bool) -> Vec<&Edge> {
      use phonograph::schema::TypeRegistryView;

      let mut type_ids = alloc::vec![type_id];
      if include_subtypes {
          type_ids.extend(self.schema.all_subtypes(type_id));
      }
      self.edges
          .values()
          .filter(|e| type_ids.iter().any(|t| e.type_labels.contains(t)))
          .collect()
  }
  ```

  **Verify:** `cargo check -p phonograph_db` — clean compile.

- [ ] **2.3 — Add unit test for subtype resolution in `OverlayGraphView`.**
  In the `#[cfg(test)] mod tests` block of `graph_view.rs`, add a test that:
  1. Creates a `SchemaCache` with a parent type `Animal` and a child type `Dog`
     where `Dog`'s `supertypes` contains `Animal`'s `TypeId`.
  2. Inserts a node typed as `Dog` into the `MockSnapshot`.
  3. Calls `view.nodes_by_type(animal_type_id, true)` and asserts the `Dog` node
     is returned.
  4. Calls `view.nodes_by_type(animal_type_id, false)` and asserts the `Dog` node
     is NOT returned.

  Use the existing test helpers (`make_node`, `MockSnapshot`, etc.) as a model.
  The `SchemaCache` needs to have types registered via `register_type()`.

  **Verify:**
  ```bash
  cargo test -p phonograph_db -- graph_view::tests::nodes_by_type_with_subtypes
  ```

- [ ] **2.4 — Add unit test for `edges_by_type` subtype resolution.**
  Same pattern as 2.3 but for edges.

  **Verify:**
  ```bash
  cargo test -p phonograph_db -- graph_view::tests::edges_by_type_with_subtypes
  ```

### ▸ Phase 2 Gate

- [ ] **Phase 2 gate:**
  ```bash
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  ```
  All pass. Subtype tests pass.

---

## Phase 3: Changeset-Scoped Preloading for `OverlayGraphView`

> **Implements:** CLAUDE.md residual concern #7 (performance)

Currently, `OverlayGraphView::build()` calls `base.all_nodes()` and `base.all_edges()`,
loading the **entire** database into memory on every commit and every inference run. For
a small changeset on a million-node graph, this is wasteful.

The fix: load only base nodes/edges whose types overlap with the changeset's affected
types, plus adjacency neighbors of changed nodes. The overlay is still eagerly owned
(no `RefCell`, no trait signature changes), but scoped to the "neighborhood" of the
changeset.

**Design:**
- `SnapshotReader` gains targeted query methods: `nodes_by_type_ids`, `edges_by_type_ids`.
- `OverlayGraphView::build()` accepts an optional `&[TypeId]` hint for affected types.
- When the hint is provided, only base entities with matching types are loaded.
- When the hint is `None` (e.g. `validate_all()`), all entities are loaded (current behavior).
- Call sites pass `changeset.affected_types()` or the inference rule's `applies_to_types()`.

**Trade-off:** If a validator calls `nodes_by_property` for a property on a type NOT in
the affected set, the result may be incomplete. In practice, validators inspect the
changeset's neighborhood, and the `applies_to_types()` hint already scopes dispatching.
This is documented as a known limitation.

- [ ] **3.1 — Expand the `SnapshotReader` trait with targeted query methods.**
  In `crates/phonograph_db/src/db/graph_view.rs`, add to the `SnapshotReader` trait:

  ```rust
  /// Returns all nodes whose type labels overlap with any of the given type IDs.
  /// Used for changeset-scoped preloading.
  fn nodes_by_type_ids(&self, type_ids: &[TypeId]) -> Vec<Node>;

  /// Returns all edges whose type labels overlap with any of the given type IDs.
  /// Used for changeset-scoped preloading.
  fn edges_by_type_ids(&self, type_ids: &[TypeId]) -> Vec<Edge>;
  ```

  Provide default impls that fall back to `all_nodes()` / `all_edges()` with a filter,
  so existing implementors don't break:
  ```rust
  fn nodes_by_type_ids(&self, type_ids: &[TypeId]) -> Vec<Node> {
      self.all_nodes()
          .into_iter()
          .filter(|n| n.type_labels.iter().any(|t| type_ids.contains(t)))
          .collect()
  }
  fn edges_by_type_ids(&self, type_ids: &[TypeId]) -> Vec<Edge> {
      self.all_edges()
          .into_iter()
          .filter(|e| e.type_labels.iter().any(|t| type_ids.contains(t)))
          .collect()
  }
  ```

  **Verify:** `cargo check -p phonograph_db` — clean compile.

- [ ] **3.2 — Override `nodes_by_type_ids` / `edges_by_type_ids` on `BaseSnapshotReader`.**
  In `write_txn.rs`, `BaseSnapshotReader` implements `SnapshotReader`. Override the new
  methods to use the storage engine's type index scan instead of loading everything:

  ```rust
  fn nodes_by_type_ids(&self, type_ids: &[TypeId]) -> Vec<Node> {
      let rtx = self.txn.as_base_read_txn();
      let mut result = Vec::new();
      for tid in type_ids {
          let start = serialization::encode_type_index_key(0x00, *tid, 0);
          let end = serialization::encode_type_index_key(0x00, *tid, u64::MAX);
          if let Ok(entries) = rtx.storage_range_scan(
              self.txn.snapshot.roots.type_index, &start, Some(&end),
          ) {
              for (key, _) in &entries {
                  let (_, _, entity_id) = serialization::decode_type_index_key(key);
                  if let Ok(Some(node)) = self.txn.read_base_node(NodeId(entity_id)) {
                      result.push(node);
                  }
              }
          }
      }
      result
  }
  ```

  Same pattern for `edges_by_type_ids` using prefix `0x01` and `read_base_edge`.
  Deduplicate by ID if a node/edge has multiple matching type labels.

  **Verify:** `cargo check -p phonograph_db` — clean compile.

- [ ] **3.3 — Add `affected_types` parameter to `OverlayGraphView::build()`.**
  Change the `build()` signature:

  Before:
  ```rust
  pub fn build(
      base: &impl SnapshotReader,
      buffer: &WriteBuffer,
      schema: &'s SchemaCache,
  ) -> Self {
  ```

  After:
  ```rust
  pub fn build(
      base: &impl SnapshotReader,
      buffer: &WriteBuffer,
      schema: &'s SchemaCache,
      affected_types: Option<&[TypeId]>,
  ) -> Self {
  ```

  In the body, replace:
  ```rust
  for node in base.all_nodes() {
      nodes.insert(node.id, node);
  }
  ```
  With:
  ```rust
  let base_nodes = match affected_types {
      Some(type_ids) => base.nodes_by_type_ids(type_ids),
      None => base.all_nodes(),
  };
  for node in base_nodes {
      nodes.insert(node.id, node);
  }
  ```

  Same for edges.

  Additionally, when `affected_types` is `Some`, also load adjacency neighbors of
  changed nodes. After loading type-scoped base nodes, iterate over the buffer's
  inserted/updated/deleted node IDs and call `base.outgoing_edges()` /
  `base.incoming_edges()` to pull in their neighbors and the edges themselves. This
  ensures validators can traverse adjacency from changed nodes.

  **Verify:** `cargo check -p phonograph_db` — expect errors at call sites.

- [ ] **3.4 — Update call sites in `write_txn.rs`.**
  There are three call sites for `OverlayGraphView::build()`:

  1. `run_validators()` — pass `Some(&affected_types)` where `affected_types` comes
     from `changeset.affected_types()` (already computed in that method).
  2. `run_inference()` → the overlay is built for the inference rule. Pass the rule's
     `applies_to_types()` if available, otherwise `None` (load everything).
  3. `validate_all()` — pass `None` (full database scan intended).

  For `run_validators`, the affected types are already computed:
  ```rust
  let affected_types = changeset.affected_types();
  let graph_view = OverlayGraphView::build(
      &self.base_reader(),
      &self.buffer,
      &self.schema_cache,
      Some(&affected_types),
  );
  ```

  **Verify:** `cargo check -p phonograph_db` — clean compile.

- [ ] **3.5 — Update test call sites in `graph_view.rs`.**
  All test calls to `OverlayGraphView::build(&snap, &buf, &schema)` need the fourth
  argument. For tests, pass `None` to preserve existing behavior:
  ```rust
  let view = OverlayGraphView::build(&snap, &buf, &schema, None);
  ```

  **Verify:**
  ```bash
  cargo test -p phonograph_db -- graph_view
  ```

- [ ] **3.6 — Add test: changeset-scoped load excludes unrelated types.**
  In `graph_view.rs` tests, add a test that:
  1. Creates a `MockSnapshot` with nodes of type A and type B.
  2. Builds an overlay with `affected_types = Some(&[type_a_id])`.
  3. Asserts that `view.get_node(type_b_node_id)` returns `None`.
  4. Asserts that `view.get_node(type_a_node_id)` returns `Some`.

  Override `nodes_by_type_ids` on `MockSnapshot` to filter properly (or rely on the
  default impl which filters `all_nodes()`).

  **Verify:**
  ```bash
  cargo test -p phonograph_db -- graph_view::tests::scoped_preload
  ```

- [ ] **3.7 — Add test: adjacency neighbors are included in scoped load.**
  Test that when a changed node (in the buffer) has outgoing edges in the base snapshot
  to a node of a different type, that neighbor node IS still loaded into the overlay.

  **Verify:**
  ```bash
  cargo test -p phonograph_db -- graph_view::tests::scoped_preload_adjacency
  ```

### ▸ Phase 3 Gate

- [ ] **Phase 3 gate:**
  ```bash
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  ```
  All pass.

---

## Phase 4: Fix `total_eq` Consistency

> **Implements:** CLAUDE.md principle A8

- [ ] **4.1 — Fix `nodes_by_property` in `OverlayGraphView`.**
  In `graph_view.rs`, change the `nodes_by_property` method:

  Before:
  ```rust
  fn nodes_by_property(&self, key: PropertyKeyId, value: &Value) -> Vec<&Node> {
      self.nodes.values()
          .filter(|n| n.properties.get(&key) == Some(value))
          .collect()
  }
  ```

  After:
  ```rust
  fn nodes_by_property(&self, key: PropertyKeyId, value: &Value) -> Vec<&Node> {
      self.nodes.values()
          .filter(|n| n.properties.get(&key).map_or(false, |v| v.total_eq(value)))
          .collect()
  }
  ```

  **Verify:** `cargo check -p phonograph_db` — clean compile.

- [ ] **4.2 — Fix `nodes_by_property` in `WriteTransaction`.**
  In `write_txn.rs`, find the `nodes_by_property` method:
  ```rust
  pub fn nodes_by_property(
      &self,
      key: PropertyKeyId,
      value: &Value,
  ) -> Result<Vec<Node>, Error> {
      let all = self.all_nodes()?;
      Ok(all
          .into_iter()
          .filter(|n| n.properties.get(&key) == Some(value))
          .collect())
  }
  ```

  Replace the filter with:
  ```rust
      .filter(|n| n.properties.get(&key).map_or(false, |v| v.total_eq(value)))
  ```

  **Verify:** `cargo check -p phonograph_db` — clean compile.

- [ ] **4.3 — Add unit test for NaN property lookup in `OverlayGraphView`.**
  In `graph_view.rs` tests, add a test that:
  1. Inserts a node with a property set to `Value::F64(f64::NAN)`.
  2. Calls `view.nodes_by_property(key, &Value::F64(f64::NAN))`.
  3. Asserts the node IS found (because `total_eq` treats NaN == NaN).

  **Verify:**
  ```bash
  cargo test -p phonograph_db -- graph_view::tests::nodes_by_property_nan
  ```

- [ ] **4.4 — Verify no remaining `PartialEq` property lookups in engine code.**
  ```bash
  grep -rn 'properties.get.*== Some' crates/phonograph_db/src/db/
  ```

  **Verify:** Grep returns empty (or only test code).

### ▸ Phase 4 Gate

- [ ] **Phase 4 gate:**
  ```bash
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  ```
  All pass.

---

## Phase 5: Counter Deserialization Bounds Check

> **Implements:** CLAUDE.md principle A5

- [ ] **5.1 — Add bounds checking to `load_schema` counter deserialization.**
  In `crates/phonograph_db/src/db/database.rs`, find the counter loading code in
  `load_schema()`:

  ```rust
  0x03 => cache.next_type_id = counter_val as u32,
  0x04 => cache.next_property_key_id = counter_val as u32,
  ```

  Replace with:
  ```rust
  0x03 => {
      cache.next_type_id = u32::try_from(counter_val).map_err(|_| {
          StorageError {
              message: alloc::format!(
                  "schema: next_type_id counter {counter_val} exceeds u32::MAX"
              ),
              #[cfg(feature = "std")]
              source: None,
          }
      })?;
  }
  0x04 => {
      cache.next_property_key_id = u32::try_from(counter_val).map_err(|_| {
          StorageError {
              message: alloc::format!(
                  "schema: next_property_key_id counter {counter_val} exceeds u32::MAX"
              ),
              #[cfg(feature = "std")]
              source: None,
          }
      })?;
  }
  ```

  **Verify:** `cargo check -p phonograph_db` — clean compile.

- [ ] **5.2 — Add a unit test for counter overflow detection.**
  In `database.rs` tests, add a test that:
  1. Creates a fresh in-memory database.
  2. Manually writes a schema counter entry with value `u64::MAX` for counter
     type 0x03 (next_type_id).
  3. Attempts to reopen the database.
  4. Asserts the open fails with a `StorageError` mentioning "exceeds u32::MAX".

  If direct B-tree manipulation is too complex, an alternative approach: create a
  helper on `SchemaCache` that validates counter bounds, and test that helper directly.

  **Verify:**
  ```bash
  cargo test -p phonograph_db -- counter_overflow
  ```
  (or whatever the test is named)

### ▸ Phase 5 Gate

- [ ] **Phase 5 gate:**
  ```bash
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  ```
  All pass.

---

## Phase 6: Inference Cache Allocation Reduction

> **Implements:** CLAUDE.md principle A7 (efficiency)

- [ ] **6.1 — Restructure `InferenceCache` to avoid allocation on lookup.**
  In `crates/phonograph_db/src/db/inference_engine.rs`, change the cache from:
  ```rust
  entries: BTreeMap<(String, u64), CacheEntry>,
  ```
  To a two-level map:
  ```rust
  entries: BTreeMap<String, BTreeMap<u64, CacheEntry>>,
  ```

  Update `get()`:
  ```rust
  pub(crate) fn get(&mut self, rule_name: &str, generation: u64) -> Option<InferenceResult> {
      if self.max_entries == 0 {
          return None;
      }
      if let Some(by_gen) = self.entries.get_mut(rule_name) {
          if let Some(entry) = by_gen.get_mut(&generation) {
              self.access_counter += 1;
              entry.last_accessed = self.access_counter;
              return Some(entry.result.clone());
          }
      }
      None
  }
  ```

  Update `insert()`: insert into the two-level structure, counting total entries
  across all rule names for eviction.

  Update `evict_lru()`: scan all inner maps for the minimum `last_accessed`.

  Update `clear()` accordingly.

  **Verify:** `cargo check -p phonograph_db` — clean compile.

- [ ] **6.2 — Verify all existing inference cache tests pass.**
  ```bash
  cargo test -p phonograph_db -- inference_engine::tests::cache
  ```

  **Verify:** All cache tests pass.

- [ ] **6.3 — Verify full inference integration tests pass.**
  ```bash
  cargo test -p phonograph_std -- inference
  ```

  **Verify:** All inference tests pass.

### ▸ Phase 6 Gate

- [ ] **Phase 6 gate:**
  ```bash
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  ```
  All pass.

---

## Phase 7: Documentation & Metadata Updates

- [ ] **7.1 — Update `CHANGELOG.md`.**
  Add an entry under a new section:
  ```markdown
  ### Changed
  - `OverlayGraphView::build()` now uses static dispatch (`&impl SnapshotReader`)
    instead of dynamic dispatch (`&dyn SnapshotReader`).
  - `OverlayGraphView` now holds a schema cache reference and correctly resolves
    subtypes in `nodes_by_type` and `edges_by_type` when `include_subtypes` is true.
  - `nodes_by_property` in both `OverlayGraphView` and `WriteTransaction` now uses
    `Value::total_eq()` instead of `PartialEq`, fixing NaN property lookups.
  - `OverlayGraphView::build()` accepts an optional `affected_types` parameter for
    changeset-scoped preloading, avoiding full database scans on commit.
  - Inference result cache restructured to avoid String allocation on lookups.

  ### Fixed
  - `OverlayGraphView::nodes_by_type` and `edges_by_type` ignored the
    `include_subtypes` parameter, returning only exact type matches. Constraint
    validators and inference rules now see correct subtype-inclusive results.
  - `nodes_by_property` could not match properties with NaN values due to IEEE 754
    `PartialEq` semantics. All engine property lookups now use `total_eq`.
  - Schema counter deserialization silently truncated `u64` values to `u32`. Now
    returns an error if a persisted counter exceeds `u32::MAX`.
  ```

  **Verify:** CHANGELOG.md is well-formatted.

- [ ] **7.2 — Update doc comments on modified methods.**
  Verify that `OverlayGraphView::build`, `nodes_by_type`, `edges_by_type`, and
  `nodes_by_property` have accurate doc comments reflecting their new behavior.
  Add a note to `nodes_by_property` mentioning `total_eq` semantics.

  **Verify:** `cargo doc --workspace --no-deps` — zero warnings.

### ▸ Phase 7 Gate

- [ ] **Phase 7 gate:**
  ```bash
  cargo doc --workspace --no-deps
  ```
  Zero warnings.

---

## Phase 8: Final Verification

- [ ] **8.1 — Full workspace build, test, lint, docs.**
  ```bash
  cargo build --workspace
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  cargo doc --workspace --no-deps
  ```
  All pass with zero warnings.

- [ ] **8.2 — `no_std` verification.**
  ```bash
  cargo check -p phonograph --no-default-features
  cargo check -p phonograph_db --no-default-features
  ```

- [ ] **8.3 — Regression test count.**
  Compare against Phase 0 baseline. All previously passing tests still pass.
  New tests appear in the count.

- [ ] **8.4 — Run all 17 verification checks from `CLAUDE.md`.**
  Execute every command from the Verification Checklist table. All 17 must pass.

  **Verify:** All 17 pass.

- [ ] **8.5 — Examples still run.**
  ```bash
  cargo run -p phonograph_std --example basic_usage
  cargo run -p phonograph_std --example owl_lite_ontology
  ```

- [ ] **8.6 — Targeted grep assertions.**
  ```bash
  # A7: no dyn SnapshotReader in production code
  grep -rn 'dyn SnapshotReader' crates/phonograph_db/src/db/graph_view.rs | grep -v '#\[cfg(test)\]' | grep -v 'mod tests' | grep -v '_assert'

  # A8: no PartialEq property lookups in engine code
  grep -rn 'properties.get.*== Some' crates/phonograph_db/src/db/

  # A9: schema field exists on OverlayGraphView
  grep 'schema:' crates/phonograph_db/src/db/graph_view.rs

  # Counter bounds checking
  grep -c 'try_from\|exceeds u32' crates/phonograph_db/src/db/database.rs
  ```
  First grep: empty (or only object-safety assertions in test code).
  Second grep: empty.
  Third grep: shows `schema: &'s SchemaCache` (or similar).
  Fourth grep: nonzero count.

### ▸ Phase 8 Gate — COMPLETE

- [ ] **All verification checks pass.**
  Write a completion report to `completion-report.md` at the project root, documenting:
  - Status
  - What was changed (summary of each phase)
  - New architectural principles installed (A7, A8, A9)
  - Review findings addressed (numbered list, referencing the original finding numbers)
  - Test count before and after
  - Files modified
  - Residual concerns (if any)
