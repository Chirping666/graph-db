# Checklist: Task 25 — Implement Query & Traversal Engine

**Parent:** Task 17 (this checklist)  
**Implements:** All code in `src/db/`, the `GraphReader` trait impl, and integration of the storage engine with the public API.

Execute items in order. After each item, run the verification command(s) listed. Do not proceed until verification passes.

---

## Phase 0: Module Scaffolding

### 0.1 — Create `src/db/mod.rs` with submodule declarations

Create `src/db/mod.rs` with:
- `#[cfg(feature = "std")]` gating on the entire module (the parent `lib.rs` declaration should already gate this)
- `pub mod config;`
- `pub mod database;`
- `pub mod read_txn;`
- `pub mod write_txn;`
- `pub mod write_buffer;`
- `pub mod schema_cache;`
- `pub mod graph_view;`
- `pub mod builders;`
- Module-level `//!` doc comment explaining the `db` module's purpose

Create empty placeholder files for each submodule (each with a `//!` doc comment).

**Verify:** `cargo check` succeeds.

### 0.2 — Add `pub mod db;` to `lib.rs`

In `src/lib.rs`, add (gated behind `std`):
```rust
#[cfg(feature = "std")]
pub mod db;
```

Add `std`-gated re-exports at the crate root:
```rust
#[cfg(feature = "std")]
pub use db::{
    database::Database,
    config::{DatabaseConfig, StorageMode},
    read_txn::ReadTransaction,
    write_txn::WriteTransaction,
    builders::{NodeBuilder, EdgeBuilder, TypeDefinitionBuilder},
};
```

**⚠ Pitfall:** The exact re-export list may need adjustment as types are created. Update as you go, but ensure every public type in `db/` is re-exported from the crate root by Phase 10.

**Verify:**
- `cargo check` succeeds.
- `cargo check --no-default-features --features alloc` succeeds (db module not compiled).

---

## Phase 1: Configuration (`src/db/config.rs`)

### 1.1 — Implement StorageMode enum

```rust
/// Determines whether the database uses persistent file storage
/// or in-memory storage.
pub enum StorageMode {
    /// Persistent storage backed by a file at the given path.
    Persistent { path: std::path::PathBuf },
    /// In-memory storage. Data is lost when the database is dropped
    /// unless explicitly snapshot to disk.
    InMemory,
}
```

Derives: `Clone, Debug`

**Verify:** `cargo check`

### 1.2 — Implement DatabaseConfig struct and builder

```rust
/// Configuration for opening a database.
pub struct DatabaseConfig {
    pub mode: StorageMode,
    pub buffer_pool_frames: usize,
    pub page_size: usize,
    pub extension_startup_check: bool,
    pub inference_cache_size: usize,
}
```

Add a builder pattern via `impl DatabaseConfig`:
- `pub fn persistent(path: impl Into<PathBuf>) -> Self` — creates config with persistent mode and sensible defaults
- `pub fn in_memory() -> Self` — creates config with in-memory mode and sensible defaults
- `pub fn buffer_pool_frames(mut self, frames: usize) -> Self`
- `pub fn page_size(mut self, size: usize) -> Self`
- `pub fn extension_startup_check(mut self, check: bool) -> Self`
- `pub fn inference_cache_size(mut self, size: usize) -> Self`

Defaults (from `012-design-document.md` §15.1):
- `buffer_pool_frames`: 1024 (min: 64)
- `page_size`: 4096 (must be power of two)
- `extension_startup_check`: true
- `inference_cache_size`: 64

Add validation in the builder setters: `page_size` must be a power of two, `buffer_pool_frames` minimum 64. If invalid, clamp or document panic behavior.

**Verify:** `cargo check`

### 1.3 — Unit tests for config

Test:
- `DatabaseConfig::persistent("/tmp/test.db")` produces a valid config with defaults.
- `DatabaseConfig::in_memory()` produces a valid config.
- Builder chaining works: `DatabaseConfig::in_memory().buffer_pool_frames(256).page_size(8192)`.
- Default values match the spec.

**Verify:** `cargo test -- config` passes.

---

## Phase 2: Schema Cache (`src/db/schema_cache.rs`)

### 2.1 — Implement SchemaCache struct

The `SchemaCache` holds the in-memory copy of all type definitions, property key definitions, and precomputed type hierarchy data. It is loaded from the Schema Store B-tree at database open and updated during write transactions.

```rust
pub(crate) struct SchemaCache {
    /// All registered type definitions, keyed by TypeId.
    types: BTreeMap<TypeId, TypeDefinition>,
    /// Type name → TypeId index for duplicate detection.
    type_names: HashMap<(String, TypeKind), TypeId>,
    /// All property key definitions, keyed by PropertyKeyId.
    property_keys: BTreeMap<PropertyKeyId, PropertyKeyDefinition>,
    /// Property key name → PropertyKeyId index.
    property_key_names: HashMap<String, PropertyKeyId>,
    /// Precomputed: TypeId → set of all subtypes (recursive).
    subtypes_cache: HashMap<TypeId, Vec<TypeId>>,
    /// Next ID counters.
    next_node_id: u64,
    next_edge_id: u64,
    next_type_id: u64,
    next_property_key_id: u64,
}
```

Where `PropertyKeyDefinition` is a simple struct:
```rust
pub(crate) struct PropertyKeyDefinition {
    pub id: PropertyKeyId,
    pub name: String,
}
```

Implement methods:
- `pub fn new() -> Self` — empty cache
- `pub fn register_type(&mut self, def: TypeDefinition) -> Result<TypeId, SchemaError>` — validates uniqueness, assigns ID, updates subtypes cache
- `pub fn get_type(&self, id: TypeId) -> Option<&TypeDefinition>`
- `pub fn get_type_by_name(&self, name: &str, kind: TypeKind) -> Option<&TypeDefinition>`
- `pub fn all_types(&self) -> Vec<&TypeDefinition>`
- `pub fn subtypes_of(&self, id: TypeId) -> &[TypeId]` — from precomputed cache
- `pub fn all_supertypes(&self, id: TypeId) -> Vec<TypeId>` — transitive closure up
- `pub fn register_property_key(&mut self, name: &str) -> Result<PropertyKeyId, SchemaError>` — assigns ID, deduplicates
- `pub fn get_or_create_property_key(&mut self, name: &str) -> PropertyKeyId` — idempotent
- `pub fn get_property_key(&self, id: PropertyKeyId) -> Option<&PropertyKeyDefinition>`
- `pub fn get_property_key_by_name(&self, name: &str) -> Option<PropertyKeyId>`
- `pub fn all_property_keys(&self) -> Vec<&PropertyKeyDefinition>`
- `pub fn allocate_node_id(&mut self) -> NodeId`
- `pub fn allocate_edge_id(&mut self) -> EdgeId`

**⚠ Pitfall — Subtypes cache invalidation:** When a new type is registered with supertypes, the `subtypes_cache` for every ancestor must be updated. Implement `rebuild_subtypes_cache()` as a full recomputation (simple and correct; the type registry is small).

**⚠ Pitfall — Cycle detection:** `register_type` must detect cycles in the type hierarchy. If type A lists B as a supertype and B lists A, reject with `SchemaError::CycleDetected`. Use a simple DFS or BFS from the new type upward through supertypes.

**Verify:** `cargo check`

### 2.2 — Implement TypeRegistryView and PropertyKeyRegistryView for SchemaCache

Implement the `TypeRegistryView` trait (from `src/schema/`) for `SchemaCache`:
```rust
impl TypeRegistryView for SchemaCache {
    fn get_type(&self, id: TypeId) -> Option<&TypeDefinition> { ... }
    fn all_types(&self) -> Vec<&TypeDefinition> { ... }
    fn subtypes_of(&self, id: TypeId) -> Vec<TypeId> { ... }
    fn all_supertypes(&self, id: TypeId) -> Vec<TypeId> { ... }
}
```

Implement the `PropertyKeyRegistryView` trait for `SchemaCache`:
```rust
impl PropertyKeyRegistryView for SchemaCache {
    fn get_property_key_name(&self, id: PropertyKeyId) -> Option<&str> { ... }
    fn get_property_key_id(&self, name: &str) -> Option<PropertyKeyId> { ... }
    fn all_property_keys(&self) -> Vec<(PropertyKeyId, &str)> { ... }
}
```

**⚠ Pitfall:** Check the exact method signatures in the trait definitions from `src/schema/mod.rs` (Task 22 output). Adapt the implementations to match the actual trait API, which may differ slightly from the design doc.

**Verify:** `cargo check`

### 2.3 — Unit tests for SchemaCache

Test:
- Register a type, retrieve it by ID and by name.
- Register a type with supertypes, verify `subtypes_of` on the parent returns the child.
- Register a chain: A → B → C (C is supertype of B, B is supertype of A). Verify `all_supertypes(A)` returns [B, C].
- Attempt to register a duplicate type name+kind, verify `SchemaError::DuplicateTypeName`.
- Cycle detection: register A with supertype B, then try to register B with supertype A. Verify `SchemaError::CycleDetected`.
- Property key registration and lookup by name.
- `get_or_create_property_key` returns same ID on repeated calls.
- ID allocation returns monotonically increasing values.

**Verify:** `cargo test -- schema_cache` passes.

---

## Phase 3: Write Buffer (`src/db/write_buffer.rs`)

### 3.1 — Implement WriteBuffer struct

The `WriteBuffer` tracks all pending mutations within a write transaction. It serves two purposes: (1) enabling read-your-own-writes by overlaying pending changes on the base snapshot, and (2) producing the `ChangeSet` at commit time.

```rust
pub(crate) struct WriteBuffer {
    /// Pending node inserts: NodeId → Node
    node_inserts: BTreeMap<NodeId, Node>,
    /// Pending node updates: NodeId → (before, after)
    node_updates: BTreeMap<NodeId, (Node, Node)>,
    /// Pending node deletes: NodeId → deleted Node
    node_deletes: BTreeMap<NodeId, Node>,
    /// Same for edges
    edge_inserts: BTreeMap<EdgeId, Edge>,
    edge_updates: BTreeMap<EdgeId, (Edge, Edge)>,
    edge_deletes: BTreeMap<EdgeId, Edge>,
    /// Schema changes (type registrations, property key registrations)
    schema_changes: Vec<SchemaChange>,
}
```

Where `SchemaChange` is an internal enum:
```rust
pub(crate) enum SchemaChange {
    TypeRegistered(TypeDefinition),
    PropertyKeyRegistered(PropertyKeyDefinition),
    ExtensionNameRegistered { kind: &'static str, name: String },
}
```

Implement methods:
- `pub fn new() -> Self`
- `pub fn insert_node(&mut self, node: Node)` — adds to `node_inserts`
- `pub fn update_node(&mut self, before: Node, after: Node)` — handles insert-then-update (collapses to single insert with updated data) and update-then-update (updates the "after" in place)
- `pub fn delete_node(&mut self, node: Node)` — handles insert-then-delete (removes from inserts, no ChangeSet entry), update-then-delete (removes from updates, adds delete with original "before")
- Same for edge operations
- `pub fn is_node_inserted(&self, id: NodeId) -> bool`
- `pub fn is_node_deleted(&self, id: NodeId) -> bool`
- `pub fn get_pending_node(&self, id: NodeId) -> Option<&Node>` — returns the latest version of a pending node (from inserts or updates)
- Same for edges
- `pub fn inserted_edge_ids_for_source(&self, source: NodeId) -> Vec<EdgeId>` — for overlay queries
- `pub fn deleted_edge_ids(&self) -> &BTreeMap<EdgeId, Edge>`
- `pub fn record_schema_change(&mut self, change: SchemaChange)`

**⚠ Pitfall — Mutation collapsing:** The ChangeSet logic must correctly handle these sequences:
- Insert → Update = Insert(final_version)
- Insert → Delete = nothing (removed entirely)
- Update → Update = Update(original_before, latest_after)
- Update → Delete = Delete(original_before)
- Delete → Insert = not meaningful (IDs are not reused within a transaction)

Implement collapsing in the `update_*` and `delete_*` methods, not at ChangeSet build time.

**Verify:** `cargo check`

### 3.2 — Implement ChangeSet production

Add a method that builds the `ChangeSet` from the WriteBuffer's current state:

```rust
impl WriteBuffer {
    pub fn build_changeset(&self) -> (Vec<NodeChange>, Vec<EdgeChange>) {
        let mut node_changes = Vec::new();
        for (_, node) in &self.node_inserts {
            node_changes.push(NodeChange::Inserted(node.clone()));
        }
        for (_, (before, after)) in &self.node_updates {
            node_changes.push(NodeChange::Modified {
                before: before.clone(),
                after: after.clone(),
            });
        }
        for (_, node) in &self.node_deletes {
            node_changes.push(NodeChange::Deleted(node.clone()));
        }
        // Same for edges
        let mut edge_changes = Vec::new();
        // ... (same pattern)
        (node_changes, edge_changes)
    }
}
```

**⚠ Pitfall:** The `ChangeSet` struct from `src/constraint/` uses borrowed slices (`&'a [NodeChange]`). The WriteBuffer produces owned Vecs. The caller (commit path) must hold the Vecs and borrow them into the ChangeSet. Plan the lifetimes accordingly.

**Verify:** `cargo check`

### 3.3 — Unit tests for WriteBuffer

Test:
- Insert a node, verify `get_pending_node` returns it.
- Insert then update: `build_changeset` produces one `Inserted` with the updated data.
- Insert then delete: `build_changeset` produces nothing for that node.
- Update (with before/after): `build_changeset` produces one `Modified`.
- Update then update: ChangeSet has one `Modified` with original before and latest after.
- Update then delete: ChangeSet has one `Deleted` with original before.
- Edge insert tracking: `inserted_edge_ids_for_source` returns correct IDs.
- `is_node_deleted` returns correct values.
- Schema change recording.

**Verify:** `cargo test -- write_buffer` passes.

---

## Phase 4: GraphView Overlay (`src/db/graph_view.rs`)

### 4.1 — Implement OverlayGraphView

This struct provides the `GraphView` trait by overlaying WriteBuffer changes on top of a base snapshot reader. It is used by constraint validators and (later) inference rules to see the "as if committed" state.

```rust
pub(crate) struct OverlayGraphView<'a> {
    /// Base snapshot reader (reads from B-trees via storage engine).
    base: &'a dyn SnapshotReader,
    /// Pending changes from the write transaction.
    buffer: &'a WriteBuffer,
    /// Schema cache for type hierarchy queries.
    schema: &'a SchemaCache,
}
```

Where `SnapshotReader` is an internal trait (or the storage engine's snapshot read interface) that provides raw B-tree reads.

Implement `GraphView` for `OverlayGraphView`:
- `get_node(id)`: Check buffer for insert/update first, then base. Return `None` if deleted.
- `get_edge(id)`: Same pattern.
- `outgoing_edges(node, type?)`: Merge base snapshot edges with buffer inserts, exclude buffer deletes, apply buffer updates. Filter by type if specified.
- `incoming_edges(node, type?)`: Same pattern.
- `nodes_by_type(type_id, include_subtypes)`: Scan base + buffer inserts, exclude buffer deletes, apply buffer updates. If `include_subtypes`, resolve subtypes from schema cache and union results.
- `edges_by_type(type_id, include_subtypes)`: Same pattern.
- `nodes_by_property(key, value)`: Full scan of base + buffer, with overlay logic.

**⚠ Pitfall — Edge merging complexity:** `outgoing_edges` must:
1. Fetch edges from the base snapshot for the given node (and optional type filter).
2. Exclude any edge whose ID is in `buffer.edge_deletes`.
3. Replace any edge whose ID is in `buffer.edge_updates` with the updated version.
4. Add any edge from `buffer.edge_inserts` where `source == node` (and type matches if filtered).
5. Return the merged result.

This is O(base_degree + buffer_size) per call, which is acceptable.

**⚠ Pitfall — Type label changes.** If a node's type labels are modified in the buffer (via `add_node_type` / `remove_node_type`), then `nodes_by_type` must use the *updated* type labels from the buffer, not the base snapshot's labels. The overlay logic in `get_node` already handles this by returning the buffered version.

**Verify:** `cargo check`

### 4.2 — Unit tests for OverlayGraphView

Use a mock `SnapshotReader` (an in-memory map of nodes and edges) to test the overlay logic:

- Base has node A, buffer has no changes → `get_node(A)` returns A.
- Base has node A, buffer deletes A → `get_node(A)` returns `None`.
- Base has no node B, buffer inserts B → `get_node(B)` returns B.
- Base has node A with properties, buffer updates A → `get_node(A)` returns updated version.
- Outgoing edges: base has edges E1, E2 from node A. Buffer deletes E1, inserts E3 from A. Result: [E2, E3].
- `nodes_by_type` correctly includes buffer-inserted nodes and excludes buffer-deleted nodes.
- `nodes_by_property` finds a value that exists only in a buffer-inserted node.

**Verify:** `cargo test -- graph_view` passes.

---

## Phase 5: Builders (`src/db/builders.rs`)

### 5.1 — Implement NodeBuilder

```rust
pub struct NodeBuilder {
    type_labels: Vec<TypeId>,
    properties: PropertyMap,
    is_anonymous: bool,
}
```

Methods:
- `pub fn new() -> Self` — empty builder
- `pub fn type_label(mut self, type_id: TypeId) -> Self` — adds a type label
- `pub fn type_labels(mut self, ids: impl IntoIterator<Item = TypeId>) -> Self`
- `pub fn property(mut self, key: PropertyKeyId, value: Value) -> Self`
- `pub fn anonymous(mut self) -> Self` — sets `is_anonymous = true`
- `pub fn build(self) -> Node` — produces a `Node` with `id: NodeId(0)` (placeholder; the database assigns the real ID)

The `build()` method must sort `type_labels` (per the sorted invariant from `006-schema-extension-spec.md`).

**Verify:** `cargo check`

### 5.2 — Implement EdgeBuilder

```rust
pub struct EdgeBuilder {
    type_labels: Vec<TypeId>,
    source: NodeId,
    target: NodeId,
    properties: PropertyMap,
}
```

Methods:
- `pub fn new(source: NodeId, target: NodeId) -> Self`
- `pub fn type_label(mut self, type_id: TypeId) -> Self`
- `pub fn type_labels(mut self, ids: impl IntoIterator<Item = TypeId>) -> Self`
- `pub fn property(mut self, key: PropertyKeyId, value: Value) -> Self`
- `pub fn build(self) -> Edge` — produces an `Edge` with `id: EdgeId(0)` (placeholder)

Sort `type_labels` in `build()`.

**Verify:** `cargo check`

### 5.3 — Implement TypeDefinitionBuilder

```rust
pub struct TypeDefinitionBuilder {
    name: String,
    kind: TypeKind,
    supertypes: Vec<TypeId>,
    property_declarations: Vec<PropertyDeclaration>,
    open: bool,
    metadata: PropertyMap,
}
```

Methods:
- `pub fn node_type(name: impl Into<String>) -> Self` — sets kind to Node
- `pub fn edge_type(name: impl Into<String>) -> Self` — sets kind to Edge
- `pub fn supertype(mut self, id: TypeId) -> Self`
- `pub fn property_declaration(mut self, decl: PropertyDeclaration) -> Self`
- `pub fn open(mut self) -> Self` — sets `open = true`
- `pub fn closed(mut self) -> Self` — sets `open = false` (default)
- `pub fn metadata(mut self, key: PropertyKeyId, value: Value) -> Self`
- `pub fn build(self) -> TypeDefinition` — produces a `TypeDefinition` with `id: TypeId(0)` (placeholder)

**Verify:** `cargo check`

### 5.4 — Unit tests for builders

Test:
- `NodeBuilder::new().type_label(t1).property(k, v).build()` produces correct Node.
- `EdgeBuilder::new(src, tgt).type_label(t1).build()` produces correct Edge.
- `TypeDefinitionBuilder::node_type("Person").supertype(t1).build()` produces correct TypeDefinition.
- Type labels are sorted in the built Node/Edge.
- Placeholder IDs are `0` in built values.

**Verify:** `cargo test -- builders` passes.

---

## Phase 6: Database Struct and Lifecycle (`src/db/database.rs`)

### 6.1 — Implement Database struct

```rust
pub struct Database {
    inner: Arc<DatabaseInner>,
}

struct DatabaseInner {
    /// The storage engine providing B-tree operations.
    storage: StorageEngine,
    /// Write lock: only one write transaction at a time.
    write_mutex: Mutex<()>,
    /// Current snapshot (latest committed root pointers).
    current_snapshot: RwLock<Arc<Snapshot>>,
    /// Active snapshot reference counts (for MVCC page reclamation).
    active_snapshots: Mutex<Vec<(u64, Arc<Snapshot>)>>,
    /// In-memory schema cache.
    schema_cache: RwLock<SchemaCache>,
    /// Registered constraint validators.
    constraint_registry: RwLock<Vec<Box<dyn ConstraintValidator>>>,
    /// Registered inference rules (placeholder for Task 26).
    inference_registry: RwLock<Vec<Box<dyn InferenceRule>>>,
    /// Extension names persisted in the database.
    persisted_extension_names: RwLock<PersistedExtensionNames>,
    /// Configuration.
    config: DatabaseConfig,
}
```

Mark `Database` as `Send + Sync`:
```rust
// SAFETY: All shared state in DatabaseInner is protected by Mutex/RwLock.
unsafe impl Send for Database {}
unsafe impl Sync for Database {}
```

**⚠ Pitfall:** The actual `StorageEngine` type comes from `src/storage/`. Adapt the field types to match the actual API from Task 24. If `StorageEngine` is not `Send + Sync`, wrap it in a `Mutex`.

**Verify:** `cargo check`

### 6.2 — Implement Database::open()

```rust
impl Database {
    pub fn open(config: DatabaseConfig) -> Result<Self, Error> { ... }
}
```

Steps:
1. Match on `config.mode`:
   - `Persistent { path }`: Create or open the database file via `FileBackend`. Pass to `StorageEngine::open(backend, config.page_size, config.buffer_pool_frames)`.
   - `InMemory`: Defer to Task 27. For now, return `Error::Storage(...)` with a message indicating in-memory mode is not yet implemented.
2. Load the active superblock from the storage engine.
3. Load the schema cache: read all type definitions and property key definitions from the Schema Store B-tree into the `SchemaCache`.
4. Load persisted extension names from the Schema Store.
5. If `config.extension_startup_check` is true, log or store the missing extensions for later query.
6. Create the `DatabaseInner` and wrap in `Arc`.

**⚠ Pitfall — Schema loading:** The storage engine provides low-level B-tree range scans. You must iterate the Schema Store with the appropriate key prefixes (`0x01` for types, `0x02` for property keys, `0x03` for counters, `0x04` for hierarchy edges) and deserialize each entry. Consult `007-graph-storage-model.md` §9.2 and `012-design-document.md` §19.2 for the exact key map.

**Verify:** `cargo check`

### 6.3 — Implement extension registration methods

```rust
impl Database {
    pub fn register_constraint(&self, validator: Box<dyn ConstraintValidator>) -> Result<(), Error> {
        let mut registry = self.inner.constraint_registry.write().unwrap();
        // Replace if same name exists
        registry.retain(|v| v.name() != validator.name());
        let name = validator.name().to_string();
        registry.push(validator);
        // Mark name for persistence in next write transaction
        self.inner.persisted_extension_names.write().unwrap()
            .mark_constraint_registered(name);
        Ok(())
    }

    pub fn unregister_constraint(&self, name: &str) -> Result<bool, Error> { ... }
    pub fn register_inference_rule(&self, rule: Box<dyn InferenceRule>) -> Result<(), Error> { ... }
    pub fn unregister_inference_rule(&self, name: &str) -> Result<bool, Error> { ... }
    pub fn constraint_names(&self) -> Vec<String> { ... }
    pub fn inference_rule_names(&self) -> Vec<String> { ... }
    pub fn missing_extensions(&self) -> MissingExtensions { ... }
}
```

**Verify:** `cargo check`

### 6.4 — Implement Database::read_txn() and write_txn()

```rust
impl Database {
    pub fn read_txn(&self) -> Result<ReadTransaction<'_>, Error> {
        let snapshot = {
            let current = self.inner.current_snapshot.read().unwrap();
            Arc::clone(&current)
        };
        // Register this snapshot as active
        // ...
        Ok(ReadTransaction::new(&self.inner, snapshot))
    }

    pub fn write_txn(&self) -> Result<WriteTransaction<'_>, Error> {
        let guard = self.inner.write_mutex.lock().unwrap();
        let snapshot = {
            let current = self.inner.current_snapshot.read().unwrap();
            Arc::clone(&current)
        };
        Ok(WriteTransaction::new(&self.inner, snapshot, guard))
    }
}
```

**⚠ Pitfall — Lifetime management:** The `'_` lifetime on `ReadTransaction<'_>` and `WriteTransaction<'_>` borrows `&self` (the `Database`). This prevents the Database from being dropped while transactions are alive. The `MutexGuard` for the write lock must be stored inside `WriteTransaction` and dropped when the transaction commits/aborts.

**Verify:** `cargo check`

### 6.5 — Implement Database Drop

```rust
impl Drop for Database {
    fn drop(&mut self) {
        // Flush dirty pages (persistent mode)
        // Close file handle via storage engine
        // This is best-effort; errors are logged, not propagated
    }
}
```

**Verify:** `cargo check`

### 6.6 — Basic lifecycle test

Test:
- Create a database with `DatabaseConfig::persistent(temp_path)`.
- Verify it can be opened.
- Close (drop) and reopen — verify no errors.
- Register a constraint validator, verify `constraint_names()` returns it.
- Register an inference rule, verify `inference_rule_names()` returns it.

**Verify:** `cargo test -- database` passes.

---

## Phase 7: ReadTransaction (`src/db/read_txn.rs`)

### 7.1 — Implement ReadTransaction struct

```rust
pub struct ReadTransaction<'db> {
    inner: &'db DatabaseInner,
    snapshot: Arc<Snapshot>,
    // _not_send: PhantomData<*const ()>,  // !Send, !Sync
}
```

The `PhantomData<*const ()>` makes the transaction `!Send` and `!Sync` per design decision A12.

Implement `Drop` for `ReadTransaction`: decrement the snapshot reference count.

**Verify:** `cargo check`

### 7.2 — Implement node/edge lookups

```rust
impl<'db> ReadTransaction<'db> {
    pub fn get_node(&self, id: NodeId) -> Result<Option<Node>, Error> {
        // Use storage engine to do a point lookup in the Node Store B-tree
        // using self.snapshot's root pointers.
        // Deserialize the NodeRecord into a Node.
    }

    pub fn get_edge(&self, id: EdgeId) -> Result<Option<Edge>, Error> { ... }

    pub fn all_nodes(&self) -> Result<Vec<Node>, Error> {
        // Full range scan of the Node Store B-tree.
    }
}
```

**⚠ Pitfall — Record deserialization:** The storage engine returns raw `NodeRecord` / `EdgeRecord` bytes. This code must deserialize them into `Node` / `Edge` types from `src/types/`. Consult `007-graph-storage-model.md` §5 for record formats and `src/storage/serialization.rs` for the deserialization API.

**Verify:** `cargo check`

### 7.3 — Implement traversal methods

```rust
impl<'db> ReadTransaction<'db> {
    pub fn outgoing_edges(
        &self, node: NodeId, edge_type: Option<TypeId>,
    ) -> Result<Vec<Edge>, Error> {
        // Range scan on Outgoing Adjacency Index B-tree.
        // If edge_type is Some(T): scan [node, T, 0]..=[node, T, MAX]
        // If edge_type is None: scan [node, 0, 0]..=[node, MAX, MAX]
        // For each EdgeId found, look up the full Edge from Edge Store.
    }

    pub fn incoming_edges(
        &self, node: NodeId, edge_type: Option<TypeId>,
    ) -> Result<Vec<Edge>, Error> {
        // Same pattern on Incoming Adjacency Index.
    }

    pub fn neighbors(
        &self, node: NodeId, edge_type: Option<TypeId>,
    ) -> Result<Vec<Node>, Error> {
        let edges = self.outgoing_edges(node, edge_type)?;
        let mut nodes = Vec::with_capacity(edges.len());
        for edge in edges {
            if let Some(n) = self.get_node(edge.target)? {
                nodes.push(n);
            }
        }
        Ok(nodes)
    }
}
```

**Verify:** `cargo check`

### 7.4 — Implement type-based and property-based queries

```rust
impl<'db> ReadTransaction<'db> {
    pub fn nodes_by_type(
        &self, type_id: TypeId, include_subtypes: bool,
    ) -> Result<Vec<Node>, Error> {
        let type_ids = if include_subtypes {
            let mut ids = vec![type_id];
            let schema = self.inner.schema_cache.read().unwrap();
            ids.extend(schema.subtypes_of(type_id));
            ids
        } else {
            vec![type_id]
        };
        let mut result = Vec::new();
        for tid in type_ids {
            // Range scan Type Index: [0x00, tid, 0]..=[0x00, tid, MAX]
            // For each NodeId found, look up the full Node from Node Store.
            // Collect into result.
        }
        Ok(result)
    }

    pub fn edges_by_type(
        &self, type_id: TypeId, include_subtypes: bool,
    ) -> Result<Vec<Edge>, Error> {
        // Same pattern with Type Index [0x01, tid, ...]
    }

    pub fn nodes_by_property(
        &self, key: PropertyKeyId, value: &Value,
    ) -> Result<Vec<Node>, Error> {
        // Full scan of Node Store B-tree.
        // For each node, deserialize properties.
        // If property `key` exists and equals `value`, include in result.
    }
}
```

**Verify:** `cargo check`

### 7.5 — Implement counting methods

```rust
impl<'db> ReadTransaction<'db> {
    pub fn node_count(&self) -> Result<u64, Error> {
        // Count entries in the Node Store B-tree.
        // This could be a full scan count or a stored counter.
    }

    pub fn edge_count(&self) -> Result<u64, Error> { ... }

    pub fn outgoing_edge_count(
        &self, node: NodeId, edge_type: Option<TypeId>,
    ) -> Result<u64, Error> {
        // Range scan count on Outgoing Adjacency Index.
    }

    pub fn incoming_edge_count(
        &self, node: NodeId, edge_type: Option<TypeId>,
    ) -> Result<u64, Error> { ... }
}
```

**Verify:** `cargo check`

### 7.6 — Implement schema access and inference stubs

```rust
impl<'db> ReadTransaction<'db> {
    pub fn type_registry(&self) -> &dyn TypeRegistryView {
        // Return reference to schema cache (which implements TypeRegistryView).
        // This requires the RwLock read guard to live long enough.
        // Consider storing a cloned SchemaCache reference in the transaction.
    }

    pub fn property_key_registry(&self) -> &dyn PropertyKeyRegistryView { ... }

    pub fn run_inference(&self, rule_name: &str) -> Result<InferenceResult, Error> {
        // STUB: Task 26 will implement.
        Err(Error::Inference(InferenceError::RuleNotFound(rule_name.to_string())))
    }

    pub fn run_all_inference(&self) -> Result<Vec<InferenceResult>, Error> {
        // STUB: Task 26 will implement.
        Ok(Vec::new())
    }

    pub fn finish(self) {
        // Explicit drop — decrements snapshot reference count.
        drop(self);
    }
}
```

**⚠ Pitfall — Lifetime of `&dyn TypeRegistryView`:** The `type_registry()` method returns a borrow that must outlive the call. If `SchemaCache` is behind an `RwLock`, you cannot return a reference through the lock guard. Solution: either (a) clone the schema cache into the transaction at creation time, or (b) use a separate `Arc<SchemaCache>` that is snapshotted at transaction start. Option (a) is simpler and acceptable because the schema cache is small.

**Verify:** `cargo check`

### 7.7 — Unit tests for ReadTransaction

These are integration-level tests that require a fully initialized database:

Test:
- Open a database. Write some nodes and edges via a WriteTransaction (Phase 8). Commit.
- Open a ReadTransaction. Verify `get_node` returns the inserted node.
- Verify `outgoing_edges` returns the correct edges.
- Verify `incoming_edges` returns the correct edges.
- Verify `neighbors` returns the correct target nodes.
- Verify `nodes_by_type` returns nodes of the correct type.
- Verify `nodes_by_type` with `include_subtypes=true` returns nodes of subtypes too.
- Verify `edges_by_type` works.
- Verify `nodes_by_property` finds nodes with matching properties.
- Verify `node_count` and `edge_count`.
- Verify `outgoing_edge_count`.
- Verify `type_registry()` returns the registered types.

**Note:** These tests depend on WriteTransaction working (Phase 8). If needed, implement Phases 7 and 8 together and test them jointly.

**Verify:** `cargo test -- read_txn` passes.

---

## Phase 8: WriteTransaction (`src/db/write_txn.rs`)

### 8.1 — Implement WriteTransaction struct

```rust
pub struct WriteTransaction<'db> {
    inner: &'db DatabaseInner,
    snapshot: Arc<Snapshot>,
    buffer: WriteBuffer,
    /// Local copy of schema cache for read-your-own-writes on schema.
    schema_cache: SchemaCache,
    /// The write lock guard — held for the duration of the transaction.
    _write_guard: MutexGuard<'db, ()>,
    /// Whether this transaction has been committed or aborted.
    finished: bool,
    // _not_send: PhantomData<*const ()>,
}
```

Implement `Drop`: if `!finished`, abort the transaction (discard WriteBuffer, the `MutexGuard` is dropped automatically releasing the write lock).

**⚠ Pitfall — MutexGuard lifetime:** The `MutexGuard<'db, ()>` borrows the `Mutex` in `DatabaseInner`. Because `WriteTransaction` already borrows `DatabaseInner` via `inner: &'db DatabaseInner`, this is sound. However, you must ensure the `MutexGuard` is the field type, not a raw `()`.

**Verify:** `cargo check`

### 8.2 — Implement read methods (read-your-own-writes)

All read methods on `WriteTransaction` must overlay the WriteBuffer on the base snapshot:

```rust
impl<'db> WriteTransaction<'db> {
    pub fn get_node(&self, id: NodeId) -> Result<Option<Node>, Error> {
        // Check WriteBuffer first
        if self.buffer.is_node_deleted(id) {
            return Ok(None);
        }
        if let Some(node) = self.buffer.get_pending_node(id) {
            return Ok(Some(node.clone()));
        }
        // Fall back to base snapshot
        self.read_from_snapshot_node(id)
    }

    // ... same pattern for all read methods
}
```

Implement all read methods from `ReadTransaction` (7.2–7.6) with overlay logic.

**⚠ Pitfall — Traversal overlay:** `outgoing_edges` must:
1. Get base edges from snapshot.
2. Filter out deleted edges.
3. Replace updated edges with their buffer versions.
4. Add inserted edges with matching source.
5. Apply type filter if specified.

This is the same logic as `OverlayGraphView` (Phase 4). Consider extracting a shared helper or reusing `OverlayGraphView` internally.

**Verify:** `cargo check`

### 8.3 — Implement schema mutation methods

```rust
impl<'db> WriteTransaction<'db> {
    pub fn register_type(
        &mut self, def: TypeDefinition,
    ) -> Result<TypeId, Error> {
        // Register in the local schema_cache copy.
        let type_id = self.schema_cache.register_type(def.clone())?;
        // Record in WriteBuffer for persistence.
        let mut registered_def = def;
        registered_def.id = type_id;
        self.buffer.record_schema_change(
            SchemaChange::TypeRegistered(registered_def)
        );
        Ok(type_id)
    }

    pub fn get_or_create_property_key(
        &mut self, name: &str,
    ) -> Result<PropertyKeyId, Error> {
        // Check local schema_cache first.
        if let Some(id) = self.schema_cache.get_property_key_by_name(name) {
            return Ok(id);
        }
        let id = self.schema_cache.get_or_create_property_key(name);
        self.buffer.record_schema_change(
            SchemaChange::PropertyKeyRegistered(PropertyKeyDefinition {
                id,
                name: name.to_string(),
            })
        );
        Ok(id)
    }
}
```

**Verify:** `cargo check`

### 8.4 — Implement node mutation methods

```rust
impl<'db> WriteTransaction<'db> {
    pub fn insert_node(&mut self, node: Node) -> Result<NodeId, Error> {
        // Allocate a new NodeId.
        let id = self.schema_cache.allocate_node_id();
        let mut node = node;
        node.id = id;
        // Sort type_labels.
        node.type_labels.sort();
        node.type_labels.dedup();
        // Record in WriteBuffer.
        self.buffer.insert_node(node);
        Ok(id)
    }

    pub fn update_node(&mut self, node: Node) -> Result<(), Error> {
        // Get the current version (from buffer or snapshot).
        let current = self.get_node(node.id)?
            .ok_or(Error::NotFound(NotFoundError::Node(node.id)))?;
        let mut updated = node;
        updated.type_labels.sort();
        updated.type_labels.dedup();
        self.buffer.update_node(current, updated);
        Ok(())
    }

    pub fn delete_node(&mut self, id: NodeId) -> Result<(), Error> {
        let node = self.get_node(id)?
            .ok_or(Error::NotFound(NotFoundError::Node(id)))?;
        // Cascade: delete all incident edges.
        let outgoing = self.outgoing_edges(id, None)?;
        for edge in outgoing {
            self.delete_edge_internal(edge)?;
        }
        let incoming = self.incoming_edges(id, None)?;
        for edge in incoming {
            self.delete_edge_internal(edge)?;
        }
        self.buffer.delete_node(node);
        Ok(())
    }

    fn delete_edge_internal(&mut self, edge: Edge) -> Result<(), Error> {
        self.buffer.delete_edge(edge);
        Ok(())
    }
}
```

**⚠ Pitfall — Cascading delete edge collection.** When collecting incoming edges for cascade delete, some of those edges may have already been deleted as outgoing edges from the same node (self-loops). The WriteBuffer's delete method should handle duplicate deletes gracefully (no-op if already deleted).

**Verify:** `cargo check`

### 8.5 — Implement edge mutation methods

```rust
impl<'db> WriteTransaction<'db> {
    pub fn insert_edge(&mut self, edge: Edge) -> Result<EdgeId, Error> {
        let id = self.schema_cache.allocate_edge_id();
        let mut edge = edge;
        edge.id = id;
        edge.type_labels.sort();
        edge.type_labels.dedup();
        // Verify source and target nodes exist.
        if self.get_node(edge.source)?.is_none() {
            return Err(Error::NotFound(NotFoundError::Node(edge.source)));
        }
        if self.get_node(edge.target)?.is_none() {
            return Err(Error::NotFound(NotFoundError::Node(edge.target)));
        }
        self.buffer.insert_edge(edge);
        Ok(id)
    }

    pub fn update_edge(&mut self, edge: Edge) -> Result<(), Error> {
        let current = self.get_edge(edge.id)?
            .ok_or(Error::NotFound(NotFoundError::Edge(edge.id)))?;
        // Ignore source/target changes (immutable endpoints, decision A10).
        let mut updated = edge;
        updated.source = current.source;
        updated.target = current.target;
        updated.type_labels.sort();
        updated.type_labels.dedup();
        self.buffer.update_edge(current, updated);
        Ok(())
    }

    pub fn delete_edge(&mut self, id: EdgeId) -> Result<(), Error> {
        let edge = self.get_edge(id)?
            .ok_or(Error::NotFound(NotFoundError::Edge(id)))?;
        self.buffer.delete_edge(edge);
        Ok(())
    }
}
```

**Verify:** `cargo check`

### 8.6 — Implement partial property update methods

```rust
impl<'db> WriteTransaction<'db> {
    pub fn set_node_property(
        &mut self, id: NodeId, key: PropertyKeyId, value: Value,
    ) -> Result<(), Error> {
        let mut node = self.get_node(id)?
            .ok_or(Error::NotFound(NotFoundError::Node(id)))?;
        let before = node.clone();
        node.properties.insert(key, value);
        self.buffer.update_node(before, node);
        Ok(())
    }

    pub fn remove_node_property(
        &mut self, id: NodeId, key: PropertyKeyId,
    ) -> Result<Option<Value>, Error> {
        let mut node = self.get_node(id)?
            .ok_or(Error::NotFound(NotFoundError::Node(id)))?;
        let before = node.clone();
        let removed = node.properties.remove(&key);
        if removed.is_some() {
            self.buffer.update_node(before, node);
        }
        Ok(removed)
    }

    // Same for edges: set_edge_property, remove_edge_property
    pub fn set_edge_property(
        &mut self, id: EdgeId, key: PropertyKeyId, value: Value,
    ) -> Result<(), Error> { ... }

    pub fn remove_edge_property(
        &mut self, id: EdgeId, key: PropertyKeyId,
    ) -> Result<Option<Value>, Error> { ... }
}
```

**Verify:** `cargo check`

### 8.7 — Implement type label mutation methods

```rust
impl<'db> WriteTransaction<'db> {
    pub fn add_node_type(
        &mut self, id: NodeId, type_id: TypeId,
    ) -> Result<(), Error> {
        let mut node = self.get_node(id)?
            .ok_or(Error::NotFound(NotFoundError::Node(id)))?;
        let before = node.clone();
        if !node.type_labels.contains(&type_id) {
            node.type_labels.push(type_id);
            node.type_labels.sort();
            self.buffer.update_node(before, node);
        }
        Ok(())
    }

    pub fn remove_node_type(
        &mut self, id: NodeId, type_id: TypeId,
    ) -> Result<bool, Error> {
        let mut node = self.get_node(id)?
            .ok_or(Error::NotFound(NotFoundError::Node(id)))?;
        let before = node.clone();
        let pos = node.type_labels.iter().position(|t| *t == type_id);
        if let Some(pos) = pos {
            node.type_labels.remove(pos);
            self.buffer.update_node(before, node);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    // Same for edges: add_edge_type, remove_edge_type
}
```

**Verify:** `cargo check`

### 8.8 — Implement validation methods

```rust
impl<'db> WriteTransaction<'db> {
    /// Dry-run validation against pending changes.
    pub fn validate(&self) -> Result<Vec<ConstraintViolation>, Error> {
        let (node_changes, edge_changes) = self.buffer.build_changeset();
        let changeset = ChangeSet::new(&node_changes, &edge_changes);
        let graph_view = OverlayGraphView::new(
            /* base snapshot reader */,
            &self.buffer,
            &self.schema_cache,
        );
        let validators = self.inner.constraint_registry.read().unwrap();
        let affected_types = changeset.affected_types();
        let mut all_violations = Vec::new();
        for validator in validators.iter() {
            if let Some(applies_to) = validator.applies_to_types() {
                if !applies_to.iter().any(|t| affected_types.contains(t)) {
                    continue; // Skip — validator doesn't care about these types.
                }
            }
            let violations = validator.validate(
                &changeset,
                &graph_view,
                &self.schema_cache,
                &self.schema_cache,
            );
            all_violations.extend(violations);
        }
        Ok(all_violations)
    }

    /// Full revalidation: treat all data as newly inserted.
    pub fn validate_all(&self) -> Result<Vec<ConstraintViolation>, Error> {
        // Build synthetic ChangeSet with every node as Inserted
        // and every edge as Inserted.
        let all_nodes = self.all_nodes()?;
        let all_edges = self.all_edges()?;
        let node_changes: Vec<NodeChange> = all_nodes.into_iter()
            .map(NodeChange::Inserted)
            .collect();
        let edge_changes: Vec<EdgeChange> = all_edges.into_iter()
            .map(EdgeChange::Inserted)
            .collect();
        let changeset = ChangeSet::new(&node_changes, &edge_changes);
        // Run validators (no type filtering — this is full revalidation)
        let graph_view = /* ... */;
        // ... same dispatch as validate()
    }
}
```

**⚠ Pitfall — `all_edges()` method.** `validate_all` needs to iterate all edges. This method may not be in the API spec. Implement it as a private helper that does a full scan of the Edge Store B-tree.

**Verify:** `cargo check`

### 8.9 — Implement commit

```rust
impl<'db> WriteTransaction<'db> {
    pub fn commit(mut self) -> Result<(), Error> {
        self.finished = true;

        // Step 1: Build ChangeSet.
        let (node_changes, edge_changes) = self.buffer.build_changeset();

        // Step 2: Run constraint validators (same as validate()).
        let changeset = ChangeSet::new(&node_changes, &edge_changes);
        let graph_view = OverlayGraphView::new(/* ... */);
        let validators = self.inner.constraint_registry.read().unwrap();
        let affected_types = changeset.affected_types();
        let mut all_violations = Vec::new();
        for validator in validators.iter() {
            // ... same dispatch logic as validate()
        }
        if !all_violations.is_empty() {
            return Err(Error::ConstraintViolation(all_violations));
        }

        // Step 3: Materialize B-tree changes via storage engine.
        // For each pending node insert: insert into Node Store B-tree
        //   + insert into Type Index for each type label
        // For each pending node update: update Node Store B-tree
        //   + update Type Index (remove old labels, add new labels)
        // For each pending node delete: delete from Node Store B-tree
        //   + delete from Type Index
        //   + delete from ID Freelist
        // Same for edges (plus Adjacency Index updates)
        // For each schema change: update Schema Store B-tree

        // Step 4: Commit via storage engine (write pages + fsync + superblock).
        // This produces new root page IDs.

        // Step 5: Update current_snapshot.
        {
            let mut current = self.inner.current_snapshot.write().unwrap();
            *current = Arc::new(new_snapshot);
        }

        // Step 6: Update the global schema cache.
        {
            let mut global_cache = self.inner.schema_cache.write().unwrap();
            *global_cache = self.schema_cache;
        }

        // Step 7: Persist extension names if any were registered.

        Ok(())
    }

    pub fn abort(mut self) {
        self.finished = true;
        // WriteBuffer is dropped automatically.
        // MutexGuard is dropped automatically, releasing write lock.
    }
}
```

**⚠ Pitfall — B-tree materialization ordering.** Schema changes must be materialized before node/edge changes, because node/edge records may reference newly registered type IDs or property key IDs.

**⚠ Pitfall — Adjacency Index updates.** Each edge insert requires two Adjacency Index inserts (outgoing and incoming) and two Type Index inserts. Each edge delete requires the corresponding deletes. Each edge update that changes type labels requires removing old index entries and adding new ones.

**⚠ Pitfall — Commit consumes `self`.** After `commit` returns (success or `ConstraintViolation` error), the transaction is gone. The `finished = true` flag prevents the `Drop` impl from trying to abort again.

**Verify:** `cargo check`

### 8.10 — Implement inference stubs on WriteTransaction

```rust
impl<'db> WriteTransaction<'db> {
    pub fn run_inference(
        &mut self, rule_name: &str, mode: InferenceMode,
    ) -> Result<InferenceResult, Error> {
        // STUB for Task 26.
        Err(Error::Inference(InferenceError::RuleNotFound(rule_name.to_string())))
    }

    pub fn run_all_inference(
        &mut self, mode: InferenceMode,
    ) -> Result<Vec<InferenceResult>, Error> {
        // STUB for Task 26.
        Ok(Vec::new())
    }

    pub fn last_materialization_mapping(&self) -> Option<&MaterializedMapping> {
        // STUB for Task 26.
        None
    }
}
```

**Verify:** `cargo check`

### 8.11 — Implement provenance stubs

```rust
impl<'db> ReadTransaction<'db> {
    pub fn is_inferred_node(&self, _id: NodeId) -> Result<bool, Error> {
        Ok(false) // STUB for Task 26
    }
    pub fn is_inferred_edge(&self, _id: EdgeId) -> Result<bool, Error> {
        Ok(false) // STUB for Task 26
    }
    pub fn node_provenance(&self, _id: NodeId) -> Result<Option<ProvenanceRecord>, Error> {
        Ok(None) // STUB for Task 26
    }
    pub fn edge_provenance(&self, _id: EdgeId) -> Result<Option<ProvenanceRecord>, Error> {
        Ok(None) // STUB for Task 26
    }
}
```

Add the same stubs to `WriteTransaction`.

**Verify:** `cargo check`

---

## Phase 9: GraphReader Trait (`src/db/graph_reader.rs`)

### 9.1 — Implement GraphReader trait for both transaction types

```rust
pub trait GraphReader {
    fn get_node(&self, id: NodeId) -> Result<Option<Node>, Error>;
    fn get_edge(&self, id: EdgeId) -> Result<Option<Edge>, Error>;
    fn outgoing_edges(&self, node: NodeId, edge_type: Option<TypeId>) -> Result<Vec<Edge>, Error>;
    fn incoming_edges(&self, node: NodeId, edge_type: Option<TypeId>) -> Result<Vec<Edge>, Error>;
    fn neighbors(&self, node: NodeId, edge_type: Option<TypeId>) -> Result<Vec<Node>, Error>;
    fn nodes_by_type(&self, type_id: TypeId, include_subtypes: bool) -> Result<Vec<Node>, Error>;
    fn edges_by_type(&self, type_id: TypeId, include_subtypes: bool) -> Result<Vec<Edge>, Error>;
    fn nodes_by_property(&self, key: PropertyKeyId, value: &Value) -> Result<Vec<Node>, Error>;
    fn type_registry(&self) -> &dyn TypeRegistryView;
    fn property_key_registry(&self) -> &dyn PropertyKeyRegistryView;
}
```

Implement for `ReadTransaction` and `WriteTransaction` by delegating to their respective methods.

**⚠ Pitfall:** Verify the trait definition matches the one from `010-api-surface-spec.md` §10.1 exactly. Adapt if the actual trait in `src/schema/` (or wherever it was placed by Task 22) differs.

**Verify:** `cargo check`

### 9.2 — Compile-time assertion for GraphReader object safety

```rust
#[cfg(test)]
fn _assert_graph_reader_object_safe(_: &dyn GraphReader) {}
```

**Verify:** `cargo test` compiles.

---

## Phase 10: Integration Tests

### 10.1 — Basic CRUD round-trip test

Create a test that exercises the full lifecycle:

1. Open a persistent database (in a temp directory).
2. Begin a write transaction.
3. Register two node types ("Person", "Organization") and one edge type ("works_at").
4. Register a property key ("name").
5. Insert two nodes (Alice: Person, Acme: Organization) with name properties.
6. Insert an edge (Alice → Acme, type: works_at).
7. Commit.
8. Begin a read transaction.
9. Verify `get_node` returns Alice with correct properties.
10. Verify `outgoing_edges(alice, None)` returns the works_at edge.
11. Verify `outgoing_edges(alice, Some(works_at))` returns the edge.
12. Verify `outgoing_edges(alice, Some(other_type))` returns empty.
13. Verify `incoming_edges(acme, None)` returns the edge.
14. Verify `neighbors(alice, Some(works_at))` returns Acme.
15. Verify `nodes_by_type(person_type, false)` returns Alice.
16. Verify `edges_by_type(works_at_type, false)` returns the edge.
17. Verify `nodes_by_property(name_key, "Alice")` returns Alice.
18. Verify `node_count()` returns 2, `edge_count()` returns 1.

**Verify:** `cargo test -- crud_round_trip` passes.

### 10.2 — Read-your-own-writes test

1. Open database, begin write transaction.
2. Insert node A. Read A within same transaction — should succeed.
3. Update A's property. Read A — should see updated property.
4. Delete A. Read A — should return None.
5. Insert node B. `nodes_by_type(B.type)` should include B.
6. Abort the transaction.
7. Begin read transaction. Verify A and B are not visible.

**Verify:** `cargo test -- read_your_own_writes` passes.

### 10.3 — Cascading node deletion test

1. Create a graph: A → B → C (two edges).
2. Also create an edge C → A (cycle).
3. Delete node B.
4. Verify edges A → B and B → C are deleted.
5. Verify edge C → A still exists.
6. Verify nodes A and C still exist.
7. Commit and verify the same state via a read transaction.

**Verify:** `cargo test -- cascading_delete` passes.

### 10.4 — Type hierarchy and subtype query test

1. Register types: Animal (root), Mammal (supertype: Animal), Dog (supertype: Mammal), Cat (supertype: Mammal).
2. Insert nodes: Fido (Dog), Whiskers (Cat), Generic (Animal).
3. Query `nodes_by_type(Animal, include_subtypes=true)` — should return all three.
4. Query `nodes_by_type(Mammal, include_subtypes=true)` — should return Fido and Whiskers.
5. Query `nodes_by_type(Dog, include_subtypes=true)` — should return only Fido.
6. Query `nodes_by_type(Dog, include_subtypes=false)` — should return only Fido.

**Verify:** `cargo test -- subtype_query` passes.

### 10.5 — Multi-hop traversal test

This is a critical test explicitly required by the task's done criterion.

Build a social/organizational graph with 4+ node types and 3+ edge types:

**Types:** Person, Team, Project, Skill  
**Edge types:** member_of (Person → Team), works_on (Person → Project), requires (Project → Skill), has_skill (Person → Skill)

**Graph:**
```
Alice --member_of--> Engineering
Alice --works_on--> ProjectX
Alice --has_skill--> Rust
Bob   --member_of--> Engineering
Bob   --works_on--> ProjectY
Bob   --has_skill--> Python
Carol --member_of--> Design
Carol --works_on--> ProjectX
ProjectX --requires--> Rust
ProjectX --requires--> Python
ProjectY --requires--> Python
```

**Multi-hop queries (composed from single-hop primitives):**

1. "Find all skills required by projects that Alice works on" (2 hops):
   - `outgoing_edges(Alice, works_on)` → ProjectX
   - `outgoing_edges(ProjectX, requires)` → [Rust, Python]
   - Verify result: {Rust, Python}

2. "Find all people who are members of the same team as Alice" (2 hops):
   - `outgoing_edges(Alice, member_of)` → Engineering
   - `incoming_edges(Engineering, member_of)` → [Alice, Bob]
   - Verify result: {Alice, Bob}

3. "Find all projects that require skills Alice has" (3 hops via matching):
   - `outgoing_edges(Alice, has_skill)` → [Rust]
   - For each skill, `incoming_edges(skill, requires)` → [ProjectX]
   - Verify result: {ProjectX}

4. "Find people who work on projects requiring Python, traversed through 4 edges" (multi-hop chain):
   - Start from Python skill node
   - `incoming_edges(Python, requires)` → [ProjectX, ProjectY]
   - For each project: `incoming_edges(project, works_on)` → people
   - For each person: `outgoing_edges(person, member_of)` → teams
   - Collect all team names reached from Python through 4 hops
   - Verify: from Python → ProjectX → [Alice, Carol] → [Engineering, Design]; from Python → ProjectY → [Bob] → [Engineering]
   - Result teams: {Engineering, Design}

**Verify:** `cargo test -- multi_hop` passes.

### 10.6 — Constraint validation at commit test

1. Implement a simple test constraint validator:
   ```rust
   struct RequireNameProperty { name_key: PropertyKeyId }
   impl ConstraintValidator for RequireNameProperty {
       fn name(&self) -> &str { "RequireNameProperty" }
       fn applies_to_types(&self) -> Option<Vec<TypeId>> { None }
       fn validate(&self, changes: &ChangeSet<'_>, graph: &dyn GraphView, ...) -> Vec<ConstraintViolation> {
           // Check that every inserted node has a "name" property.
           let mut violations = Vec::new();
           for change in changes.inserted_nodes() {
               if !change.properties.contains_key(&self.name_key) {
                   violations.push(ConstraintViolation {
                       violation_kind: "MissingName".into(),
                       message: format!("Node {:?} missing name", change.id),
                       subject: Some(ViolationSubject::Node(change.id)),
                   });
               }
           }
           violations
       }
   }
   ```
2. Register the validator.
3. Insert a node **without** a name property.
4. Attempt to commit — verify `Error::ConstraintViolation` is returned.
5. Insert a node **with** a name property.
6. Commit — verify success.

**Verify:** `cargo test -- constraint_validation` passes.

### 10.7 — Empty transaction commit test

1. Begin write transaction.
2. Commit immediately (no mutations).
3. Verify success (no crash, no error).

**Verify:** `cargo test -- empty_commit` passes.

### 10.8 — Parallel edges test

1. Insert two edges from A → B with the same type.
2. Verify `outgoing_edges(A, type)` returns both edges.
3. Delete one edge. Verify `outgoing_edges(A, type)` returns only the other.

**Verify:** `cargo test -- parallel_edges` passes.

### 10.9 — Persistence round-trip test

1. Create database, insert data, commit.
2. Drop the `Database` object (closing the file).
3. Reopen the same file path.
4. Begin a read transaction. Verify all data is present.

**Verify:** `cargo test -- persistence_round_trip` passes.

---

## Phase 11: Concurrent Access Tests

### 11.1 — Multiple concurrent readers

1. Create database, insert some data, commit.
2. Spawn 4 threads, each opening a `read_txn()`.
3. Each thread performs reads (get_node, outgoing_edges, nodes_by_type).
4. All threads complete without error.
5. Verify all threads saw the same data.

**⚠ Pitfall — `Database` sharing across threads.** `Database` is `Send + Sync`. Wrap it in `Arc<Database>` for cross-thread sharing. Transactions are `!Send` — they must be created and used on the same thread.

**Verify:** `cargo test -- concurrent_readers` passes.

### 11.2 — Reader/writer isolation

1. Create database, insert node A, commit.
2. Spawn a reader thread: begin `read_txn()`, read node A, sleep briefly.
3. On the main thread: begin `write_txn()`, insert node B, commit.
4. Reader thread: verify it can still see A but **cannot** see B (snapshot isolation).
5. Reader finishes. Begin new `read_txn()` — now B is visible.

**Verify:** `cargo test -- reader_writer_isolation` passes.

### 11.3 — Write serialization

1. Create database.
2. Spawn two threads, each trying to `write_txn()`.
3. The first to acquire the lock inserts data and commits.
4. The second blocks, then acquires the lock, inserts different data, and commits.
5. Verify both insertions are present in a final read.

**Verify:** `cargo test -- write_serialization` passes.

### 11.4 — Concurrent read during write (stress test)

1. Create database with initial data (100 nodes, 200 edges).
2. Spawn a writer thread that continuously inserts and commits in a loop (10 transactions, each inserting 10 nodes).
3. Spawn 3 reader threads that continuously read in a loop.
4. Run for a bounded duration (e.g., 2 seconds or until writer finishes).
5. Verify: no panics, no data corruption, all reads return consistent snapshots.

**Verify:** `cargo test -- concurrent_stress` passes.

---

## Phase 12: Final Verification

### 12.1 — Full no_std verification

```
cargo check --no-default-features --features alloc
```

Must succeed with zero errors. The `db/` module is not compiled under `no_std`, but nothing in the `no_std` modules should break.

### 12.2 — Full std verification

```
cargo check
```

Must succeed with zero errors.

### 12.3 — Full test suite

```
cargo test
```

All tests pass, zero failures.

### 12.4 — Clippy

```
cargo clippy --all-targets --all-features -- -D warnings
```

Zero warnings.

### 12.5 — Documentation

```
cargo doc --no-deps
```

Zero warnings. Every `pub` item in `src/db/` has a doc comment.

### 12.6 — Review against design documents

Manually verify:
- Every method in `ReadTransaction` matches `010-api-surface-spec.md` §6.1.
- Every method in `WriteTransaction` matches `010-api-surface-spec.md` §6.2.
- The `GraphReader` trait matches `010-api-surface-spec.md` §10.1.
- The `Database` methods match `010-api-surface-spec.md` §5.
- `DatabaseConfig` fields and defaults match `012-design-document.md` §15.1.
- The concurrency model matches `012-design-document.md` §10.
- The commit sequence matches `012-design-document.md` §11.2.
- Thread safety: `Database` is `Send + Sync`, transactions are `!Send + !Sync`.
- `ConstraintValidator` and `InferenceRule` `Send + Sync` assertions still pass.

Document any intentional deviations from the spec in the completion report.

---

## Post-Completion

Produce a completion report following the format in the master project prompt's Instance Rules section. Include the verification evidence from Phase 12.
