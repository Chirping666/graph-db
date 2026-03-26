# 007 — Graph Storage Model Design Specification

**Project:** Embedded Graph Database with Extensible Schema & Pluggable Inference  
**Task:** 7 — Design: Graph Storage Model  
**Status:** Complete  
**Depends on:** Task 1 (`001-db-internals-fundamentals.md`), Task 2 (`002-graph-storage-strategies.md`), Task 6 (`006-schema-extension-spec.md`)  
**Intended audience:** All downstream design and implementation tasks. A reader familiar with Rust and basic database concepts should be able to understand every data structure, layout, and design decision in this document without reference to external sources.

---

## Table of Contents

1. [Purpose and Scope](#1-purpose-and-scope)
2. [Architecture Overview: Unified CoW B-Tree](#2-architecture-overview-unified-cow-b-tree)
3. [The Pivotal Decision: Unified B-Trees vs. Hybrid Slot Stores](#3-the-pivotal-decision-unified-b-trees-vs-hybrid-slot-stores)
4. [B-Tree Catalog: Logical Trees in the Database](#4-b-tree-catalog-logical-trees-in-the-database)
5. [On-Disk Record Formats](#5-on-disk-record-formats)
6. [B-Tree Key Encoding and Ordering](#6-b-tree-key-encoding-and-ordering)
7. [Property Storage](#7-property-storage)
8. [Schema/Extension Metadata Storage](#8-schemaextension-metadata-storage)
9. [How the Schema/Extension Layer Maps to the Storage Layer](#9-how-the-schemaextension-layer-maps-to-the-storage-layer)
10. [Buffer Pool Design](#10-buffer-pool-design)
11. [Concurrency Control Strategy](#11-concurrency-control-strategy)
12. [Transaction Lifecycle](#12-transaction-lifecycle)
13. [Crash Safety and Recovery](#13-crash-safety-and-recovery)
14. [ID Allocation and Recycling](#14-id-allocation-and-recycling)
15. [ChangeSet Production for Constraint Validators](#15-changeset-production-for-constraint-validators)
16. [Performance Characteristics](#16-performance-characteristics)
17. [Design Decision Log](#17-design-decision-log)

---

## 1. Purpose and Scope

This document is the authoritative specification for the **graph storage model** — the in-memory and on-disk data representation for nodes, edges, properties, type metadata, and extension-registered metadata.

### What this document defines

- The choice of primary storage structure (CoW B-trees) and its justification
- The set of logical B-trees that compose the database
- On-disk record formats at the byte level for all stored entities
- B-tree key encoding schemes for each index
- Property bag storage strategy (inline small + overflow large)
- Schema and extension metadata persistence
- How every type from `006-schema-extension-spec.md` maps to the storage layer
- Buffer pool architecture (page frames, eviction, pinning)
- Concurrency control strategy (single-writer MVCC via CoW snapshots)
- Transaction lifecycle from begin through commit/abort
- Crash safety guarantees and recovery procedure
- ID allocation and recycling
- Performance characteristics with justification

### What this document does NOT define

- The binary file format (header layout, page table, versioning, free-space map layout) — Task 8
- The HAL trait abstraction for storage I/O — Task 9
- The public Rust API surface — Task 10
- Inference caching and invalidation infrastructure — Task 11

### Relationship to upstream documents

This document makes binding decisions that were left open by Tasks 1 and 2:

- **Task 1** recommended CoW B-trees and single-writer MVCC but deferred the formal decision to this task. **Decision: adopted.**
- **Task 2** recommended a hybrid index-free adjacency + CoW B-tree architecture as the primary option, with a pure edge-centric B-tree as a simpler alternative. It flagged the CoW-vs-slot-store interaction as the most significant open question. **Decision: we adopt the pure B-tree approach (Task 2's "simpler alternative"), with adjacency indexes that recover most traversal performance. Full rationale in Section 3.**
- **Task 6** defined the `Node`, `Edge`, `Value`, `TypeDefinition`, `PropertyMap`, and all trait interfaces. This document maps each of those types to concrete on-disk representations.

---

## 2. Architecture Overview: Unified CoW B-Tree

The storage model uses **copy-on-write B+ trees as the single storage primitive for all data**. Every piece of persistent state — node records, edge records, property blocks, adjacency indexes, type indexes, schema metadata — is stored in a CoW B+ tree (or in overflow pages reachable from one).

```
┌─────────────────────────────────────────────────────────────────────┐
│                         FILE HEADER                                 │
│  (root page IDs for each B-tree; monotonic counters; metadata)      │
├──────────┬──────────┬──────────┬──────────┬──────────┬──────────────┤
│  Node    │  Edge    │  Outgoing│  Incoming│   Type   │   Schema     │
│  B-tree  │  B-tree  │  Adj.    │  Adj.    │   Index  │   B-tree     │
│          │          │  Index   │  Index   │  B-tree  │              │
│ key:     │ key:     │ key:     │ key:     │ key:     │ key:         │
│ NodeId   │ EdgeId   │(src,type,│(tgt,type,│(type,    │ TypeId /     │
│          │          │  edge)   │  edge)   │  entity) │ PropKeyId    │
│ val:     │ val:     │          │          │          │              │
│ NodeRec  │ EdgeRec  │ (empty)  │ (empty)  │ (empty)  │ TypeDef /    │
│          │          │          │          │          │ PropKeyDef   │
├──────────┴──────────┴──────────┴──────────┴──────────┴──────────────┤
│                   Property Overflow Pages                           │
│            (large property blobs, referenced by PageId)             │
├─────────────────────────────────────────────────────────────────────┤
│                   Free Page List / Bitmap                           │
└─────────────────────────────────────────────────────────────────────┘
```

All B-trees share the same page pool and page format. The file header stores the root page ID for each logical B-tree. On commit, modified B-tree paths are written to new pages, and the file header is atomically updated with the new root page IDs. This provides MVCC (readers hold old roots), crash safety (old roots remain valid until new header is committed), and a uniform implementation path.

---

## 3. The Pivotal Decision: Unified B-Trees vs. Hybrid Slot Stores

Task 2 identified two viable architectures:

**Option A — Hybrid Index-Free Adjacency + B-Tree Indexes:**
Fixed-size slot stores for node/edge records with embedded adjacency pointers (doubly-linked lists of edges per node), plus CoW B-tree secondary indexes for type scans and filtered queries.

**Option B — Unified CoW B-Trees:**
All data stored in B-trees. No slot stores. Adjacency encoded as B-tree index entries. Node and edge records are B-tree leaf values.

### Why we chose Option B (Unified B-Trees)

**1. Eliminates the CoW/slot-store tension.**
Task 2 flagged the interaction between CoW semantics and direct-addressed slot stores as the most uncertain design area. Slot stores are direct-addressed arrays where `record_offset = base + id × record_size`. CoW requires that a write never modifies a page in place — it writes a new copy. With slot stores, a single edge insertion modifies at minimum three records (source node, target node, new edge), potentially across three different pages, each of which must be CoW-copied. Worse, the doubly-linked adjacency list means updating a node's "first edge" pointer also requires finding the old first edge and updating its "previous" pointer — a fourth record. Maintaining these cross-references under CoW is error-prone and hard to reason about. Unified B-trees eliminate this entirely: each B-tree insert/delete is a self-contained CoW path copy.

**2. Uniform crash safety.**
With unified B-trees, crash safety follows from a single mechanism: atomic root pointer swap in the file header. The hybrid approach would require either a WAL for slot stores (adding the very complexity CoW was chosen to avoid) or treating slot pages as CoW pages with an indirection table (which Task 2 noted "essentially converts index-free adjacency into something closer to the edge-centric B-tree approach for cold data"). Either way, the hybrid collapses toward the unified approach under CoW constraints.

**3. Manageable traversal cost.**
The traversal cost difference narrows significantly in practice:

| Operation | Hybrid (IFA) | Unified B-tree | With warm buffer pool |
|-----------|-------------|----------------|----------------------|
| Single-hop traversal | O(1) — 1 page read | O(log N) — ~3–4 page reads | 1 leaf page read (interior nodes cached) |
| 5-hop path | ~5 page reads | ~5 leaf page reads | Same in steady state |
| Adjacency scan (all neighbors) | O(degree) pointer follows | O(degree) range scan | Comparable |

B-tree interior nodes are small and frequently accessed — the buffer pool keeps them hot. In steady state, a B-tree lookup touches exactly one leaf page (all interior nodes are cached). This means the practical per-hop cost is one page read — identical to index-free adjacency. The difference only matters when the buffer pool is cold (startup, or working set exceeds pool size), where B-tree traversal costs 2–3 extra page reads per hop.

**4. Reduced implementation complexity.**
The unified approach has one storage subsystem (B-trees), one concurrency mechanism (CoW + root pointer swap), one page format, and one free-space management strategy. The hybrid approach has two storage subsystems (slot stores + B-trees), two concurrency mechanisms, two page formats, and must maintain referential consistency between them. For a project where the designer is learning database internals, this complexity reduction is significant.

**5. Better space efficiency for sparse graphs.**
Slot stores allocate space for every ID from 0 to max_id, even if most slots are empty (after deletions). A B-tree stores only the records that exist, naturally handling sparse ID spaces without waste.

### What we give up

- **O(1) single-hop traversal** in the cold-buffer-pool case. When interior B-tree nodes are not cached, each hop costs O(log N) page reads instead of O(1). For a database with 1 million nodes, this is ~3 extra page reads per hop at startup. Once the buffer pool warms up, this difference vanishes.
- **Direct addressing by ID.** Slot stores provide O(1) ID-to-record mapping (`base + id × record_size`). B-trees require O(log N) lookup. Again, buffered interior nodes reduce this to a single leaf-page read in practice.

### Mitigation: B-tree key design for traversal efficiency

We compensate for the loss of index-free adjacency by designing the adjacency index B-trees to make traversal a range scan rather than a point lookup. The outgoing adjacency index is keyed by `(source_id, edge_type_id, edge_id)`. To find all outgoing edges of node N, the traversal performs a single range scan starting at `(N, 0, 0)` and reading forward until `source_id > N`. This scan touches only contiguous leaf pages — excellent cache and I/O behavior.

---

## 4. B-Tree Catalog: Logical Trees in the Database

The database contains exactly seven logical B+ trees. The file header stores the root page ID for each.

| # | Name | Key | Value | Purpose |
|---|------|-----|-------|---------|
| 1 | **Node Store** | `NodeId` (u64) | `NodeRecord` | Primary store for node data |
| 2 | **Edge Store** | `EdgeId` (u64) | `EdgeRecord` | Primary store for edge data |
| 3 | **Outgoing Adjacency Index** | `(NodeId, TypeId, EdgeId)` | ∅ (key-only) | Find outgoing edges from a node, optionally filtered by type |
| 4 | **Incoming Adjacency Index** | `(NodeId, TypeId, EdgeId)` | ∅ (key-only) | Find incoming edges to a node, optionally filtered by type |
| 5 | **Type Index** | `(TypeKindTag, TypeId, EntityId)` | ∅ (key-only) | Find all nodes or edges of a given type |
| 6 | **Schema Store** | `SchemaKey` | `SchemaValue` | Type definitions, property key definitions, hierarchy, counters |
| 7 | **ID Freelist** | `(EntityKindTag, EntityId)` | ∅ (key-only) | Recycled node/edge IDs available for reuse |

**Notation:** "∅ (key-only)" means the B-tree leaf entries store only the key with no associated value payload. The key's existence is the information.

**TypeKindTag** and **EntityKindTag** are single-byte discriminators (0 = node, 1 = edge) that allow a single B-tree to index both node types and edge types.

**EntityId** is a u64 that stores either a `NodeId` or `EdgeId` value, discriminated by context (the `TypeKindTag` prefix).

### Why these seven trees

Each tree is driven by a specific access pattern from the `GraphView` trait (Section 10.3 of `006-schema-extension-spec.md`) or a storage requirement:

| GraphView method | Served by |
|-----------------|-----------|
| `get_node(id)` | Node Store (point lookup) |
| `get_edge(id)` | Edge Store (point lookup) |
| `outgoing_edges(node, type?)` | Outgoing Adjacency Index (range scan) |
| `incoming_edges(node, type?)` | Incoming Adjacency Index (range scan) |
| `nodes_by_type(type, include_subtypes)` | Type Index (range scan, one scan per subtype if include_subtypes) |
| `edges_by_type(type, include_subtypes)` | Type Index (range scan) |
| `nodes_by_property(key, value)` | Full scan of Node Store (no dedicated property index in v1 — see Section 7.5) |

---

## 5. On-Disk Record Formats

All multi-byte integers are stored in **little-endian** format (matching Rust's native representation on x86/ARM and avoiding byte-swap overhead on the dominant platforms).

### 5.1 NodeRecord

Stored as the value in the Node Store B-tree, keyed by `NodeId`.

```
NodeRecord layout (variable-length):
┌─────────────────────────────────────────────────────────────────┐
│ Offset  Size  Field                Description                  │
├─────────────────────────────────────────────────────────────────┤
│  0      1     flags                Bit flags (see below)        │
│  1      1     type_count           Number of type labels (0–255)│
│  2      4     primary_type         TypeId of first type (or 0)  │
│  6      4     property_size        Byte length of inline props  │
│ 10      8     overflow_page_id     PageId for overflow props    │
│                                    (0 if all props inline)      │
│ 18      4×N   extra_types[N-1]     Additional TypeIds (if N>1)  │
│ 18+4×(N-1)  S   inline_properties  Serialized PropertyMap       │
│                                    (S = property_size bytes)    │
└─────────────────────────────────────────────────────────────────┘

Flags byte (bit 0 = LSB):
  bit 0: is_anonymous (1 = anonymous/blank node)
  bits 1–7: reserved (must be 0)
```

**Notes:**
- `type_count` stores the total number of type labels. If `type_count == 0`, `primary_type` is `TypeId(0)` (the null sentinel). If `type_count == 1`, `primary_type` holds the single type and there are no `extra_types` entries. If `type_count > 1`, `primary_type` holds the first type and `extra_types` holds the remaining `type_count - 1` types.
- `property_size` is the byte length of the inline serialized `PropertyMap`. If the serialized properties fit within the B-tree leaf cell (see threshold in Section 7), they are stored inline. Otherwise, `property_size` is 0 and `overflow_page_id` points to the first overflow page.
- The minimum record size (no types, no properties) is 18 bytes.

### 5.2 EdgeRecord

Stored as the value in the Edge Store B-tree, keyed by `EdgeId`.

```
EdgeRecord layout (variable-length):
┌─────────────────────────────────────────────────────────────────┐
│ Offset  Size  Field                Description                  │
├─────────────────────────────────────────────────────────────────┤
│  0      1     flags                Bit flags (reserved)         │
│  1      1     type_count           Number of type labels (0–255)│
│  2      4     primary_type         TypeId of first type (or 0)  │
│  6      8     source               Source NodeId                │
│ 14      8     target               Target NodeId                │
│ 22      4     property_size        Byte length of inline props  │
│ 26      8     overflow_page_id     PageId for overflow props    │
│                                    (0 if all props inline)      │
│ 34      4×N   extra_types[N-1]     Additional TypeIds (if N>1)  │
│ 34+4×(N-1)  S   inline_properties  Serialized PropertyMap       │
└─────────────────────────────────────────────────────────────────┘

Flags byte:
  bits 0–7: reserved (must be 0)
```

**Notes:**
- Minimum record size (1 type, no properties) is 34 bytes.
- `source` and `target` are stored inline because they are needed for every edge access (traversal, validation). Storing them in the edge record avoids a second lookup.

### 5.3 Serialized PropertyMap

The `PropertyMap` (a `BTreeMap<PropertyKeyId, Value>`) is serialized using a compact binary encoding:

```
PropertyMap serialization:
┌───────────────────────────────────────────────────────────────────┐
│ Offset  Size    Field             Description                     │
├───────────────────────────────────────────────────────────────────┤
│  0      2       entry_count       Number of key-value pairs       │
│  2      ...     entries[]         Repeated entry records          │
│                                                                   │
│ Entry record:                                                     │
│  0      4       key_id            PropertyKeyId (u32)             │
│  4      1       value_tag         Value discriminant (see below)  │
│  5      ...     value_payload     Tag-dependent payload           │
└───────────────────────────────────────────────────────────────────┘

Value tags and payloads:
  0x00  Null           → no payload (0 bytes)
  0x01  Bool           → 1 byte (0 or 1)
  0x02  I64            → 8 bytes (little-endian i64)
  0x03  U64            → 8 bytes (little-endian u64)
  0x04  F64            → 8 bytes (little-endian f64)
  0x05  String         → 4 bytes length (u32) + UTF-8 bytes
  0x06  Bytes          → 4 bytes length (u32) + raw bytes
  0x07  NodeRef        → 8 bytes (NodeId as u64)
  0x08  LangString     → 4 bytes value_len + UTF-8 value +
                          4 bytes lang_len + UTF-8 lang tag
  0x09  List           → 4 bytes count (u32) + count × (tag + payload)
```

**Rationale for custom encoding over serde/MessagePack:** The property map encoding is the most frequently serialized and deserialized structure in the database. A hand-rolled format avoids external dependencies, avoids allocation overhead from generic serde frameworks, and provides exact control over byte layout. The format is deliberately simple (no schema evolution within the encoding itself — schema evolution is handled at the type system level).

### 5.4 Key-Only Index Entries

The adjacency indexes, type index, and freelist are "key-only" B-trees: the leaf entries contain only the composite key with no value payload. This is a common pattern for secondary indexes — the key's existence in the tree is the information, and the key components can be decomposed to retrieve the referenced entity (e.g., the `EdgeId` component of an adjacency index key leads to the Edge Store for the full record).

---

## 6. B-Tree Key Encoding and Ordering

All B-tree keys are encoded as byte strings with **big-endian** encoding for integer components. This ensures that the lexicographic byte ordering of encoded keys matches the logical ordering of the composite key (smallest NodeId first, then smallest TypeId, then smallest EdgeId). Little-endian integers would produce incorrect byte-level sorting.

### 6.1 Node Store key

```
[NodeId: 8 bytes, big-endian u64]
```

Total key size: 8 bytes (fixed).

### 6.2 Edge Store key

```
[EdgeId: 8 bytes, big-endian u64]
```

Total key size: 8 bytes (fixed).

### 6.3 Outgoing Adjacency Index key

```
[source NodeId: 8 bytes] [edge TypeId: 4 bytes] [EdgeId: 8 bytes]
```

Total key size: 20 bytes (fixed).

**Access patterns supported:**
- All outgoing edges from node N: range scan `[N, 0, 0]` to `[N, MAX, MAX]`
- All outgoing edges of type T from node N: range scan `[N, T, 0]` to `[N, T, MAX]`
- Check if a specific edge exists in the outgoing set: point lookup `[N, T, E]`

### 6.4 Incoming Adjacency Index key

```
[target NodeId: 8 bytes] [edge TypeId: 4 bytes] [EdgeId: 8 bytes]
```

Total key size: 20 bytes (fixed).

**Access patterns supported:** Mirror of outgoing index, for the target endpoint.

### 6.5 Type Index key

```
[TypeKindTag: 1 byte] [TypeId: 4 bytes] [EntityId: 8 bytes]
```

Total key size: 13 bytes (fixed).

`TypeKindTag`: `0x00` = node, `0x01` = edge.
`EntityId`: The `NodeId` or `EdgeId` as a `u64`.

**Access patterns supported:**
- All nodes of type T: range scan `[0x00, T, 0]` to `[0x00, T, MAX]`
- All edges of type T: range scan `[0x01, T, 0]` to `[0x01, T, MAX]`
- `include_subtypes` queries: one range scan per subtype ID (the set of subtypes is obtained from the in-memory type registry cache)

**Multi-type nodes/edges:** A node with `type_labels = [T1, T2, T3]` produces three entries in the type index: `[0x00, T1, node_id]`, `[0x00, T2, node_id]`, `[0x00, T3, node_id]`. This means a node appears once per type label. Type scans are exact — no false positives.

### 6.6 Schema Store key

The schema store is a multiplexed B-tree storing both type definitions and property key definitions. The key uses a prefix tag to separate namespaces:

```
Schema key encoding:
  0x01 [TypeId: 4 bytes]                   → TypeDefinition
  0x02 [PropertyKeyId: 4 bytes]            → PropertyKeyDefinition
  0x03 [counter_name: 1 byte]              → Monotonic counter value (u64)
  0x04 [TypeId: 4 bytes] [TypeId: 4 bytes] → Type hierarchy edge (child → parent)
  0x05 [extension_kind: 1 byte]            → Extension name list
        [name_len: 2 bytes] [name: bytes]
```

Counter names (prefix `0x03`):
- `0x01`: next NodeId
- `0x02`: next EdgeId
- `0x03`: next TypeId
- `0x04`: next PropertyKeyId

Extension kinds (prefix `0x05`):
- `0x01`: constraint validator name
- `0x02`: inference rule name

### 6.7 ID Freelist key

```
[EntityKindTag: 1 byte] [EntityId: 8 bytes]
```

Total key size: 9 bytes (fixed).

`EntityKindTag`: `0x00` = node, `0x01` = edge.

The freelist B-tree stores IDs that have been deleted and are available for reuse. On allocation, the system checks the freelist first; if empty, it increments the monotonic counter.

---

## 7. Property Storage

### 7.1 Inline vs. Overflow

Property maps are variable-length. Small property maps (common case: a few short string or integer properties) should be stored inline within the node/edge B-tree record for locality. Large property maps must overflow to separate pages to avoid bloating B-tree leaf pages.

**Inline threshold: 256 bytes.**

- If the serialized PropertyMap is ≤ 256 bytes: store inline in the node/edge record. `property_size` = actual serialized size. `overflow_page_id` = 0.
- If the serialized PropertyMap is > 256 bytes: store in overflow pages. `property_size` = 0. `overflow_page_id` = page ID of the first overflow page.

**Rationale for 256-byte threshold:** Assuming a 4 KB page size with ~50% fill factor, each B-tree leaf page holds roughly 2 KB of useful payload. If inline properties averaged 256 bytes, each leaf page could hold ~6–8 node records — reasonable occupancy. Larger inline sizes would reduce records-per-page and increase write amplification (CoW copies larger pages). 256 bytes accommodates roughly 8–12 typical properties (4-byte key + 1-byte tag + 8-byte value each).

### 7.2 Overflow Page Format

When a property map overflows, it is stored in one or more overflow pages. Each overflow page has a simple chained format:

```
Overflow page layout:
┌──────────────────────────────────────────┐
│ next_page_id: u64  (0 = last in chain)   │   8 bytes
│ data_length: u32                         │   4 bytes
│ data: [u8; data_length]                  │   variable
│ (unused remainder of page)               │
└──────────────────────────────────────────┘
```

The serialized PropertyMap bytes are split across the chain of overflow pages. On read, the chain is followed and concatenated to reconstruct the full byte stream, which is then deserialized.

### 7.3 Property Update Semantics

When a node or edge's properties are updated:
- If the old properties were inline and the new properties are also ≤ 256 bytes: update the inline data in the B-tree record (standard CoW B-tree insert/replace).
- If the old properties were inline but the new properties exceed 256 bytes: allocate overflow pages, write properties there, update the node/edge record to reference the overflow page.
- If the old properties were in overflow pages: allocate new overflow pages (CoW — never reuse old overflow pages until they are garbage collected), write new properties, update the node/edge record's overflow pointer. The old overflow pages become garbage, reclaimable after no active snapshot references them.

### 7.4 Value Serialization Notes

- **Strings and Bytes:** The 4-byte length prefix allows values up to ~4 GB. In practice, the overflow page mechanism limits a single property to the chain capacity, but this is sufficient for any reasonable use case.
- **Lists (`Value::List`):** Serialized recursively. Each list element is a `(tag, payload)` pair. Nested lists are supported to arbitrary depth. This is a correctness requirement from `006-schema-extension-spec.md` Section 4.
- **LangString:** The value and language tag are stored sequentially, each with a 4-byte length prefix. This is slightly more expensive than a single-string variant but keeps the language tag co-located with the value, as required by the schema spec.

### 7.5 Property Value Indexes (Deferred)

The `GraphView::nodes_by_property(key, value)` method will fall back to a full scan of the Node Store in the initial implementation. This is acceptable because:

1. The method is primarily used by constraint validators at commit time (not a hot path).
2. Task 6's residual concern #2 anticipated this and noted it as a storage-layer decision.
3. Adding a property value index later (a new B-tree keyed by `(PropertyKeyId, ValueHash, EntityId)`) is a backward-compatible addition — it requires no changes to the existing B-trees or record formats.

**Future property index design (for reference):** If added, the property value index would be an eighth B-tree with key `(PropertyKeyId: 4 bytes, ValueHash: 8 bytes, EntityKindTag: 1 byte, EntityId: 8 bytes)` = 21 bytes. The value hash would be a 64-bit hash of the serialized `Value` bytes, allowing exact-match lookups with hash collision verification against the actual value in the primary record.

---

## 8. Schema/Extension Metadata Storage

### 8.1 TypeDefinition Storage

Each `TypeDefinition` (from `006-schema-extension-spec.md` Section 7.4) is serialized and stored as a value in the Schema Store B-tree, keyed by `0x01 [TypeId]`.

**TypeDefinition serialization:**

```
┌───────────────────────────────────────────────────────────────────┐
│  4 bytes   type_id              TypeId (u32)                      │
│  1 byte    kind                 TypeKind (0 = Node, 1 = Edge)     │
│  1 byte    open                 0 or 1                            │
│  2 bytes   name_len             Name string length                │
│  N bytes   name                 UTF-8 name string                 │
│  2 bytes   supertype_count      Number of supertypes              │
│  4×S bytes supertypes           TypeId array                      │
│  2 bytes   decl_count           Number of property declarations   │
│  ...       declarations[]       Serialized PropertyDeclarations   │
│  ...       metadata             Serialized PropertyMap            │
└───────────────────────────────────────────────────────────────────┘
```

**PropertyDeclaration serialization:**

```
┌───────────────────────────────────────────────────────────────────┐
│  4 bytes   key                  PropertyKeyId (u32)               │
│  1 byte    value_type_tag       ValueTypeDescriptor tag           │
│  ...       value_type_extra     Recursive descriptor (for List)   │
│  1 byte    required             0 or 1                            │
│  1 byte    multi_valued         0 or 1                            │
│  ...       metadata             Serialized PropertyMap            │
└───────────────────────────────────────────────────────────────────┘
```

### 8.2 PropertyKeyDefinition Storage

Each registered property key is stored in the Schema Store keyed by `0x02 [PropertyKeyId]`:

```
┌───────────────────────────────────────────────────────────────────┐
│  4 bytes   key_id               PropertyKeyId (u32)               │
│  2 bytes   name_len             Key name string length            │
│  N bytes   name                 UTF-8 key name string             │
└───────────────────────────────────────────────────────────────────┘
```

### 8.3 Type Hierarchy Edges

The type hierarchy DAG is stored as individual edges in the Schema Store keyed by `0x04 [child TypeId] [parent TypeId]`. This allows:
- Enumerate all direct supertypes of a type: range scan on `0x04 [child] [0x00000000]` to `0x04 [child] [0xFFFFFFFF]`
- Enumerate all direct subtypes of a type: full scan of `0x04` prefix, filtering by parent column (this is acceptable because the hierarchy is small and cached in memory)
- Check for a specific hierarchy edge: point lookup on `0x04 [child] [parent]`

### 8.4 Extension Registration Persistence

Constraint validators and inference rules are registered as `Box<dyn Trait>` objects in memory (per `006-schema-extension-spec.md` Section 12). Only their **names** are persisted — the actual trait objects are re-registered by application code on each database open. The Schema Store entries keyed by `0x05 [kind] [name]` store the registered extension names so that the database can detect at startup whether expected extensions are missing.

### 8.5 Schema Cache

On database open, the entire Schema Store B-tree is scanned and its contents are deserialized into an in-memory `TypeRegistry` and `PropertyKeyRegistry`. These in-memory caches are the primary access path for schema data during normal operation. The Schema Store B-tree is only written to during schema modification transactions.

**Rationale:** Schema data is small (typically a few KB to a few tens of KB) and rarely changes. Caching it entirely in memory avoids B-tree lookups on every type-resolution operation (which would otherwise be called on every node/edge access for type-filtered operations).

---

## 9. How the Schema/Extension Layer Maps to the Storage Layer

This section maps every type and trait from `006-schema-extension-spec.md` to its storage representation, providing a complete bridge between the schema design and the storage model.

### 9.1 Core Types

| Schema Type | Storage Location | Key | Value |
|-------------|-----------------|-----|-------|
| `Node` | Node Store B-tree | `NodeId` | `NodeRecord` (Section 5.1) |
| `Edge` | Edge Store B-tree | `EdgeId` | `EdgeRecord` (Section 5.2) |
| `Node.type_labels` | Inline in `NodeRecord` + Type Index entries | — | — |
| `Edge.type_labels` | Inline in `EdgeRecord` + Type Index entries | — | — |
| `Node.properties` / `Edge.properties` | Inline in record or overflow pages | — | Serialized `PropertyMap` (Section 5.3) |
| `Node.is_anonymous` | Flags byte in `NodeRecord` | — | bit 0 |

### 9.2 Schema Types

| Schema Type | Storage Location | Key | Value |
|-------------|-----------------|-----|-------|
| `TypeDefinition` | Schema Store B-tree | `0x01 [TypeId]` | Serialized `TypeDefinition` |
| `PropertyDeclaration` | Embedded in `TypeDefinition` serialization | — | — |
| `PropertyKeyId` → name mapping | Schema Store B-tree | `0x02 [PropertyKeyId]` | Serialized `PropertyKeyDefinition` |
| Type hierarchy (supertype edges) | Schema Store B-tree | `0x04 [child] [parent]` | ∅ |
| ID counters | Schema Store B-tree | `0x03 [counter_name]` | u64 |

### 9.3 GraphView Trait Methods

The `GraphView` trait (`006-schema-extension-spec.md` Section 10.3) is the read interface that constraint validators and inference rules use. Here is how each method maps to storage operations:

| Method | Storage Operation |
|--------|------------------|
| `get_node(id)` | Node Store point lookup by `NodeId` |
| `get_edge(id)` | Edge Store point lookup by `EdgeId` |
| `outgoing_edges(node, type?)` | If `type` is `Some(T)`: range scan Outgoing Adj. Index `[node, T, 0]..=[node, T, MAX]`, then batch-lookup each `EdgeId` in Edge Store. If `type` is `None`: range scan `[node, 0, 0]..=[node, MAX, MAX]`, then batch-lookup. |
| `incoming_edges(node, type?)` | Same pattern on Incoming Adj. Index. |
| `nodes_by_type(type, include_subtypes)` | If `!include_subtypes`: range scan Type Index `[0x00, type, 0]..=[0x00, type, MAX]`, batch-lookup NodeIds in Node Store. If `include_subtypes`: compute subtype set from in-memory type registry, then union of range scans. |
| `edges_by_type(type, include_subtypes)` | Same pattern on Type Index with `0x01` prefix. |
| `nodes_by_property(key, value)` | Full scan of Node Store; for each node, deserialize properties and check for match. (v1 — no property index.) |

### 9.4 ChangeSet Production

The `ChangeSet` (`006-schema-extension-spec.md` Section 10.2) must be produced by the transaction system. During a write transaction, the storage layer maintains a write buffer (see Section 12). Every mutation (node insert, edge modify, etc.) is recorded in the transaction's in-memory change log. On commit, this change log is converted into a `ChangeSet` and passed to registered `ConstraintValidator`s. See Section 15 for details.

### 9.5 Extension Trait Interaction

`ConstraintValidator::validate()` receives `&dyn GraphView`, `&dyn TypeRegistryView`, and `&dyn PropertyKeyRegistryView`. The storage layer provides concrete implementations of these traits:

- `GraphView` is implemented by a snapshot-based reader that traverses the B-trees at a fixed root pointer (the snapshot root). Pending transaction changes are overlaid on top (see Section 12).
- `TypeRegistryView` is implemented by the in-memory schema cache.
- `PropertyKeyRegistryView` is implemented by the in-memory property key cache.

---

## 10. Buffer Pool Design

### 10.1 Overview

The buffer pool is a fixed-size region of memory that caches B-tree pages read from disk. It is the performance heart of the storage engine — a well-tuned buffer pool makes B-tree traversal nearly as fast as in-memory pointer chasing.

### 10.2 Structure

```rust
/// A frame in the buffer pool holding one cached page.
struct PageFrame {
    /// The page ID this frame is caching (or PageId(0) if empty).
    page_id: PageId,
    /// The raw page bytes.
    data: [u8; PAGE_SIZE],
    /// Whether this frame has been modified since last disk write.
    dirty: bool,
    /// Number of active references pinning this frame.
    pin_count: u32,
    /// Clock bit for eviction (1 = recently accessed).
    reference_bit: bool,
}

/// The buffer pool.
struct BufferPool {
    /// Fixed array of page frames.
    frames: Vec<PageFrame>,
    /// Maps PageId → frame index for O(1) lookup.
    page_table: HashMap<PageId, usize>,
    /// Clock hand for eviction.
    clock_hand: usize,
    /// Total number of frames (configured at startup).
    capacity: usize,
}
```

### 10.3 Operations

**fetch_page(page_id) → &PageFrame:**
1. Check `page_table` for `page_id`. If found (cache hit): set `reference_bit = true`, increment `pin_count`, return frame reference.
2. Cache miss: find a free frame using the clock eviction algorithm (Section 10.4). If the evicted frame is dirty, write it to disk. Read the requested page from disk into the frame. Insert into `page_table`. Set `pin_count = 1`, `reference_bit = true`.

**unpin_page(page_id, dirty: bool):**
Decrement `pin_count`. If `dirty`, set the frame's `dirty = true`.

**flush_page(page_id):**
Write the frame's data to disk at offset `page_id × PAGE_SIZE`. Clear the `dirty` flag.

### 10.4 Clock Eviction Policy

The clock algorithm is a constant-time approximation of LRU:

1. Starting from `clock_hand`, inspect each frame in circular order.
2. If the frame is unpinned (`pin_count == 0`) and `reference_bit == false`: evict this frame. Advance `clock_hand`.
3. If the frame is unpinned but `reference_bit == true`: clear `reference_bit`, advance `clock_hand`, continue.
4. If the frame is pinned: skip, advance `clock_hand`, continue.
5. If a full cycle completes with no evictable frame: return an out-of-buffer-pool-space error.

**Rationale for Clock over LRU:** Clock requires O(1) amortized time and no linked-list maintenance. LRU requires moving a node to the head of a linked list on every access — a significant overhead in the hot path of page access. Clock provides good-enough eviction decisions for database workloads.

### 10.5 Configuration

- **Default pool size:** 1024 frames (4 MB with 4 KB pages). Configurable by the user at database open time.
- **Minimum pool size:** 64 frames (256 KB). Below this, the database cannot operate efficiently (B-tree traversal alone requires 3–4 pages in flight simultaneously).

### 10.6 CoW Interaction

In a CoW B-tree, modified pages are written to **new** page locations (never overwriting existing pages). The buffer pool must handle this correctly:

- When a write transaction modifies a B-tree path, the modified pages are created as new `PageFrame` entries in the buffer pool with new `PageId`s. The old pages remain in the pool (they may be referenced by concurrent read snapshots).
- On transaction commit, the new pages are flushed to disk and the file header is updated. The old pages become candidates for eviction (no new snapshots will reference them, but existing snapshots may still hold them pinned).
- A page is truly reclaimable (can be added to the free-space list) only when no active snapshot references the old root from which that page was reachable. This is tracked by the snapshot reference counter (Section 11.3).

---

## 11. Concurrency Control Strategy

### 11.1 Model: Single-Writer MVCC via CoW Snapshots

**Choice: Single active writer at a time. Unlimited concurrent readers. Snapshot isolation for all transactions.**

This is the concurrency model used by LMDB and redb, and it is the model recommended by Task 1. The justification:

1. **Graph databases are read-heavy.** Multi-hop traversals, type scans, and constraint validation are all read operations. The dominant concurrency pattern is many readers, few (or one) writer.
2. **Single-writer eliminates write-write conflicts.** No deadlocks, no write conflict detection, no lock tables. The write lock is a single `Mutex`.
3. **CoW B-trees provide MVCC for free.** Each committed write produces a new set of root pointers. A reader that holds the old root pointers sees a consistent snapshot — no coordination needed.
4. **Simplicity is a project risk mitigator.** Multi-writer concurrency (2PL, SSI) is the most complex part of a database. Single-writer with snapshot isolation is dramatically simpler to implement correctly.

### 11.2 Isolation Guarantee

**Snapshot Isolation:**
- A read-only transaction sees a consistent snapshot of the database as it existed at the time the transaction began. It is unaffected by concurrent writes.
- A write transaction operates on the latest committed state. Because only one writer exists at a time, there are no write-write conflicts or write skew anomalies — the single-writer constraint makes all writes effectively serializable.
- Combined, this provides **Serializable** isolation for all practical purposes (single-writer + snapshot reads = no anomalies).

### 11.3 Snapshot Lifecycle

```
                          ┌──────────────────┐
                          │    File Header    │
                          │ roots_v1 → v2    │
                          └────┬────┬────────┘
                               │    │
                 snapshot_v1 ──┘    └── snapshot_v2 (current)
                      │                     │
              ┌───────┴───────┐     ┌───────┴───────┐
              │  Reader Txn A │     │  Reader Txn B │
              │  (started at  │     │  (started at  │
              │   v1, sees v1)│     │   v2, sees v2)│
              └───────────────┘     └───────────────┘
```

A **snapshot** is a set of B-tree root page IDs (one per logical B-tree). Creating a snapshot is O(1) — just copy the current root pointers from the file header.

**Reference counting:** Each snapshot has a reference count. When a read transaction begins, it acquires a reference to the current snapshot. When it ends (commit or abort), it releases the reference. When a snapshot's reference count drops to zero and a newer snapshot exists, the old snapshot's pages that are not reachable from the current snapshot become eligible for reclamation.

### 11.4 Write Locking

A write transaction acquires a global write mutex at `begin()` time. If another write transaction is in progress, the new one blocks until the mutex is released. The mutex is released at `commit()` or `abort()` time.

```rust
/// Concurrency primitives (conceptual).
struct DatabaseInner {
    /// The write lock. Only one write transaction at a time.
    write_mutex: Mutex<()>,
    /// The current snapshot (latest committed root pointers).
    /// Protected by an RwLock: readers acquire a read lock to
    /// copy the snapshot; the writer acquires a write lock to
    /// update it on commit.
    current_snapshot: RwLock<Snapshot>,
    /// Active read snapshots with reference counts.
    active_snapshots: Mutex<Vec<(SnapshotId, Arc<Snapshot>)>>,
}
```

---

## 12. Transaction Lifecycle

### 12.1 Read-Only Transaction

```
1. Acquire read lock on current_snapshot (RwLock::read_lock).
2. Clone the current Snapshot (set of root pointers).
3. Release read lock.
4. Increment snapshot reference count.
5. All reads use the cloned root pointers → consistent snapshot.
6. On drop/commit/abort: decrement snapshot reference count.
```

Cost: one `RwLock` read acquisition + pointer copy. No disk I/O. No coordination with writers.

### 12.2 Write Transaction

```
1. Acquire write_mutex (blocks until available).
2. Acquire read lock on current_snapshot.
3. Clone the current Snapshot as the transaction's base state.
4. Release read lock.
5. Initialize an empty WriteBuffer:
   - pending_node_inserts: Vec<(NodeId, NodeRecord)>
   - pending_node_updates: Vec<(NodeId, NodeRecord, NodeRecord)>  // (id, before, after)
   - pending_node_deletes: Vec<(NodeId, NodeRecord)>
   - (same for edges)
   - pending_schema_changes: Vec<SchemaChange>
   - new_pages: Vec<(PageId, PageData)>  // CoW-produced new pages
6. Execute mutations: each mutation modifies the WriteBuffer.
   Read operations during the write transaction see the base
   snapshot overlaid with pending changes (read-your-own-writes).
7. On commit:
   a. Build ChangeSet from WriteBuffer.
   b. Run all registered ConstraintValidators.
   c. If any validator fails: abort (discard WriteBuffer, release write_mutex).
   d. Materialize B-tree changes: for each pending change, perform
      the CoW B-tree insert/delete/update on the base snapshot's
      B-trees, producing new root pages.
   e. Flush all new pages to disk (write + fsync).
   f. Write new file header with updated root pointers (write + fsync).
   g. Acquire write lock on current_snapshot (RwLock::write_lock).
   h. Update current_snapshot to the new root pointers.
   i. Release write lock.
   j. Release write_mutex.
   k. Mark old snapshot pages as reclaimable (if no readers reference them).
8. On abort:
   a. Discard WriteBuffer.
   b. Release write_mutex.
   c. No disk I/O required (CoW: no pages were overwritten).
```

### 12.3 Read-Your-Own-Writes

During a write transaction, reads must see the transaction's pending changes overlaid on the base snapshot. This is implemented as a two-layer reader:

1. Check the WriteBuffer for the requested key.
2. If not found in the WriteBuffer, fall back to the base snapshot's B-trees.

This overlay is also what the `GraphView` implementation provides to constraint validators at commit time (step 7b).

### 12.4 Atomic Commit

The commit is atomic because of the **double-buffered file header** pattern (detailed in Task 8). The file header contains two header slots. The writer writes the new root pointers to the inactive slot, fsyncs, then atomically flips a single byte indicating which slot is active. On recovery, the database reads both slots and uses the one marked active (validated by checksum).

This guarantees that a crash during commit leaves the database in one of two states:
- The old header is active (commit did not complete) — old consistent state.
- The new header is active (commit completed) — new consistent state.

---

## 13. Crash Safety and Recovery

### 13.1 Guarantees

The CoW B-tree architecture provides the following crash safety guarantees:

1. **No data corruption.** A crash at any point during operation cannot corrupt committed data. Old pages are never overwritten; new pages are written to fresh locations and only become reachable after an atomic header commit.
2. **Committed transactions are durable.** Once `commit()` returns, the transaction's effects survive any subsequent crash. This requires that all new pages and the new file header are fsynced before `commit()` returns.
3. **Uncommitted transactions are rolled back.** If the process crashes during a write transaction (before the header flip), the old header is still active and the new pages are unreachable garbage.

### 13.2 Recovery Procedure

On database open after a crash (or a normal shutdown):

```
1. Read both file header slots.
2. Validate checksums on both slots.
3. Select the valid slot with the higher transaction counter
   (if both are valid — normal case after a clean shutdown).
4. If only one slot has a valid checksum: use that one
   (the other was being written during a crash).
5. The selected header's root pointers define the current
   consistent state — all B-trees are rooted at these pages.
6. Scan the free-space bitmap/list to identify pages that are
   allocated but not reachable from the current roots.
   These are garbage from incomplete transactions.
7. Add garbage pages to the free list.
8. Database is ready.
```

**Recovery time:** Steps 1–5 are O(1). Step 6–7 (garbage identification) can be O(pages) in the worst case, but in practice it is bounded by the size of the incomplete transaction (typically small). A page-level reference counting scheme (where each page header tracks whether it belongs to the current snapshot) can make this O(garbage pages) rather than O(all pages).

### 13.3 fsync Discipline

The following fsync rules are mandatory for correctness:

1. **Before header flip:** All new B-tree pages and overflow pages produced by the transaction must be fsynced. This ensures they are durable before the header references them.
2. **Header flip itself:** The file header write must be followed by an fsync. This ensures the new root pointers are durable.
3. **Ordering:** Fsyncs must occur in order: data pages first, then header. If the header were fsynced before data pages, a crash could leave the header pointing to pages that were not yet durable — corrupt state.

On Linux, `fdatasync` is sufficient (we don't need metadata sync for an existing file). The HAL trait (Task 9) must expose both `sync_data` and `sync_all` operations.

---

## 14. ID Allocation and Recycling

### 14.1 Allocation

Node IDs and Edge IDs are allocated by monotonic counters stored in the Schema Store B-tree (key prefix `0x03`). On each allocation:

1. Check the ID Freelist B-tree for a recycled ID of the appropriate kind.
2. If found: remove from freelist, return the recycled ID.
3. If not found: read the current counter, increment it, write back, return the old value.

Counter updates are part of the write transaction and are committed atomically with the rest of the transaction.

### 14.2 Recycling

When a node or edge is deleted, its ID is inserted into the ID Freelist B-tree. This makes the ID available for reuse by future allocations.

**Immediate reuse safety:** Recycled IDs are safe to reuse because:
- The deletion and the freelist insertion are part of the same transaction.
- A reader holding an old snapshot will not see the freelist change — it sees the old snapshot where the entity still exists with its original ID.
- A new entity with a recycled ID is only visible to snapshots created after the commit that both deleted the old entity and (later) allocated the new one. These are separate transactions, so there is no ambiguity.

### 14.3 Tombstones

No tombstones are needed. When a node is deleted:
1. Remove its `NodeRecord` from the Node Store B-tree.
2. Remove all its entries from the Type Index.
3. Delete all incident edges (which cascades to Adjacency Index and Edge Store updates).
4. Remove its properties (inline or overflow pages become garbage).
5. Insert its `NodeId` into the Freelist.

All of these operations are part of the same write transaction and become visible atomically on commit.

---

## 15. ChangeSet Production for Constraint Validators

The `ChangeSet` type (`006-schema-extension-spec.md` Section 10.2) must be produced by the transaction system to feed constraint validators at commit time.

### 15.1 Change Tracking During a Transaction

The `WriteBuffer` (Section 12.2) records every mutation as it occurs:

- **Node insert:** Store `(NodeId, new Node)`. Produces `NodeChange::Inserted(node)`.
- **Node update:** Before modification, read the current node from the base snapshot. Store `(NodeId, old Node, new Node)`. Produces `NodeChange::Modified { before, after }`.
- **Node delete:** Read the current node from the base snapshot. Store `(NodeId, old Node)`. Produces `NodeChange::Deleted(node)`.
- Edge changes: same pattern.

### 15.2 ChangeSet Construction

At commit time (step 7a in Section 12.2):

```rust
fn build_changeset(write_buffer: &WriteBuffer) -> ChangeSet {
    ChangeSet {
        node_changes: write_buffer.node_inserts.iter()
            .map(|(_, n)| NodeChange::Inserted(n.clone()))
            .chain(write_buffer.node_updates.iter()
                .map(|(_, before, after)| NodeChange::Modified {
                    before: before.clone(),
                    after: after.clone(),
                }))
            .chain(write_buffer.node_deletes.iter()
                .map(|(_, n)| NodeChange::Deleted(n.clone())))
            .collect(),
        edge_changes: /* same pattern */,
    }
}
```

The `ChangeSet` is passed to each registered `ConstraintValidator`. If any validator returns violations, the transaction is aborted (WriteBuffer discarded, write mutex released). No disk I/O has occurred for the B-tree changes yet (they are materialized in step 7d, after validation passes).

### 15.3 Validator's GraphView

The `GraphView` passed to validators (step 7b) is the overlay view described in Section 12.3: it reflects the base snapshot plus all pending changes. This means validators see the world "as if" the transaction had already committed — they can check cross-references, cardinality, uniqueness, etc.

---

## 16. Performance Characteristics

### 16.1 Operation Costs

| Operation | Storage Operations | I/O (cold) | I/O (warm buffer pool) |
|-----------|-------------------|------------|------------------------|
| **Point lookup** (get_node, get_edge) | B-tree traversal: O(log N) | ~3–4 page reads | 1 leaf page read |
| **Outgoing edge scan** (all types) | Adjacency Index range scan | O(log E + degree) pages | O(degree / fan-out) pages |
| **Outgoing edge scan** (filtered by type) | Adjacency Index range scan (narrower) | O(log E + count) pages | O(count / fan-out) pages |
| **Multi-hop traversal** (k hops) | k × (adjacency scan + point lookup) | k × (O(log E) + O(log N)) pages | k × ~2 page reads |
| **Type scan** (all nodes of type T) | Type Index range scan + batch point lookups | O(log N + count × log N) | O(count) pages |
| **Property scan** (nodes_by_property) | Full scan of Node Store | O(N) pages | O(N) pages |
| **Insert node** | Node Store insert + Type Index inserts | O(log N) writes + O(types × log N) | Same (writes always hit disk via CoW) |
| **Insert edge** | Edge Store insert + 2 Adjacency Index inserts + 2 Type Index inserts | O(log E) + O(4 × log E) writes | Same |
| **Delete node** | Node Store delete + Type Index deletes + cascade edge deletes + Freelist insert | O(log N + degree × log E) writes | Same |
| **Commit** | Write all new CoW pages + fsync + header flip + fsync | O(modified pages) writes + 2 fsyncs | Same |

### 16.2 Write Amplification

CoW B-trees have inherent write amplification: modifying a single leaf page requires rewriting the entire path from leaf to root (O(height) pages, typically 3–4). For a transaction that touches `M` leaf pages, the total pages written is at most `M × height` (worst case, no shared interior nodes) or as few as `M + height` (best case, all modifications share the same root path).

For a single edge insertion (touching 5 B-tree leaf pages across 3 B-trees), worst-case write amplification is ~5 × 4 = 20 pages. At 4 KB per page, this is 80 KB per edge insert. This is the cost of CoW's simplicity. For comparison, an in-place B-tree with WAL would write ~5 page modifications + 5 WAL records ≈ 10 writes — but with the added complexity of WAL management and recovery.

**Mitigations:**
1. **Batch writes.** Multiple mutations in the same transaction share interior-node copies. A transaction inserting 100 edges into the same region of the Adjacency Index will produce far fewer than 500 × 4 new pages — most interior nodes are shared.
2. **Lazy page allocation.** New pages are allocated from the free list (recycled from old snapshots), so write amplification does not increase file size proportionally.
3. **The 2-fsync commit.** Regardless of how many pages a transaction produces, commit requires only 2 fsyncs (data pages, then header). Group flush of all new pages in one fsync call amortizes the fsync latency.

### 16.3 Space Utilization

B-tree pages have a typical fill factor of 50%–70% (after splits). With 4 KB pages and an average node record of ~100 bytes, each leaf page holds ~20–30 node records. The total space overhead compared to the raw data size is approximately 1.5×–2×, which includes:
- B-tree interior nodes (~1% of total pages for large datasets)
- Page fragmentation (unused space within pages)
- CoW garbage (old pages awaiting reclamation — bounded by snapshot retention)

### 16.4 Scalability Characteristics

| Dimension | Scalability | Notes |
|-----------|------------|-------|
| Number of nodes | O(log N) for lookups, O(N) for full scans | B-tree height grows logarithmically |
| Number of edges | O(log E) for lookups, O(E) for full scans | Same |
| Node degree (edges per node) | O(degree) for neighbor scan | Linear in adjacency list length |
| Number of types | O(T) for include_subtypes scans | T = number of subtypes; cached in memory |
| Property bag size | O(1) for inline, O(chain) for overflow | Overflow chain length depends on property size |
| Concurrent readers | Unlimited | Each reader holds a snapshot; no contention |
| Concurrent writers | 1 | Serialized by write mutex |
| Buffer pool size | Directly affects warm-path performance | Larger pool = more hot pages = fewer disk reads |

---

## 17. Design Decision Log

| # | Decision | Alternatives Considered | Rationale |
|---|----------|------------------------|-----------|
| G1 | Unified CoW B-tree architecture (no slot stores) | Hybrid index-free adjacency + CoW B-tree indexes | Eliminates CoW/slot-store tension; uniform crash safety; manageable traversal cost with warm buffer pool; reduced implementation complexity. See Section 3 for full analysis. |
| G2 | Seven logical B-trees | Fewer (combined stores); more (separate per-type trees) | Seven trees cover all access patterns from GraphView. Combined stores would require complex key schemes; per-type trees would proliferate and complicate management. |
| G3 | Key-only adjacency and type indexes (no value payload) | Store edge data inline in adjacency index | Key-only indexes are smaller (better fan-out, fewer pages) and avoid data duplication. The EdgeId in the key provides a direct lookup into the Edge Store for full data. |
| G4 | Big-endian keys for B-tree sort order | Little-endian keys; key comparator function | Big-endian encoding makes byte-level lexicographic order match integer order. No custom comparator needed — memcmp is sufficient. Standard practice in database key encoding. |
| G5 | Little-endian record values | Big-endian; platform-native | Record values are deserialized, not compared as byte strings. Little-endian avoids byte-swap overhead on x86/ARM (dominant platforms). Matches Rust's native `to_le_bytes()`. |
| G6 | Inline properties up to 256 bytes, overflow beyond | All properties out-of-line; all inline (fixed-size padding) | 256 bytes accommodates most real-world property bags inline (zero indirection), while avoiding B-tree page bloat for large blobs. Threshold is tunable. |
| G7 | Single-writer MVCC via CoW snapshots | Multi-writer with 2PL; multi-writer with SSI | Single-writer eliminates write-write conflicts and deadlocks. Graph workloads are read-heavy. CoW provides MVCC naturally. See Section 11. |
| G8 | Snapshot Isolation (effectively Serializable via single writer) | Read Committed; SSI | Single-writer makes all writes serializable. CoW snapshots provide SI for readers. Combined = full serializability without SSI overhead. |
| G9 | Clock eviction policy for buffer pool | LRU; LRU-K; FIFO | Clock is O(1) amortized, simple to implement, and provides adequate eviction quality. LRU has higher overhead per access. LRU-K adds complexity not needed for initial implementation. |
| G10 | ID recycling via Freelist B-tree | No recycling (monotonic only); bitmap-based free list | B-tree freelist is consistent with the unified B-tree approach. Monotonic-only would waste space after many delete/insert cycles. Bitmap is simpler but harder to integrate with CoW. |
| G11 | Schema cached entirely in memory | On-demand schema lookups from Schema Store B-tree | Schema data is small and read on nearly every operation. In-memory cache avoids per-operation B-tree traversal overhead. Consistent with Task 6's design (Section 7.1). |
| G12 | Double-buffered file header for atomic commit | Single header with WAL protection; shadow paging of header page | Double-buffered header is a well-proven pattern (used by LMDB). Simple, atomic (flip one byte + fsync), and no WAL needed. |
| G13 | Custom binary property serialization | serde + bincode/MessagePack; CBOR | Custom encoding avoids external dependencies, provides exact control over layout, and minimizes allocation. Property serialization is the hottest serialization path. |
| G14 | Multiplexed Schema Store (one B-tree for all schema data) | Separate B-trees for types, property keys, hierarchy, counters | Schema data is small; a single B-tree avoids managing multiple root pointers and reduces header complexity. Prefix tags cleanly separate namespaces within one tree. |
| G15 | Property value index deferred to future version | Build property index in v1 | Full scan for `nodes_by_property()` is acceptable for commit-time validation. Adding the index later is backward-compatible. Keeps initial scope manageable. |
| G16 | Type hierarchy edges in Schema Store (not graph edges) | Store hierarchy as graph edges; separate hierarchy B-tree | Schema Store is the natural location. Avoids bootstrap problems of hierarchy-as-graph-edges. Consistent with Task 2's recommendation. |
| G17 | ChangeSet built from WriteBuffer at commit time | Stream changes to validators during the transaction | Building the ChangeSet at commit time is simpler and matches the `ConstraintValidator` API contract (receives a complete ChangeSet). Streaming would require a different validator interface. |

---

## Completion Report: Task 7 — Graph Storage Model

### Status: COMPLETE

### Done Criterion:

The criterion requires:
1. In-memory data structures (buffer pool, caches) — ✓ Sections 10 (buffer pool), 8.5 (schema cache), 12 (transaction write buffer)
2. On-disk layout at the byte/page level — ✓ Sections 5 (record formats with byte-level layouts), 6 (key encodings), 7 (property storage and overflow pages), 8 (schema serialization)
3. How the schema/extension layer maps onto the graph storage layer — ✓ Section 9 (complete mapping table for every type, trait method, and extension interaction)
4. Concurrency control strategy (MVCC, locking, or hybrid — with justification) — ✓ Section 11 (single-writer MVCC via CoW snapshots, with detailed justification)
5. Performance characteristics with justification — ✓ Section 16 (operation costs, write amplification analysis, space utilization, scalability table)

All criteria met.

### Deliverables:
- `007-graph-storage-model.md` — this document

### Summary:

Made the pivotal architectural decision to adopt a **unified CoW B-tree architecture** rather than the hybrid index-free adjacency + B-tree approach recommended as the primary option by Task 2. This decision resolves the most significant open question from Task 2 (the CoW/slot-store interaction) by eliminating slot stores entirely. The full justification is in Section 3.

Defined seven logical B-trees that cover all access patterns required by the `GraphView` trait from Task 6. Specified byte-level record formats for nodes, edges, properties, and schema metadata. Designed key encoding schemes that support efficient range scans for adjacency traversal, type scans, and schema lookups.

Chose single-writer MVCC via CoW snapshots as the concurrency model, providing snapshot isolation for readers and serializable writes. Designed a clock-based buffer pool, a double-buffered file header for atomic commit, and a freelist-based ID recycling scheme.

### Context for Next Task:

**Task 8 (Single-File Format)** should read `007-graph-storage-model.md` (this deliverable) and will also need `001-db-internals-fundamentals.md` and `004-embedded-db-architectures.md`. Key items for Task 8:

- This document defines the logical B-trees (Section 4) and their key/value formats (Sections 5–6). Task 8 must define the physical page format, file header layout (including the double-buffered header described in Section 12.4), free-space management, and the B-tree page structure that hosts these records.
- The CoW B-tree commit protocol (Section 12.2 steps 7d–7f) describes what Task 8's file format must support: writing new pages to arbitrary locations, then atomically flipping the file header.
- The overflow page format (Section 7.2) is specified here; Task 8 should incorporate it into the page type taxonomy.
- The fsync discipline rules (Section 13.3) are constraints on the file format's write ordering.
- The recovery procedure (Section 13.2) depends on the file header layout that Task 8 will define.

**Task 10 (API Surface)** should read this deliverable for transaction lifecycle (Section 12), concurrency guarantees (Section 11), and the buffer pool configuration model (Section 10.5).

**Task 11 (Inference Hook Architecture)** should be aware that the ChangeSet is produced at commit time (Section 15) and that the GraphView provided to inference rules will be the snapshot overlay described in Section 12.3.

### Residual Concerns:

1. **Property value index deferred.** The `nodes_by_property()` method will use a full scan in v1. This is acceptable for commit-time validation but may be a performance concern for large graphs with frequent property-based queries. A property value index is designed in Section 7.5 for future addition.

2. **Write amplification for edge-heavy transactions.** A single edge insertion touches 5 B-tree leaf pages. In CoW, this means writing ~20 pages (5 leaves × ~4 levels). For batch edge insertions (e.g., importing a graph), the amortization is significant, but single-edge-insert latency may be noticeable. Benchmarking during implementation will determine if this needs optimization (e.g., deferred index updates within a transaction).

3. **include_subtypes performance.** A `nodes_by_type(T, include_subtypes=true)` query requires one Type Index range scan per subtype of T. For type hierarchies with many subtypes, this could be slow. A materialized "all subtypes" index is a possible future optimization. The in-memory type hierarchy cache makes subtype enumeration itself fast, but the I/O cost is proportional to the number of subtypes.

4. **Overflow page garbage collection timing.** When a node's properties shrink (moving from overflow to inline), the old overflow pages become garbage. These are reclaimable only after all snapshots referencing the old property pointer are released. The snapshot reference counting mechanism (Section 11.3) handles this, but long-lived read transactions can delay reclamation. This is a known characteristic of CoW systems (LMDB has the same behavior).

5. **Buffer pool sizing guidance.** Section 10.5 provides a minimum (64 frames) and default (1024 frames), but the optimal size depends heavily on the workload. The documentation (Task 29) should include guidance on sizing the buffer pool relative to database size and access patterns.

### Upstream Flags:

1. **Task 2 primary recommendation not adopted — ADVISORY.**
   - What was discovered: The hybrid index-free adjacency + B-tree approach (Task 2's primary recommendation) was not adopted in favor of the simpler unified B-tree approach (Task 2's alternative recommendation).
   - Which task(s) it likely affects: Task 2's recommendation was advisory, not binding. No downstream task depends on the specific choice of hybrid vs. unified. Task 8 (file format) and Task 9 (HAL) are not affected — they work with pages and B-trees regardless.
   - Severity: ADVISORY
   - Suggested action: No action needed. The decision is fully justified in Section 3 of this document. Task 2's recommendation was explicitly presented with both options and their tradeoffs, and the final decision was deferred to Task 7.

2. **Task 6's `GraphView::nodes_by_property()` will be slow in v1 — ADVISORY.**
   - What was discovered: The property value index is deferred (Section 7.5). Task 6 flagged this in residual concern #2.
   - Which task(s) it likely affects: Task 10 (API), Task 11 (inference — if any inference rule uses property-based queries on large graphs).
   - Severity: ADVISORY
   - Suggested action: Task 10 should document that `nodes_by_property()` performs a full scan in v1. Task 11 should note that inference rules relying heavily on property-based queries may be slow on large graphs until a property index is added.
