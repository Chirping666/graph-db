# Checklist: Task 22 — Implement Core Data Model & Types

**Parent:** Task 22 (this checklist)  
**Implements:** All types in `src/types/`, `src/schema/`, `src/constraint/`, `src/inference/`, `src/error/`, and `src/lib.rs`.

Execute items in order. After each item, run the verification command(s) listed. Do not proceed until verification passes.

---

## Phase 0: Project Scaffolding

### 0.1 — Initialize crate and Cargo.toml

Create `Cargo.toml` with:
- `name = "graph_db"`
- `edition = "2021"`
- Feature flags: `default = ["std"]`, `std = ["alloc"]`, `alloc = []`
- No dependencies in `[dependencies]` (except `crc32fast` may be added later by Task 23+).
- Dev dependencies: `tempfile` (for future integration tests).

**Verify:** `cargo check` succeeds (empty lib).

### 0.2 — Create lib.rs with no_std scaffolding

Create `src/lib.rs` with:
```rust
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;
```

Add module declarations (all as `pub mod`):
```rust
pub mod types;
pub mod schema;
pub mod constraint;
pub mod inference;
pub mod error;
```

Create empty `mod.rs` files for each module with a placeholder `//!` module doc comment.

**Verify:**
- `cargo check` succeeds.
- `cargo check --no-default-features --features alloc` succeeds.

---

## Phase 1: Identity Types (`src/types/mod.rs`)

### 1.1 — Implement NodeId, EdgeId, TypeId, PropertyKeyId

Define all four newtype structs with:
- `pub` inner field (e.g., `pub struct NodeId(pub u64);`)
- Derives: `Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug`
- `/// ` doc comment on each struct and on the inner field
- Associated constant `NULL` (e.g., `pub const NULL: NodeId = NodeId(0);`)
- Method `pub fn is_null(self) -> bool` returning `self.0 == 0`
- Implement `core::fmt::Display` for each (displays as the inner integer)

**⚠ Pitfall:** Use `core::fmt`, not `std::fmt`. These are `no_std` types.

**Verify:**
- `cargo check --no-default-features --features alloc`
- `cargo doc --no-deps` — no warnings on ID types.

### 1.2 — Unit tests for ID types

In `#[cfg(test)] mod tests` within `src/types/mod.rs`:
- Test construction: `NodeId(42)` has inner value 42.
- Test `NULL` constant: `NodeId::NULL.is_null()` returns true.
- Test non-null: `NodeId(1).is_null()` returns false.
- Test `Ord`: `NodeId(1) < NodeId(2)`.
- Test `Display`: `format!("{}", NodeId(42))` produces `"42"`.
- Repeat for all four ID types.

**Verify:** `cargo test -- types` passes.

---

## Phase 2: Value System (`src/types/mod.rs`)

### 2.1 — Implement the Value enum

Define `Value` with exactly these variants (from `006-schema-extension-spec.md` §4.1):
```
Null, Bool(bool), I64(i64), U64(u64), F64(f64),
String(String), Bytes(Vec<u8>), NodeRef(NodeId),
LangString { value: String, lang: String },
List(Vec<Value>)
```

Derives: `Clone, Debug, PartialEq` — **NOT `Eq`** (f64 prevents it).

Use `alloc::string::String` and `alloc::vec::Vec`.

Add `/// ` doc comments on the enum and every variant, matching the spec.

**⚠ Pitfall:** Do not derive `Eq`. The `PartialEq` for `F64` follows IEEE 754 (NaN ≠ NaN). This is documented behavior.

**Verify:** `cargo check --no-default-features --features alloc`

### 2.2 — Implement the ValueTypeDescriptor enum

Define `ValueTypeDescriptor` with exactly these variants (from `006-schema-extension-spec.md` §4.2):
```
Any, Bool, I64, U64, F64, String, Bytes, NodeRef, LangString,
List(Box<ValueTypeDescriptor>)
```

Derives: `Clone, Debug, PartialEq, Eq` — `Eq` IS valid here (no `f64`).

Use `alloc::boxed::Box` for the `List` variant.

**Verify:** `cargo check --no-default-features --features alloc`

### 2.3 — Implement Value::matches_descriptor()

Add a method on `Value`:
```rust
/// Returns true if this value matches the given type descriptor.
pub fn matches_descriptor(&self, descriptor: &ValueTypeDescriptor) -> bool
```

Logic:
- `ValueTypeDescriptor::Any` → always true
- Each specific variant matches the corresponding `Value` variant
- `ValueTypeDescriptor::String` matches both `Value::String` and `Value::LangString` (per §4.2: "Must be a String (plain or language-tagged)")
- `ValueTypeDescriptor::List(inner)` matches `Value::List(items)` where all items match `inner`
- `Value::Null` matches `ValueTypeDescriptor::Any` only (null does not match specific type descriptors)

**Verify:** `cargo check`

### 2.4 — Implement Value helper methods

Add convenience methods:
- `pub fn is_null(&self) -> bool`
- `pub fn as_bool(&self) -> Option<bool>`
- `pub fn as_i64(&self) -> Option<i64>`
- `pub fn as_u64(&self) -> Option<u64>`
- `pub fn as_f64(&self) -> Option<f64>`
- `pub fn as_str(&self) -> Option<&str>` (works for `String` variant only)
- `pub fn as_bytes(&self) -> Option<&[u8]>`
- `pub fn as_node_ref(&self) -> Option<NodeId>`

Each returns `Some(inner)` for the matching variant, `None` otherwise.

**Verify:** `cargo check`

### 2.5 — Unit tests for Value and ValueTypeDescriptor

Test:
- Construction of every `Value` variant.
- `matches_descriptor` for every (Value variant × ValueTypeDescriptor variant) combination — at minimum test every matching pair returns true, every non-matching pair returns false, and `Any` matches everything.
- `Value::Null` matches only `Any`.
- `Value::LangString` matches both `String` and `LangString` descriptors.
- `Value::List(vec![Value::I64(1)])` matches `List(Box::new(I64))`.
- `Value::List(vec![Value::I64(1), Value::String("x".into())])` does NOT match `List(Box::new(I64))`.
- Empty `Value::List(vec![])` matches `List(Box::new(Any))` and any `List(Box::new(...))`.
- All `as_*` helper methods.
- Confirm `Value` does NOT implement `Eq` — this is a compile-time property, document in a comment.

**Verify:** `cargo test -- types` passes.

---

## Phase 3: PropertyMap, Node, Edge (`src/types/mod.rs`)

### 3.1 — Define PropertyMap type alias

```rust
pub type PropertyMap = BTreeMap<PropertyKeyId, Value>;
```

Use `alloc::collections::BTreeMap`. Add a `/// ` doc comment.

**Verify:** `cargo check --no-default-features --features alloc`

### 3.2 — Implement Node struct

Define `Node` with fields (from `006-schema-extension-spec.md` §6.1):
- `pub id: NodeId`
- `pub type_labels: Vec<TypeId>` (sorted)
- `pub properties: PropertyMap`
- `pub is_anonymous: bool`

Derives: `Clone, Debug, PartialEq` — **NOT `Eq`** (PropertyMap contains Value which contains f64).

Add doc comments on the struct and every field.

**Verify:** `cargo check --no-default-features --features alloc`

### 3.3 — Implement Edge struct

Define `Edge` with fields (from `006-schema-extension-spec.md` §6.2):
- `pub id: EdgeId`
- `pub type_labels: Vec<TypeId>` (sorted)
- `pub source: NodeId`
- `pub target: NodeId`
- `pub properties: PropertyMap`

Derives: `Clone, Debug, PartialEq` — **NOT `Eq`**.

Add doc comments on the struct and every field.

**Verify:** `cargo check --no-default-features --features alloc`

### 3.4 — Unit tests for Node and Edge

Test:
- Construct a `Node` with id, type labels, properties, and is_anonymous flag.
- Construct an `Edge` with id, type labels, source, target, and properties.
- `PartialEq` works: two identical nodes are equal; two nodes differing in any field are not.
- Sorted type_labels invariant: test/document that callers are responsible for sorting type_labels (the struct does not enforce sorting at construction time — enforcement is a downstream concern).

**Verify:** `cargo test -- types` passes.

---

## Phase 4: Type System (`src/types/mod.rs`)

### 4.1 — Implement TypeKind enum

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TypeKind {
    Node,
    Edge,
}
```

Implement `core::fmt::Display` for TypeKind.

**Verify:** `cargo check --no-default-features --features alloc`

### 4.2 — Implement PropertyDeclaration struct

Define with fields (from `006-schema-extension-spec.md` §7.3):
- `pub key: PropertyKeyId`
- `pub value_type: ValueTypeDescriptor`
- `pub required: bool`
- `pub multi_valued: bool`
- `pub metadata: PropertyMap`

Derives: `Clone, Debug, PartialEq` — **NOT `Eq`** (metadata contains Value).

Add doc comments on the struct and every field, including the note that the core stores these declarations but does NOT enforce them — enforcement is done by downstream `ConstraintValidator` implementations.

**Verify:** `cargo check --no-default-features --features alloc`

### 4.3 — Implement TypeDefinition struct

Define with fields (from `006-schema-extension-spec.md` §7.4):
- `pub id: TypeId`
- `pub name: String`
- `pub kind: TypeKind`
- `pub supertypes: Vec<TypeId>`
- `pub property_declarations: Vec<PropertyDeclaration>`
- `pub open: bool`
- `pub metadata: PropertyMap`

Derives: `Clone, Debug, PartialEq` — **NOT `Eq`** (metadata and property_declarations contain Value).

Add doc comments on the struct and every field.

**Verify:** `cargo check --no-default-features --features alloc`

### 4.4 — Unit tests for TypeKind, PropertyDeclaration, TypeDefinition

Test:
- `TypeKind::Node != TypeKind::Edge`.
- `TypeKind` implements `Eq` (compile-time verification).
- Construct a `PropertyDeclaration` with all fields.
- Construct a `TypeDefinition` with supertypes and property declarations.
- `PartialEq` works for `TypeDefinition`.
- `Display` for `TypeKind`.

**Verify:** `cargo test -- types` passes.

---

## Phase 5: Schema Traits (`src/schema/mod.rs`)

### 5.1 — Implement GraphView trait

Define the `GraphView` trait (from `006-schema-extension-spec.md` §10.3):

```rust
pub trait GraphView {
    fn get_node(&self, id: NodeId) -> Option<&Node>;
    fn get_edge(&self, id: EdgeId) -> Option<&Edge>;
    fn outgoing_edges(&self, node: NodeId, edge_type: Option<TypeId>) -> Vec<&Edge>;
    fn incoming_edges(&self, node: NodeId, edge_type: Option<TypeId>) -> Vec<&Edge>;
    fn nodes_by_type(&self, type_id: TypeId, include_subtypes: bool) -> Vec<&Node>;
    fn edges_by_type(&self, type_id: TypeId, include_subtypes: bool) -> Vec<&Edge>;
    fn nodes_by_property(&self, key: PropertyKeyId, value: &Value) -> Vec<&Node>;
}
```

Add `/// ` doc comments on the trait and every method.

**⚠ Pitfall — Object safety:** The trait must be usable as `&dyn GraphView`. Verify with:
```rust
fn _assert_graph_view_object_safe(_: &dyn GraphView) {}
```

Import `Node`, `Edge`, `NodeId`, `EdgeId`, `TypeId`, `PropertyKeyId`, `Value` from `crate::types`.

**Verify:**
- `cargo check --no-default-features --features alloc`
- The object-safety assertion compiles.

### 5.2 — Implement TypeRegistryView trait

Define the `TypeRegistryView` trait (from `006-schema-extension-spec.md` §7.6):

```rust
pub trait TypeRegistryView {
    fn get_type(&self, id: TypeId) -> Option<&TypeDefinition>;
    fn get_type_by_name(&self, name: &str, kind: TypeKind) -> Option<&TypeDefinition>;
    fn all_types(&self) -> &[TypeDefinition];
    fn types_by_kind(&self, kind: TypeKind) -> Vec<&TypeDefinition>;
    fn direct_supertypes(&self, id: TypeId) -> Option<&[TypeId]>;
    fn all_supertypes(&self, id: TypeId) -> Vec<TypeId>;
    fn direct_subtypes(&self, id: TypeId) -> Vec<TypeId>;
    fn all_subtypes(&self, id: TypeId) -> Vec<TypeId>;
    fn is_subtype_of(&self, candidate: TypeId, ancestor: TypeId) -> bool;
    fn effective_property_declarations(&self, id: TypeId) -> Vec<PropertyDeclaration>;
}
```

Add doc comments on the trait and every method.

**⚠ Pitfall — Object safety:** Verify with `fn _assert_type_registry_view_object_safe(_: &dyn TypeRegistryView) {}`. Note that `all_types() -> &[TypeDefinition]` returns a slice reference — this assumes the implementation stores types contiguously (documented design decision). `types_by_kind` and hierarchy traversal methods return `Vec` because they compute results dynamically.

**Verify:** `cargo check --no-default-features --features alloc`

### 5.3 — Implement PropertyKeyRegistryView trait

Define the `PropertyKeyRegistryView` trait (from `006-schema-extension-spec.md` §9.1):

```rust
pub trait PropertyKeyRegistryView {
    fn get_key_id(&self, name: &str) -> Option<PropertyKeyId>;
    fn get_key_name(&self, id: PropertyKeyId) -> Option<&str>;
    fn all_keys(&self) -> Vec<(PropertyKeyId, &str)>;
}
```

Add doc comments on the trait and every method.

Verify object safety.

**Verify:**
- `cargo check --no-default-features --features alloc`
- `cargo doc --no-deps` — no warnings on schema module.

### 5.4 — Compile-time object safety test

Add a `#[cfg(test)]` module in `src/schema/mod.rs` with static assertion functions for all three traits:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    fn _assert_graph_view_object_safe(_: &dyn GraphView) {}
    fn _assert_type_registry_view_object_safe(_: &dyn TypeRegistryView) {}
    fn _assert_property_key_registry_view_object_safe(_: &dyn PropertyKeyRegistryView) {}
}
```

**Verify:** `cargo test -- schema` compiles (the functions themselves are never called — their existence proves object safety).

---

## Phase 6: Constraint Types (`src/constraint/mod.rs`)

### 6.1 — Implement NodeChange and EdgeChange enums

From `006-schema-extension-spec.md` §10.2:

```rust
#[derive(Clone, Debug)]
pub enum NodeChange {
    Inserted(Node),
    Modified { before: Node, after: Node },
    Deleted(Node),
}

#[derive(Clone, Debug)]
pub enum EdgeChange {
    Inserted(Edge),
    Modified { before: Edge, after: Edge },
    Deleted(Edge),
}
```

Add doc comments on each enum and variant.

**Verify:** `cargo check --no-default-features --features alloc`

### 6.2 — Implement ChangeSet struct with methods

From `006-schema-extension-spec.md` §10.2:

```rust
pub struct ChangeSet<'a> {
    node_changes: &'a [NodeChange],
    edge_changes: &'a [EdgeChange],
}
```

Fields are **private**. Provide:
- `pub fn new(node_changes: &'a [NodeChange], edge_changes: &'a [EdgeChange]) -> Self`
- `pub fn node_changes(&self) -> &[NodeChange]`
- `pub fn edge_changes(&self) -> &[EdgeChange]`
- `pub fn inserted_nodes(&self) -> impl Iterator<Item = &Node> + '_`
- `pub fn modified_nodes(&self) -> impl Iterator<Item = (&Node, &Node)> + '_`
- `pub fn deleted_nodes(&self) -> impl Iterator<Item = &Node> + '_`
- `pub fn inserted_edges(&self) -> impl Iterator<Item = &Edge> + '_`
- `pub fn modified_edges(&self) -> impl Iterator<Item = (&Edge, &Edge)> + '_`
- `pub fn deleted_edges(&self) -> impl Iterator<Item = &Edge> + '_`
- `pub fn affected_types(&self) -> Vec<TypeId>` — collects unique TypeIds from all type_labels on all changed nodes and edges.

Add doc comments on the struct, constructor, and every method.

**⚠ Pitfall:** The `affected_types()` method should use `BTreeSet` (from `alloc`) to deduplicate, then convert to `Vec`. Do NOT use `HashSet` — it requires `std` or `hashbrown`.

**Verify:** `cargo check --no-default-features --features alloc`

### 6.3 — Implement ConstraintViolation and ViolationSubject

From `006-schema-extension-spec.md` §10.4:

```rust
#[derive(Clone, Debug)]
pub struct ConstraintViolation {
    pub violation_kind: String,
    pub message: String,
    pub subject: Option<ViolationSubject>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViolationSubject {
    Node(NodeId),
    Edge(EdgeId),
    Type(TypeId),
}
```

Add doc comments.

**Verify:** `cargo check --no-default-features --features alloc`

### 6.4 — Implement ConstraintValidator trait

From `006-schema-extension-spec.md` §10.5:

```rust
pub trait ConstraintValidator: Send + Sync {
    fn name(&self) -> &str;
    fn applies_to_types(&self) -> Option<Vec<TypeId>>;
    fn validate(
        &self,
        changes: &ChangeSet<'_>,
        graph: &dyn GraphView,
        types: &dyn TypeRegistryView,
        keys: &dyn PropertyKeyRegistryView,
    ) -> Vec<ConstraintViolation>;
}
```

Import `GraphView`, `TypeRegistryView`, `PropertyKeyRegistryView` from `crate::schema`.

Add full doc comments including the `# Lifecycle` and `# Thread Safety` sections from the spec.

**⚠ Pitfall — `Send + Sync` requirement:** This is a supertrait bound. The trait MUST include `: Send + Sync`. Without it, `Box<dyn ConstraintValidator>` cannot be stored in the multi-threaded `Database`.

**Verify:**
- `cargo check --no-default-features --features alloc`
- Add compile-time assertion: `fn _assert_constraint_validator_object_safe(_: &dyn ConstraintValidator) {}`

### 6.5 — Unit tests for constraint types

Test:
- Construct `NodeChange::Inserted`, `NodeChange::Modified`, `NodeChange::Deleted`.
- Construct a `ChangeSet` and verify all iterator methods return correct subsets.
- `affected_types()` returns deduplicated type IDs from all changes.
- Construct a `ConstraintViolation` with and without a subject.
- Object-safety assertion for `ConstraintValidator` compiles.

**Verify:** `cargo test -- constraint` passes.

---

## Phase 7: Inference Types (`src/inference/mod.rs`)

### 7.1 — Implement InferredFact enum

From `006-schema-extension-spec.md` §11.2:

```rust
#[derive(Clone, Debug)]
pub enum InferredFact {
    NewNode { type_labels: Vec<TypeId>, properties: PropertyMap, is_anonymous: bool },
    NewEdge { type_labels: Vec<TypeId>, source: NodeId, target: NodeId, properties: PropertyMap },
    NodePropertyUpdate { node: NodeId, key: PropertyKeyId, value: Value },
    EdgePropertyUpdate { edge: EdgeId, key: PropertyKeyId, value: Value },
    NodeTypeAssignment { node: NodeId, type_id: TypeId },
    EdgeTypeAssignment { edge: EdgeId, type_id: TypeId },
}
```

Add doc comments on the enum and every variant and field.

**⚠ Pitfall:** `InferredFact` does NOT derive `Eq` (contains `PropertyMap` which contains `Value` which contains `f64`).

**Verify:** `cargo check --no-default-features --features alloc`

### 7.2 — Implement InferenceResult struct

From `006-schema-extension-spec.md` §11.3:

```rust
#[derive(Clone, Debug)]
pub struct InferenceResult {
    pub facts: Vec<InferredFact>,
    pub rule_name: String,
}
```

Add doc comments.

**Verify:** `cargo check --no-default-features --features alloc`

### 7.3 — Implement InferenceMode enum

From `010-api-surface-spec.md` §6.3:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InferenceMode {
    Ephemeral,
    Materialized,
}
```

Add doc comments.

**Verify:** `cargo check --no-default-features --features alloc`

### 7.4 — Implement ProvenanceRecord and InferredEntity

From `011-inference-hook-design.md` §8.2:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenanceRecord {
    pub rule_name: String,
    pub materialized_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InferredEntity {
    Node(NodeId),
    Edge(EdgeId),
    NodeProperty { node: NodeId, key: PropertyKeyId },
    EdgeProperty { edge: EdgeId, key: PropertyKeyId },
    NodeType { node: NodeId, type_id: TypeId },
    EdgeType { edge: EdgeId, type_id: TypeId },
}
```

**⚠ Pitfall:** `InferredEntity` MUST derive `Eq, Ord, Hash` — it is used as a `BTreeMap` key in the provenance registry. This is safe because it does not contain `Value`.

Add doc comments.

**Verify:** `cargo check --no-default-features --features alloc`

### 7.5 — Implement MaterializedMapping struct

From `011-inference-hook-design.md` §7.3:

```rust
#[derive(Clone, Debug)]
pub struct MaterializedMapping {
    pub new_node_ids: Vec<(usize, NodeId)>,
    pub new_edge_ids: Vec<(usize, EdgeId)>,
}
```

The `usize` is the index into the original `InferenceResult::facts` vector.

Add doc comments explaining the index mapping.

**Verify:** `cargo check --no-default-features --features alloc`

### 7.6 — Implement InferenceRule trait

From `006-schema-extension-spec.md` §11.4:

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

Import `GraphView`, `TypeRegistryView`, `PropertyKeyRegistryView` from `crate::schema`.

Add full doc comments including the `# Lifecycle` and `# Thread Safety` sections from the spec.

**Verify:**
- `cargo check --no-default-features --features alloc`
- Compile-time assertion: `fn _assert_inference_rule_object_safe(_: &dyn InferenceRule) {}`

### 7.7 — Unit tests for inference types

Test:
- Construct every `InferredFact` variant.
- Construct `InferenceResult` with a non-empty facts list.
- `InferenceMode` equality.
- `ProvenanceRecord` equality.
- `InferredEntity` ordering: `Node(1) < Node(2)`, `Node(_) < Edge(_)` (derived `Ord` on enum is variant-order first, then field values).
- `InferredEntity` as `BTreeMap` key (construct a small map).
- `MaterializedMapping` construction.
- Object-safety assertion for `InferenceRule`.
- `Send + Sync` compile assertion for `InferenceRule`:
  ```rust
  fn _assert_send_sync<T: Send + Sync>() {}
  // In test: _assert_send_sync::<Box<dyn InferenceRule>>();
  ```

**Verify:** `cargo test -- inference` passes.

---

## Phase 8: Error Types (`src/error/mod.rs`)

### 8.1 — Implement inner error types

From `010-api-surface-spec.md` §4.2:

**SchemaError:**
```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaError {
    DuplicateTypeName { name: String, kind: TypeKind },
    TypeNotFound(TypeId),
    CycleDetected { child: TypeId, would_be_parent: TypeId },
    SupertypeNotFound(TypeId),
    KindMismatch { expected: TypeKind, found: TypeKind },
    DuplicatePropertyKey { name: String },
    PropertyKeyNotFound(PropertyKeyId),
}
```

**NotFoundError:**
```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotFoundError {
    Node(NodeId),
    Edge(EdgeId),
    Type(TypeId),
    PropertyKey(PropertyKeyId),
}
```

**StorageError:**
```rust
#[derive(Debug)]
pub struct StorageError {
    pub message: String,
    #[cfg(feature = "std")]
    pub source: Option<std::io::Error>,
}
```

**TransactionError:**
```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransactionError {
    ReadOnly,
    AlreadyFinished,
    WriteLockTimeout,
}
```

**InferenceError:**
```rust
#[derive(Clone, Debug)]
pub enum InferenceError {
    RuleNotFound(String),
    InvalidFact { rule_name: String, message: String },
}
```

Add doc comments on every type and variant.

**⚠ Pitfall — StorageError conditional compilation:** The `source` field only exists under `std`. Constructors and `Display` impl must handle both configurations.

**Verify:** `cargo check --no-default-features --features alloc` and `cargo check`.

### 8.2 — Implement Display for all error types

Implement `core::fmt::Display` for each error type:
- `SchemaError` — format a clear message for each variant.
- `NotFoundError` — e.g., `"Node with id 42 not found"`.
- `StorageError` — use the message field; under `std`, append source if present.
- `TransactionError` — human-readable message per variant.
- `InferenceError` — include rule name and message.

**⚠ Pitfall:** Use `core::fmt::Display`, not `std::fmt::Display`. They are the same trait, but importing from `core` works in `no_std`.

**Verify:** `cargo check --no-default-features --features alloc`

### 8.3 — Implement the top-level Error enum

```rust
#[derive(Debug)]
pub enum Error {
    Schema(SchemaError),
    ConstraintViolation(Vec<ConstraintViolation>),
    Storage(StorageError),
    NotFound(NotFoundError),
    Transaction(TransactionError),
    Inference(InferenceError),
}
```

Import `ConstraintViolation` from `crate::constraint`.

Implement `core::fmt::Display` for `Error` — delegate to the inner type's Display.

Implement `From<SchemaError>`, `From<NotFoundError>`, `From<StorageError>`, `From<TransactionError>`, `From<InferenceError>` for `Error`.

Do NOT implement `From<Vec<ConstraintViolation>>` — callers should explicitly construct `Error::ConstraintViolation(violations)` to avoid accidental conversion of empty violation lists into errors.

**Verify:** `cargo check --no-default-features --features alloc`

### 8.4 — Conditional std::error::Error implementations

Under `#[cfg(feature = "std")]`, implement `std::error::Error` for:
- `Error` (with `source()` delegating to inner types where applicable)
- `SchemaError`
- `NotFoundError`
- `StorageError` (with `source()` returning the wrapped `io::Error`)
- `TransactionError`
- `InferenceError`

**⚠ Pitfall:** `std::error::Error` requires `Display + Debug`. Both are already implemented from step 8.2.

**Verify:** `cargo check` (with default `std` feature)

### 8.5 — Unit tests for error types

Test:
- Construct every `SchemaError` variant.
- Construct every `NotFoundError` variant.
- Construct `StorageError` with a message (and with a source under `std`).
- `Display` output for each error type is human-readable and includes the relevant details.
- `From` conversions: `Error::from(SchemaError::TypeNotFound(TypeId(1)))` produces `Error::Schema(...)`.
- Under `std`, verify `std::error::Error::source()` returns the wrapped `io::Error` for `StorageError`.

**Verify:** `cargo test -- error` passes.

---

## Phase 9: Crate Root and Re-exports (`src/lib.rs`)

### 9.1 — Add public re-exports

In `src/lib.rs`, add re-exports so that common types are accessible from the crate root:

```rust
// Re-export primary types for convenience
pub use types::{
    NodeId, EdgeId, TypeId, PropertyKeyId,
    Value, ValueTypeDescriptor, PropertyMap,
    Node, Edge, TypeKind, TypeDefinition, PropertyDeclaration,
};
pub use schema::{GraphView, TypeRegistryView, PropertyKeyRegistryView};
pub use constraint::{
    ConstraintValidator, ChangeSet, NodeChange, EdgeChange,
    ConstraintViolation, ViolationSubject,
};
pub use inference::{
    InferenceRule, InferredFact, InferenceResult, InferenceMode,
    ProvenanceRecord, InferredEntity, MaterializedMapping,
};
pub use error::{Error, SchemaError, StorageError, NotFoundError, TransactionError, InferenceError};
```

Add crate-level doc comments (`//!`) at the top of `lib.rs`:
- Brief project description
- Architecture overview sentence
- Feature flag documentation (`std`, `alloc`)

**Verify:**
- `cargo check --no-default-features --features alloc`
- `cargo check`
- `cargo doc --no-deps` — no warnings.

### 9.2 — Add crate-level compile-time assertions

At the bottom of `lib.rs`, add:

```rust
#[cfg(test)]
mod compile_tests {
    use super::*;

    // Verify Send + Sync on Box<dyn ConstraintValidator>
    fn _assert_validator_send_sync(_: Box<dyn ConstraintValidator>) {}
    // Verify Send + Sync on Box<dyn InferenceRule>
    fn _assert_rule_send_sync(_: Box<dyn InferenceRule>) {}
    // Verify all trait objects are object-safe
    fn _assert_graph_view(_: &dyn GraphView) {}
    fn _assert_type_registry_view(_: &dyn TypeRegistryView) {}
    fn _assert_property_key_registry_view(_: &dyn PropertyKeyRegistryView) {}
}
```

**Verify:** `cargo test` compiles (the assertion functions are never called).

---

## Phase 10: Final Verification

### 10.1 — Full no_std verification

```
cargo check --no-default-features --features alloc
```

Must succeed with zero errors.

### 10.2 — Full std verification

```
cargo check
```

Must succeed with zero errors.

### 10.3 — Full test suite

```
cargo test
```

All tests pass, zero failures.

### 10.4 — Clippy

```
cargo clippy --all-targets --all-features -- -D warnings
```

Zero warnings.

### 10.5 — Documentation

```
cargo doc --no-deps
```

Zero warnings. Every `pub` item has a doc comment.

### 10.6 — Review against design documents

Manually verify:
- Every type in the module layout table in `CLAUDE.md` (project root) §Module Layout Reference is defined and in the correct module.
- Field names and types match `006-schema-extension-spec.md`.
- Error types match `010-api-surface-spec.md` §4.
- Inference types match `011-inference-hook-design.md` §7–8.
- Derive traits match the design:
  - ID types: `Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug`
  - Types containing `Value` (directly or indirectly): `Clone, Debug, PartialEq` — **NOT `Eq`**
  - Types NOT containing `Value`: may derive `Eq` where appropriate
  - `InferredEntity`: `Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash`

Document any intentional deviations from the spec in the completion report.

---

## Post-Completion

Produce a completion report following the format in the master project prompt's Instance Rules section. Include the verification evidence from Phase 10.
