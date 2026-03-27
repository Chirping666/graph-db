# 002 — Graph Storage Strategies

**Project:** Embedded Graph Database with Extensible Schema & Pluggable Inference
**Task:** 2 — Research: Graph Storage Strategies
**Status:** Complete
**Intended audience:** The designer of this project and all downstream Claude instances. A reader familiar with general data structures but not graph database internals should be able to read this document, understand each strategy, and follow the recommendation rationale.

---

## Table of Contents

1. [The Problem Space](#1-the-problem-space)
2. [Strategy 1: Adjacency Lists](#2-strategy-1-adjacency-lists)
3. [Strategy 2: Adjacency Matrix](#3-strategy-2-adjacency-matrix)
4. [Strategy 3: Compressed Sparse Row (CSR)](#4-strategy-3-compressed-sparse-row-csr)
5. [Strategy 4: Index-Free Adjacency (Native Graph Storage)](#5-strategy-4-index-free-adjacency-native-graph-storage)
6. [Strategy 5: Edge-Centric Storage](#6-strategy-5-edge-centric-storage)
7. [Strategy 6: Non-Native Graph Storage (Relational / Key-Value Decomposition)](#7-strategy-6-non-native-graph-storage-relational--key-value-decomposition)
8. [Cross-Cutting Concern: Property Storage](#8-cross-cutting-concern-property-storage)
9. [Cross-Cutting Concern: Type Metadata Storage](#9-cross-cutting-concern-type-metadata-storage)
10. [Comparison Matrix](#10-comparison-matrix)
11. [Suitability for Single-File Embedded Context](#11-suitability-for-single-file-embedded-context)
12. [Recommendation Shortlist](#12-recommendation-shortlist)
13. [Interaction with Database Internals (Task 1 Integration)](#13-interaction-with-database-internals-task-1-integration)

---

## 1. The Problem Space

A property graph consists of:

- **Nodes** — entities with an ID, one or more types, and a property bag (key → value map)
- **Directed edges** — connections with an ID, a type, a source node ID, a target node ID, and a property bag
- **Types** — a schema layer (node types, edge types, property type definitions, type hierarchies)

The dominant access patterns in a typed property graph database are:

| Pattern | Description | Frequency |
|---------|-------------|-----------|
| Node lookup | Retrieve a node by ID | Very high |
| Edge traversal | Given a node, find all its outgoing or incoming edges | Very high |
| Multi-hop traversal | Follow edges N hops from a starting node | High |
| Type-filtered traversal | Find all edges of a given type from a node | High |
| Type scan | Find all nodes/edges of a given type | Moderate |
| Property filter scan | Find nodes/edges where property X = Y | Moderate |
| Insert node/edge | Add a new node or edge | Moderate |
| Update properties | Modify a node's or edge's properties | Moderate |
| Delete node/edge | Remove a node or edge (and incident edges) | Low |

**The central tension:** Traversal performance (reading adjacency) and point lookup performance pull in opposite directions from write flexibility. Strategies that store adjacency information inline near the node (e.g., index-free adjacency) are fast to traverse but expensive to update. Strategies that separate adjacency into indexes are flexible but add indirection.

A second tension is between **static graphs** (large, rarely updated, analytically queried — e.g., knowledge graphs) and **dynamic graphs** (frequently updated, transactionally queried — e.g., application backends). This project must support both, with a slight emphasis on the OLTP-style (transactional, dynamic) use case because it is a general-purpose foundation for ontology systems.

---

## 2. Strategy 1: Adjacency Lists

### Description

The simplest graph representation. Each node has an associated list of its neighbors (or more precisely, its outgoing edges). In the most basic form:

```
Node 1: [Edge(1→3, type=knows), Edge(1→7, type=works_at)]
Node 3: [Edge(3→1, type=knows), Edge(3→9, type=owns)]
...
```

In a property graph, the list entries typically store edge IDs (not inline edge data), and a separate store maps edge IDs to their full data (type, properties, source, target).

### In-memory layout

A `HashMap<NodeId, Vec<EdgeId>>` (or separate lists for outgoing/incoming). In practice, most implementations maintain two lists per node:
- **Outgoing adjacency list:** edges where this node is the source
- **Incoming adjacency list:** edges where this node is the target

This doubles storage but enables bidirectional traversal without full scans.

### On-disk layout

Each node's adjacency list is typically stored as a variable-length record in a B-tree or hash table, keyed by node ID. Insertions extend the list; deletions remove entries. Variable-length records complicate page layout (overflow pages are common for high-degree nodes).

### Performance assessment

| Operation | Complexity | Notes |
|-----------|-----------|-------|
| Node lookup | O(1) | Hash map or B-tree lookup |
| Find all neighbors | O(degree) | Direct list scan |
| Find edge by type | O(degree) | Linear scan of adjacency list |
| Multi-hop traversal | O(edges visited) | BFS/DFS over adjacency lists |
| Insert edge | O(1) amortized | Append to list |
| Delete edge | O(degree) | Find and remove from list |
| Type scan (all nodes of type X) | O(N) naively | Requires secondary index |

### Strengths
- Simple to implement and reason about
- Insert is fast (append)
- Works naturally with dynamic graphs
- Memory layout maps cleanly to Rust `Vec<EdgeId>` per node

### Weaknesses
- High-degree nodes cause large variable-length records → overflow pages
- Type-filtered traversal requires linear scan unless a secondary index is maintained
- No inherent spatial locality — nodes and their adjacency lists may be scattered across pages
- Incoming edges require a separate inverted structure or full scan

### Relevance to this project
Adjacency lists are a natural in-memory representation and a reasonable starting point. Their weakness is on-disk: variable-length records interact poorly with fixed-size pages. A hybrid approach (fixed-size node record + separate edge index) is more practical on disk.

---

## 3. Strategy 2: Adjacency Matrix

### Description

A 2D boolean (or weighted) matrix where entry `M[i][j]` is non-zero if there is an edge from node i to node j. For property graphs, `M[i][j]` could store an edge ID.

```
     1   2   3   4
1  [ 0   1   1   0 ]
2  [ 1   0   0   1 ]
3  [ 1   0   0   0 ]
4  [ 0   1   0   0 ]
```

### Performance assessment

| Operation | Complexity | Notes |
|-----------|-----------|-------|
| Node lookup | O(1) | Row access |
| Edge existence check | O(1) | `M[i][j]` lookup |
| Find all neighbors | O(N) | Scan row i |
| Multi-hop traversal | O(N²) per hop (or O(N^k) for k hops) | Very poor |
| Insert node | O(N²) | Must resize matrix |
| Memory usage | O(N²) | Prohibitive for large sparse graphs |

### Suitability for this project

**Not recommended.** Adjacency matrices are appropriate only for dense graphs where most pairs of nodes are connected. Knowledge graphs and ontology-backed graphs are inherently sparse (a node might have 10–1000 edges, not 10,000+). The O(N²) memory cost alone disqualifies this approach. It is included here for completeness.

---

## 4. Strategy 3: Compressed Sparse Row (CSR)

### Description

CSR is a standard format from sparse matrix research, widely used in graph analytics (GraphX, Ligra, GraphBLAS). It represents a graph as two arrays:

- **`offsets` array** (size N+1): `offsets[i]` is the index in the `adjacency` array where node i's neighbors begin. `offsets[i+1] - offsets[i]` is node i's degree.
- **`adjacency` array** (size E): the concatenated neighbor lists of all nodes, in node-ID order.

```
Graph: 0→1, 0→2, 1→2, 2→0

offsets:   [0,  2,  3,  4]
adjacency: [1, 2, 2, 0]
           ↑       ↑   ↑
           Node 0  Node 1 Node 2
           starts  starts starts
```

For a property graph, the adjacency array stores edge IDs rather than raw target node IDs.

### In-memory layout

Two flat arrays. Extremely cache-friendly for traversal: to visit all neighbors of node i, read `adjacency[offsets[i]..offsets[i+1]]` — a contiguous memory range. This is the best possible cache locality for read-heavy traversal.

### On-disk layout

Two flat arrays mapped to disk. Can be stored as two fixed-size page sequences (offsets array on pages 0..K, adjacency array on pages K..M). No variable-length records; page boundaries are clean.

### Performance assessment

| Operation | Complexity | Notes |
|-----------|-----------|-------|
| Node lookup | O(1) | Index into offsets array |
| Find all neighbors | O(degree) | Contiguous array scan — excellent cache perf |
| Multi-hop traversal | O(edges visited) | Best cache behavior of any representation |
| Type-filtered traversal | O(degree) | Linear scan, possibly with secondary sort |
| Insert node (append) | O(1) amortized | Append to arrays |
| Insert edge | O(N + E) worst case | Must shift all entries after insertion point |
| Delete edge | O(N + E) worst case | Must compact arrays |
| Memory usage | O(N + E) | Optimal |

### Strengths
- **Best cache behavior** of any representation for traversal
- Minimal memory overhead (just two arrays)
- Trivially serialized to disk (flat binary arrays)
- Standard in high-performance graph analytics

### Weaknesses
- **Poor write performance.** Inserting or deleting an edge in the middle of the adjacency array requires shifting all subsequent elements — O(N + E). This makes CSR essentially **read-only** in practice.
- Not suitable for dynamic graphs (frequent inserts/deletes).
- Requires knowing the full graph at construction time (or expensive rebuilds).

### Suitability for this project

**Not recommended as the primary storage structure**, but highly relevant as:
1. An **export/snapshot format** for analytics queries
2. An **in-memory working representation** built transiently for a specific traversal operation
3. A reference point for cache performance

The project must support dynamic updates (inserting/deleting nodes and edges within transactions), which disqualifies static CSR as the primary structure. However, an adapter that materializes a subgraph into CSR for an analytics query is a useful future extension.

---

## 5. Strategy 4: Index-Free Adjacency (Native Graph Storage)

### Description

**Index-free adjacency** is the defining architectural feature of native graph databases (Neo4j being the most famous example). The key idea: **each node stores a direct pointer to its first edge record, and each edge record stores direct pointers to the next edge in each endpoint's adjacency list.** This forms a doubly-linked intrusive list structure embedded in the records themselves.

```
Node Record (fixed size):
  ┌────────────────────────────────────────┐
  │ node_id: u64                           │
  │ type_id: u32                           │
  │ first_outgoing_edge_ptr: PageOffset    │  ← direct pointer to first outgoing edge
  │ first_incoming_edge_ptr: PageOffset    │  ← direct pointer to first incoming edge
  │ property_store_ptr: PageOffset         │
  └────────────────────────────────────────┘

Edge Record (fixed size):
  ┌────────────────────────────────────────┐
  │ edge_id: u64                           │
  │ type_id: u32                           │
  │ source_node_ptr: PageOffset            │
  │ target_node_ptr: PageOffset            │
  │ next_outgoing_from_source: PageOffset  │  ← next edge in source's outgoing list
  │ prev_outgoing_from_source: PageOffset  │  ← prev edge (for O(1) delete)
  │ next_incoming_to_target: PageOffset    │  ← next edge in target's incoming list
  │ prev_incoming_to_target: PageOffset    │  ← prev edge
  │ property_store_ptr: PageOffset         │
  └────────────────────────────────────────┘
```

Traversal requires **no index lookup.** To find all outgoing edges of node N:
1. Read node N's record. Follow `first_outgoing_edge_ptr` to the first edge.
2. Read that edge record. Process it. Follow `next_outgoing_from_source` to the next edge.
3. Repeat until the pointer is null.

Each step is one disk read (one page load), with no B-tree traversal needed.

### Fixed-size records and their importance

The critical enabler of index-free adjacency is **fixed-size records**. If node records and edge records are fixed-size, then a record's disk location can be computed directly from its ID:

```
node_byte_offset = node_store_base + node_id × NODE_RECORD_SIZE
edge_byte_offset = edge_store_base + edge_id × EDGE_RECORD_SIZE
```

This makes "follow pointer" O(1): compute offset, read one page (or one record within a cached page). Neo4j uses this exact mechanism: node records are 15 bytes, edge records are 34 bytes, with a dedicated store file for each type.

### Performance assessment

| Operation | Complexity | Notes |
|-----------|-----------|-------|
| Node lookup by ID | O(1) | Direct offset computation |
| Find all outgoing edges | O(degree) | Pointer-following, no index |
| Find all incoming edges | O(degree) | Pointer-following, no index |
| Multi-hop traversal | O(edges visited) | Each hop is O(1) pointer follow |
| Type-filtered traversal | O(degree) | Linear scan of edge chain; secondary index helps |
| Insert edge | O(1) | Prepend to adjacency lists; update 4 pointers |
| Delete edge | O(1) | Doubly-linked list remove; update 4 pointers |
| Type scan (all nodes of type X) | O(N) naively | Requires secondary index |

### Strengths
- **Best traversal performance.** O(1) per hop with direct pointer following.
- **Fixed-size records** enable direct offset addressing — no page table or B-tree traversal for node/edge access.
- **O(1) insert and delete** — just update a small number of pointers.
- Simple and well-understood conceptually.
- Proven at scale (Neo4j has used this architecture for 15+ years).

### Weaknesses
- **Fixed-size records are inflexible.** Properties must be stored out-of-line (in a separate property store), adding an extra indirection for property access.
- **Pointer chasing** across disk pages is cache-unfriendly: each hop may require loading a different page. For high-degree nodes, this is worse than CSR.
- **Maintaining pointers during updates** is complex: node/edge IDs must be stable (never reused in ways that break live pointers), and the doubly-linked list management is error-prone.
- **Space overhead.** Fixed-size records waste space for sparse nodes (few properties, few edges) and cannot represent nodes with many properties inline.
- **No support for multi-valued or variable-length properties** in the main record without an overflow store.

### Variant: Semi-index-free adjacency with B-tree skip

A practical middle ground: store a **primary edge pointer** in the node record (for O(1) traversal start), but also maintain a **B-tree secondary index** mapping `(node_id, edge_type_id) → first_edge_of_that_type_ptr`. This enables type-filtered traversal in O(log N) rather than O(degree), without sacrificing the O(1) traversal for unfiltered queries.

### Relevance to this project

Index-free adjacency is highly relevant. Its O(1) per-hop traversal is ideal for graph workloads. The fixed-size record constraint is manageable by storing properties out-of-line (which is the natural approach for variable-length property bags anyway). The main challenge is integrating fixed-size record files with a CoW B-tree storage model (see section 13).

---

## 6. Strategy 5: Edge-Centric Storage

### Description

In standard node-centric storage, the organizing structure is the set of nodes and each node "owns" its adjacency list. In **edge-centric storage**, the edge is the primary record, and nodes are secondary. The most famous system using edge-centric storage is **PowerGraph** (for distributed graph analytics) and, to some degree, columnar graph stores.

**Primary structure:** An edge table, sorted by (source, type, target):

```
Edge Table (sorted by source_id, then type_id, then target_id):
  (1, knows, 3, {weight: 0.9})
  (1, knows, 7, {weight: 0.5})
  (1, works_at, 12, {})
  (3, knows, 1, {weight: 0.9})
  (3, owns, 9, {})
  ...
```

Nodes are referenced by edge endpoints. A separate node property table exists but is secondary.

### In-memory / on-disk layout

The edge table is stored as a sorted B-tree or sorted array. For a single-file format, this maps to a B-tree keyed by `(source_id, type_id, target_id)`. To find all outgoing edges of type T from node N: seek to key `(N, T, MIN)` and scan forward — a range scan on the B-tree.

### Performance assessment

| Operation | Complexity | Notes |
|-----------|-----------|-------|
| Find all outgoing edges of node N | O(log E + degree) | B-tree range scan |
| Find all outgoing edges of type T from N | O(log E + count) | Narrower range scan |
| Multi-hop traversal | O(hops × log E + edges visited) | Per-hop index lookup |
| Insert edge | O(log E) | B-tree insert |
| Delete edge | O(log E) | B-tree delete |
| Node lookup | O(log N) | Separate node table |
| Type scan | O(log E + count) | Range scan on type dimension |

### Strengths
- **Type-filtered traversal is efficient** — the sort key places edges of the same type together.
- **Simple conceptual model** — a single sorted structure captures all graph relationships.
- **Insertions and deletions are O(log E)** — dynamic updates are efficient.
- Naturally supports **edge property access** without indirection.
- No pointer maintenance required.

### Weaknesses
- **Every traversal requires a B-tree lookup** — no O(1) hop performance.
- **Node properties require a separate store** — a node's properties are not co-located with its edge list.
- **Not cache-friendly for multi-hop traversal** — each hop is a separate index lookup.
- **Incoming edges** require a second B-tree sorted by (target, type, source).

### Relevance to this project

Edge-centric storage maps naturally onto B-tree indexes. It is the approach taken by most **non-native graph databases** (graph features built on top of an RDBMS or key-value store). For this project, an edge-centric B-tree index is a natural complement to index-free adjacency: use index-free adjacency for traversal, but also maintain edge B-tree indexes for type-filtered lookups and range scans. The two are not mutually exclusive.

---

## 7. Strategy 6: Non-Native Graph Storage (Relational / Key-Value Decomposition)

### Description

Many databases add graph capabilities by decomposing the graph into relational tables or key-value entries. This is called **non-native graph storage** — the underlying store is not designed for graphs, and graph operations are translated into relational operations.

**Relational decomposition:**
```sql
CREATE TABLE nodes (id BIGINT, type_id INT, ...);
CREATE TABLE edges (id BIGINT, type_id INT, source_id BIGINT, target_id BIGINT, ...);
CREATE TABLE node_properties (node_id BIGINT, key TEXT, value BLOB);
CREATE TABLE edge_properties (edge_id BIGINT, key TEXT, value BLOB);
```

Traversal: `SELECT * FROM edges WHERE source_id = ?` — a table scan with an index on source_id.

**Key-value decomposition (e.g., on top of RocksDB):**
```
"node:{id}"       → node type + metadata
"edge:{id}"       → edge type + source + target
"out:{node}:{type}:{edge}" → "" (key existence encodes membership)
"in:{node}:{type}:{edge}"  → ""
"prop:node:{id}:{key}"     → value bytes
```

Traversal: prefix scan on `"out:{node}:"` keys.

### Performance assessment

| Operation | Complexity | Notes |
|-----------|-----------|-------|
| Node lookup | O(log N) | Index or KV lookup |
| Find all outgoing edges | O(log E + degree) | Index range scan |
| Multi-hop traversal | O(hops × log E + edges visited) | Per-hop index lookup |
| Insert/delete | O(log E) | B-tree or LSM insert |
| Type scan | O(log E + count) if indexed | Depends on index design |

### Strengths
- **Simplest to implement** — reuses existing storage primitives.
- **Flexible schema** — adding a new index is a first-class operation.
- **Well-understood query patterns** — maps to range scans and point lookups.
- No custom data structures required.

### Weaknesses
- **No O(1) traversal.** Every hop requires a B-tree lookup — 3–4 disk reads minimum.
- **High traversal overhead.** For a 5-hop path query, this means 15–20 disk reads minimum, versus 5 disk reads with index-free adjacency.
- **Not a "native" graph database** — graph performance is inherently limited by the underlying relational model.
- **Multi-key atomicity** is complex (each node/edge spans multiple key-value entries).

### Relevance to this project

Non-native storage is **not recommended as the primary architecture**, but:
1. The project will maintain **B-tree indexes** for type scans and property queries — this is essentially a selective use of the non-native pattern for secondary indexes.
2. Understanding the pattern is important because it clarifies *what* index-free adjacency is better at — and motivates maintaining both adjacency pointers and B-tree indexes.

---

## 8. Cross-Cutting Concern: Property Storage

All graph storage strategies must answer: **where and how are properties stored?**

### Option A: Inline (small properties only)

Store a small fixed number of common properties directly in the node/edge record. Overflow to a separate store for additional properties.

- **Pro:** Zero indirection for common properties
- **Con:** Fixed-size record bloat; wastes space for nodes with few properties; inflexible

### Option B: External property store (variable-length records)

Each node/edge record stores a pointer to its property block. The property block contains encoded key-value pairs (e.g., a compact binary format like MessagePack or a hand-rolled encoding).

```
Property block layout:
  [count: u16]
  [key_id: u32, value_type: u8, value_len: u32, value_bytes: ...]×count
```

- **Pro:** Flexible; variable-length; properties of any size
- **Con:** One extra disk read per node/edge access (unless cached)

### Option C: Property B-tree (columnar-adjacent)

Store all properties in a B-tree keyed by `(node_id, property_key_id)`. Each entry stores the property value.

- **Pro:** Efficient range scans by property key; easy to add new property keys
- **Con:** High overhead for retrieving all properties of a single node (multiple B-tree lookups or a range scan)

### Recommendation for this project

**Option B** (external property store) is the most natural fit:
- It decouples property storage from node/edge records, allowing both to have clean fixed-size designs
- It supports variable-length values (strings, blobs, nested structures)
- One extra pointer per node/edge is acceptable; the buffer pool makes this cost near-zero for hot data
- Property blocks for small nodes/edges fit within a single page; the buffer pool keeps them warm

A **property key registry** (mapping string property keys to compact integer key IDs) should be maintained globally to avoid storing key strings inline in every property block.

---

## 9. Cross-Cutting Concern: Type Metadata Storage

The schema/type system (node types, edge types, property type definitions, type hierarchies) must be stored persistently. Options:

### Option A: Dedicated schema pages

Reserve the first N pages of the database file for schema/type metadata. Fixed layout; fast startup (always read these pages first).

### Option B: Schema as graph nodes

Store type definitions as special nodes in the graph itself, with a reserved type ID (e.g., type 0 = "meta-type"). The type hierarchy is expressed as edges between type nodes.

- **Pro:** Elegant; type queries use the same traversal API
- **Con:** Bootstrap problem (what type does a type node have?); special-case handling required everywhere

### Option C: Separate B-tree for schema

A dedicated B-tree for type metadata, co-resident in the same file, separate from the main graph B-trees.

### Recommendation for this project

**Option C** (separate B-tree for schema) in dedicated schema pages at a well-known location in the file. This is clean, avoids bootstrap problems, and is consistent with the rest of the storage design. Schema data is small and rarely updated; it can fit in a small number of pages loaded at startup.

---

## 10. Comparison Matrix

Strategies compared on the axes specified by the task done criterion:

| Strategy | Read Perf | Write Perf | Memory Layout | Traversal Efficiency | Single-File Embedded Suitability |
|----------|-----------|------------|---------------|---------------------|----------------------------------|
| **Adjacency List** | Good | Good | Variable-length, scattered | Good (O(degree)) | Moderate — variable-length records complicate page layout |
| **Adjacency Matrix** | N/A | Poor | O(N²), dense | Very poor (O(N) per neighbor scan) | Not suitable |
| **CSR** | Excellent | Very poor (no updates) | Flat arrays, cache-optimal | Excellent (contiguous memory) | Read-only snapshots only; not for dynamic graphs |
| **Index-Free Adjacency** | Excellent | Excellent | Fixed-size records, pointer chains | Excellent (O(1) per hop) | **Strong** — fixed-size records map cleanly to pages |
| **Edge-Centric (B-tree)** | Good | Good | B-tree sorted by (src, type, tgt) | Good (O(log E) per hop) | **Strong** — natural fit for B-tree storage; good for type scans |
| **Non-Native (KV/Relational)** | Moderate | Good | Multiple B-trees/tables | Moderate (O(log E) per hop) | Moderate — multiple structures to maintain; traversal overhead high |

**Detail notes:**

**Adjacency List:**
- Read performance is good for unfiltered adjacency scans, but type-filtered queries degrade to O(degree) linear scans without secondary indexes.
- Write performance is good (append), but deletion requires searching within a variable-length list.
- The variable-length nature of lists makes page layout complex (overflow pages needed for high-degree nodes).

**CSR:**
- Traversal is the best of any representation due to cache locality, but only in the static case. Any insertion or deletion requires O(N+E) work to shift elements. Suitable only as a secondary, read-only view.

**Index-Free Adjacency:**
- O(1) per-hop is unmatched. The pointer-following pattern is cache-unfriendly across pages (each hop may be a new page), but the buffer pool ameliorates this for hot subgraphs.
- Fixed-size records are ideal for an embedded single-file database: `node_id × RECORD_SIZE` gives the byte offset directly. No B-tree traversal needed for individual record access.
- Delete is O(1) with doubly-linked edge lists — just update pointers.
- The weakness is type-filtered traversal (still O(degree) without a secondary index).

**Edge-Centric (B-tree):**
- A natural complement to index-free adjacency rather than a standalone strategy. Efficient for type-filtered queries and range scans; each hop requires a B-tree lookup (O(log E)).
- Write performance is O(log E) for inserts/deletes — good for dynamic graphs.

---

## 11. Suitability for Single-File Embedded Context

The project constraint (single-file, embedded, no external database dependencies) adds specific requirements that not all strategies satisfy equally:

### Fixed vs. variable-length records on-disk

A single-file database must multiplex multiple data structures within one file, organized as pages. Fixed-size records integrate cleanly with this model:
- A node record store can be a contiguous region of the file divided into fixed-size slots
- A given node's record can be located by direct offset computation: `base_offset + node_id × record_size`
- No B-tree traversal is needed just to find a node's record
- Page boundaries are clean; no cross-page record splitting

Variable-length records require the full slotted-page machinery (slot arrays, overflow chains) — more complex.

**Conclusion:** Fixed-size records for nodes and edges, with variable-length properties stored out-of-line, is the optimal layout for a single-file embedded database.

### CoW B-tree compatibility

As recommended in `001-db-internals-fundamentals.md`, this project leans toward a CoW B-tree for crash safety and MVCC. The graph storage layer must be compatible with this choice:

- **Index-free adjacency with fixed-size records:** Can coexist with a CoW B-tree. The node/edge record stores are "slot files" with their own free-space management. Pointer updates on insert/delete still require CoW semantics (copy the modified page, update the pointer in the new page version). This works but requires care — the doubly-linked pointer chain means a single edge insertion modifies the source node record, the target node record, and the edge record — three pages to copy.

- **Edge-centric B-tree indexes:** Directly compatible with CoW B-trees. An insert/delete to the edge B-tree follows standard CoW B-tree procedures.

- **Hybrid (both):** Maintain both fixed-size record stores (for index-free traversal) and B-tree indexes (for type-filtered queries and range scans). The CoW B-trees serve as the indexes; the slot stores serve as the primary record stores. This is the recommended architecture.

### File layout sketch

```
File regions (conceptual):
┌──────────────────────────────────────────────────────┐
│ File header (page 0)                                 │
│ - Magic number, version, page size                   │
│ - Root pointers: node B-tree root, edge B-tree root  │
│ - Schema B-tree root                                 │
│ - Free-space map root                                │
│ - Node store base page, edge store base page         │
├──────────────────────────────────────────────────────┤
│ Node record store (fixed-size slots)                 │
│ - Slot i = node with id i                            │
│ - Each slot: type_id, first_out_edge_id,             │
│   first_in_edge_id, property_ptr, flags              │
├──────────────────────────────────────────────────────┤
│ Edge record store (fixed-size slots)                 │
│ - Slot i = edge with id i                            │
│ - Each slot: type_id, src_id, tgt_id,                │
│   next/prev_out, next/prev_in, property_ptr, flags   │
├──────────────────────────────────────────────────────┤
│ Property store (variable-length, slotted pages)      │
│ - Referenced by pointer in node/edge records         │
│ - Each block: encoded key-value pairs                │
├──────────────────────────────────────────────────────┤
│ B-tree region (CoW B+ trees)                         │
│ - Type index: (type_id, node_id) → node_id           │
│ - Edge type index: (type_id, src_id, edge_id)        │
│ - Property index (optional): (type_id, key, value)   │
│ - Schema B-tree: type definitions and hierarchy      │
├──────────────────────────────────────────────────────┤
│ Free-space map                                       │
│ - Tracks free slots in node/edge stores              │
│ - Tracks free pages in property store                │
└──────────────────────────────────────────────────────┘
```

This layout is discussed further in the context of the file format spec (task 8). It is presented here to show how the graph storage strategies land in a concrete single-file design.

---

## 12. Recommendation Shortlist

### Primary recommendation: Hybrid Index-Free Adjacency + CoW B-tree Indexes

**What it is:**
- **Node and edge records are fixed-size** and stored in dedicated "slot stores" (compact arrays of fixed-size records, each addressable by direct offset from ID).
- Each node record contains **direct pointers** (edge IDs, which resolve to byte offsets in the edge store) to the first outgoing and incoming edge — enabling O(1) adjacency traversal without index lookup.
- Each edge record contains doubly-linked pointers to the next/previous edge in each endpoint's adjacency list — enabling O(1) insert/delete.
- **Properties are stored out-of-line** in a separate property store (variable-length, slotted pages), referenced by pointer in the node/edge record.
- **CoW B-tree indexes** are maintained for:
  - Type scans: find all nodes/edges of a given type
  - Type-filtered adjacency: find all edges of a given type from a given node (secondary index)
  - Property scans: find nodes/edges with a given property value (optional/extensible)

**Why this recommendation:**

1. **Traversal performance.** The core operation of a graph database — multi-hop traversal — becomes a sequence of pointer follows, each resolved by offset computation (no disk seeks beyond the page containing the record). The buffer pool keeps hot node/edge records in RAM, making traversal of popular subgraphs nearly free.

2. **Write performance.** Insert/delete is O(1) for the slot stores (update a few pointers) plus O(log E) for B-tree index updates. This is excellent for a dynamic, transactional workload.

3. **Fixed-size record cleanliness.** Fixed-size records eliminate overflow page complexity for the primary node/edge data. They integrate cleanly with CoW B-tree crash safety (page modifications are always whole-page writes).

4. **Complementary B-tree strengths.** The B-tree indexes handle the cases where index-free adjacency is weak (type scans, property-filtered queries) without sacrificing traversal performance.

5. **Proven architecture.** This hybrid is essentially the architecture of Neo4j (for traversal) combined with any RDBMS's B-tree secondary indexes. It's well-understood.

**Tradeoffs to acknowledge:**

- **Increased implementation complexity** vs. a pure edge-centric B-tree approach: two storage subsystems (slot stores + B-trees) must be maintained consistently.
- **Pointer maintenance during CoW** requires care: an edge insertion modifies 3 records (source node, target node, edge). In a CoW B-tree model, this means copying the pages containing those 3 records. This is manageable but must be done correctly.
- **ID stability requirement:** Node and edge IDs must be stable (never change after assignment) because they are used as slot indices and as pointer values. Deletions must mark slots as free (tombstone), not reuse IDs immediately (or use a careful recycling scheme).
- **High-degree node hotspot:** For a node with 1 million outgoing edges, traversing all edges still requires 1 million record reads — there's no way around this. The buffer pool helps, but this is inherent to graph workloads, not specific to this strategy.

### Alternative: Pure Edge-Centric B-tree (simpler, slower traversal)

**What it is:** Drop the index-free adjacency (no pointers in node/edge records). Store all graph relationships in B-tree indexes: an edge B-tree keyed by `(source_id, type_id, edge_id)` and an inverted edge B-tree keyed by `(target_id, type_id, edge_id)`. Node records are small (just type + property pointer).

**When to prefer this:**
- If implementation simplicity is the overriding concern (fewer structures to maintain)
- If write throughput is more important than traversal latency
- If the graph is sparse and traversals are typically short (1–2 hops)

**Tradeoff:** Each traversal hop requires a B-tree lookup (O(log E), ~3–4 page reads). For a 5-hop path query over a graph with 1 million edges, this is ~20 page reads vs. ~5 page reads with index-free adjacency. For graph databases, this difference is significant.

### Not recommended: Pure CSR or Adjacency Matrix

Disqualified by inability to support dynamic updates efficiently (CSR) or by O(N²) space (matrix).

---

## 13. Interaction with Database Internals (Task 1 Integration)

Drawing on `001-db-internals-fundamentals.md`, the recommended hybrid strategy interacts with the database internals layer as follows:

### Buffer pool interaction

The node and edge record stores are hot data — traversal touches node and edge records in rapid succession. The buffer pool should:
- Prioritize pages from the node/edge stores with a higher eviction priority than B-tree index pages (since index pages can be reconstructed from the data, while record pages are the primary data)
- In practice, the buffer pool's LRU/Clock eviction policy will naturally keep frequently-accessed records warm

The property store is relatively cool — properties are read only when needed after traversal locates the relevant node/edge. Keeping the node/edge record pages warm but allowing property pages to be evicted is a natural working set behavior.

### CoW B-tree and slot stores

The slot stores (node record store, edge record store) are not themselves B-trees. They are direct-addressed arrays. CoW semantics for slot stores work differently:

- **On transaction commit:** The modified slot pages (pages containing modified node/edge records) are written to new locations in the file. The free-space map is updated. The file header's slot-store root is updated atomically.
- **Alternative:** Use the slot stores as append-only (immutable once written) with a separate indirection table (a B-tree mapping `node_id → current_slot_page_offset`). This restores full CoW semantics but adds one B-tree lookup per node/edge access — essentially converting index-free adjacency into something closer to the edge-centric B-tree approach for cold data.

**Recommendation:** Use simple in-place updates for slot store pages, protected by a WAL, rather than full CoW. This is a pragmatic choice: CoW for the B-tree indexes (which are the complex structures) and WAL for the simpler slot stores. Alternatively, treat the slot stores as another type of B-tree page (CoW applies uniformly). Task 7 must decide this.

### MVCC and snapshots

MVCC requires that older versions of records remain visible to transactions that started before an update. With slot stores:
- Simple approach: mark updated records with a version (transaction ID). Readers check visibility before returning a record.
- CoW approach: old slot pages are retained until all readers that might need them are done.

The CoW B-tree indexes handle MVCC naturally (old root = old snapshot). The slot stores require explicit version tracking or CoW-of-pages to match.

### Transaction scope

An edge insertion must be atomic across 3 record updates (source node's adjacency list head, target node's adjacency list head, new edge record). This is a multi-record transaction. The WAL or CoW mechanism must ensure these 3 updates are either all committed or all rolled back.

---

## Completion Report: Task 2 — Graph Storage Strategies

### Status: COMPLETE

### Done Criterion:
The criterion requires comparing at least 4 strategies on read/write performance, memory layout, traversal efficiency, and suitability for a single-file embedded context, plus a recommendation shortlist with tradeoffs.

Strategies covered:
- Adjacency Lists (Section 2) ✓
- Adjacency Matrix (Section 3) ✓
- CSR (Section 4) ✓
- Index-Free Adjacency (Section 5) ✓
- Edge-Centric Storage (Section 6) ✓
- Non-Native Relational/KV (Section 7) ✓
- Cross-cutting: Property Storage (Section 8) ✓
- Cross-cutting: Type Metadata Storage (Section 9) ✓

Comparison matrix on all required axes (Section 10) ✓
Single-file embedded suitability analysis with concrete file layout sketch (Section 11) ✓
Recommendation shortlist with tradeoffs (Section 12) ✓

### Deliverables:
- `002-graph-storage-strategies.md` — this document

### Summary:
Surveyed six graph storage strategies in depth, analyzed two cross-cutting storage concerns (properties and type metadata), produced a comparison matrix, and recommended a **hybrid index-free adjacency + CoW B-tree index** architecture as the primary recommendation, with a pure edge-centric B-tree as a simpler alternative. Included a concrete single-file layout sketch showing how the recommended architecture maps to pages within the file.

The document identifies a key tension (traversal performance vs. write flexibility) and explains how the hybrid architecture resolves it by using different storage structures for different access patterns.

### Context for Next Task:
This document is a dependency for **Task 7 (Graph Storage Model)**. Task 7 will also depend on `001-db-internals-fundamentals.md` and `006-schema-extension-spec.md` (Task 6's output).

Key inputs from this document for Task 7:
1. **Primary recommendation:** Hybrid index-free adjacency (fixed-size slot stores) + CoW B-tree secondary indexes.
2. **Alternative:** Pure edge-centric B-tree if simplicity is prioritized.
3. **Property storage:** Out-of-line variable-length property blocks, referenced by pointer in node/edge records.
4. **Type metadata:** Dedicated schema B-tree in the same file.
5. **Open design question for Task 7:** How to handle CoW semantics for slot stores — WAL for slot stores + CoW for B-trees, or uniform CoW for everything. This is the most significant unresolved decision from this research phase.

This document may also provide useful context for **Task 8 (Single-File Format)** in the section on file layout (Section 11).

### Residual Concerns:
1. **CoW + slot store interaction** is the most uncertain area. The interaction between CoW B-tree semantics and the direct-addressed slot stores needs careful design in Task 7. Two valid paths exist (WAL for slot stores vs. treating slot pages as CoW pages), and the correct choice depends on the overall CoW vs. WAL strategy decided in Task 7.
2. **High-degree node performance:** The recommended architecture does not address pathological performance for nodes with millions of edges (e.g., a hub node in a social graph). A skip index or degree-based secondary structure could help but adds complexity. This is a known risk but acceptable for the initial design.
3. **ID recycling:** The fixed-size slot store requires stable IDs. The decision on whether and how to recycle deleted IDs (to avoid unbounded slot store growth) is deferred to Task 7.
4. **CSR as a materialization target:** The document recommends CSR only as a secondary format. If the project later adds an analytics query mode, CSR materialization of subgraphs is a natural optimization. This is out of scope for the initial design but worth noting.

### Upstream Flags:
None. All concerns are scoped within the storage design branch (Tasks 7, 8).
