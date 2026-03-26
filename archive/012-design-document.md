# 012 — Design Synthesis Document

**Project:** Embedded Graph Database with Extensible Schema & Pluggable Inference  
**Task:** 12 — Design Synthesis  
**Status:** Complete  
**Depends on:** Task 6 (`006-schema-extension-spec.md`), Task 7 (`007-graph-storage-model.md`), Task 8 (`008-file-format-spec.md`), Task 9 (`009-hal-trait-design.md`), Task 10 (`010-api-surface-spec.md`), Task 11 (`011-inference-hook-design.md`)  
**Intended audience:** All implementation and preparation tasks (Tasks 13–29). This is the **single source of truth** for the project's design. A developer should be able to implement the entire system by reading only this document. Sub-documents are referenced for byte-level details and extended rationale; no design decision is left answered *only* in a sub-document.

---

## Table of Contents

1. [Purpose and Reading Guide](#1-purpose-and-reading-guide)
2. [Architecture Overview](#2-architecture-overview)
3. [Crate Structure, Feature Flags, and Dependencies](#3-crate-structure-feature-flags-and-dependencies)
4. [Core Data Model](#4-core-data-model)
5. [Type System and Schema](#5-type-system-and-schema)
6. [Graph Storage Model](#6-graph-storage-model)
7. [Single-File Format](#7-single-file-format)
8. [Hardware Abstraction Layer (HAL)](#8-hardware-abstraction-layer-hal)
9. [Buffer Pool](#9-buffer-pool)
10. [Concurrency Model](#10-concurrency-model)
11. [Transaction Lifecycle](#11-transaction-lifecycle)
12. [Crash Safety and Recovery](#12-crash-safety-and-recovery)
13. [Constraint Validation System](#13-constraint-validation-system)
14. [Inference Hook Architecture](#14-inference-hook-architecture)
15. [Public API Surface](#15-public-api-surface)
16. [Cross-Cutting Concerns](#16-cross-cutting-concerns)
17. [Consolidated Design Decision Log](#17-consolidated-design-decision-log)
18. [Known Limitations and Deferred Work](#18-known-limitations-and-deferred-work)
19. [Consolidated B-Tree Catalog and Schema Store Key Map](#19-consolidated-b-tree-catalog-and-schema-store-key-map)
20. [Document Cross-Reference Index](#20-document-cross-reference-index)

---

## 1. Purpose and Reading Guide

This document synthesizes all design decisions from Tasks 6–11 into a single authoritative reference. It is the sole design input for the implementation preparation phase (Tasks 13–21) and the implementation phase (Tasks 22–29).

**How to read this document:**

- **For a full understanding of the architecture:** Read sections 2–16 sequentially.
- **For implementation of a specific subsystem:** Use the Table of Contents to navigate to the relevant section. Each section is self-contained with cross-references to other sections where interfaces connect.
- **For the rationale behind a specific decision:** Section 17 consolidates all design decisions across all sub-documents with their rationale.
- **For byte-level format details:** This document specifies layouts at the structural level. The authoritative byte-level reference for on-disk formats remains `008-file-format-spec.md` (page headers, cell formats, superblock layout) and `007-graph-storage-model.md` (record formats, key encodings). This document reproduces the key structural facts and refers to those documents for exhaustive field-by-field layouts.

**Conflict resolution:** Where this document and a sub-document disagree, this document takes precedence. Known conflicts between sub-documents are resolved explicitly in this document (see Section 19 for the consolidated B-tree catalog, which resolves the Schema Store key prefix collision between Tasks 7 and 11).

---

## 2. Architecture Overview

The embedded graph database is a single-crate Rust library that provides a typed property graph with extensible schema, pluggable constraint validation, and pluggable inference. It stores all data in a single file (or in memory) and provides concurrent read access with serializable writes.

### 2.1 Architectural layers

```
┌──────────────────────────────────────────────────────────┐
│                   PUBLIC API LAYER                         │
│  Database, ReadTransaction, WriteTransaction               │
│  GraphReader trait, Builder helpers                        │
│  Error types                                              │
├──────────────────────────────────────────────────────────┤
│                 EXTENSION SYSTEM LAYER                     │
│  ConstraintValidator trait    InferenceRule trait           │
│  ChangeSet / ConstraintViolation                          │
│  InferenceEngine (cache, provenance, dispatch)            │
│  ExtensionRegistry (registration, lifecycle)              │
├──────────────────────────────────────────────────────────┤
│                 SCHEMA / TYPE LAYER                        │
│  TypeRegistry, PropertyKeyRegistry                        │
│  TypeDefinition, PropertyDeclaration                      │
│  Type hierarchy DAG, acyclicity enforcement               │
│  In-memory schema cache                                   │
├──────────────────────────────────────────────────────────┤
│                  STORAGE ENGINE LAYER                      │
│  CoW B+ tree operations (insert, delete, range scan)      │
│  Buffer pool (clock eviction, page frames, pinning)       │
│  WriteBuffer (pending mutations, overlay reads)           │
│  Page allocator, free-space management                    │
│  ID allocator (monotonic counters + freelist recycling)   │
├──────────────────────────────────────────────────────────┤
│                  FILE FORMAT LAYER                         │
│  Identity header, dual superblocks                        │
│  Page types (interior, leaf, overflow, free)              │
│  Common page header (24 bytes, CRC32C)                    │
│  Commit protocol (2-fsync atomic commit)                  │
├──────────────────────────────────────────────────────────┤
│              HARDWARE ABSTRACTION LAYER (HAL)             │
│  ReadAt, WriteAt, hal::Sync traits (no_std + alloc)       │
│  StorageBackend = ReadAt + WriteAt + hal::Sync             │
│  FileBackend (std) │ MemoryBackend (alloc)                │
│  OpenableBackend, LockableBackend (std-only lifecycle)    │
└──────────────────────────────────────────────────────────┘
```

### 2.2 Key architectural decisions

| Decision | Choice | Rationale | Source |
|----------|--------|-----------|--------|
| Storage primitive | Unified CoW B+ trees | Eliminates CoW/slot-store tension; uniform crash safety; manageable traversal cost with warm buffer pool | Task 7 §3 |
| Concurrency model | Single-writer MVCC via CoW snapshots | Eliminates write-write conflicts and deadlocks; graph workloads are read-heavy; provides Serializable isolation | Task 7 §11 |
| Crash safety | Dual-superblock atomic commit, no WAL | CoW B-trees make WAL unnecessary; 2-fsync commit provides full durability | Task 8 §13 |
| File format | Single file, page-based, extensible | Simplest deployment model; page-based for buffer pool integration; extensible via feature flags and reserved fields | Task 8 §1 |
| Schema model | Dynamic typed property graph with persistent type registry | Supports multi-label nodes/edges, type hierarchy DAG, property declarations — sufficient for OWL, SKOS, PG-Schema, frames | Task 6 §7 |
| Extension model | Trait-based: `ConstraintValidator` + `InferenceRule` | Full Rust expressivity; no DSL or configuration strings; `Send + Sync` for thread safety | Task 6 §10, §11 |
| `no_std` strategy | `no_std + alloc` core with `std` feature for the database engine | Types and traits usable on embedded platforms; database engine requires OS facilities | Task 9 §3 |
| HAL design | Three-trait decomposition: `ReadAt` + `WriteAt` + `hal::Sync` | Minimal surface; each trait has clear responsibility; blanket `StorageBackend` impl | Task 9 §5–6 |

### 2.3 Data flow overview

**Write path:** Application → `WriteTransaction` → WriteBuffer (in-memory mutations) → ChangeSet → ConstraintValidators → CoW B-tree path copy → new pages to buffer pool → fsync data pages → write new superblock → fsync superblock → update current snapshot → release write lock.

**Read path:** Application → `ReadTransaction` → snapshot root pointers → B-tree traversal via buffer pool → page frames → deserialized records returned as owned values.

**Inference path:** Application → `run_inference(rule, mode)` → InferenceEngine checks cache → cache miss: invoke `rule.infer(graph_view, types, keys)` → cache result → if Materialized: cleanup previous facts from this rule, write new facts to WriteBuffer, record provenance → return InferenceResult.

---

## 3. Crate Structure, Feature Flags, and Dependencies

### 3.1 Module layout

```
graph_db/
├── lib.rs                  // #![cfg_attr(not(feature = "std"), no_std)]
│                           // Re-exports at crate root
├── types/                  // Core types (no_std + alloc)
│   ├── mod.rs              // NodeId, EdgeId, TypeId, PropertyKeyId
│   │                       // Value, ValueTypeDescriptor, PropertyMap
│   │                       // Node, Edge, TypeKind, TypeDefinition
│   │                       // PropertyDeclaration
├── schema/                 // Schema traits (no_std + alloc)
│   ├── mod.rs              // TypeRegistryView, PropertyKeyRegistryView
├── constraint/             // Constraint traits and types (no_std + alloc)
│   ├── mod.rs              // ConstraintValidator, ChangeSet,
│   │                       // NodeChange, EdgeChange,
│   │                       // ConstraintViolation, ViolationSubject
├── inference/              // Inference traits and types (no_std + alloc)
│   ├── mod.rs              // InferenceRule, InferredFact,
│   │                       // InferenceResult, InferenceMode,
│   │                       // ProvenanceRecord, InferredEntity,
│   │                       // MaterializedMapping
├── error/                  // Error types (no_std + alloc core; std extensions)
│   ├── mod.rs              // Error, SchemaError, StorageError,
│   │                       // NotFoundError, TransactionError, InferenceError
├── hal/                    // HAL trait definitions (no_std + alloc)
│   ├── mod.rs
│   ├── error.rs            // StorageErrorKind, StorageError trait,
│   │                       // StorageErrorType
│   ├── traits.rs           // ReadAt, WriteAt, hal::Sync, StorageBackend
│   └── lifecycle.rs        // OpenableBackend, LockableBackend (trait defs)
├── hal_std/                // std persistent backend (std feature only)
│   ├── mod.rs
│   └── file_backend.rs     // FileBackend, FileBackendConfig, FileLockGuard
├── hal_mem/                // In-memory backend (alloc feature)
│   ├── mod.rs
│   └── memory_backend.rs   // MemoryBackend
├── storage/                // Storage engine internals (alloc core; std for engine)
│   ├── btree/              // B+ tree operations
│   ├── page/               // Page types, headers, serialization
│   ├── buffer_pool.rs      // Buffer pool, clock eviction
│   ├── allocator.rs        // Page allocator, file growth
│   └── serialization.rs    // Property/record serialization
├── db/                     // Database engine (std feature only)
│   ├── config.rs           // DatabaseConfig, StorageMode
│   ├── database.rs         // Database struct
│   ├── read_txn.rs         // ReadTransaction
│   ├── write_txn.rs        // WriteTransaction
│   ├── write_buffer.rs     // WriteBuffer, change tracking
│   ├── schema_cache.rs     // In-memory TypeRegistry, PropertyKeyRegistry
│   └── inference_engine.rs // InferenceEngine, InferenceCache,
│                           // ProvenanceRegistry
```

### 3.2 Feature flags

```toml
[features]
default = ["std"]
std = ["alloc"]   # Enables Database, transactions, FileBackend, file I/O
alloc = []        # Enables MemoryBackend; implied by std
```

When `std` is disabled and only `alloc` is active: the `types`, `schema`, `constraint`, `inference`, `error`, `hal`, and `hal_mem` modules are available. This allows downstream `no_std` crates to depend on the type definitions and trait interfaces without pulling in the database engine.

When `std` is active: all modules are available, including `hal_std`, `storage`, and `db`.

**Rationale:** Task 9 §3 established this structure. The split follows the `embedded-hal` pattern: interface is universal, implementations are platform-gated.

### 3.3 Dependencies

**Core (`no_std + alloc`):**
- `crc32fast` — CRC32C checksums for page integrity (Task 8 §5). Must be verified for `no_std + alloc` compatibility (Task 8 residual concern #3).

**`std` feature only:**
- `libc` (Unix) / `windows-sys` (Windows) — thin FFI bindings for `pread`/`pwrite`, `flock`, `fdatasync`, `F_FULLFSYNC` (Task 9 §9, decision D9).

**Explicitly excluded:**
- No external database crate dependencies (project constraint).
- No `serde` in core (serialization is custom binary for performance — Task 7 decision G13). `serde` support may be offered as an optional feature for user-facing types if desired.

### 3.4 Re-exports at crate root

```rust
// Always available (no_std + alloc)
pub use types::{
    NodeId, EdgeId, TypeId, PropertyKeyId,
    Value, ValueTypeDescriptor, PropertyMap,
    Node, Edge, TypeKind, TypeDefinition, PropertyDeclaration,
};
pub use schema::{TypeRegistryView, PropertyKeyRegistryView};
pub use constraint::{
    ConstraintValidator, ConstraintViolation, ViolationSubject,
    ChangeSet, NodeChange, EdgeChange,
};
pub use inference::{
    InferenceRule, InferredFact, InferenceResult, InferenceMode,
    ProvenanceRecord, InferredEntity, MaterializedMapping,
};
pub use error::Error;
pub use hal::{
    StorageErrorKind, StorageError, StorageErrorType,
    ReadAt, WriteAt, StorageBackend,
};

// std-only
#[cfg(feature = "std")]
pub use db::{Database, DatabaseConfig, StorageMode,
             ReadTransaction, WriteTransaction, MissingExtensions};
#[cfg(feature = "std")]
pub use hal_std::FileBackend;
#[cfg(feature = "alloc")]
pub use hal_mem::MemoryBackend;
```

---

## 4. Core Data Model

### 4.1 Identity

Every node and edge has a unique, stable 64-bit identifier. Types and property keys use 32-bit identifiers. All IDs are monotonically assigned by the database.

```rust
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct NodeId(pub u64);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct EdgeId(pub u64);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TypeId(pub u32);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PropertyKeyId(pub u32);
```

**Reserved values:** `NodeId(0)`, `EdgeId(0)`, `TypeId(0)`, and `PropertyKeyId(0)` are null sentinels — they never refer to valid entities. This allows fixed-size records to use 0 as "no reference" without `Option` overhead.

**Rationale:** 64-bit for nodes/edges is sufficient for billions of entities with optimal B-tree sequential insertion. 32-bit for types/keys saves space in records (type registries are small). Newtype wrappers prevent accidental ID mixing at zero runtime cost. (Task 6 §3, decisions D1–D2.)

### 4.2 Value type system

Property values are dynamically typed:

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    I64(i64),
    U64(u64),
    F64(f64),
    String(String),
    Bytes(Vec<u8>),
    NodeRef(NodeId),
    LangString { value: String, lang: String },
    List(Vec<Value>),
}
```

**Key constraints:**
- `Value` implements `PartialEq` but **not** `Eq` because `f64` is not `Eq`. This means `Value` cannot be used as a `BTreeMap` key or `HashSet` element directly. Property *keys* are `PropertyKeyId` (integer), so this is not a practical limitation. (Task 6 residual concern #1.)
- `LangString` is a dedicated variant (not a convention over string naming) for self-describing language-tagged strings, as required by SKOS/RDF. (Task 6 §4, decision D4.)
- No nested maps: structured data is modeled as subgraphs. (Task 6 §4, decision D5.)

**Schema declarations** use `ValueTypeDescriptor` to describe expected types without holding values:

```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValueTypeDescriptor {
    Any, Bool, I64, U64, F64, String, Bytes, NodeRef, LangString,
    List(Box<ValueTypeDescriptor>),
}
```

### 4.3 Property bags

A property bag is an ordered map from `PropertyKeyId` to `Value`:

```rust
pub type PropertyMap = BTreeMap<PropertyKeyId, Value>;
```

`BTreeMap` is used because it is available in `alloc` (no `std` required) and provides deterministic iteration order for reproducible serialization. (Task 6 §5, decision D6.)

### 4.4 Node

```rust
pub struct Node {
    pub id: NodeId,
    pub type_labels: Vec<TypeId>,  // sorted, may be empty
    pub properties: PropertyMap,
    pub is_anonymous: bool,
}
```

Nodes may have zero or multiple type labels. Anonymous nodes (RDF blank nodes) are tracked via the `is_anonymous` flag. Type labels are stored as a sorted `Vec<TypeId>` for compact representation and efficient binary search. (Task 6 §6, decisions D3, D7.)

### 4.5 Edge

```rust
pub struct Edge {
    pub id: EdgeId,
    pub type_labels: Vec<TypeId>,  // sorted, typically 1
    pub source: NodeId,
    pub target: NodeId,
    pub properties: PropertyMap,
}
```

Edges are directed. Multiple parallel edges between the same source and target are permitted (multi-graph). Edge endpoints are **immutable** after creation — changing endpoints requires delete-and-recreate. Multiple type labels on edges are allowed for RDF/OWL compatibility. (Task 6 §6, Task 10 §9.2, decision A10.)

---

## 5. Type System and Schema

### 5.1 Type definitions

```rust
pub struct TypeDefinition {
    pub id: TypeId,
    pub name: String,                           // unique within kind namespace
    pub kind: TypeKind,                          // Node or Edge
    pub supertypes: Vec<TypeId>,                 // DAG parents
    pub property_declarations: Vec<PropertyDeclaration>,
    pub open: bool,                              // instances may carry undeclared properties?
    pub metadata: PropertyMap,                   // arbitrary annotations
}

pub struct PropertyDeclaration {
    pub key: PropertyKeyId,
    pub value_type: ValueTypeDescriptor,
    pub required: bool,
    pub multi_valued: bool,
    pub metadata: PropertyMap,     // e.g., default values, facets
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeKind { Node, Edge }
```

**Design principles:** The core stores type definitions and property declarations as metadata. It does **not** enforce them. Enforcement is the responsibility of registered `ConstraintValidator` implementations. This separation of storage from meaning (Task 6 Principle #4) keeps the core general-purpose. The `open` flag, `required` flag, and edge endpoint constraints (stored as metadata properties `__allowed_source_types` / `__allowed_target_types`) are all conventions that downstream validators interpret. (Task 6 §7, decisions D8–D9.)

### 5.2 Type hierarchy

The type hierarchy is a DAG with multiple inheritance. Acyclicity is enforced at registration time by walking the entire ancestor chain — O(|V|) where |V| is the number of types, which is small. Diamond inheritance is permitted. (Task 6 §8.)

**Property declaration shadowing:** When a subtype declares a property with the same key as a supertype, the subtype's declaration takes precedence. `effective_property_declarations()` collects declarations in breadth-first topological order with this shadowing rule. (Task 6 §7.7, decision D10.)

**Separate name namespaces:** Node types and edge types have independent name namespaces — a node type named "Person" and an edge type named "Person" can coexist. (Task 6 §7.7, decision D8.)

### 5.3 Type registry trait

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

The type registry is **cached entirely in memory** at database open time (loaded from the Schema Store B-tree). Schema data is small and read on nearly every operation — in-memory caching avoids per-operation B-tree traversal. (Task 7 §8.5, decision G11.)

### 5.4 Property key registry

```rust
pub trait PropertyKeyRegistryView {
    fn get_key_id(&self, name: &str) -> Option<PropertyKeyId>;
    fn get_key_name(&self, id: PropertyKeyId) -> Option<&str>;
    fn all_keys(&self) -> Vec<(PropertyKeyId, &str)>;
}
```

Property key names are interned to compact integer IDs. Once assigned, a key ID never changes. Keys are registered implicitly on first use or explicitly via the API. (Task 6 §9.)

### 5.5 Named subgraphs

Named subgraphs (for OWL ontology grouping, RDF named graphs, Topic Maps scope) are a **convention over existing primitives**, not a first-class concept. A subgraph context is a regular node with a designated type. Membership is represented by a property (`__subgraph` → `Value::NodeRef(context_node_id)`) or by edges. The core does not special-case this. (Task 6 §13, decision D16.)

### 5.6 Schema modification after data exists

Modifying type definitions (adding supertypes, changing property declarations) after data exists is permitted. The core does **not** automatically revalidate existing data. Downstream code should run `validate_all()` after schema changes. (Task 6 §8.4, Task 10 §7, design principle #5: predictability over magic.)

---

## 6. Graph Storage Model

### 6.1 Unified CoW B+ tree architecture

All persistent state is stored in copy-on-write B+ trees. There are no slot stores, no WAL, and no separate storage subsystems. This decision (Task 7 §3) eliminates the CoW/slot-store tension identified by Task 2, provides uniform crash safety via a single mechanism (atomic root pointer swap), and reduces implementation complexity.

**What this gives up:** O(1) single-hop traversal in the cold-buffer-pool case becomes O(log N). In practice, with a warm buffer pool, B-tree interior nodes are cached and each hop costs exactly one leaf page read — identical to index-free adjacency. (Task 7 §3.)

### 6.2 Complete B-tree catalog

The database contains **eight** logical B+ trees — seven data B-trees (Task 7 §4) plus one infrastructure B-tree (Page Freelist, Task 8 §11):

| # | Name | Key | Value | Purpose |
|---|------|-----|-------|---------|
| 1 | **Node Store** | `NodeId` (u64 BE) | `NodeRecord` | Primary store for node data |
| 2 | **Edge Store** | `EdgeId` (u64 BE) | `EdgeRecord` | Primary store for edge data |
| 3 | **Outgoing Adjacency Index** | `(NodeId, TypeId, EdgeId)` (20 bytes BE) | ∅ (key-only) | Outgoing edges from a node, optionally filtered by type |
| 4 | **Incoming Adjacency Index** | `(NodeId, TypeId, EdgeId)` (20 bytes BE) | ∅ (key-only) | Incoming edges to a node, optionally filtered by type |
| 5 | **Type Index** | `(TypeKindTag, TypeId, EntityId)` (13 bytes BE) | ∅ (key-only) | All nodes or edges of a given type |
| 6 | **Schema Store** | `SchemaKey` (variable, see §19) | `SchemaValue` (variable) | Type definitions, property keys, hierarchy, counters, extension names, provenance |
| 7 | **ID Freelist** | `(EntityKindTag, EntityId)` (9 bytes BE) | ∅ (key-only) | Recycled node/edge IDs for reuse |
| 8 | **Page Freelist** | `(FreedTxnId, PageId)` (16 bytes BE) | ∅ (key-only) | MVCC-safe free page tracking for the CoW allocator |

The file superblock stores the root page ID for each B-tree. On commit, modified B-tree paths are written to new pages, and the superblock is atomically updated with new root page IDs.

**Key encoding convention:** All B-tree keys use **big-endian** encoding so that byte-level lexicographic order matches integer order (memcmp sufficiency — no custom comparator needed). Record values use **little-endian** (machine-native on x86/ARM, avoids byte-swap overhead). (Task 7 §6, decisions G4–G5.)

### 6.3 Record formats

**NodeRecord** (variable-length, stored as Node Store value):

| Field | Size | Description |
|-------|------|-------------|
| `flags` | 1 byte | Bit 0: `is_anonymous` |
| `type_count` | 1 byte | Number of type labels (0–255) |
| `primary_type` | 4 bytes (LE) | First TypeId (or 0) |
| `property_size` | 4 bytes (LE) | Byte length of inline properties |
| `overflow_page_id` | 8 bytes (LE) | PageId for overflow (0 if inline) |
| `extra_types[N-1]` | 4×(N-1) bytes (LE) | Additional TypeIds if N > 1 |
| `inline_properties` | S bytes | Serialized PropertyMap (S = property_size) |

**EdgeRecord** follows the same pattern with additional `source` (8 bytes LE) and `target` (8 bytes LE) fields after `flags`.

Full byte-level layouts: `007-graph-storage-model.md` §5.

### 6.4 Property storage: inline vs. overflow

Properties are stored inline when the serialized `PropertyMap` is ≤ **256 bytes**, and in overflow pages when larger. The 256-byte threshold accommodates ~8–12 typical properties while keeping B-tree leaf pages from bloating. (Task 7 §7, decision G6.)

**Overflow pages** use a simple chained format: `[next_page_id: u64] [data_length: u32] [data: variable]`. The serialized bytes are split across the chain. On read, the chain is followed and concatenated. (Task 7 §7.2.)

**Value serialization** is custom binary (not serde/bincode) for performance and control. Each value is encoded as `[type_tag: u8] [payload]`. Strings and bytes use a 4-byte length prefix. Lists are serialized recursively. LangString encodes value and language tag sequentially with individual length prefixes. (Task 7 §7.4, decision G13.)

### 6.5 How the schema maps to storage

| Schema concept | Storage location | Reference |
|----------------|-----------------|-----------|
| `Node` | Node Store B-tree (key: NodeId, value: NodeRecord) | Task 7 §9.1 |
| `Edge` | Edge Store B-tree (key: EdgeId, value: EdgeRecord) | Task 7 §9.1 |
| `Node.type_labels` | Inline in NodeRecord + one Type Index entry per label | Task 7 §9.1 |
| `Node.properties` | Inline in NodeRecord or overflow pages | Task 7 §7 |
| `TypeDefinition` | Schema Store B-tree (key: `0x01 [TypeId]`) | Task 7 §8.1 |
| `PropertyKeyId ↔ name` | Schema Store B-tree (key: `0x02 [PropertyKeyId]`) | Task 7 §8.2 |
| Type hierarchy edges | Schema Store B-tree (key: `0x04 [child] [parent]`) | Task 7 §8.3 |
| ID counters | Schema Store B-tree (key: `0x03 [counter_name]`) | Task 7 §6.6 |
| Extension names | Schema Store B-tree (key: `0x05 [kind] [name]`) | Task 7 §8.4 |
| Provenance records | Schema Store B-tree (key: `0x06 [entity]`) | Task 11 §8.4 (prefix corrected; see §19) |

### 6.6 ID allocation and recycling

Node and edge IDs are allocated from monotonic counters (stored in the Schema Store). On allocation, the ID Freelist B-tree is checked first for recycled IDs; if empty, the counter is incremented. Deleted entity IDs are inserted into the ID Freelist. Recycled IDs are safe to reuse because deletions and freelist insertions are atomic within the same transaction, and readers holding old snapshots see the old state. No tombstones are needed. (Task 7 §14.)

---

## 7. Single-File Format

### 7.1 File structure

```
┌──────────────────────────────────────────────────┐
│ Page 0: File Identity Header (32 bytes)          │
│         + Superblock A (160 bytes)               │
│         + Reserved space + padding to page_size  │
├──────────────────────────────────────────────────┤
│ Page 1: Superblock B (160 bytes)                 │
│         + Reserved space + padding to page_size  │
├──────────────────────────────────────────────────┤
│ Pages 2+: B-tree pages (interior, leaf),         │
│           overflow pages, free pages             │
└──────────────────────────────────────────────────┘
```

The file identity header (32 bytes, immutable after creation) contains: magic bytes (`GRAPHDB\0`, 8 bytes), format version (major: u16 BE, minor: u16 BE), page size encoded as `log2(page_size) - 12` (u8), application ID (u32 LE), creation timestamp (u64 LE), and reserved bytes.

Full byte-level header layout: `008-file-format-spec.md` §3.

### 7.2 Dual-superblock design

Two independent superblock slots (pages 0 and 1) provide atomic commit. Each superblock contains:

- Magic bytes (14 bytes) for identification
- `transaction_id` (u64 LE) — monotonically increasing
- `total_pages` (u64 LE)
- Root page IDs for all eight B-trees (8 × u64 LE)
- Feature flags (u64 LE, bitfield: high 32 bits = required features, low 32 bits = advisory features)
- Checksum (u32 LE) covering all preceding bytes

Total superblock: 160 bytes with 64 bytes reserved for future expansion = 192 bytes total usable area per superblock page. (Task 8 §4, decision F12.)

**Active superblock selection:** On startup, both superblocks are read and validated (magic + checksum). The valid superblock with the higher `transaction_id` is the active one. If both are valid with equal `transaction_id`, both represent the same state (first commit writes to both). If only one is valid, it is used (the other was being written during a crash). (Task 8 §4.3.)

### 7.3 Common page header

Every data page (pages 2+) begins with a 24-byte header:

| Offset | Size | Field | Description |
|--------|------|-------|-------------|
| 0 | 8 | `page_id` | Self-referential page ID (u64 LE) |
| 8 | 1 | `page_type` | Page type discriminant (u8) |
| 9 | 1 | `flags` | Page-type-specific flags |
| 10 | 2 | `_padding` | Must be zero |
| 12 | 8 | `txn_id` | Transaction that wrote this page (u64 LE) |
| 20 | 4 | `checksum` | CRC32C over all page bytes (with checksum field zeroed) |

Usable payload per page: `page_size - 24` bytes (4072 bytes at 4 KB page size). The `page_id` self-reference detects corruption where a page ends up at the wrong offset. (Task 8 §5.)

### 7.4 Page types

| Tag | Type | Description |
|-----|------|-------------|
| `0x01` | B-tree interior | Slotted page with child pointers and separator keys |
| `0x02` | B-tree leaf | Slotted page with key-value cells |
| `0x03` | Overflow | Chained data for large property blobs |
| `0x04` | Free | Available for allocation |

B-tree pages use a **slotted page** format with a cell pointer array at the top and cell data growing from the bottom. Leaf pages are linked in a doubly-linked list for efficient range scans. Interior pages store separator keys and child page IDs.

Full page format details: `008-file-format-spec.md` §§6–10.

### 7.5 Free-space management

The Page Freelist B-tree (B-tree #8) tracks free disk pages with keys `(freed_txn_id, page_id)`. This encoding enables MVCC-safe reclamation: only pages freed before the oldest active reader's snapshot can be reused. Allocation priority: (1) reclaimable free pages, (2) file extension. (Task 8 §11.)

**Circular dependency resolution:** Inserting entries into the Page Freelist during commit may itself require new pages (if the freelist B-tree splits). Pages freed during the freelist's own CoW operations are **deferred** to the next transaction's commit. This breaks the circularity at the cost of 1–3 temporarily leaked pages, recoverable by `compact()`. (Task 8 §11.3, decision F8.)

**File growth:** Scaling increments based on database size — 8 pages for small databases up to 1024 pages for large ones. The file does not shrink automatically; explicit `compact()` reclaims space. (Task 8 §12, decisions F10–F11.)

### 7.6 Versioning and extensibility

The format uses a two-part version: `format_major` (breaking changes) and `format_minor` (backward-compatible additions). Feature flags in the superblock allow optional features to be declared as "required" (reader must understand) or "advisory" (reader may ignore). Reserved fields in the superblock and page header provide room for future expansion without format changes. (Task 8 §16.)

---

## 8. Hardware Abstraction Layer (HAL)

### 8.1 Core traits

Three primitive I/O traits compose into the `StorageBackend` supertrait:

```rust
// All in no_std + alloc

pub trait StorageErrorType {
    type Error: StorageError;
}

pub trait ReadAt: StorageErrorType {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Self::Error>;
    fn len(&self) -> Result<u64, Self::Error>;
}

pub trait WriteAt: StorageErrorType {
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<(), Self::Error>;
    fn set_len(&mut self, size: u64) -> Result<(), Self::Error>;
}

pub trait Sync: StorageErrorType {
    fn sync_data(&mut self) -> Result<(), Self::Error>;
    fn sync_all(&mut self) -> Result<(), Self::Error>;
}

pub trait StorageBackend: ReadAt + WriteAt + hal::Sync {}
impl<T: ReadAt + WriteAt + hal::Sync> StorageBackend for T {}
```

**Key design decisions:**
- `ReadAt` takes `&self` for concurrent reads; `WriteAt` and `Sync` take `&mut self` for serialized writes. (Task 9 §5, decisions D4, G6.)
- Two sync methods: `sync_data()` (data only, maps to `fdatasync` on Linux) and `sync_all()` (data + metadata, maps to `fsync`). Required by the commit protocol — `sync_all()` is used when the file was extended, `sync_data()` otherwise. (Task 9 §5.3, decision D12.)
- All traits are object-safe. `dyn StorageBackend` works for runtime backend selection. (Task 9 §2, goal G2.)
- No `append()` method — the file format uses `set_len()` + `write_at()`. (Task 9 decision D2.)

### 8.2 Error types

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StorageErrorKind {
    OutOfBounds, Io, ReadOnly, StorageFull,
    MediaCorruption, Interrupted, LockContention, Other,
}

pub trait StorageError: core::fmt::Debug + core::fmt::Display {
    fn kind(&self) -> StorageErrorKind;
}
```

`StorageError` requires `Display` (not just `Debug`) for user-facing error messages. (Task 9 §4, decision D1.)

### 8.3 Lifecycle and locking (std-only)

```rust
#[cfg(feature = "std")]
pub trait OpenableBackend: StorageBackend + Sized {
    type Config;
    fn open(config: Self::Config) -> Result<Self, Self::Error>;
    fn create(config: Self::Config) -> Result<Self, Self::Error>;
    fn open_or_create(config: Self::Config) -> Result<Self, Self::Error>
    where Self::Config: Clone;
}

#[cfg(feature = "std")]
pub trait LockableBackend: StorageErrorType {
    type LockGuard: Send;
    fn try_lock_exclusive(&mut self) -> Result<Self::LockGuard, Self::Error>;
}
```

`LockableBackend` uses non-blocking lock only (`try_lock_exclusive`) — the database should fail immediately on contention. The lock guard is RAII (dropped = unlocked). (Task 9 §8, decisions D6–D7.)

### 8.4 Backend implementations

**FileBackend (std):** Uses `pread`/`pwrite` for offset-based I/O, `flock` for file locking, `fdatasync`/`fsync`/`F_FULLFSYNC` for durability. On macOS, both sync methods map to `F_FULLFSYNC` because macOS `fsync()` does not guarantee persistence to physical media. On Windows, `FlushFileBuffers()` serves both sync methods. (Task 9 §9, §13.)

**MemoryBackend (alloc):** Backed by `Vec<u8>`. Auto-extends on `write_at` beyond current length. Sync methods are no-ops. Provides `snapshot_to_bytes()` / `load_from_bytes()` helpers and (with `std`) `snapshot_to_file()` / `load_from_file()`. Uses `Infallible` as its error type for simplicity. (Task 9 §10.)

### 8.5 Error propagation chain

HAL errors are type-erased at the public API boundary to prevent backend types from leaking:

```
Backend::Error (concrete, per-backend)
    → StorageErrorKind + Display message (via StorageError trait)
        → error::StorageError { message: String, source: Option<io::Error> }
            → error::Error::Storage(StorageError)
```

This gives users meaningful error messages and categories without coupling application code to the backend type. (Task 9 §12, decision D10.)

---

## 9. Buffer Pool

### 9.1 Structure

The buffer pool is a fixed-size region of memory that caches B-tree pages. It is the performance heart of the storage engine.

```rust
struct PageFrame {
    page_id: PageId,        // which page (or 0 if empty)
    data: [u8; PAGE_SIZE],  // raw page bytes
    dirty: bool,            // modified since last write?
    pin_count: u32,         // active references
    reference_bit: bool,    // clock eviction bit
}

struct BufferPool {
    frames: Vec<PageFrame>,
    page_table: HashMap<PageId, usize>,  // PageId → frame index
    clock_hand: usize,
    capacity: usize,
}
```

### 9.2 Operations

- **fetch_page:** Check page_table → cache hit: set reference_bit, pin, return. Cache miss: find victim via clock, flush if dirty, read page from disk, insert into page_table.
- **unpin_page:** Decrement pin_count. If dirty flag set, mark frame dirty.
- **flush_page:** Write dirty frame to disk. Clear dirty flag.

### 9.3 Eviction: Clock algorithm

The clock algorithm sweeps frames circularly. Each frame has a reference bit set on access. The clock hand advances, clearing reference bits. When a frame with reference_bit=false and pin_count=0 is found, it is evicted. This is O(1) amortized and simpler than LRU while providing adequate eviction quality. (Task 7 §10.4, decision G9.)

### 9.4 Configuration

| Parameter | Default | Minimum | Notes |
|-----------|---------|---------|-------|
| `buffer_pool_frames` | 1024 | 64 | 4 MB at 4 KB pages |
| `page_size` | 4096 | 4096 | Must be power of two |

The buffer pool sizing directly affects warm-path performance. Larger pool = more hot pages = fewer disk reads. Interior B-tree nodes are small and frequently accessed — the pool keeps them hot, making steady-state traversal ~1 leaf page read per hop. (Task 7 §10.5.)

---

## 10. Concurrency Model

### 10.1 Single-writer MVCC

The database uses **single-writer, multiple-reader** concurrency with MVCC via CoW snapshots:

- **One write transaction** at a time, serialized by a global write mutex.
- **Unlimited concurrent read transactions**, each holding an immutable snapshot (a set of B-tree root page IDs).
- **Snapshot Isolation** for readers. Combined with single-writer, this provides **Serializable** isolation — no anomalies are possible.

(Task 7 §11, decision G7–G8.)

### 10.2 Snapshot lifecycle

A snapshot is a set of B-tree root page IDs. Creating one is O(1) — copy the current root pointers from the active superblock. Each snapshot has a reference count. When a read transaction begins, it increments the count for the current snapshot. When it ends, it decrements. When a snapshot's count reaches zero and a newer snapshot exists, the old snapshot's unreachable pages become eligible for reclamation. (Task 7 §11.3.)

### 10.3 Write locking

```rust
struct DatabaseInner {
    write_mutex: Mutex<()>,
    current_snapshot: RwLock<Snapshot>,
    active_snapshots: Mutex<Vec<(SnapshotId, Arc<Snapshot>)>>,
}
```

A write transaction acquires the `write_mutex` at `begin()` and releases it at `commit()` or `abort()`. This serializes all writes. The `current_snapshot` is protected by an `RwLock`: readers take a read lock to clone the snapshot; the writer takes a write lock to update it on commit. (Task 7 §11.4.)

### 10.4 Thread safety of transactions

`Database` is `Send + Sync` (internal state protected by `Mutex`/`RwLock`). Transactions are `!Send` and `!Sync` — they hold references into the buffer pool and snapshot state. Making them `Send` would require per-page-access atomic reference counting, which is excessive overhead for a capability most users don't need. (Task 10 decision A12.)

---

## 11. Transaction Lifecycle

### 11.1 Read-only transaction

```
1. Acquire read lock on current_snapshot.
2. Clone the current Snapshot (set of root pointers).
3. Release read lock.
4. Increment snapshot reference count.
5. All reads traverse B-trees from the cloned roots → consistent snapshot.
6. On drop/finish: decrement snapshot reference count.
```

Cost: one `RwLock` read acquisition + pointer copy. No disk I/O. No coordination with writers. (Task 7 §12.1.)

### 11.2 Write transaction

```
1.  Acquire write_mutex.
2.  Clone current Snapshot as base state.
3.  Initialize empty WriteBuffer.
4.  Execute mutations → each modifies the WriteBuffer.
5.  Reads during the transaction see base snapshot + WriteBuffer (read-your-own-writes).
6.  On commit:
    a. Build ChangeSet from WriteBuffer.
    b. Run all registered ConstraintValidators.
    c. If any validator fails: return Error::ConstraintViolation. Transaction consumed.
    d. Materialize B-tree changes: CoW path copies producing new root pages.
    e. Write all new pages to disk.
    f. fsync data pages (sync_all if file extended, sync_data otherwise).
    g. Write new superblock with updated roots and incremented transaction_id.
    h. fsync superblock (sync_data — superblock doesn't change file size).
    i. Update current_snapshot under write lock.
    j. Release write_mutex.
    k. Mark old snapshot pages as reclaimable.
7.  On abort: discard WriteBuffer, release write_mutex. No disk I/O.
```

**Important behavioral note:** `commit(self)` **consumes** the transaction on both success and failure. On constraint violation failure, the caller receives the violations in the error and must construct a new transaction. This is simpler than allowing retry with ambiguous pending state. (Task 10 §6.4, decision A2.)

### 11.3 Read-your-own-writes

During a write transaction, reads check the WriteBuffer first, then fall back to the base snapshot's B-trees. This overlay is also what `GraphView` provides to constraint validators and inference rules at commit time. (Task 7 §12.3.)

---

## 12. Crash Safety and Recovery

### 12.1 Guarantees

The CoW B-tree architecture provides:

1. **No data corruption.** Old pages are never overwritten; new pages are written to fresh locations and become reachable only after atomic superblock commit.
2. **Committed transactions are durable.** Once `commit()` returns, all new pages and the superblock have been fsynced.
3. **Uncommitted transactions are rolled back.** If the process crashes before the superblock flip, the old superblock is still active and new pages are unreachable.

(Task 7 §13.1, Task 8 §14.)

### 12.2 Recovery procedure

```
1. Read both superblock slots.
2. Validate checksums.
3. Select the valid slot with the higher transaction_id.
4. If only one valid: use that one (other was mid-write during crash).
5. The selected superblock's root pointers define the consistent state.
6. (Optional) Scan for unreachable pages from incomplete transactions.
7. Database is ready.
```

Steps 1–5 are O(1). The recovery procedure is identical to a normal startup — there is no separate recovery mode. (Task 7 §13.2, Task 8 §14.)

### 12.3 fsync discipline

| When | Operation | Rationale |
|------|-----------|-----------|
| After all new data pages are written | `sync_all()` if file was extended, `sync_data()` otherwise | Ensures data pages are durable before superblock references them |
| After new superblock is written | `sync_data()` | Ensures superblock is durable. Does not change file size. |

These two fsyncs per commit are the minimum required for crash safety. The ordering is critical: data pages must be synced before the superblock, otherwise a crash could leave the superblock pointing to non-durable pages. (Task 8 §15, Task 9 §13.4.)

---

## 13. Constraint Validation System

### 13.1 Overview

Downstream code registers `ConstraintValidator` implementations that run at commit time. Validators receive a read-only view of the pending changes and the full database state. If any validator returns violations, the commit is rejected. (Task 6 §10.)

### 13.2 The ConstraintValidator trait

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

**Key design points:**
- `Send + Sync` because validators may be called from any thread. Validators should be stateless pure functions of their inputs. (Task 6 §10.5, decision D13.)
- `applies_to_types()` is a performance optimization: if a transaction's change set doesn't touch any types the validator cares about, it is skipped. (Task 6 §10.6.)
- Validators receive `&dyn` trait objects for context, decoupling them from the database's internal types. (Task 6 §10.6.)
- Violations are `Vec<ConstraintViolation>` (empty = pass). Multiple violations per call are allowed. (Task 6 decision D12.)

### 13.3 ChangeSet

The `ChangeSet` captures all inserts, updates, and deletes within a transaction:

```rust
pub enum NodeChange {
    Inserted(Node),
    Modified { before: Node, after: Node },
    Deleted(Node),
}

pub enum EdgeChange {
    Inserted(Edge),
    Modified { before: Edge, after: Edge },
    Deleted(Edge),
}

pub struct ChangeSet<'a> {
    node_changes: &'a [NodeChange],
    edge_changes: &'a [EdgeChange],
}
```

The ChangeSet is built from the WriteBuffer at commit time (step 6a in §11.2). It is passed to validators before any B-tree mutations occur — if validation fails, no disk I/O has been performed. (Task 7 §15, decision G17.)

### 13.4 Validation modes

- **Incremental (at commit):** Validators see only the current transaction's changes. This is the normal mode.
- **Full revalidation:** `validate_all()` synthesizes a ChangeSet treating every node and edge as newly inserted, then runs all validators. Useful after schema changes. **Performance warning:** O(N) in database size. (Task 10 §6.2, decision A8.)
- **Dry-run:** `validate()` on a write transaction runs validators against pending changes without committing. (Task 10 §6.2.)

---

## 14. Inference Hook Architecture

### 14.1 Overview

The inference system allows downstream code to register rules that derive new facts from existing facts. Inference runs **only when explicitly requested** — never automatically. (Task 6 Principle #5, Task 11 Principle #1.)

### 14.2 InferenceEngine

The `InferenceEngine` is an internal component of `Database`, consisting of three sub-components:

- **Rule Registry:** `BTreeMap<String, Box<dyn InferenceRule>>`. Protected by `RwLock` (read for invocation, write for registration).
- **Result Cache:** In-memory LRU cache keyed by `(rule_name, data_generation)`. Default capacity: 64 entries. Configurable via `DatabaseConfig::inference_cache_size`. Not persisted to disk.
- **Provenance Registry:** `BTreeMap<InferredEntity, ProvenanceRecord>` + reverse index `BTreeMap<String, Vec<InferredEntity>>`. Persisted in the Schema Store B-tree. Loaded into memory at startup.

(Task 11 §5.)

### 14.3 The InferenceRule trait

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

Rules receive the full graph state (not incremental changes) for simplicity and correctness. Incremental inference is a potential future optimization. (Task 6 §11.5, decision D18.)

### 14.4 Triggering modes

| Entry point | Transaction | Mode |
|-------------|-------------|------|
| `ReadTransaction::run_inference(name)` | Read-only snapshot | Always ephemeral |
| `ReadTransaction::run_all_inference()` | Read-only snapshot | Always ephemeral |
| `WriteTransaction::run_inference(name, mode)` | Snapshot + WriteBuffer | Caller chooses |
| `WriteTransaction::run_all_inference(mode)` | Snapshot + WriteBuffer | Caller chooses |

In write transactions, the `GraphView` provided to rules reflects the base snapshot overlaid with pending mutations (read-your-own-writes). (Task 11 §6.3.)

### 14.5 Inference modes

```rust
pub enum InferenceMode { Ephemeral, Materialized }
```

- **Ephemeral:** Results returned to the caller without writing to the graph. Natural for "what-if" queries.
- **Materialized:** Inferred facts are written to the WriteBuffer as part of the transaction. New nodes/edges receive real IDs. Materialized facts participate in constraint validation at commit time.

The mode is chosen by the **caller** per invocation, not by the rule. The same rule can be used both ways. (Task 6 decision D17, Task 11 §7.)

### 14.6 Materialization lifecycle

When materializing:

1. Validate each `InferredFact` (check referenced entities exist, types are registered).
2. **Clean up** all previously materialized facts from this rule (via the provenance reverse index).
3. Write new facts to the WriteBuffer (node/edge inserts, property updates, type assignments).
4. Record provenance for each new entity in the ProvenanceRegistry.
5. Return `InferenceResult` with a `MaterializedMapping` available via `WriteTransaction::last_materialization_mapping()`.

**Re-inference model:** The cleanup-and-reinsert approach is simpler and more correct than diff-based updates. (Task 11 §10, decision I1.)

### 14.7 Caching and invalidation

The cache uses the `transaction_id` (data generation) as a freshness key. A cache hit requires both the same rule name and the same generation. Cache entries are naturally invalidated when any commit occurs (generation increments). In write transactions with pending mutations, the cache is bypassed because pending mutations change the effective graph state. (Task 11 §9, decisions I2–I4.)

No automatic invalidation of materialized facts occurs. When base data changes, previously materialized inferred facts remain until the caller explicitly re-runs inference. This is predictable and avoids hidden performance costs. (Task 11 §9.5, decision I10.)

### 14.8 Provenance tracking

```rust
pub struct ProvenanceRecord {
    pub rule_name: String,
    pub materialized_at: u64,   // transaction ID
}

pub enum InferredEntity {
    Node(NodeId),
    Edge(EdgeId),
    NodeProperty { node: NodeId, key: PropertyKeyId },
    EdgeProperty { edge: EdgeId, key: PropertyKeyId },
    NodeType { node: NodeId, type_id: TypeId },
    EdgeType { edge: EdgeId, type_id: TypeId },
}
```

Provenance is stored in the Schema Store B-tree with key prefix `0x06` (see §19 for the corrected key encoding). The public API exposes provenance as read-only queries: `is_inferred_node()`, `is_inferred_edge()`, `node_provenance()`, `edge_provenance()`. (Task 11 §8.)

### 14.9 Sequential rule execution

`run_all_inference` executes rules in registration order, sequentially. This enables rule chaining — rule B can see rule A's materialized results. Parallel execution would prevent chaining and add synchronization complexity. (Task 11 §6.4, decision I7.)

---

## 15. Public API Surface

### 15.1 Database lifecycle

```rust
pub struct DatabaseConfig {
    mode: StorageMode,                // Persistent { path } or InMemory
    buffer_pool_frames: usize,        // default: 1024, min: 64
    page_size: usize,                 // default: 4096, must be power of two
    extension_startup_check: bool,    // default: true
    inference_cache_size: usize,      // default: 64
}

pub struct Database { /* internal state */ }
// Database: Send + Sync

impl Database {
    pub fn open(config: DatabaseConfig) -> Result<Self, Error>;
    pub fn read_txn(&self) -> Result<ReadTransaction<'_>, Error>;
    pub fn write_txn(&self) -> Result<WriteTransaction<'_>, Error>;

    // Extension registration (not transactional)
    pub fn register_constraint(&self, validator: Box<dyn ConstraintValidator>) -> Result<(), Error>;
    pub fn unregister_constraint(&self, name: &str) -> Result<bool, Error>;
    pub fn register_inference_rule(&self, rule: Box<dyn InferenceRule>) -> Result<(), Error>;
    pub fn unregister_inference_rule(&self, name: &str) -> Result<bool, Error>;
    pub fn constraint_names(&self) -> Vec<String>;
    pub fn inference_rule_names(&self) -> Vec<String>;
    pub fn missing_extensions(&self) -> MissingExtensions;

    // In-memory mode only
    pub fn snapshot_to_file(&self, path: impl AsRef<Path>) -> Result<(), Error>;
    pub fn load_from_file(&self, path: impl AsRef<Path>) -> Result<(), Error>;
}
```

Extensions are registered on `Database` (not in transactions) because they are long-lived objects. Internal locking handles thread safety. Extension names are persisted in the Schema Store so the startup check can detect missing extensions. (Task 10 §5.4, decision A3.)

### 15.2 ReadTransaction

Provides consistent-snapshot reads:

```rust
pub struct ReadTransaction<'db> { /* borrows Database; !Send, !Sync */ }

impl<'db> ReadTransaction<'db> {
    // Node/edge lookups
    pub fn get_node(&self, id: NodeId) -> Result<Option<Node>, Error>;
    pub fn get_edge(&self, id: EdgeId) -> Result<Option<Edge>, Error>;
    pub fn all_nodes(&self) -> Result<Vec<Node>, Error>;

    // Traversal
    pub fn outgoing_edges(&self, node: NodeId, edge_type: Option<TypeId>) -> Result<Vec<Edge>, Error>;
    pub fn incoming_edges(&self, node: NodeId, edge_type: Option<TypeId>) -> Result<Vec<Edge>, Error>;
    pub fn neighbors(&self, node: NodeId, edge_type: Option<TypeId>) -> Result<Vec<Node>, Error>;

    // Type-based queries
    pub fn nodes_by_type(&self, type_id: TypeId, include_subtypes: bool) -> Result<Vec<Node>, Error>;
    pub fn edges_by_type(&self, type_id: TypeId, include_subtypes: bool) -> Result<Vec<Edge>, Error>;
    pub fn nodes_by_property(&self, key: PropertyKeyId, value: &Value) -> Result<Vec<Node>, Error>;

    // Counting
    pub fn node_count(&self) -> Result<u64, Error>;
    pub fn edge_count(&self) -> Result<u64, Error>;
    pub fn outgoing_edge_count(&self, node: NodeId, edge_type: Option<TypeId>) -> Result<u64, Error>;
    pub fn incoming_edge_count(&self, node: NodeId, edge_type: Option<TypeId>) -> Result<u64, Error>;

    // Schema
    pub fn type_registry(&self) -> &dyn TypeRegistryView;
    pub fn property_key_registry(&self) -> &dyn PropertyKeyRegistryView;

    // Inference (always ephemeral)
    pub fn run_inference(&self, rule_name: &str) -> Result<InferenceResult, Error>;
    pub fn run_all_inference(&self) -> Result<Vec<InferenceResult>, Error>;

    // Provenance queries
    pub fn is_inferred_node(&self, id: NodeId) -> Result<bool, Error>;
    pub fn is_inferred_edge(&self, id: EdgeId) -> Result<bool, Error>;
    pub fn node_provenance(&self, id: NodeId) -> Result<Option<ProvenanceRecord>, Error>;
    pub fn edge_provenance(&self, id: EdgeId) -> Result<Option<ProvenanceRecord>, Error>;

    pub fn finish(self);
}
```

### 15.3 WriteTransaction

Provides read-your-own-writes + mutations:

```rust
pub struct WriteTransaction<'db> { /* borrows Database; !Send, !Sync */ }

impl<'db> WriteTransaction<'db> {
    // All ReadTransaction read methods (see above), seeing pending changes

    // Schema mutations
    pub fn register_type(&mut self, definition: TypeDefinition) -> Result<TypeId, Error>;
    pub fn update_type(&mut self, definition: TypeDefinition) -> Result<(), Error>;
    pub fn add_supertype(&mut self, child: TypeId, parent: TypeId) -> Result<(), Error>;
    pub fn remove_supertype(&mut self, child: TypeId, parent: TypeId) -> Result<bool, Error>;
    pub fn get_or_create_property_key(&mut self, name: &str) -> Result<PropertyKeyId, Error>;

    // Node mutations
    pub fn insert_node(&mut self, node: Node) -> Result<NodeId, Error>;
    pub fn update_node(&mut self, node: Node) -> Result<(), Error>;
    pub fn delete_node(&mut self, id: NodeId) -> Result<(), Error>;  // cascading delete
    pub fn set_node_property(&mut self, node_id: NodeId, key: PropertyKeyId, value: Value) -> Result<(), Error>;
    pub fn remove_node_property(&mut self, node_id: NodeId, key: PropertyKeyId) -> Result<Option<Value>, Error>;
    pub fn add_node_type(&mut self, node_id: NodeId, type_id: TypeId) -> Result<(), Error>;
    pub fn remove_node_type(&mut self, node_id: NodeId, type_id: TypeId) -> Result<bool, Error>;

    // Edge mutations
    pub fn insert_edge(&mut self, edge: Edge) -> Result<EdgeId, Error>;
    pub fn update_edge(&mut self, edge: Edge) -> Result<(), Error>;  // endpoints immutable
    pub fn delete_edge(&mut self, id: EdgeId) -> Result<(), Error>;
    pub fn set_edge_property(&mut self, edge_id: EdgeId, key: PropertyKeyId, value: Value) -> Result<(), Error>;
    pub fn remove_edge_property(&mut self, edge_id: EdgeId, key: PropertyKeyId) -> Result<Option<Value>, Error>;
    pub fn add_edge_type(&mut self, edge_id: EdgeId, type_id: TypeId) -> Result<(), Error>;
    pub fn remove_edge_type(&mut self, edge_id: EdgeId, type_id: TypeId) -> Result<bool, Error>;

    // Inference (caller-chosen mode)
    pub fn run_inference(&mut self, rule_name: &str, mode: InferenceMode) -> Result<InferenceResult, Error>;
    pub fn run_all_inference(&mut self, mode: InferenceMode) -> Result<Vec<InferenceResult>, Error>;
    pub fn last_materialization_mapping(&self) -> Option<&MaterializedMapping>;

    // Validation
    pub fn validate(&self) -> Result<Vec<ConstraintViolation>, Error>;
    pub fn validate_all(&self) -> Result<Vec<ConstraintViolation>, Error>;

    // Lifecycle
    pub fn commit(self) -> Result<(), Error>;
    pub fn abort(self);
}
```

### 15.4 GraphReader trait

Both transaction types implement a shared read trait for generic code:

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

Note: `GraphReader` (public API, returns owned values, fallible) is distinct from `GraphView` (internal, returns borrowed values, used by validators/rules). (Task 10 §10.2.)

### 15.5 Builder helpers

```rust
pub struct NodeBuilder { /* ... */ }
impl NodeBuilder {
    pub fn new() -> Self;
    pub fn type_label(self, t: TypeId) -> Self;
    pub fn property(self, k: PropertyKeyId, v: Value) -> Self;
    pub fn anonymous(self) -> Self;
    pub fn build(self) -> Node;
}

pub struct EdgeBuilder { /* ... */ }
impl EdgeBuilder {
    pub fn new(source: NodeId, target: NodeId) -> Self;
    pub fn type_label(self, t: TypeId) -> Self;
    pub fn property(self, k: PropertyKeyId, v: Value) -> Self;
    pub fn build(self) -> Edge;
}

pub struct TypeBuilder { /* ... */ }
impl TypeBuilder {
    pub fn node(name: impl Into<String>) -> Self;
    pub fn edge(name: impl Into<String>) -> Self;
    pub fn supertype(self, t: TypeId) -> Self;
    pub fn property(self, decl: PropertyDeclaration) -> Self;
    pub fn closed(self) -> Self;
    pub fn metadata(self, k: PropertyKeyId, v: Value) -> Self;
    pub fn build(self) -> TypeDefinition;
}
```

Builders eliminate placeholder IDs and PropertyMap ceremony. Direct struct construction remains available. (Task 10 §14, decision A6.)

### 15.6 Error types

```rust
pub enum Error {
    Schema(SchemaError),
    ConstraintViolation(Vec<ConstraintViolation>),
    Storage(StorageError),
    NotFound(NotFoundError),
    Transaction(TransactionError),
    Inference(InferenceError),
}
```

All public methods return `Result<T, Error>`. The `ConstraintViolation` variant carries all violations from all validators, enabling the caller to report all problems at once. (Task 10 §4.)

---

## 16. Cross-Cutting Concerns

### 16.1 Error handling strategy

- **HAL layer:** Each backend defines its own concrete error type implementing `StorageError`. Errors carry a `StorageErrorKind` category for generic handling.
- **Storage engine:** HAL errors are wrapped in `error::StorageError` (type-erased: message string + optional `io::Error` source).
- **Public API:** All errors converge into `error::Error`, a single enum. Callers use `?` uniformly. Pattern matching on variants distinguishes error kinds.
- **Panics:** Reserved for programmer errors only (e.g., using a transaction after commit). All recoverable conditions return `Result`.

### 16.2 Naming conventions

- **Types:** PascalCase, domain-specific prefixes avoided (e.g., `Node` not `GraphNode`).
- **Methods:** snake_case, verb-first for mutations (`insert_node`, `delete_edge`), noun-first for accessors (`type_registry`, `node_count`).
- **Transaction methods:** `read_txn`, `write_txn` — short and idiomatic, consistent with redb and other Rust database crates. (Task 10 decision A11.)
- **Feature flags:** `std` (default), `alloc` — minimal, no proliferation. (Task 9 §3.)
- **Module names:** lowercase, underscore-separated for multi-word (`hal_std`, `hal_mem`, `write_buffer`).

### 16.3 Concurrency guarantees

| Component | Thread safety | Mechanism |
|-----------|--------------|-----------|
| `Database` | `Send + Sync` | Internal `Mutex` / `RwLock` |
| `ReadTransaction` | `!Send`, `!Sync` | Holds buffer pool references |
| `WriteTransaction` | `!Send`, `!Sync` | Holds buffer pool references + WriteBuffer |
| `ConstraintValidator` | `Send + Sync` required | Stateless; called from any thread |
| `InferenceRule` | `Send + Sync` required | Stateless; called from any thread |
| `InferenceEngine` | Internal `RwLock` on rule registry | Read lock for invocation, write lock for registration |
| Buffer pool | Internal synchronization | `Mutex` on page table; `RwLock` on page frames |

### 16.4 `no_std + alloc` boundary

Everything in `types/`, `schema/`, `constraint/`, `inference/`, `error/` (core), `hal/`, and `hal_mem/` compiles under `no_std + alloc`. This includes all core data types, all extension traits, the inference provenance types, and the memory backend.

The `db/`, `hal_std/`, and `storage/` modules (the database engine) require `std` for: `Mutex`, `RwLock`, `HashMap` (buffer pool page table), file I/O, system clocks, and OS-level file locking.

### 16.5 Serialization strategy

All on-disk serialization uses **custom binary encoding** — no serde, bincode, or CBOR. This provides exact control over layout, minimizes allocation, and avoids external dependencies. The serialization format is:

- **Keys:** Big-endian integers, concatenated. Byte-level lexicographic order matches semantic order.
- **Values:** Little-endian integers. Variable-length fields use 2-byte or 4-byte length prefixes.
- **Properties:** Each property is `[PropertyKeyId: 4 LE] [type_tag: 1] [payload]`. Strings/bytes have 4-byte length prefixes. Lists are recursive.
- **Records:** Fixed-size header + variable-length property data.

(Task 7 §5, §7.4, decisions G4–G5, G13.)

### 16.6 Dependency policy

- **Core (`no_std + alloc`):** Only `crc32fast`. No other external dependencies.
- **`std` feature:** `libc` (Unix) and `windows-sys` (Windows) for FFI. These are thin bindings, not database crates.
- **Explicitly prohibited:** External database crates, heavyweight serialization frameworks in core.
- **Acceptable but not required:** `serde` as an optional feature for user-facing types (not in the core serialization path).

### 16.7 Documentation requirements

Every public item must have a doc comment. Every method documents its errors, panics (if any), and performance characteristics. Module-level documentation explains the subsystem's purpose and relationship to other modules. The crate root documentation provides a quick-start example and architecture overview.

---

## 17. Consolidated Design Decision Log

This section consolidates all significant design decisions from Tasks 6–11. Each entry includes the source task and section for full rationale.

| ID | Decision | Alternatives Considered | Rationale | Source |
|----|----------|------------------------|-----------|--------|
| D1 | 64-bit node/edge IDs, 32-bit type/property-key IDs | Uniform 64-bit; 128-bit UUIDs | Sufficient range; 32-bit saves space for types/keys | Task 6 §3 |
| D2 | Reserved `*Id(0)` as null sentinels | `Option<*Id>` everywhere | Avoids `Option` overhead in fixed-size records | Task 6 §3 |
| D3 | Anonymous nodes via `is_anonymous` flag | Separate `BlankNodeId` type | Uniform data path | Task 6 §6 |
| D4 | `Value::LangString` dedicated variant | Convention over string naming | Self-describing; simplifies SKOS/RDF | Task 6 §4 |
| D5 | No `Value::Map` variant | Nested map support | Model as subgraphs; simpler serialization | Task 6 §4 |
| D6 | `BTreeMap` for property bags | `HashMap`; `Vec<(K,V)>` | `no_std` compatible; deterministic order | Task 6 §5 |
| D7 | Sorted `Vec<TypeId>` for type labels | `BTreeSet`; `SmallVec` | Small sets; compact; binary search | Task 6 §6 |
| D8 | Separate name namespaces for node/edge types | Single namespace | Avoids artificial conflicts; matches RDF/OWL | Task 6 §7 |
| D9 | Edge endpoint constraints as metadata | Dedicated fields | Keeps TypeDefinition uniform; core doesn't interpret | Task 6 §7 |
| D10 | Property declaration shadowing in inheritance | Reject duplicates; merge | Matches frame inheritance; subtype is more specific | Task 6 §7 |
| D11 | Validators receive `&dyn GraphView` + `&ChangeSet` | Only ChangeSet; full clone | ChangeSet for incremental; GraphView for cross-reference | Task 6 §10 |
| D12 | Validators return `Vec<Violation>` (empty=pass) | `Result<(), Vec<Violation>>` | Simpler; no ambiguity | Task 6 §10 |
| D13 | `Send + Sync` on validators and rules | No thread-safety | Required for multi-threaded database | Task 6 §10 |
| D14 | Extensions as `Box<dyn Trait>`, names persisted | Serialize logic | Cannot serialize Rust logic; name-based re-registration | Task 6 §12 |
| D15 | Replacement on duplicate extension name | Reject duplicates | Simplifies upgrades | Task 6 §12 |
| D16 | Named subgraphs as property convention | First-class field | Minimal core impact | Task 6 §13 |
| D17 | Inference mode chosen by caller, not rule | Mode on rule | Same rule may be used both ways | Task 6 §11 |
| D18 | `infer()` receives full graph, not change set | Incremental change set | Simpler; correct; incremental is optimization | Task 6 §11 |
| G1 | Unified CoW B-trees (no slot stores) | Hybrid IFA + B-tree | Eliminates CoW/slot tension; uniform crash safety | Task 7 §3 |
| G2 | Seven data B-trees + one infrastructure B-tree | Fewer; more per-type | Covers all GraphView access patterns | Task 7 §4, Task 8 §11 |
| G4 | Big-endian keys for B-tree sort | Little-endian; custom comparator | memcmp sufficiency | Task 7 §6 |
| G5 | Little-endian record values | Big-endian | Avoids byte-swap on x86/ARM | Task 7 §5 |
| G6 | Inline properties ≤256 bytes, overflow beyond | All inline; all out-of-line | Covers common case; prevents page bloat | Task 7 §7 |
| G7 | Single-writer MVCC via CoW | Multi-writer 2PL; SSI | No write conflicts; no deadlocks; read-heavy workloads | Task 7 §11 |
| G8 | Snapshot Isolation (Serializable via single writer) | Read Committed; SSI | Full serializability without SSI overhead | Task 7 §11 |
| G9 | Clock eviction for buffer pool | LRU; LRU-K | O(1) amortized; simple; adequate quality | Task 7 §10 |
| G10 | ID recycling via Freelist B-tree | No recycling; bitmap | Consistent with unified B-tree; avoids space waste | Task 7 §14 |
| G11 | Schema cached in memory | On-demand lookups | Small data; read on every operation | Task 7 §8 |
| G13 | Custom binary property serialization | serde + bincode | No external deps; exact control; minimal allocation | Task 7 §7 |
| G14 | Multiplexed Schema Store (one B-tree) | Separate B-trees | Schema small; reduces root pointer count | Task 7 §8 |
| G17 | ChangeSet built from WriteBuffer at commit | Stream during txn | Matches validator API contract | Task 7 §15 |
| F1 | Dual-superblock atomic commit (no WAL) | WAL; journal | CoW eliminates WAL need; 2-fsync commit | Task 8 §4 |
| F2 | CRC32C checksums per page | SHA-256; Adler-32 | Fast (hardware accelerated); sufficient for corruption detection | Task 8 §5 |
| F8 | Deferred secondary freed pages | Recursive insertion; WAL | Breaks circularity; typically 1–3 pages | Task 8 §11 |
| F12 | 192-byte superblock with 64 reserved | Minimal; fill page | Room for evolution without format break | Task 8 §4 |
| H1 | Three-trait decomposition (ReadAt+WriteAt+Sync) | Single trait | Clear responsibility per trait; blanket impl | Task 9 §5–6 |
| H2 | `ReadAt` takes `&self`; write/sync take `&mut` | All `&mut`; all `&self` | Concurrent reads; serialized writes | Task 9 §5 |
| H3 | Two sync methods (sync_data, sync_all) | Single flush() | Required by fsync discipline | Task 9 §5 |
| H7 | Non-blocking lock only | Also blocking | Fail fast on contention | Task 9 §8 |
| H10 | Type erasure at API boundary | Propagate generic E | Prevents backend type leaking | Task 9 §12 |
| A1 | Transactions as unit of work (no auto-commit) | Auto-commit; implicit txn | Explicit; clear performance characteristics | Task 10 §2 |
| A2 | `commit(self)` consumes transaction | `commit(&mut self)` | Simplicity; no ambiguous pending state | Task 10 §6 |
| A5 | Owned returns (`Vec<Node>`) | Borrowed; iterator | Simpler; no buffer pool leakage | Task 10 §10 |
| A9 | Cascading delete on node deletion | Manual edge removal | Prevents dangling edges; least surprising | Task 10 §8 |
| A10 | Immutable edge endpoints | Allow endpoint change | Semantically different relationship; delete+create | Task 10 §9 |
| A12 | Transactions `!Send`, `!Sync` | Make `Send` | Avoids per-page-access atomic overhead | Task 10 §6 |
| I1 | Cleanup-and-reinsert on re-inference | Diff-based update | Simpler; correct | Task 11 §10 |
| I2 | In-memory cache, not persisted | Persist to disk | Eliminates coherence bugs | Task 11 §9 |
| I3 | Generation-based cache keying (txn_id) | Timestamp; hash | Cheap; monotonic; already maintained | Task 11 §9 |
| I5 | Provenance in Schema Store B-tree | Dedicated B-tree; per-record flag | Avoids superblock change; doesn't pollute data model | Task 11 §8 |
| I7 | Sequential rule execution | Parallel | Enables rule chaining | Task 11 §6 |
| I10 | No automatic invalidation of materialized facts | Auto-invalidate | Predictable; no hidden costs | Task 11 §9 |

---

## 18. Known Limitations and Deferred Work

### 18.1 v1 limitations

1. **`nodes_by_property()` full scan.** No property value index in v1. Adequate for commit-time validation; may be slow for large graphs in hot-path queries. A future eighth data B-tree keyed by `(PropertyKeyId, ValueHash, EntityKindTag, EntityId)` is designed in `007-graph-storage-model.md` §7.5. (Task 7 residual #1.)

2. **Owned return values.** Query methods return `Vec<Node>` / `Vec<Edge>`, materializing entire result sets. A cursor-based iterator API is deferred to avoid v1 API complexity. (Task 10 residual #1.)

3. **No batch insert API.** Single-entity inserts are functional but slow for graph import. A `batch_insert_nodes(Vec<Node>)` optimization is deferred. (Task 10 residual #2.)

4. **Write lock timeout.** `write_txn()` blocks indefinitely. A configurable timeout returning `Error::Transaction(WriteLockTimeout)` is easy to add to `DatabaseConfig`. (Task 10 residual #3.)

5. **`include_subtypes` cost.** Requires one Type Index range scan per subtype. Deep hierarchies with many subtypes may be slow. A materialized "all subtypes" index is a possible future optimization. (Task 7 residual #3.)

6. **Provenance memory footprint.** Loaded entirely into memory. For databases with millions of inferred entities, this could consume ~50 MB. Lazy loading is a future optimization. (Task 11 residual #2.)

7. **Deferred secondary freed pages.** 1–3 pages may be temporarily leaked per transaction that triggers freelist B-tree splits. Recovered by `compact()`. (Task 8 residual #1.)

### 18.2 Deferred features

- **Property value index** (backward-compatible B-tree addition)
- **Cursor/iterator-based query API** (API extension, not breaking)
- **Batch insert/import API** (performance optimization)
- **Write lock timeout** (configuration addition)
- **Type-aware cache invalidation** for inference (Task 11 §9.6)
- **Async I/O** in the HAL (Task 9 goal G3 defers this explicitly)

### 18.3 `Value` equality concern

`Value` implements `PartialEq` but not `Eq` (due to `f64`). For `nodes_by_property` comparisons, `PartialEq` works correctly for all types except NaN (which returns `false`). This is documented behavior. If total ordering is needed in the future, a wrapper type with NaN/−0 conventions can be introduced. (Task 6 residual #1, Task 10 residual #4.)

---

## 19. Consolidated B-Tree Catalog and Schema Store Key Map

This section serves as the authoritative reference for the complete B-tree catalog and the Schema Store key encoding, resolving conflicts between upstream documents.

### 19.1 Complete B-tree catalog (8 trees)

| # | Name | Superblock field | Key encoding | Value |
|---|------|-----------------|--------------|-------|
| 1 | Node Store | `root_node_store` | `NodeId` (8 bytes BE) | NodeRecord |
| 2 | Edge Store | `root_edge_store` | `EdgeId` (8 bytes BE) | EdgeRecord |
| 3 | Outgoing Adjacency Index | `root_outgoing_adj` | `NodeId‖TypeId‖EdgeId` (20 bytes BE) | ∅ |
| 4 | Incoming Adjacency Index | `root_incoming_adj` | `NodeId‖TypeId‖EdgeId` (20 bytes BE) | ∅ |
| 5 | Type Index | `root_type_index` | `TypeKindTag‖TypeId‖EntityId` (13 bytes BE) | ∅ |
| 6 | Schema Store | `root_schema_store` | See §19.2 | Variable |
| 7 | ID Freelist | `root_id_freelist` | `EntityKindTag‖EntityId` (9 bytes BE) | ∅ |
| 8 | Page Freelist | `root_page_freelist` | `FreedTxnId‖PageId` (16 bytes BE) | ∅ |

Trees 1–7 are defined by Task 7 §4. Tree 8 (Page Freelist) is defined by Task 8 §11. The superblock stores root page IDs for all eight.

### 19.2 Schema Store key encoding (authoritative)

**Conflict resolution:** Task 7 §6.6 assigns prefix `0x03` to monotonic counters. Task 11 §8.4 also claims prefix `0x03` for provenance records. **This document resolves the collision by assigning provenance to prefix `0x06`.**

| Prefix | Key format | Value | Source |
|--------|-----------|-------|--------|
| `0x01` | `[TypeId: 4B]` | Serialized TypeDefinition | Task 7 §8.1 |
| `0x02` | `[PropertyKeyId: 4B]` | Serialized PropertyKeyDefinition | Task 7 §8.2 |
| `0x03` | `[counter_name: 1B]` | Counter value (u64 LE) | Task 7 §6.6 |
| `0x04` | `[child TypeId: 4B] [parent TypeId: 4B]` | ∅ (key-only hierarchy edge) | Task 7 §8.3 |
| `0x05` | `[extension_kind: 1B] [name_len: 2B] [name: bytes]` | ∅ (key-only extension name) | Task 7 §8.4 |
| `0x06` | `[entity_kind: 1B] [entity_id: 8B BE] [sub_id: 4B BE]` | Provenance value (see below) | Task 11 §8.4 (prefix corrected) |
| `0x07`–`0xFF` | Reserved for future use | — | — |

**Counter names** (prefix `0x03`): `0x01`=next NodeId, `0x02`=next EdgeId, `0x03`=next TypeId, `0x04`=next PropertyKeyId.

**Extension kinds** (prefix `0x05`): `0x01`=constraint validator, `0x02`=inference rule.

**Provenance entity kinds** (prefix `0x06`): `0x01`=Node, `0x02`=Edge, `0x03`=NodeProperty, `0x04`=EdgeProperty, `0x05`=NodeType, `0x06`=EdgeType. For Node/Edge entities, `sub_id` is zero. For property/type entities, `sub_id` is the `PropertyKeyId` or `TypeId`.

**Provenance value encoding:** `[txn_id: 8B LE] [rule_name_len: 2B LE] [rule_name: UTF-8 bytes]`.

---

## 20. Document Cross-Reference Index

This section maps every design topic to both its authoritative section in this document and the upstream sub-document for extended detail.

| Topic | This document | Upstream detail |
|-------|--------------|-----------------|
| Core types (Node, Edge, Value, IDs) | §4 | `006` §§3–6 |
| Type system and registry | §5 | `006` §§7–9 |
| Type hierarchy DAG | §5.2 | `006` §8 |
| Property key registry | §5.4 | `006` §9 |
| Named subgraphs | §5.5 | `006` §13 |
| Constraint validation | §13 | `006` §10 |
| Inference rule trait | §14.3 | `006` §11 |
| Extension lifecycle | §15.1 | `006` §12, `010` §13 |
| B-tree architecture decision | §6.1 | `007` §3 |
| B-tree catalog | §6.2, §19 | `007` §4, `008` §11 |
| Record formats (byte-level) | §6.3 | `007` §5 |
| Key encoding (byte-level) | §6.2 | `007` §6 |
| Property storage (inline/overflow) | §6.4 | `007` §7 |
| Schema-to-storage mapping | §6.5 | `007` §9 |
| Buffer pool | §9 | `007` §10 |
| Concurrency model | §10 | `007` §11 |
| Transaction lifecycle | §11 | `007` §12 |
| Crash safety | §12 | `007` §13, `008` §14 |
| ChangeSet production | §13.3 | `007` §15 |
| ID allocation/recycling | §6.6 | `007` §14 |
| Performance characteristics | — | `007` §16 |
| File structure (byte-level) | §7 | `008` §§2–10 |
| Dual superblock | §7.2 | `008` §4 |
| Page header | §7.3 | `008` §5 |
| Page types | §7.4 | `008` §§6–10 |
| Free-space management | §7.5 | `008` §11 |
| Commit protocol | §11.2 | `008` §13 |
| fsync discipline | §12.3 | `008` §15 |
| Versioning/extensibility | §7.6 | `008` §16 |
| HAL traits | §8 | `009` §§4–6 |
| FileBackend | §8.4 | `009` §9 |
| MemoryBackend | §8.4 | `009` §10 |
| File locking | §8.3 | `009` §8 |
| Error propagation | §8.5, §16.1 | `009` §12 |
| Public API (all methods) | §15 | `010` §§5–14 |
| Database lifecycle | §15.1 | `010` §5 |
| Transaction API | §15.2–15.3 | `010` §6 |
| GraphReader trait | §15.4 | `010` §10 |
| Builder helpers | §15.5 | `010` §14 |
| Error types | §15.6 | `010` §4 |
| Inference engine internals | §14.2 | `011` §5 |
| Inference triggering | §14.4 | `011` §6 |
| Inference caching | §14.7 | `011` §9 |
| Provenance tracking | §14.8 | `011` §8 |
| Materialization lifecycle | §14.6 | `011` §10 |
| Schema Store key map (authoritative) | §19.2 | `007` §6.6, `011` §8.4 |
| Out of scope items | — | `006` §14 |

---

## Completion Report: Task 12 — Design Synthesis

### Status: COMPLETE

### Done Criterion:

The criterion requires:

1. **Schema and extension system** — ✓ Sections 4, 5, 13, 14, and 15.1 (extension registration).
2. **Graph storage model** — ✓ Section 6 (B-tree catalog, record formats, property storage, schema mapping, ID allocation).
3. **File format** — ✓ Section 7 (file structure, dual superblock, page header, page types, free-space management, versioning).
4. **HAL traits** — ✓ Section 8 (core traits, error types, lifecycle, backends, error propagation).
5. **Public API** — ✓ Section 15 (Database, ReadTransaction, WriteTransaction, GraphReader, builders, errors).
6. **Inference hook architecture** — ✓ Section 14 (engine, trait, modes, materialization, caching, provenance).
7. **Cross-cutting concerns (error handling, concurrency, naming, crate structure, feature flags, dependencies)** — ✓ Section 16.
8. **Every design decision includes its rationale** — ✓ Rationale is integrated throughout; Section 17 provides a consolidated decision log with 50+ entries, each citing its source.
9. **No design question left answered only in a sub-document** — ✓ All decisions are reproduced or summarized here. Sub-documents are referenced only for byte-level exhaustive detail.

All criteria met.

### Deliverables:

- `012-design-document.md` — this document

### Summary:

Synthesized all design decisions from Tasks 6–11 into a single authoritative reference document covering the complete architecture of the embedded graph database. The document is organized into 20 sections spanning the full stack from HAL traits through the public API.

**Key synthesis contributions beyond aggregation:**

1. **Resolved the Schema Store key prefix collision.** Task 7 assigned prefix `0x03` to monotonic counters; Task 11 independently claimed `0x03` for provenance records. This document assigns provenance to prefix `0x06` and establishes the authoritative key encoding map in §19.2.

2. **Established the authoritative B-tree count at eight.** Task 7 defined seven data B-trees; Task 8 added the Page Freelist infrastructure B-tree. This document consolidates both into a single catalog (§19.1) and confirms all eight root page IDs must appear in the superblock.

3. **Incorporated Task 11's API additions.** The provenance query methods (`is_inferred_node`, `node_provenance`, etc.), `MaterializedMapping`, `last_materialization_mapping()`, and `inference_cache_size` configuration — all introduced by Task 11 — are woven into the consolidated API surface (§15).

4. **Unified error propagation chain.** Traced the complete error path from HAL through storage engine to public API, making the type erasure boundary explicit (§8.5, §16.1).

5. **Consolidated 50+ design decisions.** Section 17 provides a single-table reference of every significant design choice with its rationale and source document.

### Context for Next Task:

**Task 13 (Generate: Top-Level CLAUDE.md)** should read `012-design-document.md` (this deliverable) as its primary input. The CLAUDE.md governs Claude Code's behavior for all implementation sessions. Key items for Task 13:

- The crate structure (§3.1) and feature flags (§3.2) define the project layout.
- The dependency policy (§3.3, §16.6) defines what external crates are allowed.
- The `no_std + alloc` boundary (§16.4) determines which modules compile under which feature configurations.
- The naming conventions (§16.2) and documentation requirements (§16.7) should be codified as project-wide rules in CLAUDE.md.
- The design decision log (§17) captures decisions that implementation should not contradict.

### Residual Concerns:

1. **`hal::Sync` naming conflict with `core::marker::Sync`.** Task 9 residual #1 flagged this. The recommendation stands: use module-qualified `hal::Sync` or rename to `DurabilityControl` / `StorageSync` if implementation proves awkward. Task 15 decides.

2. **Error type for `dyn StorageBackend`.** Task 9 residual #4 noted that `dyn StorageBackend` needs a concrete error type. `Box<dyn StorageError>` works but allocates on the error path. A `BoxedStorageError` wrapper is an alternative. Task 15 should finalize.

3. **`Value` without `Eq`.** Task 6 residual #1. Documented in §18.3. No action required for v1; a total-ordering wrapper can be introduced if needed.

4. **Dual representation of type hierarchy.** Task 6 residual #5. The type hierarchy exists both in the registry DAG and potentially as graph edges created by downstream OWL code. Keeping them in sync is the downstream crate's responsibility. The core does not auto-create graph edges on hierarchy changes.

5. **Extension name collision across kinds.** Task 6 residual #3, Task 10 residual #5. A constraint validator and inference rule can share a name. Documented as a convention issue; namespacing (`"constraint::X"`, `"inference::Y"`) is recommended in documentation.

### Upstream Flags:

None. This task synthesizes all upstream work. All inter-document conflicts have been resolved within this document (specifically the Schema Store key prefix collision in §19.2). No sibling or parent-level concerns remain.
