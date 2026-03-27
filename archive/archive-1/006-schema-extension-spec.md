# 006 — Core Schema & Extension System Design Specification

**Project:** Embedded Graph Database with Extensible Schema & Pluggable Inference  
**Task:** 6 — Design: Core Schema & Extension System  
**Status:** Complete  
**Depends on:** Task 3 (`003-ontology-models-survey.md`)  
**Intended audience:** All downstream design and implementation tasks. A reader familiar with Rust and basic database concepts should be able to understand every type, trait, and decision in this document without reference to external sources. A reader coming from the ontology survey (Task 3) will recognize how every foundation requirement maps to a concrete design element.

---

## Table of Contents

1. [Purpose and Scope](#1-purpose-and-scope)
2. [Design Principles](#2-design-principles)
3. [Identity and ID System](#3-identity-and-id-system)
4. [Value Type System](#4-value-type-system)
5. [Property Bag Design](#5-property-bag-design)
6. [Node and Edge Data Model](#6-node-and-edge-data-model)
7. [Type Registry and Type Definitions](#7-type-registry-and-type-definitions)
8. [Type Hierarchy](#8-type-hierarchy)
9. [Property Key Registry](#9-property-key-registry)
10. [Constraint Validation System](#10-constraint-validation-system)
11. [Inference Hook System](#11-inference-hook-system)
12. [Extension Registration and Lifecycle](#12-extension-registration-and-lifecycle)
13. [Named Subgraphs](#13-named-subgraphs)
14. [Out of Scope for the Core Crate](#14-out-of-scope-for-the-core-crate)
15. [Validation Walkthrough: OWL Lite](#15-validation-walkthrough-owl-lite)
16. [Validation Walkthrough: SKOS](#16-validation-walkthrough-skos)
17. [Validation Walkthrough: Typed Property Graph (PG-Schema)](#17-validation-walkthrough-typed-property-graph-pg-schema)
18. [Validation Walkthrough: Frame-Based System](#18-validation-walkthrough-frame-based-system)
19. [Design Decision Log](#19-design-decision-log)

---

## 1. Purpose and Scope

This document is the authoritative specification for the **core schema system, constraint validation traits, and inference hook traits** of the embedded graph database crate. These are the extension points that downstream ontology systems build on.

### What this document defines

- The concrete Rust types for node/edge/property representation
- The type registry: how node types, edge types, and property type declarations are defined, stored, and queried
- The type hierarchy: DAG representation, traversal, and acyclicity enforcement
- The `ConstraintValidator` trait: interface, inputs, outputs, registration, lifecycle
- The `InferenceRule` trait: interface, inputs, outputs, registration, lifecycle
- The extension registration mechanism: how downstream code plugs into the system
- Design decisions and rationale for every choice

### What this document does NOT define

- On-disk storage layout (Task 7)
- File format (Task 8)
- HAL traits for I/O abstraction (Task 9)
- The full public API surface (Task 10 — though this document defines the types and traits that the API will expose)
- Detailed inference architecture including caching and invalidation (Task 11)
- Any domain-specific ontology semantics (downstream crates)

### Relationship to foundation requirements

Every design element in this document traces back to one or more requirements from `003-ontology-models-survey.md` Section 10. Requirements are cited as **[A1]**, **[B1]**, etc. throughout.

---

## 2. Design Principles

These principles guide every design decision in this document. When two concerns conflict, higher-numbered principles yield to lower-numbered ones.

1. **`no_std + alloc` compatibility.** All types and traits in this document must work in `no_std` environments with an allocator. No `std`-only types (`std::io::Error`, `std::path::Path`, etc.) appear in the core schema layer. We use `alloc::string::String`, `alloc::vec::Vec`, `alloc::collections::BTreeMap`, and `alloc::boxed::Box`.

2. **Zero built-in domain semantics.** The core ships with no ontology vocabulary. All types in the type registry are registered by user code. The core enforces only structural invariants (e.g., type hierarchy acyclicity, ID uniqueness) — never domain constraints.

3. **Extension via traits, not configuration.** Downstream code extends the system by implementing traits (`ConstraintValidator`, `InferenceRule`), not by passing configuration objects or DSL strings. This gives downstream code full Rust expressivity.

4. **Separation of storage from meaning.** The schema system stores type definitions, property declarations, and metadata. It does not interpret them. A property declaration that says "required: true" is just metadata — enforcement is done by a registered `ConstraintValidator`, not by the core.

5. **Predictability over magic.** Inference runs only on explicit request. Constraints run only at commit time (or explicit validation). No background processes, no triggers, no automatic schema enforcement.

6. **Minimal surface, maximum composability.** Traits have the fewest methods that are useful. Downstream code composes simple primitives into complex behavior.

---

## 3. Identity and ID System

### 3.1 Design

Every node and every edge in the graph has a unique, stable identifier. IDs are assigned by the database at creation time and never change.

```rust
/// A unique identifier for a node in the graph.
/// Internally a 64-bit unsigned integer, monotonically assigned.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct NodeId(pub u64);

/// A unique identifier for an edge in the graph.
/// Internally a 64-bit unsigned integer, monotonically assigned.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct EdgeId(pub u64);

/// A unique identifier for a registered type (node type or edge type).
/// Internally a 32-bit unsigned integer.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TypeId(pub u32);

/// A unique identifier for a registered property key.
/// Internally a 32-bit unsigned integer.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PropertyKeyId(pub u32);
```

### 3.2 Rationale

**[A1, A2]** — Nodes and edges need stable, unique IDs.

- **64-bit node/edge IDs:** Sufficient for billions of entities. Monotonic assignment is simple, deterministic, and plays well with B-tree indexes (sequential inserts are optimal).
- **32-bit type IDs:** Type registries are small (typically hundreds to low thousands of types). 32-bit is more than sufficient and saves space in fixed-size node/edge records where type IDs are stored inline.
- **32-bit property key IDs:** Same reasoning as type IDs. The property key registry is small.
- **Newtype wrappers:** Prevent accidental mixing of IDs across domains (a `NodeId` cannot be passed where an `EdgeId` is expected). Zero runtime cost.
- **Derive traits:** All IDs are `Copy`, `Eq`, `Ord`, `Hash` — essential for use as keys in indexes and collections.

### 3.3 Anonymous nodes

**[A3]** — Anonymous nodes (RDF blank nodes, CG generic referents) are represented as regular nodes with a normal `NodeId`. There is no structural distinction at the ID level. Instead, anonymous-ness is tracked as a boolean flag on the node record (Section 6). This keeps the ID system uniform while allowing downstream code to distinguish named from anonymous entities.

### 3.4 Reserved ID values

- `NodeId(0)` and `EdgeId(0)` are reserved as "null" sentinels — they never refer to a valid node or edge. This allows fixed-size records to use 0 as a "no reference" marker without requiring `Option<NodeId>` in hot paths.
- `TypeId(0)` is reserved as a "no type" sentinel for the same reason.
- `PropertyKeyId(0)` is reserved similarly.

---

## 4. Value Type System

### 4.1 Design

Property values in the graph are dynamically typed. The `Value` enum represents all supported value types.

```rust
use alloc::string::String;
use alloc::vec::Vec;

/// A dynamically-typed property value.
///
/// This is the value half of key-value pairs stored in node and edge
/// property bags, and also in type definition metadata.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// Null / absent value.
    Null,

    /// Boolean.
    Bool(bool),

    /// Signed 64-bit integer.
    I64(i64),

    /// Unsigned 64-bit integer.
    U64(u64),

    /// 64-bit IEEE 754 floating point.
    F64(f64),

    /// UTF-8 string.
    String(String),

    /// Arbitrary binary data.
    Bytes(Vec<u8>),

    /// A reference to another node (by ID).
    NodeRef(NodeId),

    /// A language-tagged string: (value, BCP 47 language tag).
    LangString { value: String, lang: String },

    /// A homogeneous list of values.
    List(Vec<Value>),
}
```

### 4.2 Value type descriptors

For schema declarations (property type declarations on types), we need a way to describe *expected* value types without holding actual values:

```rust
/// Describes the expected type of a property value in a schema declaration.
/// Used in property type declarations to specify what kind of value is expected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValueTypeDescriptor {
    /// Any value type is acceptable.
    Any,

    /// Must be a Bool.
    Bool,

    /// Must be an I64.
    I64,

    /// Must be a U64.
    U64,

    /// Must be a F64.
    F64,

    /// Must be a String (plain or language-tagged).
    String,

    /// Must be Bytes.
    Bytes,

    /// Must be a NodeRef.
    NodeRef,

    /// Must be a LangString.
    LangString,

    /// Must be a List whose elements match the inner descriptor.
    List(Box<ValueTypeDescriptor>),
}
```

### 4.3 Rationale

**[A4]** — The requirement specifies boolean, integers (64-bit), floats (64-bit), string, blob, node reference, list, and null. All are present.

**[A5]** — Language-tagged strings are a dedicated variant (`LangString`) rather than a convention over property-key namespacing. The rationale: a dedicated variant is self-describing (the language tag is always co-located with the string value), which simplifies both serialization and downstream SKOS/RDF code. It avoids the ambiguity of "is `name_en` a language tag or a property name?" conventions.

**Design decision — `F64` equality:** `Value` derives `PartialEq` but NOT `Eq` because `f64` does not implement `Eq` (NaN). This has a downstream implication: `Value` cannot be used as a `BTreeMap` key or `HashSet` element directly. This is acceptable — property *keys* are `PropertyKeyId` (integer), not `Value`.

**Design decision — no nested maps:** `Value` does not include a `Map(BTreeMap<String, Value>)` variant. Nested structured data is modeled as subgraphs (additional nodes with edges). This keeps serialization simple and avoids unbounded nesting in the value type.

---

## 5. Property Bag Design

### 5.1 Design

A property bag is an ordered map from `PropertyKeyId` to `Value`. Every node, every edge, and every type definition carries one.

```rust
use alloc::collections::BTreeMap;

/// An ordered map of property key IDs to values.
///
/// Used as the property bag for nodes, edges, and type definitions.
/// Backed by a BTreeMap for deterministic ordering by key ID —
/// this is important for serialization stability and comparison.
pub type PropertyMap = BTreeMap<PropertyKeyId, Value>;
```

### 5.2 Rationale

- **`BTreeMap` over `HashMap`:** `BTreeMap` is available in `alloc` (no `std` required) and provides deterministic iteration order. `HashMap` requires `std` or the `hashbrown` crate. Deterministic order is valuable for reproducible serialization and testing.
- **Keyed by `PropertyKeyId`, not `String`:** String keys are interned in the property key registry (Section 9). The property bag stores only compact integer keys. This saves memory and makes comparison/lookup fast. **[B5]**

---

## 6. Node and Edge Data Model

### 6.1 Node

```rust
/// A node in the graph.
///
/// A node has a stable identity, zero or more type labels, a property bag,
/// and metadata flags.
pub struct Node {
    /// The unique, stable identifier for this node.
    pub id: NodeId,

    /// The set of type labels assigned to this node.
    /// A node may have zero types (untyped) or multiple types.
    /// Stored as a sorted Vec for compact representation and
    /// efficient iteration. Empty is valid.
    pub type_labels: Vec<TypeId>,

    /// The property bag: named, typed values attached to this node.
    pub properties: PropertyMap,

    /// Whether this node is anonymous (blank node / internal-only).
    /// Anonymous nodes have no user-assigned external name.
    pub is_anonymous: bool,
}
```

### 6.2 Edge

```rust
/// A directed edge in the graph.
///
/// An edge connects a source node to a target node, has a type label,
/// and carries its own property bag. Multiple parallel edges between
/// the same source and target are permitted (multi-graph).
pub struct Edge {
    /// The unique, stable identifier for this edge.
    pub id: EdgeId,

    /// The type labels assigned to this edge.
    /// Typically exactly one for property-graph-style usage.
    /// Multiple types are permitted for RDF/OWL compatibility.
    pub type_labels: Vec<TypeId>,

    /// The source (origin) node of this directed edge.
    pub source: NodeId,

    /// The target (destination) node of this directed edge.
    pub target: NodeId,

    /// The property bag: named, typed values attached to this edge.
    pub properties: PropertyMap,
}
```

### 6.3 Rationale

**[A1]** — Nodes have a stable unique ID, one or more type labels, and a mutable property bag. ✓

**[A2]** — Edges have a stable unique ID, one or more type labels, source and target node IDs, and a mutable property bag. Parallel edges are permitted by the model (nothing prevents two edges with the same source, target, and type). ✓

**[A3]** — Anonymous nodes are distinguished by the `is_anonymous` flag, not by a separate type. This keeps the node data path uniform. ✓

**Design decision — `Vec<TypeId>` for type labels:** Multiple type labels are stored as a sorted `Vec<TypeId>`. We chose `Vec` over `BTreeSet` because type label sets are small (typically 1–5 elements), making `Vec` more cache-friendly and lower overhead. Sorted order enables binary search for membership checks and deterministic serialization.

**Design decision — type labels on edges:** The survey found that property graphs traditionally use exactly one type per edge, while RDF/OWL edges can have multiple types. We allow multiple types on edges for generality. Downstream property-graph code can enforce single-type-per-edge as a constraint if desired. **[P5]**

**Design decision — no `is_inferred` flag on Node/Edge:** Inferred facts are tracked separately by the inference system (Section 11), not as a per-record flag. This avoids polluting the core data model with inference concerns. Task 11 will design the detailed tracking mechanism.

---

## 7. Type Registry and Type Definitions

### 7.1 Overview

The type registry is a persistent, in-database store of all registered types: node types, edge types, and their associated property declarations. It is loaded at database open time, cached in memory, and updated transactionally.

**[B1, B3, B4]** — The registry is persistent, extensible at runtime, and stores node types, edge types, and property type declarations.

### 7.2 TypeKind

```rust
/// Whether a type definition describes nodes or edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeKind {
    /// This type classifies nodes.
    Node,

    /// This type classifies edges.
    Edge,
}
```

### 7.3 PropertyDeclaration

A property declaration describes what properties instances of a type are expected to carry. Declarations are metadata — the core stores them but does not enforce them.

```rust
/// A declaration of a property that instances of a type are expected to carry.
///
/// This is metadata stored in the type definition. The core does not enforce
/// these declarations — enforcement is done by registered ConstraintValidators.
#[derive(Clone, Debug, PartialEq)]
pub struct PropertyDeclaration {
    /// The property key (interned in the property key registry).
    pub key: PropertyKeyId,

    /// The expected value type.
    pub value_type: ValueTypeDescriptor,

    /// Whether this property is required on instances of the type.
    /// The core stores this flag; a downstream ConstraintValidator enforces it.
    pub required: bool,

    /// Whether this property can hold multiple values (a list) or only one.
    pub multi_valued: bool,

    /// Arbitrary metadata on this declaration (e.g., default values,
    /// display hints, facet information for frame-based systems).
    pub metadata: PropertyMap,
}
```

### 7.4 TypeDefinition

```rust
/// A registered type definition in the type registry.
///
/// Type definitions describe the schema for nodes or edges. They include
/// the type's name, its position in the type hierarchy, its property
/// declarations, and arbitrary metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct TypeDefinition {
    /// The unique type identifier, assigned at registration time.
    pub id: TypeId,

    /// The human-readable name of this type.
    /// Must be unique within its TypeKind (node types and edge types
    /// have separate name namespaces).
    pub name: String,

    /// Whether this type classifies nodes or edges.
    pub kind: TypeKind,

    /// The direct supertypes of this type (parent types in the DAG).
    /// Empty for root types.
    pub supertypes: Vec<TypeId>,

    /// Property declarations: what properties instances of this type
    /// are expected to carry.
    pub property_declarations: Vec<PropertyDeclaration>,

    /// Whether this type is "open" (instances may carry undeclared
    /// properties/labels) or "closed" (instances must conform exactly
    /// to the declared schema).
    /// Default: Open.
    /// The core stores this flag; enforcement is done by a downstream
    /// ConstraintValidator.
    pub open: bool,

    /// Arbitrary metadata on this type definition.
    /// Used for: rdfs:label, rdfs:comment, owl:versionInfo, display
    /// hints, application-specific annotations, canonical graph
    /// references, etc.
    pub metadata: PropertyMap,
}
```

### 7.5 Edge type endpoint constraints (metadata, not enforced)

For edge types, downstream code often wants to declare allowed source and target node types. Rather than adding dedicated fields to `TypeDefinition`, these are stored as metadata properties using well-known property keys:

```rust
// Well-known property key names (registered in the property key registry
// by downstream code or by a setup helper):
//
// "__allowed_source_types" → Value::List(vec![Value::U64(type_id), ...])
// "__allowed_target_types" → Value::List(vec![Value::U64(type_id), ...])
//
// These are conventions, not enforced by the core. A ConstraintValidator
// reads them from the edge type's metadata to enforce source/target type
// constraints.
```

**Rationale for metadata-based approach:** Putting source/target constraints in the metadata (rather than as first-class fields) keeps `TypeDefinition` uniform across node types and edge types. It also means the core doesn't need to understand what "allowed source type" means — that's downstream semantics. This aligns with Design Principle #2 (zero built-in domain semantics) and Principle #4 (separation of storage from meaning). **[B1]**

### 7.6 TypeRegistry trait

The type registry is accessed through a trait, allowing different backing implementations (persistent B-tree, in-memory map, etc.):

```rust
/// Errors that can occur during type registry operations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeRegistryError {
    /// A type with this name already exists in the same TypeKind namespace.
    DuplicateName { name: String, kind: TypeKind },

    /// The specified type ID does not exist in the registry.
    TypeNotFound(TypeId),

    /// Adding this supertype would create a cycle in the type hierarchy.
    CycleDetected { child: TypeId, would_be_parent: TypeId },

    /// One or more referenced supertypes do not exist.
    SupertypeNotFound(TypeId),

    /// The supertypes reference a type of the wrong kind
    /// (e.g., a node type listing an edge type as a supertype).
    KindMismatch { expected: TypeKind, found: TypeKind },

    /// A storage-layer error occurred.
    StorageError(String),
}

/// Read-only access to the type registry.
///
/// This trait is implemented by the database's type registry and is
/// provided to ConstraintValidators and InferenceRules as part of
/// their execution context.
pub trait TypeRegistryView {
    /// Look up a type definition by its ID.
    fn get_type(&self, id: TypeId) -> Option<&TypeDefinition>;

    /// Look up a type definition by name and kind.
    fn get_type_by_name(&self, name: &str, kind: TypeKind) -> Option<&TypeDefinition>;

    /// Return all registered type definitions.
    fn all_types(&self) -> &[TypeDefinition];

    /// Return all registered type definitions of a given kind.
    fn types_by_kind(&self, kind: TypeKind) -> Vec<&TypeDefinition>;

    /// Return the direct supertypes of a type.
    fn direct_supertypes(&self, id: TypeId) -> Option<&[TypeId]>;

    /// Return all transitive supertypes of a type (ancestors in the DAG),
    /// in topological order (immediate parents first, root types last).
    fn all_supertypes(&self, id: TypeId) -> Vec<TypeId>;

    /// Return all direct subtypes of a type.
    fn direct_subtypes(&self, id: TypeId) -> Vec<TypeId>;

    /// Return all transitive subtypes of a type (descendants in the DAG).
    fn all_subtypes(&self, id: TypeId) -> Vec<TypeId>;

    /// Check if `candidate` is equal to or a subtype of `ancestor`.
    fn is_subtype_of(&self, candidate: TypeId, ancestor: TypeId) -> bool;

    /// Collect all property declarations for a type, including
    /// inherited declarations from all supertypes.
    /// Declarations from subtypes shadow declarations from supertypes
    /// when they share the same property key.
    fn effective_property_declarations(
        &self,
        id: TypeId,
    ) -> Vec<PropertyDeclaration>;
}
```

### 7.7 Rationale and design decisions

**[B2]** — The type hierarchy is a DAG with multiple supertypes allowed. Acyclicity is enforced by the registry at registration time (see Section 8).

**[B6]** — The open/closed flag is stored on the type definition. The core stores it; downstream code enforces it via a `ConstraintValidator`.

**[B7]** — The `TypeRegistryView` provides `all_supertypes`, `all_subtypes`, `direct_supertypes`, `direct_subtypes`, `is_subtype_of`, and `effective_property_declarations`. This fully satisfies the hierarchy traversal requirement.

**Design decision — `effective_property_declarations` with shadowing:** When a subtype declares a property with the same key as a supertype, the subtype's declaration takes precedence. This mirrors frame-based inheritance (a subframe can override a parent's slot definition). Without this rule, multiple inheritance could produce ambiguous property declarations.

**Design decision — separate name namespaces for node types and edge types:** A node type named "Person" and an edge type named "Person" can coexist. This avoids artificial naming conflicts and matches how RDF/OWL work (a URI can be both a class and a property in different contexts).

**Design decision — `all_types() -> &[TypeDefinition]`:** Returning a slice reference assumes the registry stores types contiguously. This is an implementation hint — the in-memory type registry cache will use a `Vec<TypeDefinition>` indexed by `TypeId`. If an implementation needs a different backing structure, it can return a temporary allocation. The trait returns `&[TypeDefinition]` for the common case.

---

## 8. Type Hierarchy

### 8.1 DAG structure

Type hierarchy is encoded in the `supertypes` field of each `TypeDefinition`. The hierarchy must form a DAG — no cycles.

### 8.2 Acyclicity enforcement

When a new type is registered or an existing type's supertypes are modified, the type registry performs a **cycle check**: starting from the proposed supertypes, it walks the entire ancestor chain. If the new type's own ID appears among its ancestors, the operation is rejected with `TypeRegistryError::CycleDetected`.

```
Algorithm: Cycle detection on type registration
Input: new TypeDefinition T with supertypes S1, S2, ...

1. For each Si in T.supertypes:
   a. If Si == T.id → reject (direct self-loop)
   b. Compute all_supertypes(Si)
   c. If T.id is in all_supertypes(Si) → reject (cycle)
2. Accept registration.
```

This is an O(|V|) walk in the worst case, where |V| is the number of types in the registry. Since type registries are small (typically hundreds of types), this is fast.

### 8.3 Multiple inheritance

Multiple supertypes are allowed without restriction. Diamond inheritance (type C has supertypes A and B, both of which have supertype X) is permitted. The `effective_property_declarations` method handles this by collecting declarations in breadth-first topological order and applying the shadowing rule (Section 7.7).

### 8.4 Type hierarchy modification after data exists

Modifying a type's supertypes after nodes/edges of that type exist is permitted by the schema system. The core does not automatically revalidate existing data — that is the responsibility of downstream code (which can run a `ConstraintValidator` or dry-run validation after a schema change). This matches Design Principle #5 (predictability over magic).

**[B4]** — New types can be registered at runtime. Existing type definitions can be modified (e.g., adding supertypes) within a transaction.

---

## 9. Property Key Registry

### 9.1 Design

The property key registry is a bidirectional map between human-readable property key names (strings) and compact `PropertyKeyId` values. It is shared across all types — the same key name always maps to the same key ID.

```rust
/// Read-only access to the property key registry.
pub trait PropertyKeyRegistryView {
    /// Look up a property key ID by name. Returns None if unregistered.
    fn get_key_id(&self, name: &str) -> Option<PropertyKeyId>;

    /// Look up a property key name by ID. Returns None if unregistered.
    fn get_key_name(&self, id: PropertyKeyId) -> Option<&str>;

    /// Return all registered (name, id) pairs.
    fn all_keys(&self) -> Vec<(PropertyKeyId, &str)>;
}
```

### 9.2 Registration

Property keys are registered implicitly (on first use) or explicitly (via a registration API). When a property bag is constructed with a string key name, the registry intern the name and returns a `PropertyKeyId`. Once assigned, a key ID never changes.

### 9.3 Persistence

The property key registry is persisted as part of the database (alongside the type registry). It is loaded into memory at database open time. **[B5]**

### 9.4 Rationale

Interning property key names into integer IDs serves three purposes:
1. **Space savings:** Property bags store 4-byte key IDs instead of variable-length strings. Since the same key appears on many nodes/edges, this is a significant saving.
2. **Fast comparison:** Integer comparison is O(1) vs. string comparison O(n).
3. **Stable serialization:** Key IDs are stable across database sessions, enabling efficient on-disk property encoding.

---

## 10. Constraint Validation System

### 10.1 Overview

The constraint system allows downstream code to register validators that run at transaction commit time. Validators receive a read-only view of the transaction's changes and the current database state, and return a list of violations (or empty for success). If any validator reports violations, the transaction is rejected.

**[C1, C2, C3, C5]** — Trait-based, multiple validators, entirely downstream-implemented.

### 10.2 Change set representation

Validators need to know what changed in the current transaction. The change set captures inserts, updates, and deletes.

```rust
/// A single change to a node within a transaction.
#[derive(Clone, Debug)]
pub enum NodeChange {
    /// A new node was inserted.
    Inserted(Node),

    /// An existing node was modified. Contains the node state
    /// before and after the modification.
    Modified { before: Node, after: Node },

    /// A node was deleted. Contains the node as it was before deletion.
    Deleted(Node),
}

/// A single change to an edge within a transaction.
#[derive(Clone, Debug)]
pub enum EdgeChange {
    /// A new edge was inserted.
    Inserted(Edge),

    /// An existing edge was modified.
    Modified { before: Edge, after: Edge },

    /// An edge was deleted.
    Deleted(Edge),
}

/// The complete set of changes in a transaction, provided to validators.
pub struct ChangeSet<'a> {
    /// All node changes in this transaction, in the order they occurred.
    node_changes: &'a [NodeChange],

    /// All edge changes in this transaction, in the order they occurred.
    edge_changes: &'a [EdgeChange],
}

impl<'a> ChangeSet<'a> {
    /// All node changes.
    pub fn node_changes(&self) -> &[NodeChange] {
        self.node_changes
    }

    /// All edge changes.
    pub fn edge_changes(&self) -> &[EdgeChange] {
        self.edge_changes
    }

    /// All inserted nodes in this transaction.
    pub fn inserted_nodes(&self) -> impl Iterator<Item = &Node> {
        self.node_changes.iter().filter_map(|c| match c {
            NodeChange::Inserted(n) => Some(n),
            _ => None,
        })
    }

    /// All modified nodes in this transaction.
    pub fn modified_nodes(&self) -> impl Iterator<Item = (&Node, &Node)> {
        self.node_changes.iter().filter_map(|c| match c {
            NodeChange::Modified { before, after } => Some((before, after)),
            _ => None,
        })
    }

    /// All deleted nodes in this transaction.
    pub fn deleted_nodes(&self) -> impl Iterator<Item = &Node> {
        self.node_changes.iter().filter_map(|c| match c {
            NodeChange::Deleted(n) => Some(n),
            _ => None,
        })
    }

    /// All inserted edges in this transaction.
    pub fn inserted_edges(&self) -> impl Iterator<Item = &Edge> {
        self.edge_changes.iter().filter_map(|c| match c {
            EdgeChange::Inserted(e) => Some(e),
            _ => None,
        })
    }

    /// All modified edges in this transaction.
    pub fn modified_edges(&self) -> impl Iterator<Item = (&Edge, &Edge)> {
        self.edge_changes.iter().filter_map(|c| match c {
            EdgeChange::Modified { before, after } => Some((before, after)),
            _ => None,
        })
    }

    /// All deleted edges in this transaction.
    pub fn deleted_edges(&self) -> impl Iterator<Item = &Edge> {
        self.edge_changes.iter().filter_map(|c| match c {
            EdgeChange::Deleted(e) => Some(e),
            _ => None,
        })
    }

    /// Returns the set of TypeIds touched by this transaction
    /// (union of all type labels on changed nodes and edges).
    pub fn affected_types(&self) -> Vec<TypeId> {
        // Implementation collects unique TypeIds from all changes.
        // Omitted for brevity; the contract is what matters.
        todo!()
    }
}
```

### 10.3 Database snapshot view for validators

Validators also need read-only access to the full database state (not just the changes) for cross-referencing. This is provided through a read-only graph view trait:

```rust
/// A read-only view of the graph database, provided to ConstraintValidators
/// and InferenceRules.
///
/// This view reflects the state of the database as it will be after
/// the current transaction commits (i.e., pending changes are visible).
pub trait GraphView {
    /// Look up a node by its ID.
    fn get_node(&self, id: NodeId) -> Option<&Node>;

    /// Look up an edge by its ID.
    fn get_edge(&self, id: EdgeId) -> Option<&Edge>;

    /// Return all outgoing edges from a node, optionally filtered by edge type.
    fn outgoing_edges(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Vec<&Edge>;

    /// Return all incoming edges to a node, optionally filtered by edge type.
    fn incoming_edges(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Vec<&Edge>;

    /// Return all nodes with a given type label.
    /// If `include_subtypes` is true, also returns nodes whose type
    /// is a subtype of the given type.
    fn nodes_by_type(
        &self,
        type_id: TypeId,
        include_subtypes: bool,
    ) -> Vec<&Node>;

    /// Return all edges with a given type label.
    fn edges_by_type(
        &self,
        type_id: TypeId,
        include_subtypes: bool,
    ) -> Vec<&Edge>;

    /// Find nodes that have a specific property key-value pair.
    /// This may perform a full scan if no secondary index exists.
    fn nodes_by_property(
        &self,
        key: PropertyKeyId,
        value: &Value,
    ) -> Vec<&Node>;
}
```

### 10.4 Constraint violation representation

```rust
/// A single constraint violation detected by a ConstraintValidator.
#[derive(Clone, Debug)]
pub struct ConstraintViolation {
    /// A machine-readable identifier for the type of violation
    /// (defined by the downstream validator, not the core).
    pub violation_kind: String,

    /// A human-readable description of the violation.
    pub message: String,

    /// The node or edge that violated the constraint, if applicable.
    pub subject: Option<ViolationSubject>,
}

/// Identifies the graph element that a constraint violation applies to.
#[derive(Clone, Debug)]
pub enum ViolationSubject {
    Node(NodeId),
    Edge(EdgeId),
    Type(TypeId),
}
```

### 10.5 The ConstraintValidator trait

```rust
/// A trait implemented by downstream code to define custom constraints.
///
/// Validators are registered with the database and called at transaction
/// commit time (or on explicit validation request). If any validator
/// returns violations, the transaction is rejected.
///
/// # Lifecycle
///
/// 1. Downstream code creates a struct implementing `ConstraintValidator`.
/// 2. The struct is registered with the database via the extension
///    registration API.
/// 3. On each transaction commit, the database calls `validate()` on
///    all registered validators (in registration order).
/// 4. If any validator returns a non-empty list of violations, the
///    commit is aborted and the violations are returned to the caller.
///
/// # Thread Safety
///
/// Validators must be `Send + Sync` because the database may call them
/// from any thread. Validators must not hold mutable state — they
/// receive all context through their method parameters.
pub trait ConstraintValidator: Send + Sync {
    /// A unique name for this validator, used in error messages
    /// and for registration management.
    fn name(&self) -> &str;

    /// The set of type IDs that this validator is interested in.
    /// If this returns `Some(set)`, the validator is only called when
    /// the transaction's change set includes entities with at least
    /// one of the listed types. If this returns `None`, the validator
    /// is called on every transaction.
    ///
    /// This is a performance optimization — it allows the database
    /// to skip calling validators that are irrelevant to a given
    /// transaction.
    fn applies_to_types(&self) -> Option<Vec<TypeId>>;

    /// Validate the current transaction.
    ///
    /// `changes` — the set of inserts, modifications, and deletes
    ///   in this transaction.
    /// `graph` — a read-only view of the database state (with
    ///   pending changes applied).
    /// `types` — read-only access to the type registry.
    /// `keys` — read-only access to the property key registry.
    ///
    /// Returns an empty Vec if validation passes, or a list of
    /// violations if it fails.
    fn validate(
        &self,
        changes: &ChangeSet<'_>,
        graph: &dyn GraphView,
        types: &dyn TypeRegistryView,
        keys: &dyn PropertyKeyRegistryView,
    ) -> Vec<ConstraintViolation>;
}
```

### 10.6 Design decisions and rationale

**[C2]** — The validator receives: change set (inserts, deletes, modifications), database state (via `GraphView`), type definitions (via `TypeRegistryView`), and property key names (via `PropertyKeyRegistryView`). Returns a list of violations. ✓

**[C3]** — Multiple validators run in registration order. All must pass for the transaction to commit. ✓

**[C4]** — `applies_to_types()` enables per-type scoping. If the change set's `affected_types()` has no overlap with a validator's `applies_to_types()`, the validator is skipped. ✓

**[C5]** — No built-in constraints. The core ships with zero `ConstraintValidator` implementations. ✓ (Type hierarchy acyclicity is enforced in the type registry itself, not as a `ConstraintValidator`.)

**[C6]** — Dry-run validation is supported by calling the same `validate()` method with a synthetic or empty `ChangeSet` and the current database state. The API surface (Task 10) will expose a dedicated `validate_all()` method.

**Design decision — `Send + Sync` requirement:** Validators are trait objects stored in the database's extension registry. Since the database supports multi-threaded access, validators must be callable from any thread. This rules out validators that hold interior mutability without synchronization — which is intentional. Validators should be pure functions of their inputs.

**Design decision — `&dyn` trait objects:** Validators receive their context as `&dyn TraitName` references, not concrete types. This keeps the trait decoupled from the database's internal implementation. The downside is one layer of dynamic dispatch on context access — acceptable for commit-time validation, which is not a hot path.

**Design decision — violations as `Vec<ConstraintViolation>` (not `Result`):** A validator can return *multiple* violations in one call. Using `Result<(), Vec<ConstraintViolation>>` would work too, but `Vec` (empty = success) is simpler and avoids the question of "what does `Ok(())` with a non-empty violations list mean?"

---

## 11. Inference Hook System

### 11.1 Overview

The inference system allows downstream code to register rules that derive new facts from existing facts. Inference runs **only when explicitly requested** by the caller — never automatically. The system supports two modes: materialized (inferred facts written to the graph) and ephemeral (inferred facts returned as an in-memory result set).

**[D1, D2, D6]** — Trait-based, explicit triggering only, isolated from constraints.

### 11.2 Inferred fact representation

```rust
/// A fact derived by an inference rule.
///
/// Inferred facts can be new nodes, new edges, or new property assignments
/// on existing nodes/edges.
#[derive(Clone, Debug)]
pub enum InferredFact {
    /// A new node to add to the graph.
    NewNode {
        /// Type labels for the new node.
        type_labels: Vec<TypeId>,
        /// Properties for the new node.
        properties: PropertyMap,
        /// Whether this node is anonymous.
        is_anonymous: bool,
    },

    /// A new edge to add to the graph.
    NewEdge {
        /// Type labels for the new edge.
        type_labels: Vec<TypeId>,
        /// Source node.
        source: NodeId,
        /// Target node.
        target: NodeId,
        /// Properties for the new edge.
        properties: PropertyMap,
    },

    /// A property to add or update on an existing node.
    NodePropertyUpdate {
        /// The node to update.
        node: NodeId,
        /// The property key.
        key: PropertyKeyId,
        /// The new value.
        value: Value,
    },

    /// A property to add or update on an existing edge.
    EdgePropertyUpdate {
        /// The edge to update.
        edge: EdgeId,
        /// The property key.
        key: PropertyKeyId,
        /// The new value.
        value: Value,
    },

    /// A type label to add to an existing node.
    NodeTypeAssignment {
        /// The node to update.
        node: NodeId,
        /// The type label to add.
        type_id: TypeId,
    },

    /// A type label to add to an existing edge.
    EdgeTypeAssignment {
        /// The edge to update.
        edge: EdgeId,
        /// The type label to add.
        type_id: TypeId,
    },
}
```

### 11.3 Inference result

```rust
/// The result of running one or more inference rules.
///
/// Contains the set of inferred facts, grouped by the rule that
/// produced them.
#[derive(Clone, Debug)]
pub struct InferenceResult {
    /// The inferred facts, in the order they were produced.
    pub facts: Vec<InferredFact>,

    /// The name of the inference rule that produced these facts.
    pub rule_name: String,
}
```

### 11.4 The InferenceRule trait

```rust
/// A trait implemented by downstream code to define custom inference rules.
///
/// Rules are registered with the database and invoked **only when the
/// caller explicitly requests inference**. There is no automatic
/// background inference.
///
/// # Lifecycle
///
/// 1. Downstream code creates a struct implementing `InferenceRule`.
/// 2. The struct is registered with the database via the extension
///    registration API.
/// 3. The caller explicitly requests inference (either all rules or
///    a specific named rule).
/// 4. The database calls `infer()` on the requested rule(s).
/// 5. The caller chooses whether to materialize the results (write
///    inferred facts to the graph) or treat them as ephemeral.
///
/// # Thread Safety
///
/// Rules must be `Send + Sync` for the same reasons as ConstraintValidators.
pub trait InferenceRule: Send + Sync {
    /// A unique name for this rule, used for selective invocation
    /// and for labeling inferred facts.
    fn name(&self) -> &str;

    /// The set of type IDs that this rule operates over.
    /// If this returns `Some(set)`, the rule is only relevant when
    /// the database contains entities of at least one of the listed
    /// types. If this returns `None`, the rule potentially applies
    /// to the entire graph.
    ///
    /// This is advisory — it helps the caller decide which rules to
    /// invoke and enables future optimizations.
    fn applies_to_types(&self) -> Option<Vec<TypeId>>;

    /// Run inference and produce a set of derived facts.
    ///
    /// `graph` — a read-only view of the current database state.
    /// `types` — read-only access to the type registry.
    /// `keys` — read-only access to the property key registry.
    ///
    /// Returns a set of inferred facts. The caller decides whether
    /// to materialize them or treat them as ephemeral.
    fn infer(
        &self,
        graph: &dyn GraphView,
        types: &dyn TypeRegistryView,
        keys: &dyn PropertyKeyRegistryView,
    ) -> InferenceResult;
}
```

### 11.5 Design decisions and rationale

**[D3]** — The rule receives: read-only graph view, type definitions, property key names. Returns inferred facts. ✓

**[D4]** — Two modes (materialized and ephemeral) are supported. The mode is chosen by the *caller* at invocation time, not by the rule itself. This is a deliberate design choice: the same rule might be run in materialized mode during a batch process and in ephemeral mode during an interactive query.

**[D5]** — Dependency tracking and invalidation of materialized facts is deferred to Task 11 (Inference Hook Architecture). This document defines the core trait; Task 11 will add the caching, invalidation, and dependency tracking infrastructure on top.

**[D7]** — `applies_to_types()` enables rule scoping, mirroring the constraint system's approach. ✓

**Design decision — `InferenceRule::infer()` receives a `&dyn GraphView`, not a `ChangeSet`:** Inference rules reason over the full graph state, not incremental changes. Incremental inference (only processing what changed) is an optimization that Task 11 can layer on top — the base trait operates on the full graph for simplicity and correctness.

**Design decision — `InferenceResult` includes `rule_name`:** When multiple rules are run, the caller needs to know which rule produced which facts. This is essential for selective materialization and for debugging.

**Design decision — inferred facts can include new nodes:** Some inference rules derive the existence of new entities (e.g., OWL anonymous individuals, CG canonical graph expansion). `InferredFact::NewNode` supports this. The database assigns a new `NodeId` when materializing.

---

## 12. Extension Registration and Lifecycle

### 12.1 Overview

The extension registry is the central point where downstream code registers its `ConstraintValidator` and `InferenceRule` implementations. The registry is managed by the database and has a well-defined lifecycle.

### 12.2 ExtensionRegistry trait

```rust
/// The extension registry: manages registered constraint validators
/// and inference rules.
///
/// Extensions are registered in a write transaction and become active
/// immediately. They persist across database sessions because the
/// database records which extensions are registered (by name) in its
/// metadata — but the trait objects themselves must be re-registered
/// by the application at startup.
///
/// This is a deliberate design choice: extension *logic* (the trait
/// object) lives in application code and cannot be serialized to disk.
/// The database records extension *names* so that it can warn at
/// startup if expected extensions are missing.
pub trait ExtensionRegistry {
    /// Register a constraint validator.
    ///
    /// The validator's `name()` must be unique among all registered
    /// validators. If a validator with the same name is already
    /// registered, this call replaces it.
    fn register_constraint(
        &mut self,
        validator: Box<dyn ConstraintValidator>,
    );

    /// Unregister a constraint validator by name.
    /// Returns true if a validator with that name was found and removed.
    fn unregister_constraint(&mut self, name: &str) -> bool;

    /// Return the names of all registered constraint validators.
    fn constraint_names(&self) -> Vec<&str>;

    /// Register an inference rule.
    ///
    /// The rule's `name()` must be unique among all registered rules.
    /// If a rule with the same name is already registered, this call
    /// replaces it.
    fn register_inference_rule(
        &mut self,
        rule: Box<dyn InferenceRule>,
    );

    /// Unregister an inference rule by name.
    /// Returns true if a rule with that name was found and removed.
    fn unregister_inference_rule(&mut self, name: &str) -> bool;

    /// Return the names of all registered inference rules.
    fn inference_rule_names(&self) -> Vec<&str>;
}
```

### 12.3 Lifecycle

1. **Database opens.** The database reads its persisted metadata, which includes the names of previously-registered extensions.

2. **Application registers extensions.** The application calls `register_constraint()` and `register_inference_rule()` for each extension it uses. These calls can happen at any time (not just startup), but typically happen during initialization.

3. **Startup check (advisory).** After registration, the application can compare the set of registered extension names against the set of names persisted in the database metadata. If any expected extensions are missing, the database logs a warning (or the application can treat it as an error). This catches the case where a database was created with Extension X but the current application version has removed it.

4. **Normal operation.** Constraint validators run at commit time. Inference rules run on explicit request.

5. **Unregistration.** Extensions can be unregistered at any time. An unregistered validator no longer runs at commit time. An unregistered inference rule can no longer be invoked.

6. **Database closes.** The database persists the current set of registered extension names in its metadata.

### 12.4 Rationale

**Design decision — extensions are trait objects, not serialized:** It's tempting to try to serialize constraint/inference logic to disk so that extensions "just work" when reopening a database. But serializing arbitrary Rust logic is impractical (no reflection, no runtime code loading in safe Rust). Instead, the database persists only extension *names*. The application must re-register the same extensions at startup. This is the pattern used by SQLite (custom functions must be re-registered each session) and redb (custom comparators).

**Design decision — replacement on duplicate name:** Registering a validator with the same name as an existing one replaces the old one. This simplifies upgrades — when an application is updated with a new version of a validator, it just registers it under the same name. The alternative (rejecting duplicates) would force applications to unregister-then-register, which is error-prone.

**Design decision — `Box<dyn Trait>` for registration:** Using boxed trait objects allows any struct implementing the trait to be registered. This is the standard Rust pattern for type-erased extension points. The `Send + Sync` bounds on the traits ensure thread safety.

---

## 13. Named Subgraphs

### 13.1 Design

**[F3]** — The foundation should support named subgraphs for OWL ontology grouping, RDF named graphs, and Topic Maps scope.

Named subgraphs are implemented as a **convention over the existing data model**, not as a first-class structural concept:

1. A "subgraph context" is a **regular node** with a designated type (e.g., a type named `__SubgraphContext` or any user-chosen name).
2. Membership of a node or edge in a subgraph is represented by an **edge** from the node/edge-as-node to the subgraph context node. (For edge membership, a proxy node representing the edge is used, since edges cannot be endpoints of other edges.)

Alternatively, downstream code can use a **property-based convention**: a well-known property key (e.g., `__subgraph`) on nodes and edges whose value is a `NodeRef` pointing to the context node.

### 13.2 Rationale

**Why not first-class subgraphs:** Adding a `subgraph: Option<NodeId>` field to every node and edge record would waste space in the common case (most use cases don't need subgraphs) and complicate the core data model. A convention over existing primitives (typed nodes + typed edges or properties) is sufficient and keeps the core simple.

**Why not a reserved property key:** Reserving a specific property key in the core would violate Design Principle #2 (zero built-in semantics). The convention is documented here for downstream crates to follow, but the core crate does not special-case it.

Task 3 residual concern #2 asked us to decide the exact mechanism. The decision is: **property-based convention with a `NodeRef` value pointing to a context node.** This is the simplest encoding, requires no structural changes to nodes/edges, and is compatible with all downstream models.

---

## 14. Out of Scope for the Core Crate

This section explicitly lists things that are **not** part of the core schema and extension system. These items are either downstream concerns or belong to other design tasks.

### 14.1 Domain-specific types and semantics

| Excluded | Reason | Where it belongs |
|----------|--------|-----------------|
| `rdf:type`, `rdfs:subClassOf`, `owl:Class`, `skos:Concept` | Domain-specific vocabulary | Downstream ontology crate |
| Cardinality enforcement | A specific constraint type | Downstream `ConstraintValidator` |
| Domain/range enforcement | A specific constraint type | Downstream `ConstraintValidator` |
| Disjointness checking | A specific constraint type | Downstream `ConstraintValidator` |
| OWL classification / tableau reasoning | A specific inference algorithm | Downstream `InferenceRule` |
| SKOS transitive closure | A specific inference algorithm | Downstream `InferenceRule` |
| Subclass propagation | A specific inference rule | Downstream `InferenceRule` |
| Symmetry / transitivity of named properties | Specific inference rules | Downstream `InferenceRule` |
| Any built-in node types or edge types | Domain-specific | Empty type registry at creation |
| Any built-in property keys | Domain-specific | Empty property key registry at creation |

### 14.2 Query language

No SPARQL, GQL, Cypher, or Gremlin. The crate provides a Rust API (Task 10) for graph traversal and lookup. Query languages are downstream.

### 14.3 Serialization formats

No RDF/XML, Turtle, JSON-LD, or any RDF serialization. The crate stores data in its own binary format (Task 8). Import/export adapters are downstream.

### 14.4 URI/IRI handling

No built-in support for URIs, IRIs, or namespaces. Node and type names are plain UTF-8 strings. If an RDF downstream crate wants URI-based names, it uses full URI strings as names (or implements its own namespace prefix registry as an application-level concern).

### 14.5 Inference caching, invalidation, and dependency tracking

The `InferenceRule` trait defined in this document is the interface. The infrastructure for caching materialized inferred facts, tracking dependencies between base facts and inferred facts, and invalidating stale inferred facts is the responsibility of Task 11 (Inference Hook Architecture).

### 14.6 Secondary indexes

The `GraphView` trait includes `nodes_by_property()` which may perform a full scan. Secondary indexes (for efficient property-value lookups) are a storage-layer concern (Task 7/8) and an API concern (Task 10). The schema system does not manage indexes.

### 14.7 Schema migration tooling

The type registry supports adding new types and modifying existing type definitions at runtime. However, automated schema migration (e.g., "rename property X to Y across all existing nodes") is an application-level tool, not a core crate feature.

---

## 15. Validation Walkthrough: OWL Lite

This section demonstrates how an OWL Lite ontology layer would be built on top of the schema and extension system defined in this document.

### 15.1 Type registration

The OWL Lite downstream crate registers the following types at initialization:

**Node types:**
- `owl:Class` — represents an OWL class. Property declarations: `rdfs:label` (String, optional), `rdfs:comment` (String, optional), `owl:versionInfo` (String, optional).
- `owl:Individual` — represents an OWL individual (root type for all user-defined instance types).
- `owl:Restriction` — represents an OWL property restriction (anonymous class expression). Property declarations: `owl:onProperty` (U64 = edge type ID), `owl:allValuesFrom` (U64 = class node ID, optional), `owl:someValuesFrom` (U64 = class node ID, optional), `owl:maxCardinality` (U64, optional), `owl:minCardinality` (U64, optional).

**Edge types:**
- `rdf:type` — membership edge from individual to class node.
- `rdfs:subClassOf` — subsumption edge between class nodes. (Also encoded in the type hierarchy DAG via `supertypes`.)
- `owl:equivalentClass` — equivalence edge between class nodes.
- `owl:disjointWith` — disjointness edge between class nodes.
- Various object property types declared by the user's ontology.

**Property key registration:** `rdfs:label`, `rdfs:comment`, `owl:versionInfo`, `owl:onProperty`, `owl:allValuesFrom`, `owl:someValuesFrom`, `owl:maxCardinality`, `owl:minCardinality`, `owl:TransitiveProperty`, `owl:SymmetricProperty`, `owl:FunctionalProperty`.

### 15.2 Type hierarchy usage

OWL class subsumption is stored in two places:
1. The type registry's DAG (via `supertypes` on each class's `TypeDefinition`) — for efficient `is_subtype_of()` queries.
2. As `rdfs:subClassOf` edges in the graph — for querying and for downstream reasoner traversal.

This dual representation is deliberate: the type registry DAG provides O(1) subtype checks used by `nodes_by_type(..., include_subtypes: true)`, while the graph edges represent the same information in a form that inference rules and constraint validators can traverse.

### 15.3 Constraint validators registered

1. **CardinalityValidator** — implements `ConstraintValidator`. On commit, for each modified node whose type has an `owl:Restriction` with cardinality, counts outgoing edges of the restricted property type and rejects if the count violates the restriction. Uses `applies_to_types()` to scope to types that have cardinality restrictions.

2. **FunctionalPropertyValidator** — for edge types with `owl:FunctionalProperty: true` metadata, ensures at most one outgoing edge of that type per source node.

3. **DisjointnessValidator** — for each `owl:disjointWith` edge between class nodes, checks that no individual has `rdf:type` edges to both classes.

### 15.4 Inference rules registered

1. **SubclassPropagation** — implements `InferenceRule`. For each individual with `rdf:type A`, walks the `rdfs:subClassOf` chain upward and produces `InferredFact::NodeTypeAssignment` for each ancestor class.

2. **TransitivePropertyClosure** — for each edge type with `owl:TransitiveProperty: true` metadata, computes transitive closure and produces `InferredFact::NewEdge` entries.

3. **SymmetricPropertyMirroring** — for each edge type with `owl:SymmetricProperty: true` metadata, produces a reverse edge for every existing edge.

### 15.5 Interaction with the core

All of this is built using the public traits and types from Sections 4–12. The OWL Lite crate:
- Calls the type registration API to register its types
- Calls the extension registry to register its validators and rules
- Uses `GraphView` in its validator and rule implementations
- Uses `TypeRegistryView` to traverse the class hierarchy
- Never modifies the core crate's code

**Foundation requirements exercised:** A1, A2, A3, A4, B1–B7, C1–C5, D1–D4, D6–D7, E1, E3 ✓

---

## 16. Validation Walkthrough: SKOS

### 16.1 Type registration

**Node types:**
- `skos:Concept` — a concept. Property declarations: `skos:prefLabel` (LangString, required), `skos:altLabel` (LangString, optional, multi-valued), `skos:hiddenLabel` (LangString, optional, multi-valued), `skos:definition` (LangString, optional), `skos:scopeNote` (LangString, optional).
- `skos:ConceptScheme` — a collection of concepts. Property declarations: `skos:prefLabel` (LangString, required).

**Edge types:**
- `skos:broader` — broader concept relationship.
- `skos:narrower` — narrower concept relationship.
- `skos:related` — associative relationship.
- `skos:broaderTransitive` — transitive closure of broader (typically inferred).
- `skos:narrowerTransitive` — transitive closure of narrower (typically inferred).
- `skos:inScheme` — membership of a concept in a scheme.
- `skos:hasTopConcept` — roots of a scheme.
- `skos:exactMatch`, `skos:closeMatch`, `skos:broadMatch`, `skos:narrowMatch` — mapping relationships.

### 16.2 Constraint validators registered

1. **PrefLabelUniquenessValidator** — for each `skos:Concept` node, checks that it has at most one `skos:prefLabel` per language tag. Uses `applies_to_types()` scoped to `skos:Concept`.

2. **HierarchicalCycleValidator** — checks that the `skos:broader` edge graph is acyclic (a concept cannot be transitively broader than itself). Uses DFS cycle detection.

3. **DisjointLabelValidator** — verifies that `skos:prefLabel`, `skos:altLabel`, and `skos:hiddenLabel` values do not overlap for a given concept (a SKOS integrity condition).

### 16.3 Inference rules registered

1. **BroaderNarrowerInverse** — for every `skos:broader(A, B)` edge, produces `InferredFact::NewEdge` of type `skos:narrower(B, A)`, and vice versa.

2. **TransitiveClosure** — computes `skos:broaderTransitive` as the transitive closure of `skos:broader`. Produces `InferredFact::NewEdge` for each inferred transitive link.

3. **RelatedSymmetry** — for every `skos:related(A, B)` edge, produces `skos:related(B, A)` if not already present.

### 16.4 Language-tagged string usage

SKOS labels are stored as `Value::LangString { value: "Cat", lang: "en" }`. The `PrefLabelUniquenessValidator` groups labels by language tag to enforce one-per-language uniqueness. This validates the `LangString` value type design (Section 4).

### 16.5 Interaction with the core

The SKOS crate uses the same extension points as OWL Lite but with much simpler validators and rules. SKOS requires no anonymous nodes, no cardinality restrictions, no complex class expressions — just typed nodes/edges, language-tagged strings, and a few simple constraints.

**Foundation requirements exercised:** A1, A2, A4, A5, B1, B4, B5, C1–C4, D1–D4, D7, E1 ✓

---

## 17. Validation Walkthrough: Typed Property Graph (PG-Schema)

### 17.1 Type registration

A PG-Schema layer registers types that directly correspond to the user's application schema:

**Node types (example):**
- `Person` — property declarations: `name` (String, required, single), `age` (I64, optional, single), `email` (String, optional, multi-valued). Open type.
- `Organization` — property declarations: `name` (String, required, single), `founded` (I64, optional, single). Open type.
- `Employee` — supertypes: [`Person`]. Additional property declarations: `employee_id` (String, required, single). Closed type.

**Edge types (example):**
- `WORKS_FOR` — source type: `Employee`, target type: `Organization`. Property declarations: `since` (I64, optional, single), `role` (String, optional, single).
- `KNOWS` — source type: `Person`, target type: `Person`. No additional properties.

Source/target type constraints are stored as metadata on the edge type definition (Section 7.5 convention).

### 17.2 Constraint validators registered

1. **RequiredPropertyValidator** — for each modified node/edge, checks that all properties declared as `required: true` in the type's `effective_property_declarations()` are present. Uses `TypeRegistryView::effective_property_declarations()` to include inherited declarations.

2. **ValueTypeValidator** — checks that actual property values match the declared `ValueTypeDescriptor` in the property declaration.

3. **ClosedTypeValidator** — for closed types (where `TypeDefinition::open == false`), rejects nodes/edges that carry properties or type labels not declared in the schema.

4. **EndpointTypeValidator** — for each inserted/modified edge, reads the edge type's `__allowed_source_types` and `__allowed_target_types` metadata and verifies that the edge's source and target nodes have compatible types (using `TypeRegistryView::is_subtype_of()`).

5. **UniquenessValidator** — for property declarations with a uniqueness metadata flag, checks that no two nodes of the same type have the same value for that property. Uses `GraphView::nodes_by_property()` for the lookup.

### 17.3 No inference rules

PG-Schema does not define inference rules. The SKOS and OWL walkthroughs demonstrate inference; PG-Schema demonstrates a constraint-heavy, inference-free usage pattern.

### 17.4 Interaction with the core

This walkthrough validates that the schema system supports the "traditional property graph with schema" model without any ontology-specific features. The key elements used: type hierarchy with inheritance, property declarations with `required`/`multi_valued` flags, open/closed flag, metadata-based endpoint constraints, and multiple independent constraint validators.

**Foundation requirements exercised:** A1, A2, A4, B1–B7, C1–C6, E1–E3 ✓

---

## 18. Validation Walkthrough: Frame-Based System

### 18.1 Type registration

A frame-based layer maps frames to node types with rich property declarations:

**Node types:**
- `Person` — property declarations:
  - `name` (String, required, single, metadata: `{}`)
  - `age` (I64, optional, single, metadata: `{ "default": Value::I64(0) }`)
  - `knows` (NodeRef, optional, multi-valued, metadata: `{ "range_type": Value::U64(person_type_id) }`)
- `Employee` — supertypes: [`Person`]. Additional property declarations:
  - `employer` (NodeRef, required, multi-valued, metadata: `{ "range_type": Value::U64(org_type_id) }`)
  - `salary` (F64, optional, single, metadata: `{}`)

### 18.2 Slot facets as declaration metadata

Frame slot facets (default values, inverse slots, range restrictions, cardinality) are stored in the `PropertyDeclaration::metadata` field. For example:

```
PropertyDeclaration {
    key: key_id_for("age"),
    value_type: ValueTypeDescriptor::I64,
    required: false,
    multi_valued: false,
    metadata: {
        key_id_for("default"): Value::I64(0),
        key_id_for("min_cardinality"): Value::U64(0),
        key_id_for("max_cardinality"): Value::U64(1),
    },
}
```

This validates Task 3's observation (Section 7.4) that the foundation must be able to store metadata on property declarations. The `PropertyDeclaration::metadata` field (a `PropertyMap`) provides this capability.

### 18.3 Constraint validators registered

1. **SlotCardinalityValidator** — reads `min_cardinality` and `max_cardinality` from declaration metadata. For NodeRef-typed slots, counts outgoing edges of the appropriate type. For non-ref slots, checks the property value.

2. **RangeTypeValidator** — reads `range_type` from declaration metadata. For NodeRef properties, verifies that the referenced node has the declared range type.

3. **RequiredSlotValidator** — same as PG-Schema's `RequiredPropertyValidator`.

### 18.4 Inference rules registered

1. **DefaultValueInference** — for each node missing a property that has a `default` in its declaration metadata, produces `InferredFact::NodePropertyUpdate` with the default value.

2. **SlotInheritance** — for each node type with supertypes, ensures inherited slot values propagate correctly (using `effective_property_declarations`).

### 18.5 Interaction with the core

Frame-based systems use the deepest set of features: multiple inheritance in the type hierarchy, declaration metadata for slot facets, both constraints and inference, and `effective_property_declarations()` with shadowing for frame inheritance.

**Foundation requirements exercised:** A1, A2, A4, B1–B7, C1–C5, D1–D4, D6–D7, E1, E3 ✓

---

## 19. Design Decision Log

This section consolidates all design decisions made in this document for easy reference.

| # | Decision | Alternatives Considered | Rationale |
|---|----------|------------------------|-----------|
| D1 | 64-bit node/edge IDs, 32-bit type/property-key IDs | Uniform 64-bit for all; 128-bit UUIDs | 64-bit is sufficient for nodes/edges; 32-bit saves space in records for types/keys which are few |
| D2 | `NodeId(0)`, `EdgeId(0)`, `TypeId(0)` reserved as null sentinels | `Option<NodeId>` everywhere | Avoids `Option` overhead in fixed-size records; sentinel pattern is standard in DB internals |
| D3 | Anonymous nodes via `is_anonymous` flag, not separate ID type | Separate `BlankNodeId` type; special ID range | Uniform data path; downstream code distinguishes via flag |
| D4 | `Value::LangString` as dedicated variant | Convention over string + key namespacing | Self-describing; avoids ambiguity; simplifies SKOS/RDF code |
| D5 | No `Value::Map` variant | Nested map support | Nested structured data modeled as subgraphs; keeps serialization simple |
| D6 | `BTreeMap` for property bags | `HashMap`; `Vec<(K,V)>` | `no_std` compatible; deterministic order; adequate performance for small bags |
| D7 | `Vec<TypeId>` (sorted) for type labels | `BTreeSet<TypeId>`; `SmallVec` | Small sets; `Vec` is simpler, more cache-friendly; sorted enables binary search |
| D8 | Separate name namespaces for node types and edge types | Single namespace | Avoids artificial naming conflicts; matches RDF/OWL model |
| D9 | Edge type endpoint constraints as metadata, not fields | Dedicated `allowed_source`/`allowed_target` fields | Keeps `TypeDefinition` uniform; avoids core interpreting semantics |
| D10 | Property declaration shadowing in inheritance | Reject duplicate declarations; merge declarations | Matches frame inheritance; simple rule; subtype is more specific |
| D11 | `ConstraintValidator` receives `&dyn GraphView` + `&ChangeSet` | Only `ChangeSet`; full graph clone | Change set for incremental work; graph view for cross-reference; trait objects for decoupling |
| D12 | Validators return `Vec<ConstraintViolation>` (empty = pass) | `Result<(), Vec<Violation>>` | Simpler; no ambiguity about `Ok(())` semantics |
| D13 | `Send + Sync` requirement on validators and rules | No thread-safety requirement | Required for multi-threaded database; enforces stateless validators |
| D14 | Extensions registered as `Box<dyn Trait>`, names persisted | Serialize extension logic | Rust cannot serialize arbitrary logic; name-based re-registration matches SQLite pattern |
| D15 | Replacement on duplicate extension name | Reject duplicates | Simplifies upgrades; application registers new version under same name |
| D16 | Named subgraphs as property-based convention (NodeRef to context node) | First-class `subgraph` field; reserved property key | Minimal core impact; no built-in semantics; sufficient for all surveyed models |
| D17 | Inference mode (materialized vs. ephemeral) chosen by caller, not rule | Mode declared on the rule | Same rule may be used both ways; caller has context to decide |
| D18 | `InferenceRule::infer()` receives full graph, not change set | Incremental change set | Full graph is simpler and correct; incremental is an optimization for Task 11 |
| D19 | Type hierarchy acyclicity enforced in registry, not as `ConstraintValidator` | Implement as a built-in constraint | It's a structural invariant of the schema itself, not a data constraint; must be enforced before any data exists |
| D20 | `ConstraintViolation` uses `String` for `violation_kind` | Enum of violation kinds; integer codes | Downstream-defined violations; extensible strings avoid needing a central enum |

---

## Completion Report: Task 6 — Core Schema & Extension System

### Status: COMPLETE

### Done Criterion:
The criterion requires:
1. Define core primitive types (node types, edge types, property types, type hierarchies) — ✓ Sections 3–8
2. Trait interface for custom constraint validators — ✓ Section 10
3. Trait interface for custom inference rules — ✓ Section 11
4. Registration/lifecycle mechanism for extensions — ✓ Section 12
5. "Out of scope for the core crate" section — ✓ Section 14
6. Validation walkthroughs for 3+ ontology models — ✓ Sections 15–18 (OWL Lite, SKOS, PG-Schema, Frame-Based — 4 models)

All criteria met.

### Deliverables:
- `006-schema-extension-spec.md` — this document

### Summary:
Designed the complete core schema and extension system based on the foundation requirements from Task 3. The system provides: a typed property graph data model with 64-bit node/edge IDs, a dynamically-typed value system with language-tagged strings, a persistent type registry with DAG-based type hierarchy and property declarations, a `ConstraintValidator` trait for downstream constraint logic, an `InferenceRule` trait for downstream inference logic, and an extension registration/lifecycle mechanism. All designs are `no_std + alloc` compatible. Named subgraphs are handled as a convention over existing primitives.

Validated the design against four ontology models (OWL Lite, SKOS, PG-Schema, Frame-Based Systems), confirming each can be fully implemented using only the defined types, traits, and extension points without any changes to the core.

Key residual concerns from Task 3 were addressed: language-tagged strings (now a dedicated `Value::LangString` variant), named subgraphs (property-based convention with NodeRef), and property declaration metadata for frame-based slot facets (via `PropertyDeclaration::metadata`).

### Context for Next Task:
**Task 7 (Graph Storage Model)** should read `006-schema-extension-spec.md` (this deliverable) and will also need `001-db-internals-fundamentals.md` and `002-graph-storage-strategies.md` from the project knowledge. Key items for Task 7:

- The `Node` and `Edge` structs (Section 6) define what must be stored per record. Task 7 must design the on-disk layout for these.
- `PropertyMap` (Section 5) is variable-length — Task 7 must decide how to store property bags (inline vs. overflow).
- `Vec<TypeId>` type labels are variable-length — Task 7 should consider the recommendation from Task 3 Section 13.2 (primary type inline + secondary type index B-tree).
- The type registry (Section 7) needs its own storage — a schema B-tree as recommended by Task 3 Section 13.1.
- The `GraphView` trait (Section 10.3) defines the query interface that the storage layer must support efficiently. Task 7 should design indexes accordingly.
- The `ChangeSet` (Section 10.2) must be produced by the transaction system — Task 7/8 must design how change tracking integrates with the WAL or CoW mechanism.

**Task 10 (API Surface)** should read this deliverable. The types and traits here form the core of the public API. Task 10 wraps them in ergonomic builder patterns and transaction APIs.

**Task 11 (Inference Hook Architecture)** should read this deliverable, particularly Section 11. The `InferenceRule` trait and `InferredFact` type are defined here; Task 11 builds the caching, invalidation, and dependency tracking infrastructure on top.

### Residual Concerns:

1. **`Value` does not implement `Eq`.** Because `f64` is not `Eq`, `Value` only implements `PartialEq`. This may complicate downstream code that wants to use `Value` in sets or as map keys. If this becomes a problem, a wrapper type with a total-ordering convention for floats (NaN = NaN, −0 = +0) could be introduced. Deferred to implementation phase.

2. **`GraphView::nodes_by_property()` may be slow without secondary indexes.** The trait requires only a full scan fallback. Task 7 should design optional secondary indexes to make this efficient for common use cases (e.g., uniqueness checking by PG-Schema validators). The schema system does not manage indexes — that's a storage-layer concern.

3. **Extension name uniqueness is global, not per-kind.** A constraint validator and an inference rule could technically have the same name. This is currently allowed (they are stored in separate registries). If it causes confusion, Task 10 could namespace them (e.g., `constraint:MyValidator`, `inference:MyRule`).

4. **Schema modification while data exists.** The design permits modifying type definitions (adding supertypes, changing property declarations) after data exists. The core does not revalidate existing data against the new schema. Downstream code should run a dry-run validation (`C6`) after schema changes. This is a documentation/API ergonomics concern for Task 10.

5. **Dual representation of subclass relationships** (type hierarchy DAG + graph edges). The OWL Lite walkthrough (Section 15.2) stores subsumption in both the registry DAG and as graph edges. Keeping these in sync is the downstream crate's responsibility. The core could optionally auto-create graph edges when type hierarchy changes — but that would violate Principle #5 (no magic). Deferred.

### Upstream Flags:
None. All findings are scoped to the schema and extension system. No sibling task dependencies are affected.
