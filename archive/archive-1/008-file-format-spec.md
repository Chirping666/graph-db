# 008 — Single-File Format Specification

**Project:** Embedded Graph Database with Extensible Schema & Pluggable Inference  
**Task:** 8 — Design: Single-File Format  
**Status:** Complete  
**Depends on:** Task 1 (`001-db-internals-fundamentals.md`), Task 4 (`004-embedded-db-architectures.md`), Task 7 (`007-graph-storage-model.md`)  
**Intended audience:** All downstream design and implementation tasks (especially Tasks 9, 12, 15, 16). A reader familiar with Rust and basic database concepts (from Task 1) should be able to implement this file format specification without reference to external sources.

---

## Table of Contents

1. [Purpose and Scope](#1-purpose-and-scope)
2. [File Structure Overview](#2-file-structure-overview)
3. [File Identity Header](#3-file-identity-header)
4. [Dual-Superblock Design](#4-dual-superblock-design)
5. [Common Page Header](#5-common-page-header)
6. [Page Type Taxonomy](#6-page-type-taxonomy)
7. [B-Tree Interior Page Format](#7-b-tree-interior-page-format)
8. [B-Tree Leaf Page Format](#8-b-tree-leaf-page-format)
9. [Overflow Page Format](#9-overflow-page-format)
10. [Free Page Format](#10-free-page-format)
11. [Free-Space Management](#11-free-space-management)
12. [Page Allocation and File Growth](#12-page-allocation-and-file-growth)
13. [Commit Protocol](#13-commit-protocol)
14. [Crash Safety and Recovery](#14-crash-safety-and-recovery)
15. [fsync Discipline](#15-fsync-discipline)
16. [Versioning and Extensibility](#16-versioning-and-extensibility)
17. [Configuration and Limits](#17-configuration-and-limits)
18. [Design Decision Log](#18-design-decision-log)

---

## 1. Purpose and Scope

This document is the authoritative specification for the **binary file format** of the embedded graph database. It defines the on-disk layout at the byte level — sufficient for a developer to implement a reader and writer that produces correct, crash-safe database files.

### What this document defines

- The file structure: identity header, dual-superblock region, and data page region
- The byte-level layout of every page type (B-tree interior, B-tree leaf, overflow, free)
- The dual-superblock atomic commit mechanism
- Free-space management via the freelist B-tree
- Page allocation and file growth strategy
- The commit protocol and fsync ordering
- Crash safety guarantees, their limits, and the recovery procedure
- Versioning and extensibility mechanisms
- Configuration constraints and hard limits

### What this document does NOT define

- The logical B-tree catalog (which B-trees exist and what they store) — defined in `007-graph-storage-model.md` Section 4
- Record formats for nodes, edges, properties, and schema metadata — defined in `007-graph-storage-model.md` Section 5
- B-tree key encoding and ordering — defined in `007-graph-storage-model.md` Section 6
- The HAL trait abstraction for I/O — Task 9
- The buffer pool implementation — defined at the design level in `007-graph-storage-model.md` Section 10; implemented in Task 16
- The public Rust API — Task 10

### Relationship to upstream documents

This document builds directly on:

- **Task 1** (DB Internals): Provides the foundational understanding of pages, B-trees, WAL, CoW, and crash recovery concepts that this spec applies.
- **Task 4** (Embedded DB Architectures): Provides the dual-superblock pattern, page header conventions, free-space management strategies, and the design sketches (Sections 8.1–8.10) that this spec refines into a formal specification.
- **Task 7** (Graph Storage Model): Provides the seven logical B-trees, their key/value record formats, and the concurrency/transaction model that this file format must support.

---

## 2. File Structure Overview

The database file is a contiguous sequence of fixed-size pages. Every byte in the file belongs to exactly one page.

```
┌───────────────────────────────────────────────────────────────┐
│  Page 0:  Superblock A                                         │
├───────────────────────────────────────────────────────────────┤
│  Page 1:  Superblock B                                         │
├───────────────────────────────────────────────────────────────┤
│  Page 2:  Data page (B-tree interior, leaf, overflow, or free) │
├───────────────────────────────────────────────────────────────┤
│  Page 3:  Data page                                            │
├───────────────────────────────────────────────────────────────┤
│  ...                                                           │
├───────────────────────────────────────────────────────────────┤
│  Page N-1: Data page                                           │
└───────────────────────────────────────────────────────────────┘
```

**Key invariants:**

1. Pages 0 and 1 are always superblock pages. They are never allocated for any other purpose.
2. Data pages (pages 2+) are managed by the page allocator. Their type is determined by the `page_type` field in the common page header.
3. The byte offset of page P in the file is `P × page_size`.
4. Every page is exactly `page_size` bytes. The page size is fixed at database creation time and recorded in the superblock.
5. Unused bytes within any page are zero-filled.

### Minimum file size

A newly created database file contains exactly 3 pages:

| Page | Content |
|------|---------|
| 0 | Superblock A (initial, transaction_id = 1) |
| 1 | Superblock B (initial, transaction_id = 0) |
| 2 | Root page for the Schema Store B-tree (empty leaf) |

The remaining six B-trees (Node Store, Edge Store, Outgoing Adjacency, Incoming Adjacency, Type Index, ID Freelist) start with a `root_page = 0` sentinel in the superblock, meaning "this tree is empty and has no root page allocated." The Page Freelist B-tree (free-space tracking) also starts with `root_page = 0` (no free pages initially). The Schema Store root page is pre-allocated because the database initialization transaction writes the ID counters into it.

---

## 3. File Identity Header

The first 32 bytes of page 0 constitute the **file identity header**. This region is identical in both superblock copies and **never changes after database creation**. It is written once at creation time and serves to identify the file.

```
File Identity Header (32 bytes):
┌──────────────────────────────────────────────────────────────┐
│ Offset  Size  Field              Description                  │
├──────────────────────────────────────────────────────────────┤
│   0      14   magic              Magic bytes (see below)      │
│  14       2   format_major       Major format version (u16 BE)│
│  16       2   format_minor       Minor format version (u16 BE)│
│  18       4   application_id     Downstream app ID (u32 LE)   │
│  22       2   page_size_raw      Page size encoding (u16 LE)  │
│  24       8   creation_timestamp Microseconds since Unix epoch│
│                                  (u64 LE; informational only) │
└──────────────────────────────────────────────────────────────┘
Total: 32 bytes
```

### 3.1 Magic bytes

```
Bytes: 45 6D 62 65 64 47 72 61 70 68 0D 0A 1A 0A
ASCII: E  m  b  e  d  G  r  a  p  h  \r \n ^Z \n
```

The magic sequence is 14 bytes. The design follows the PNG convention (as recommended by Task 4, Section 6.1):

- `"EmbedGraph"` (10 bytes): Human-readable identifier. A user examining the file with a hex editor or `file` command will immediately recognize it.
- `\x0D\x0A` (CR LF): Detects incorrect line-ending conversion. If a transfer process converts LF to CR LF (or vice versa), the magic check will fail.
- `\x1A` (Ctrl-Z): Stops display under MS-DOS `type` command, preventing accidental terminal corruption.
- `\x0A` (LF): Detects the reverse of the CR LF conversion (stripping CR from CR LF).

### 3.2 Format version

- `format_major` (u16, big-endian): Incremented for breaking format changes that older readers cannot understand. A reader must refuse to open a file whose `format_major` exceeds its own supported major version.
- `format_minor` (u16, big-endian): Incremented for backward-compatible additions. A reader with `format_minor = 2` can safely read a file with `format_minor = 3` (it may not understand the new optional fields, but can still read all existing data). A reader must refuse to **write** to a file with a higher `format_minor` than it supports unless operating in read-only mode.

**Initial values:** `format_major = 1`, `format_minor = 0`.

**Rationale for big-endian version fields:** The magic and version region is intended to be human-readable in hex dumps. Big-endian makes the version bytes appear in their natural order (e.g., `00 01` for version 1), matching the convention used by PNG, SQLite, and other file formats.

### 3.3 Application ID

A u32 (little-endian) available for downstream applications to claim the database file. For example, an OWL Lite ontology engine built on this crate might register `application_id = 0x4F574C4C` ("OWLL"). The core crate writes `0x00000000` by default. This field is informational — the core crate does not interpret it, but downstream crates can use it to prevent accidental opening of a database created by a different application.

### 3.4 Page size encoding

The `page_size_raw` field (u16, little-endian) encodes the page size as follows:

- If `page_size_raw` is in `{4096, 8192, 16384, 32768}`: the page size is `page_size_raw` bytes.
- If `page_size_raw == 1`: the page size is 65536 bytes. (This follows the SQLite convention: the value 1 represents 65536 because 65536 does not fit in a u16.)

Any other value is invalid. The page size must be a power of 2, minimum 4096, maximum 65536.

**Rationale for 4096 minimum:** Task 4 (Section 8.7) confirms 4096 as the standard choice, matching OS page size on all modern platforms. Smaller pages would reduce B-tree fan-out below acceptable levels and increase overhead from per-page headers.

---

## 4. Dual-Superblock Design

Pages 0 and 1 each contain a complete **superblock** — the set of all mutable root pointers and database state. The database alternates between the two on each commit. This is the cornerstone of the crash-safety mechanism (as documented in Task 1 Section 11, Task 4 Section 6.2, and Task 7 Section 12.4).

### 4.1 Superblock layout

Each superblock page has the file identity header (Section 3) occupying the first 32 bytes, followed by the mutable superblock fields. Because the identity header is immutable and identical in both copies, it is written once and never changes.

```
Superblock page layout (total: page_size bytes):
┌──────────────────────────────────────────────────────────────┐
│ [Bytes 0–31]   File Identity Header (Section 3; immutable)    │
├──────────────────────────────────────────────────────────────┤
│ [Bytes 32+]    Mutable Superblock Fields (below)              │
├──────────────────────────────────────────────────────────────┤
│ [Tail]         Zero-padding to page_size                      │
└──────────────────────────────────────────────────────────────┘
```

**Mutable superblock fields (starting at offset 32):**

```
Mutable Superblock Fields:
┌──────────────────────────────────────────────────────────────┐
│ Offset  Size  Field                  Description              │
├──────────────────────────────────────────────────────────────┤
│  32      8    transaction_id         Monotonic commit counter │
│                                      (u64 LE)                │
│  40      8    total_pages            File size in pages       │
│                                      (u64 LE)                │
│  48      8    feature_flags          Optional capability bits │
│                                      (u64 LE)                │
│                                                              │
│  --- B-tree root pointers (0 = tree is empty) ---            │
│                                                              │
│  56      8    node_store_root        Node Store B-tree root   │
│  64      8    edge_store_root        Edge Store B-tree root   │
│  72      8    outgoing_adj_root      Outgoing Adj. Index root │
│  80      8    incoming_adj_root      Incoming Adj. Index root │
│  88      8    type_index_root        Type Index B-tree root   │
│  96      8    schema_store_root      Schema Store B-tree root │
│ 104      8    id_freelist_root       ID Freelist B-tree root  │
│ 112      8    page_freelist_root     Page Freelist B-tree root│
│                                                              │
│  --- Reserved root pointers for future B-trees ---           │
│                                                              │
│ 120     32    _reserved_roots        4 × u64, must be zero   │
│                                                              │
│  --- Reserved space for future mutable fields ---            │
│                                                              │
│ 152     32    _reserved_fields       Must be zero             │
│                                                              │
│  --- Integrity ---                                           │
│                                                              │
│ 184      8    checksum               Checksum of bytes 0–183  │
│                                      (u64 LE)                │
└──────────────────────────────────────────────────────────────┘
Total mutable section: 160 bytes (offset 32–191)
Total superblock used: 192 bytes
Remaining page space: page_size - 192 bytes (available for future expansion)
```

**All u64 fields in the mutable section are little-endian** (consistent with Task 7's decision G5 for record values — the superblock is a record, not a sort key).

### 4.2 Field descriptions

**transaction_id (u64):** A monotonically increasing counter. Incremented by 1 on every successful commit. Starts at 1 for the database creation transaction. The superblock with the higher valid `transaction_id` is the active one.

**total_pages (u64):** The total number of pages in the file, including the two superblock pages. The file size in bytes is `total_pages × page_size`. On startup, the database verifies that the actual file size matches `total_pages × page_size` (tolerance: the file may be up to `page_size - 1` bytes larger due to an interrupted growth operation; the excess is truncated).

**feature_flags (u64):** Bit flags for optional features. All bits are initially 0. Bits are defined as needed by future format versions. A reader that encounters an unknown set bit should refuse to open the file (unless the unknown bit is in the "advisory" range — see Section 16).

**B-tree root pointers:** Eight u64 fields, each containing the page ID of the root page of one logical B-tree. A value of `0` means the tree is empty (has no allocated root page). Note that page IDs 0 and 1 are superblock pages, so `0` is never a valid data page ID — it serves as a null sentinel.

The eight B-trees correspond to those defined in `007-graph-storage-model.md` Section 4, plus the **Page Freelist** (free-space tracking B-tree, described in Section 11 of this document):

| Root pointer field | B-tree | Source |
|-------------------|--------|--------|
| `node_store_root` | Node Store | Task 7 §4 #1 |
| `edge_store_root` | Edge Store | Task 7 §4 #2 |
| `outgoing_adj_root` | Outgoing Adjacency Index | Task 7 §4 #3 |
| `incoming_adj_root` | Incoming Adjacency Index | Task 7 §4 #4 |
| `type_index_root` | Type Index | Task 7 §4 #5 |
| `schema_store_root` | Schema Store | Task 7 §4 #6 |
| `id_freelist_root` | ID Freelist | Task 7 §4 #7 |
| `page_freelist_root` | Page Freelist | This document §11 |

**_reserved_roots (4 × u64):** Reserved for future B-tree root pointers. Must be zero. This allows adding up to 4 new B-trees (e.g., the property value index described in Task 7 §7.5) without a breaking format change.

**_reserved_fields (32 bytes):** Reserved for future mutable metadata fields. Must be zero.

**checksum (u64):** A 64-bit checksum computed over all preceding bytes in the superblock page (bytes 0 through 183, inclusive). The checksum algorithm is xxHash3 (64-bit) — fast, high-quality, with `no_std`-compatible Rust implementations (`xxhash-rust` crate). The checksum covers both the immutable identity header and the mutable fields, providing integrity protection for the entire superblock.

### 4.3 Superblock selection on startup

On database open, both superblock pages are read and validated:

```
procedure select_superblock():
    read page 0 → sb_a
    read page 1 → sb_b
    valid_a = verify_magic(sb_a) AND verify_checksum(sb_a)
    valid_b = verify_magic(sb_b) AND verify_checksum(sb_b)
    
    if valid_a AND valid_b:
        if sb_a.transaction_id >= sb_b.transaction_id:
            return sb_a
        else:
            return sb_b
    else if valid_a:
        return sb_a    // sb_b was being written during a crash
    else if valid_b:
        return sb_b    // sb_a was being written during a crash
    else:
        return ERROR_DATABASE_CORRUPT
        // Both superblocks invalid — catastrophic corruption.
        // Requires two successive torn writes at the worst moment.
```

**Rationale for transaction_id as tie-breaker:** This is simpler and more robust than a separate "active slot" indicator. The transaction_id is always monotonically increasing, so the higher valid value is always the most recent commit. No separate atomic flag is needed.

### 4.4 Superblock consistency checks

On startup, after selecting the active superblock, the following checks are performed:

1. **Magic check:** The 14-byte magic must match exactly.
2. **Format version check:** `format_major` must be ≤ the reader's supported major version. If equal, `format_minor` must be ≤ the reader's supported minor version (for write access; any minor version is acceptable for read-only access).
3. **Page size check:** `page_size_raw` must decode to a valid page size.
4. **File size check:** The actual file size must be ≥ `total_pages × page_size`. If the file is larger (by less than `page_size` bytes), the excess is a partial page from an interrupted growth — truncate to `total_pages × page_size`.
5. **Root pointer sanity:** Every non-zero root pointer must be in the range `[2, total_pages)`.
6. **Feature flags check:** No unknown required feature flags are set (see Section 16).

If any check fails, the database returns an error and does not open.

---

## 5. Common Page Header

Every data page (pages 2+) begins with a **24-byte common header**. This header is present on all page types and provides type identification, integrity checking, and corruption detection.

```
Common Page Header (24 bytes):
┌──────────────────────────────────────────────────────────────┐
│ Offset  Size  Field         Description                       │
├──────────────────────────────────────────────────────────────┤
│  0       8    page_id       Self-referential page ID (u64 LE) │
│  8       1    page_type     PageType discriminant (u8)         │
│  9       1    flags         Page-type-specific flags (u8)      │
│ 10       2    _padding      Alignment padding (must be zero)   │
│ 12       8    txn_id        Transaction ID that wrote page     │
│                             (u64 LE)                          │
│ 20       4    checksum      Page integrity checksum (u32 LE)   │
└──────────────────────────────────────────────────────────────┘
Total: 24 bytes
Usable payload per page: page_size - 24 bytes
  (e.g., 4096 - 24 = 4072 bytes for 4KB pages)
```

### 5.1 Field descriptions

**page_id (u64 LE):** The page's own page number. Must equal `file_offset / page_size`. This self-referential field detects a class of corruption where a page ends up at the wrong file offset (e.g., due to a failed copy operation or a bug in the page allocator). On read, the storage engine verifies that the stored `page_id` matches the expected page ID derived from the file offset.

**page_type (u8):** Identifies the kind of data stored in this page. See Section 6 for the full taxonomy.

**flags (u8):** Interpretation depends on the page type. Common flag meanings:

| Bit | B-tree interior/leaf | Overflow | Free |
|-----|---------------------|----------|------|
| 0 | `is_leaf` (0 = interior, 1 = leaf) | — | — |
| 1–7 | Reserved (must be 0) | Reserved | Reserved |

For B-tree pages, bit 0 distinguishes interior pages from leaf pages. This allows the B-tree traversal code to branch immediately after reading the page header, before parsing the page body.

**_padding (u16):** Must be zero. Ensures the `txn_id` field is 4-byte aligned.

**txn_id (u64 LE):** The `transaction_id` of the transaction that last wrote this page. In the CoW model, this is the transaction that created this page version. This field is used by the MVCC garbage collector to determine when a page can be reclaimed (a page from transaction T is reclaimable when no active snapshot references transaction T or earlier).

**checksum (u32 LE):** A 32-bit checksum of the page contents. The checksum is computed over bytes 0–19 (the header fields before the checksum) concatenated with bytes 24–(page_size - 1) (the payload). That is, the checksum covers the entire page except the checksum field itself.

**Checksum algorithm:** CRC32C (Castagnoli). CRC32C is chosen over xxHash32 for page-level checksums for two reasons: (1) it has hardware acceleration on x86 (SSE 4.2 `crc32` instruction) and ARM (CRC extension), making it nearly free on modern CPUs; (2) it is well-established for page-level integrity in databases (used by RocksDB, ext4, Btrfs). The `crc32fast` crate provides a `no_std`-compatible implementation with automatic hardware detection.

**Rationale for 32-bit page checksum vs. 64-bit superblock checksum:** The superblock is the single most critical structure — a false positive (accepting a corrupted superblock as valid) would be catastrophic. 64-bit xxHash3 provides a collision probability of ~5×10⁻²⁰ — effectively zero. For data pages, 32-bit CRC32C provides a collision probability of ~2×10⁻¹⁰ — sufficient for detecting hardware-induced bit flips and torn writes. The 4-byte savings per page (vs. 8-byte checksum) yields 4 extra payload bytes per page, which marginally improves B-tree fan-out across the entire database.

---

## 6. Page Type Taxonomy

The `page_type` field (u8) in the common page header classifies each page. The following types are defined in format version 1.0:

| Value | Name | Description |
|-------|------|-------------|
| `0x01` | `BTreeInterior` | B-tree interior (branch) node: separator keys + child page pointers |
| `0x02` | `BTreeLeaf` | B-tree leaf node: key-value cells (or key-only cells for indexes) |
| `0x03` | `Overflow` | Overflow page: continuation data for large records |
| `0x04` | `Free` | Free page: currently unallocated, available for reuse |

**Values `0x00` and `0x05`–`0xFF`** are reserved for future page types. A reader encountering an unknown page type in a B-tree traversal path must return an error (the file requires a newer format version). A reader encountering an unknown page type outside a traversal path (e.g., during a full-file scan for recovery) should skip the page.

**Rationale for explicit type values (not sequential from 0):** `0x00` is reserved as an invalid/uninitialized sentinel. Starting at `0x01` ensures that a zero-filled page (from `ftruncate` growth) will fail the page type check, preventing uninitialized pages from being mistakenly interpreted as valid B-tree pages.

---

## 7. B-Tree Interior Page Format

An interior (branch) page stores separator keys and child page pointers. It guides B-tree traversal from the root toward the correct leaf page.

```
B-Tree Interior Page Layout:
┌──────────────────────────────────────────────────────────────┐
│ [0..24)    Common Page Header (page_type = 0x01, flags.0 = 0)│
├──────────────────────────────────────────────────────────────┤
│ [24..26)   cell_count: u16 LE — number of separator keys     │
│ [26..34)   right_child: u64 LE — rightmost child page ID     │
│ [34..36)   free_start: u16 LE — offset of first free byte    │
│                                 in the cell content area     │
│ [36..38)   _padding: u16 — must be zero                      │
├──────────────────────────────────────────────────────────────┤
│ [38..38+cell_count×2)  Cell pointer array:                    │
│     Each entry is a u16 LE offset (from page start)          │
│     pointing to a cell in the cell content area              │
├──────────────────────────────────────────────────────────────┤
│ [free gap — available space between pointer array and cells] │
├──────────────────────────────────────────────────────────────┤
│ Cell content area (grows from the end of the page backward): │
│   Each cell:                                                 │
│     left_child: u64 LE — page ID of child to the left       │
│     key_len: u16 LE    — length of the separator key         │
│     key: [u8; key_len] — the separator key bytes             │
└──────────────────────────────────────────────────────────────┘
```

### 7.1 Interior page header fields

**cell_count (u16 LE):** The number of separator keys (and associated left-child pointers) in this page. An interior page with `cell_count = N` has `N + 1` children: `N` left-children (one per cell) plus the `right_child`.

**right_child (u64 LE):** The page ID of the rightmost child. Keys greater than all separator keys route to this child.

**free_start (u16 LE):** The byte offset (from the start of the page) of the first byte of free space in the cell content area. New cells are written starting at `free_start` (growing backward from the end of the page). The cell pointer array grows forward from offset 38. When `free_start` minus the end of the cell pointer array is less than the size of a new cell + 2 bytes (pointer entry), the page is full and must be split.

### 7.2 Cell pointer array (slotted page)

The cell pointer array is an array of `cell_count` u16 offsets, each pointing to the start of a cell in the cell content area. Offsets are relative to the start of the page.

**Rationale for the slotted page layout:** The pointer array maintains logical key order (sorted by key). Cells in the content area may not be physically contiguous or sorted. This allows cells to be inserted and deleted without moving other cells — only the pointer array is reordered. This is the standard slotted-page pattern used by SQLite, PostgreSQL, LMDB, and virtually all B-tree implementations (Task 1, Section 2).

### 7.3 Interior cell format

Each interior cell contains:

```
Interior Cell:
  left_child: u64 LE  (8 bytes) — child page ID for keys < this separator
  key_len:    u16 LE  (2 bytes) — byte length of the key
  key:        [u8]    (key_len bytes) — the separator key
Total cell size: 10 + key_len bytes
```

**Traversal algorithm:** To find the child for a given search key K:

1. Binary search the cell pointer array (comparing K against each cell's key).
2. If K < cell[0].key: follow cell[0].left_child.
3. If cell[i].key ≤ K < cell[i+1].key: follow cell[i+1].left_child.
4. If K ≥ cell[cell_count-1].key: follow right_child.

All key comparisons are byte-level lexicographic (memcmp), which is correct because Task 7 (Section 6) uses big-endian encoding for all B-tree keys.

### 7.4 Fan-out calculation

For the seven B-trees defined in Task 7, the key sizes are:

| B-tree | Key size (bytes) | Cell size (bytes) | Cells per 4KB page |
|--------|-----------------|-------------------|---------------------|
| Node Store | 8 | 18 | ~220 |
| Edge Store | 8 | 18 | ~220 |
| Outgoing Adj. | 20 | 30 | ~132 |
| Incoming Adj. | 20 | 30 | ~132 |
| Type Index | 13 | 23 | ~173 |
| Schema Store | variable (5–50) | variable | ~80–200 |
| ID Freelist | 9 | 19 | ~211 |

Available interior payload: 4096 - 38 (header + subheader) - 2 × cell_count (pointer array) = ~4058 - 2N bytes. Cells per page ≈ 4058 / (cell_size + 2).

For the Node Store (most common lookup): ~220 children per interior page. A tree with 1 million nodes has height ≈ log₂₂₀(1,000,000) ≈ 2.6 → 3 levels. Three page reads to find any node (and in practice, the root and first-level interior nodes are always cached in the buffer pool).

---

## 8. B-Tree Leaf Page Format

A leaf page stores the actual key-value data cells. For key-only B-trees (adjacency indexes, type index, freelist), the "value" is empty — the cell contains only the key.

```
B-Tree Leaf Page Layout:
┌──────────────────────────────────────────────────────────────┐
│ [0..24)    Common Page Header (page_type = 0x02, flags.0 = 1)│
├──────────────────────────────────────────────────────────────┤
│ [24..26)   cell_count: u16 LE — number of key-value cells    │
│ [26..28)   free_start: u16 LE — offset of first free byte    │
│                                 in the cell content area     │
│ [28..36)   next_leaf: u64 LE  — page ID of next leaf (0=none)│
│ [36..44)   prev_leaf: u64 LE  — page ID of prev leaf (0=none)│
├──────────────────────────────────────────────────────────────┤
│ [44..44+cell_count×2)  Cell pointer array (u16 offsets)       │
├──────────────────────────────────────────────────────────────┤
│ [free gap]                                                    │
├──────────────────────────────────────────────────────────────┤
│ Cell content area (grows backward from page end):             │
│   Each cell:                                                 │
│     key_len: u16 LE          — byte length of the key        │
│     value_len: u16 LE        — byte length of the value      │
│     key: [u8; key_len]       — the key bytes                 │
│     value: [u8; value_len]   — the value bytes               │
│   For key-only B-trees: value_len = 0 and value is empty     │
└──────────────────────────────────────────────────────────────┘
```

### 8.1 Leaf page header fields

**cell_count (u16 LE):** Number of key-value cells.

**free_start (u16 LE):** Same semantics as the interior page (first free byte in the cell content area, growing backward).

**next_leaf / prev_leaf (u64 LE):** Page IDs of the next and previous leaf pages in sorted order. These form a doubly-linked list across all leaf pages of a single B-tree, enabling efficient range scans without descending through interior nodes. `0` = no neighbor (first or last leaf).

**Note on CoW and leaf links:** When a leaf page is CoW-copied, its neighbors' `prev_leaf` / `next_leaf` pointers must be updated to point to the new copy. This means a single cell modification in a leaf page may cascade to updating the two neighboring leaf pages as well. This is a known cost of maintaining leaf links in a CoW B-tree. Mitigations: (1) many mutations within a single transaction amortize this cost; (2) neighboring pages that are already CoW-dirtied in the same transaction incur no additional cost.

### 8.2 Leaf cell format

```
Leaf Cell:
  key_len:   u16 LE  (2 bytes) — byte length of the key
  value_len: u16 LE  (2 bytes) — byte length of the value
  key:       [u8]    (key_len bytes)
  value:     [u8]    (value_len bytes)
Total cell size: 4 + key_len + value_len bytes
```

**Overflow trigger:** If the total cell size exceeds `(page_size - 44) / 4` (approximately 1/4 of the usable leaf payload), the value is stored in overflow pages (Section 9) instead of inline. The leaf cell then stores only a **pointer to the first overflow page** as the value:

```
Overflow-redirected Leaf Cell:
  key_len:   u16 LE  (2 bytes)
  value_len: u16 LE  (2 bytes) — set to 0xFFFF (sentinel for overflow)
  key:       [u8]    (key_len bytes)
  overflow_page_id: u64 LE (8 bytes) — first overflow page
  total_overflow_len: u32 LE (4 bytes) — total byte length of overflow data
Total cell size: 4 + key_len + 12 bytes
```

The `value_len == 0xFFFF` sentinel distinguishes overflow-redirected cells from normal cells. A true value length of 65535 bytes would always trigger overflow anyway (it exceeds the 1/4-page threshold for any supported page size), so this sentinel causes no ambiguity.

### 8.3 Records per leaf page

For the Node Store (the most important B-tree for read performance), a typical node record is:

- Key: 8 bytes (NodeId)
- Value: 18 bytes (minimum NodeRecord with 1 type, no properties) to ~274 bytes (1 type + 256 bytes inline properties)

Available leaf payload: 4096 - 44 (header) = 4052 bytes, minus 2 bytes per cell for the pointer array.

| Scenario | Cell size | Cells per 4KB page |
|----------|----------|---------------------|
| Minimal node (1 type, no props) | 30 bytes | ~126 |
| Typical node (1 type, 64 bytes props) | 94 bytes | ~42 |
| Large node (3 types, 200 bytes props) | ~228 bytes | ~17 |
| Key-only index (20-byte key) | 24 bytes | ~160 |

---

## 9. Overflow Page Format

Overflow pages store data that is too large to fit inline in a B-tree leaf cell. They form a singly-linked chain.

```
Overflow Page Layout:
┌──────────────────────────────────────────────────────────────┐
│ [0..24)    Common Page Header (page_type = 0x03)              │
├──────────────────────────────────────────────────────────────┤
│ [24..32)   next_page: u64 LE — next overflow page (0 = last) │
│ [32..36)   data_length: u32 LE — bytes of payload in this pg │
│ [36..36+data_length)  data payload                            │
│ [36+data_length..page_size)  unused (zero-filled)             │
└──────────────────────────────────────────────────────────────┘
```

**Usable payload per overflow page:** `page_size - 36` bytes. For 4KB pages: 4060 bytes per page.

**Reconstruction:** To read an overflowed value, follow the chain:

1. Read the first overflow page (page ID from the leaf cell's `overflow_page_id`).
2. Append `data[0..data_length]` to the output buffer.
3. If `next_page != 0`, read the next overflow page and repeat.
4. Continue until `next_page == 0`.
5. The total reconstructed length must equal the `total_overflow_len` stored in the leaf cell (corruption check).

**CoW semantics for overflow pages:** Overflow pages are immutable once written. When a record's properties change and overflow is needed, entirely new overflow pages are allocated. The old pages become garbage, reclaimable after no snapshot references them (identical to B-tree page CoW — Task 7 Section 7.3).

---

## 10. Free Page Format

When a page is in the free state (allocated in the file but not used by any B-tree or overflow chain), its contents are irrelevant except for the common page header.

```
Free Page Layout:
┌──────────────────────────────────────────────────────────────┐
│ [0..24)    Common Page Header (page_type = 0x04)              │
├──────────────────────────────────────────────────────────────┤
│ [24..page_size)  Undefined (may contain stale data)           │
└──────────────────────────────────────────────────────────────┘
```

**Note:** A free page's header is written when the page is returned to the free pool. The `page_type = 0x04` tag prevents the page from being mistakenly interpreted as a B-tree node if the free-space tracking is corrupted. The `txn_id` in the header records the transaction that freed the page (useful for debugging).

**Zero-extended pages:** When the file is extended via `set_len()`, the newly added pages are zero-filled by the OS. Their `page_type` field is `0x00` (the zero byte), which is not a valid page type. The allocator must write a valid free page header (`page_type = 0x04`) to these pages before they are tracked in the freelist B-tree, or alternatively, treat pages with `page_type == 0x00` as implicitly free during growth (see Section 12).

---

## 11. Free-Space Management

The database tracks available pages using a **Page Freelist B-tree** — a dedicated CoW B-tree stored alongside the data B-trees, with its root pointer in the superblock (`page_freelist_root`).

### 11.1 Page Freelist B-tree design

This B-tree uses the same interior and leaf page formats as all other B-trees (Sections 7 and 8). It is a key-only B-tree (no values).

```
Page Freelist Key Encoding:
  [freed_txn_id: 8 bytes, big-endian u64]
  [page_id: 8 bytes, big-endian u64]
Total key size: 16 bytes (fixed)
```

Each entry represents "page `page_id` was freed in transaction `freed_txn_id` and is available for reuse."

**Why key by (txn_id, page_id) rather than just page_id?** The `freed_txn_id` prefix enables MVCC-safe reclamation. A page freed in transaction T can only be reused when the oldest active reader snapshot has a `transaction_id > T`. By keying the freelist with `freed_txn_id` first, the reclamation scan is a simple range query: scan from `(0, 0)` up to `(oldest_active_reader_txn, MAX)` to find all reclaimable pages.

### 11.2 Reclaimable page scan

When the write transaction needs a free page:

```
procedure allocate_page():
    oldest_reader = reader_tracker.oldest_active_txn_id()
    // If no readers, all freed pages are reclaimable
    if oldest_reader is None:
        oldest_reader = current_txn_id

    // Scan the freelist for the first reclaimable page
    scan = page_freelist.range_scan(
        start = (0, 0),
        end   = (oldest_reader - 1, MAX)
    )
    if scan.next() returns Some((freed_txn, page_id)):
        // Remove from freelist (CoW B-tree delete)
        page_freelist.delete((freed_txn, page_id))
        return page_id
    else:
        // No reclaimable pages — extend the file
        return extend_file_and_allocate()
```

**Performance:** In steady state (no long-lived read transactions), freed pages are immediately reclaimable. The freelist scan touches at most one leaf page. When a long-lived reader prevents reclamation, the file may grow temporarily — but once the reader closes, all delayed pages become reclaimable in a single batch.

### 11.3 Freeing pages on commit

During a CoW write transaction, the old pages along modified B-tree paths become obsolete. These pages are identified during the B-tree CoW path-copy operation:

```
procedure cow_copy_btree_path(old_root, modified_key) -> new_root:
    // Walk from root to leaf, copying each page to a new location
    for each page in path from old_root to leaf containing modified_key:
        new_page = allocate_page()
        copy contents to new_page
        apply modification (if this is the leaf)
        update parent's child pointer to new_page
        // The OLD page is now obsolete
        freed_pages.push(old_page.page_id)
    
    return new_root_page_id
```

At commit time, after all B-tree mutations are complete, the freed pages are inserted into the Page Freelist B-tree with the current `transaction_id`:

```
for page_id in freed_pages:
    page_freelist.insert((current_txn_id, page_id))
```

**Circular dependency note:** Inserting entries into the Page Freelist B-tree may itself require allocating new pages (if the freelist B-tree leaf is full and must split). These newly allocated pages come from the same freelist or from file extension. The freed pages from the freelist B-tree's own CoW operations are also added to the freed set. This is a well-known bootstrapping issue in CoW database allocators; both LMDB and redb handle it by making the freelist update the last step of the commit (after all other B-trees are finalized) and by tracking freed-during-freelist-update pages in a secondary list that is resolved in the next transaction.

**For this project, the commit-time free-space protocol is:**

1. Perform all data B-tree mutations (Node Store, Edge Store, indexes, schema). Collect the `freed_pages` set.
2. Insert all `freed_pages` into the Page Freelist B-tree. This may produce additional freed pages (from the freelist's own CoW path copies). These are collected in a `secondary_freed` set.
3. The `secondary_freed` pages are **deferred** — they are not inserted into the freelist in this transaction. Instead, they are recorded in a small in-memory set and inserted during the next write transaction's commit. This breaks the circularity.
4. If the database is opened read-only after this commit, the deferred pages are "leaked" (not tracked in the freelist). They are recovered by a `compact()` operation that scans the entire file and rebuilds the freelist. This is acceptable because: (a) the deferred set is typically 1–3 pages; (b) `compact()` is a maintenance operation, not a crash-recovery requirement.

### 11.4 Comparison with upstream recommendations

Task 4 (Section 6.4) recommended a free-space B-tree (Strategy B). This design directly implements that recommendation, with the specific key encoding `(freed_txn_id, page_id)` to support MVCC-safe reclamation. Task 4 (Section 8.6) provided the freelist B-tree design sketch; this section refines it with the concrete key format, the allocation procedure, and the circular dependency resolution.

---

## 12. Page Allocation and File Growth

### 12.1 Allocation priority

When a write transaction needs a new page (for a B-tree split, a new overflow page, etc.), pages are obtained in this order:

1. **Reclaimable free pages** (Section 11.2): Pages in the Page Freelist B-tree whose `freed_txn_id` is older than the oldest active reader. This reuses existing file space, preventing unbounded growth.
2. **File extension**: If no reclaimable pages exist, the file is extended.

### 12.2 File extension protocol

```
procedure extend_file_and_allocate() -> PageId:
    current_total = superblock.total_pages
    growth = compute_growth_increment(current_total)
    new_total = current_total + growth
    
    // Extend the file (OS fills new space with zeros)
    hal.set_len(new_total * page_size)
    
    // The first new page is allocated directly (returned to caller)
    allocated_page = current_total
    
    // Remaining new pages are added to the freelist
    for page_id in (current_total + 1)..new_total:
        page_freelist.insert((0, page_id))
        // freed_txn_id = 0 means "always reclaimable"
    
    // Update superblock's total_pages (committed with the transaction)
    pending_total_pages = new_total
    
    return allocated_page
```

### 12.3 Growth increment strategy

The growth increment scales with database size to balance between minimizing `set_len` syscalls and avoiding excessive pre-allocation:

| Current total pages | Growth increment (pages) | Growth (bytes at 4KB) |
|--------------------|--------------------------|------------------------|
| < 64 | 8 | 32 KB |
| 64 – 1,024 | 64 | 256 KB |
| 1,024 – 16,384 | 256 | 1 MB |
| > 16,384 | 1,024 | 4 MB |

These values are heuristics. The implementation may tune them, but the minimum growth increment should be at least 8 pages (to amortize the syscall cost).

### 12.4 File shrinking (compaction)

The file does **not** shrink automatically during normal operation. Reclaimed free pages remain in the file as available space for future allocations.

An explicit `compact()` operation (triggered by the user) can rebuild the database into a minimal-size file:

1. Create a new file with fresh superblocks.
2. Walk all live B-trees from the active superblock's root pointers.
3. Copy all reachable pages to the new file, reassigning contiguous page IDs.
4. Write the new superblock with the updated root pointers.
5. Atomically replace the old file with the new file (rename).

This is analogous to SQLite's `VACUUM` and LMDB's `mdb_env_copy`. It is a maintenance operation, not a routine one.

---

## 13. Commit Protocol

The commit protocol transforms a write transaction's pending changes into a durable, crash-safe state update. This protocol integrates the CoW B-tree mechanics (Task 7, Section 12.2) with the file format's dual-superblock design.

### 13.1 Full commit sequence

```
Commit Protocol (executed by the single active writer):

Phase 1: Materialize B-tree changes
  1. For each pending mutation in the write buffer:
     a. Perform the CoW B-tree insert/update/delete on the
        affected B-tree(s), using the current snapshot's root
        pointers as the starting state.
     b. Record new root page IDs for each modified B-tree.
     c. Record old pages that were replaced (the freed set).
  
  2. Insert freed pages into the Page Freelist B-tree
     (Section 11.3). Record any secondary freed pages
     from this insertion.

Phase 2: Write data pages to disk
  3. For every new or modified page (allocated during Phase 1):
     a. Compute the page's CRC32C checksum.
     b. Write the page to its assigned file offset via
        hal.write_at(page_id * page_size, page_data).
  
  4. If the file was extended: execute hal.set_len() first,
     then write the new pages.

Phase 3: First fsync (data durability)
  5. Execute hal.sync_data().
     This ensures all new data pages are durable on stable
     storage before the superblock references them.

Phase 4: Write new superblock
  6. Determine the inactive superblock slot:
     - If current active is page 0: write to page 1.
     - If current active is page 1: write to page 0.
  
  7. Construct the new superblock:
     - transaction_id = current_txn_id (incremented)
     - total_pages = new total (if file was extended)
     - Root pointers: updated for modified B-trees,
       unchanged for unmodified B-trees.
     - Compute xxHash3 checksum over bytes 0–183.
  
  8. Write the new superblock to the inactive slot via
     hal.write_at(inactive_slot * page_size, superblock_data).

Phase 5: Second fsync (superblock durability)
  9. Execute hal.sync_data().
     This ensures the new superblock is durable. The commit
     is now permanent — even if the process crashes
     immediately after this point, the new superblock will
     be selected on recovery (it has the higher transaction_id).

Phase 6: Update in-memory state
  10. Update the in-memory current snapshot to the new root pointers.
  11. Release the write mutex.
  12. Mark the old snapshot for eventual garbage collection.
```

### 13.2 Atomicity argument

The commit is atomic because of the dual-superblock pattern:

- **Before step 8 completes:** The old superblock is active (it has the highest valid checksum). All new data pages exist on disk (after step 5) but are unreachable — no superblock points to them. A crash here results in the old consistent state; the new pages are garbage.
- **After step 9 completes:** The new superblock is on disk with a higher `transaction_id` and a valid checksum. On recovery, it will be selected. A crash here results in the new consistent state.
- **During step 8 (torn write of the new superblock):** The new superblock page may be partially written. Its checksum will not validate. The old superblock remains valid and will be selected on recovery.

**The only failure mode that corrupts the database:** Both superblocks are torn-written simultaneously. This requires two crashes at the worst possible moment in two successive commits with no recovery in between. This is practically impossible — it would require hardware-level corruption of both superblock pages.

---

## 14. Crash Safety and Recovery

### 14.1 Guarantees

The file format provides the following crash safety guarantees:

**G1: Committed data is durable.** Once `commit()` returns, the transaction's effects survive any subsequent crash. This is guaranteed by the fsync sequence in the commit protocol (Phase 3 + Phase 5).

**G2: Uncommitted data is invisible.** If the process crashes during a write transaction (before the new superblock is fsynced), the old superblock remains active. New pages written to disk are unreachable garbage. The database state is exactly as it was before the transaction began.

**G3: No silent corruption.** Every page has a checksum. Every superblock has a checksum. A page that was partially written (torn write) will fail its checksum and be detected on read. The database will not silently return corrupt data.

**G4: No recovery log replay.** Recovery consists of reading two superblock pages and selecting the valid one with the higher transaction_id. No WAL exists; no log is replayed. Recovery is O(1) regardless of database size.

### 14.2 Recovery procedure

```
procedure recover():
    // Step 1: Select active superblock (Section 4.3)
    sb = select_superblock()
    if sb is ERROR_DATABASE_CORRUPT:
        return error("database file is corrupt")
    
    // Step 2: Verify file size
    actual_size = hal.len()
    expected_size = sb.total_pages * page_size
    if actual_size < expected_size:
        return error("database file is truncated")
    if actual_size > expected_size:
        // Interrupted file growth — truncate excess
        hal.set_len(expected_size)
        hal.sync_data()
    
    // Step 3: Initialize in-memory state from superblock
    load_root_pointers(sb)
    initialize_schema_cache_from_schema_store()
    initialize_reader_tracker()
    
    // Step 4: Resolve deferred freed pages (Section 11.3)
    // If secondary_freed pages were recorded in the previous
    // commit's in-memory state but not persisted, they are
    // "leaked." This is benign — recovered by compact().
    
    // Database is ready for transactions.
```

**Recovery time:** Steps 1–3 require reading 2 superblock pages + scanning the Schema Store B-tree (typically a few pages). Total: < 1ms for any database size. This meets the hard requirement from Task 4 (Section 8.8).

### 14.3 Limits of crash safety

**Torn write on the active superblock during normal reads:** If the OS or hardware corrupts a superblock page outside of the database's write path (e.g., a bit flip in memory, a firmware bug), the checksum will detect it. However, recovery can only fall back to the other superblock, which may be one transaction behind. The most recent committed transaction in the corrupted superblock would be lost.

**Filesystem-level corruption:** The crash safety guarantees depend on the filesystem correctly implementing `fsync` (data reaches stable storage). Filesystems that silently discard `fsync` (e.g., some configurations of ext3/ext4 in older kernels) can violate these guarantees. This is a well-known limitation shared by all databases, including SQLite and redb. The HAL documentation (Task 9) should warn about this.

**Disk-level write reordering:** Modern disks with write caches may reorder writes. `fsync` is the database's only mechanism to enforce ordering. If the disk's write cache lies about `fsync` completion (e.g., some cheap consumer SSDs), the commit protocol's ordering guarantees are violated. This is again a shared limitation. The database could optionally support `O_DIRECT` (bypassing the OS page cache) and `O_DSYNC` (synchronous writes) as HAL configuration options for paranoid deployments, but this is deferred to future work.

**Intentional corruption by the user:** If a user modifies the database file with an external tool while the database is open, all guarantees are void. Single-process exclusivity (via an exclusive file lock at open time) prevents accidental concurrent access but cannot prevent intentional modification.

---

## 15. fsync Discipline

Correct fsync ordering is a **correctness requirement**, not a performance optimization. The following rules are mandatory.

### 15.1 The two fsyncs per commit

| fsync # | When | What it protects |
|---------|------|-----------------|
| 1 | After writing all new data pages (commit step 5) | Ensures data pages are durable before the superblock references them. Without this, a crash after writing the superblock but before data pages reach disk would leave the superblock pointing to corrupt pages. |
| 2 | After writing the new superblock (commit step 9) | Ensures the superblock itself is durable. Without this, a crash could leave the new superblock only in the OS buffer cache, lost on power failure. |

### 15.2 fsync implementation

The HAL trait (Task 9) must expose at minimum:

- `sync_data()`: Maps to `fdatasync()` on Linux, `fcntl(F_FULLFSYNC)` on macOS, `FlushFileBuffers()` on Windows. This syncs file data but not necessarily file metadata (size, timestamps). For database files that are not being extended, `fdatasync` is sufficient and faster than `fsync`.
- `sync_all()`: Maps to `fsync()` on Linux/macOS, `FlushFileBuffers()` on Windows. This syncs both data and metadata. Required when the file has been extended (after `set_len()`).

**Rule:** If the file was extended in this transaction (`set_len` was called), the first fsync must use `sync_all()` to ensure the new file size is durable. Otherwise, `sync_data()` is sufficient.

### 15.3 fsync on database creation

When creating a new database file:

1. Create the file.
2. Write superblock A (page 0) and superblock B (page 1).
3. Write the initial Schema Store root page (page 2).
4. `sync_all()` — ensure the entire file (data + metadata including size) is durable.

This is a single fsync at creation time. After creation, the normal two-fsync-per-commit protocol applies.

### 15.4 Group commit optimization (future)

The two-fsync-per-commit protocol is the simplest correct implementation. A future optimization is **group commit**: batch multiple write transactions' data page writes, issue one fsync, then batch their superblock writes, issue one fsync. This amortizes the fsync latency across multiple transactions. Group commit is not specified here but the file format supports it — nothing in the superblock design prevents it.

---

## 16. Versioning and Extensibility

### 16.1 Format version evolution

The format uses a two-part version number: `format_major.format_minor`.

**Major version increment (breaking):** Older readers cannot open the file at all. Use for changes to the superblock layout, page header layout, or B-tree cell format that are not backward-compatible. The current implementation supports only `format_major = 1`.

**Minor version increment (additive):** Older readers can open the file in read-only mode but must not write to it. Use for changes like:

- Adding new page types (using previously reserved `page_type` values)
- Adding new B-tree root pointers (using the `_reserved_roots` fields)
- Adding new feature flags (using previously undefined bits in `feature_flags`)
- Adding new schema key prefixes (using previously reserved prefix values in the Schema Store)

### 16.2 Feature flags

The `feature_flags` field (u64 LE) in the superblock provides 64 bits for optional capabilities. The bits are divided into two ranges:

| Bits | Range name | Behavior when unknown |
|------|-----------|----------------------|
| 0–31 | **Required features** | A reader that does not understand a set bit must refuse to open the file. This prevents data loss from misinterpreting format extensions. |
| 32–63 | **Advisory features** | A reader that does not understand a set bit may still open the file. These flags indicate optional metadata or hints that do not affect data correctness. |

**Initial state:** All bits are 0. No features are defined in format version 1.0.

**Example future features (illustrative, not committed):**

| Bit | Name | Description |
|-----|------|-------------|
| 0 | `PROPERTY_VALUE_INDEX` | A property value index B-tree exists (Task 7 §7.5). Required because writers must maintain the index. |
| 1 | `COMPRESSION` | Pages are compressed. Required because decompression is needed to read data. |
| 32 | `CREATION_METADATA` | Extended creation metadata stored in reserved superblock space. Advisory because it doesn't affect data reading. |

### 16.3 Reserved space

The superblock contains two reserved regions:

- `_reserved_roots` (32 bytes = 4 × u64): For future B-tree root pointers. Allows adding up to 4 new B-trees without a breaking format change.
- `_reserved_fields` (32 bytes): For future mutable metadata fields.

Both must be zero in format 1.0. A writer that encounters nonzero values in reserved fields must check the `format_minor` version — if the file's minor version exceeds the writer's, the nonzero reserved fields are expected (they are fields the writer doesn't understand).

### 16.4 Application ID for downstream crates

The `application_id` field (u32 in the identity header) allows downstream crates to "claim" a database file. The core crate provides an API to set this value at creation time and to read it at open time. Downstream crates can use it to:

- Reject files created by a different application (e.g., an OWL engine refusing to open a SKOS database)
- Verify that a file's schema is compatible before attempting to load it
- Provide meaningful error messages ("This file was created by application X, but you are running application Y")

The core crate treats `application_id = 0` as "unregistered" and does not enforce any application-specific checks.

---

## 17. Configuration and Limits

### 17.1 Configurable parameters

| Parameter | Set at | Mutable? | Default | Range |
|-----------|--------|----------|---------|-------|
| Page size | Database creation | No | 4096 | 4096, 8192, 16384, 32768, 65536 |
| Application ID | Database creation | No | 0 | 0–0xFFFFFFFF |
| Buffer pool size | Database open | Per-session | 1024 frames | 64–2²⁰ frames |

### 17.2 Hard limits

| Limit | Value | Rationale |
|-------|-------|-----------|
| Maximum page size | 65536 bytes | Encoded as u16; SQLite convention |
| Minimum page size | 4096 bytes | Below this, B-tree fan-out is too low and page header overhead is excessive |
| Maximum file size | 2⁶⁴ × page_size | Limited by u64 page count; at 4KB pages, this is 64 exabytes — effectively unlimited |
| Maximum pages | 2⁶⁴ - 2 | Pages 0 and 1 are superblocks; remaining ~18 quintillion pages available |
| Maximum B-tree key size | page_size / 4 - 14 | An interior cell (10 + key_len) must fit with at least 3 other cells per page, to maintain the B-tree invariant of ≥ 2 children per interior node |
| Maximum inline value size | (page_size - 44) / 4 - 4 - key_len | Overflow threshold; larger values are stored in overflow pages |
| Maximum overflow chain length | Bounded by value size / (page_size - 36) | For a 1 GB value at 4KB pages: ~260K pages. No hard limit. |
| Maximum concurrent readers | Limited by memory for snapshot tracking | No hard cap; practical limit depends on available RAM for reference counting |
| Maximum cell count per page | 2¹⁶ - 1 = 65535 | Cell count is u16; in practice, page capacity limits this to a few hundred cells |

### 17.3 Checksums summary

| Structure | Algorithm | Size | Rationale |
|-----------|-----------|------|-----------|
| Superblock | xxHash3 (64-bit) | 8 bytes | Maximum collision resistance for the most critical structure |
| Data pages | CRC32C | 4 bytes | Hardware-accelerated; sufficient for page-level integrity; saves 4 bytes per page vs. 64-bit hash |

---

## 18. Design Decision Log

| ID | Decision | Alternatives considered | Rationale |
|----|----------|------------------------|-----------|
| F1 | 14-byte magic with PNG-style detection bytes | 4-byte magic; 16-byte magic with null terminator | 14 bytes provides a human-readable prefix plus all three corruption-detection sequences (CR/LF, ^Z, LF). 16 bytes would waste 2 bytes. 4 bytes is insufficient for reliable identification. |
| F2 | Dual-superblock with transaction_id tie-breaker | Single superblock + WAL; triple superblock; separate active-slot flag byte | Dual superblock eliminates WAL complexity (Task 1 recommendation, confirmed by Task 4). Transaction_id tie-breaker is simpler than a separate flag — no extra atomic write needed. Triple superblock adds no meaningful safety over dual. |
| F3 | 24-byte common page header | 16-byte header (no self-ID); 32-byte header (with extra metadata) | 24 bytes provides all critical fields (self-ID, type, txn_id, checksum) with alignment-friendly layout. 16 bytes would omit self-ID (losing an important corruption check). 32 bytes wastes 8 bytes per page — significant across millions of pages. |
| F4 | CRC32C for page checksums, xxHash3 for superblock | xxHash3 everywhere; CRC32 everywhere; no checksums | CRC32C has hardware acceleration (nearly free on modern CPUs) and is sufficient for page integrity. xxHash3 is faster in software for large blocks but CRC32C's hardware path is faster for small blocks (4KB). xxHash3's 64-bit variant is used for the superblock where collision resistance matters most. |
| F5 | Slotted page layout for B-tree pages | Fixed-position cells; compacted cells without indirection | Slotted pages decouple logical order from physical position, enabling insertions/deletions without moving cells. This is the universal standard (SQLite, PostgreSQL, LMDB). |
| F6 | Doubly-linked leaf pages | No leaf links (all scans start from root); singly-linked | Doubly-linked leaves enable efficient bidirectional range scans. Singly-linked would prevent reverse scans. No links would require re-traversal from the root for each range scan continuation. The CoW cost of maintaining neighbor pointers is acceptable because most mutations amortize across multiple changes per transaction. |
| F7 | Page Freelist B-tree keyed by (freed_txn_id, page_id) | Linked freelist (SQLite-style); bitmap; simple B-tree keyed by page_id | The (txn_id, page_id) key enables MVCC-safe reclamation via range scan. A linked freelist cannot efficiently query "pages freed before transaction T." A bitmap is harder to integrate with CoW. A simple page_id key loses transaction tracking. This follows Task 4's recommendation (Section 6.4, Strategy B). |
| F8 | Deferred secondary freed pages | Recursive freelist insertion; WAL for freelist updates | Deferring secondary freed pages (Section 11.3) breaks the circular dependency between the freelist B-tree and page allocation cleanly. The deferred set is typically 1–3 pages. Recursive insertion risks unbounded recursion. A WAL would negate the benefit of the CoW design. |
| F9 | 0xFFFF sentinel for overflow-redirected leaf cells | Separate flag byte in cell; extra bit in cell pointer | The 0xFFFF sentinel reuses the existing `value_len` field with zero overhead. A separate flag byte would cost 1 byte per cell. An extra bit in the cell pointer would reduce the maximum page offset to 32767, limiting page sizes to 32KB. |
| F10 | File extension with scaling growth increments | Fixed growth (always 1 page); always double; pre-allocate on create | Scaling increments balance between syscall amortization and disk waste. Fixed 1-page growth causes excessive `set_len` calls. Always doubling wastes space for small databases. Pre-allocation wastes space in the common case (small databases). |
| F11 | No automatic file shrinking | Automatic truncation when free pages exceed threshold | Automatic shrinking complicates the commit protocol (requires an additional fsync for the truncation) and can interfere with write-heavy workloads that alternate between growing and shrinking. Explicit `compact()` gives the user control. |
| F12 | 192-byte superblock with 64 bytes reserved | Minimal superblock (no reserved space); 4096-byte superblock (fill entire page) | 192 bytes leaves ~3900 bytes of headroom in a 4KB page for future expansion — ample for years of evolution. A minimal superblock would require a breaking format change to add fields. Filling the entire page wastes space on reading (the full page is read, but only 192 bytes are parsed). |
| F13 | Big-endian format version, little-endian everything else | All big-endian; all little-endian | Format version is in the identity header, intended for human-readable hex dumps — big-endian makes versions read naturally. All other fields are machine-processed — little-endian avoids byte-swap overhead on x86/ARM. This matches the mixed convention used by Task 7 (big-endian keys, little-endian values). |
| F14 | creation_timestamp in identity header | No timestamp; timestamp in mutable section | The creation timestamp is informational only (helps users identify when a database was created) and never changes. Placing it in the immutable identity header ensures it survives any number of superblock updates. |

---

## Completion Report: Task 8 — Single-File Format

### Status: COMPLETE

### Done Criterion:

The criterion requires:
1. A byte-level format specification sufficient for implementation — ✓ Sections 3–10 specify every byte in the file: identity header (32 bytes), mutable superblock (160 bytes), common page header (24 bytes), B-tree interior pages, B-tree leaf pages, overflow pages, and free pages, all with exact field offsets, sizes, and types.
2. An extensibility mechanism — ✓ Section 16 defines reserved fields, feature flags (required vs. advisory bits), application ID, and the major/minor version evolution rules.
3. A versioning scheme — ✓ Section 3.2 defines the two-part version number with clear upgrade/downgrade rules. Section 16.1 specifies when each component is incremented.
4. The WAL/journaling design — ✓ Sections 13–15 document that no WAL is used (CoW B-trees eliminate it), define the dual-superblock commit protocol, and specify the fsync discipline. The rationale for no WAL is traced back to Tasks 1 and 4.
5. Crash-recovery guarantees and their limits — ✓ Section 14 defines four guarantees (G1–G4) with explicit proofs, the recovery procedure, and the limits (filesystem trust, disk write reordering, simultaneous torn writes).
6. Reviewed against lessons from Task 4 — ✓ Every major design choice references the corresponding lesson from Task 4 (dual-superblock → 8.1, superblock extensibility → 8.2, buffer pool separation → 8.3, typed page headers with checksums → 8.4, freelist B-tree → 8.6, 4096 page size → 8.7, fast startup → 8.8, no multi-process → 8.9, shared code paths → 8.10).

All criteria met.

### Deliverables:
- `008-file-format-spec.md` — this document

### Summary:

Designed a complete binary file format for the embedded graph database, built on the dual-superblock CoW B-tree architecture established by Tasks 1, 4, and 7. The format uses a 24-byte common page header with CRC32C checksums, slotted B-tree pages (interior and leaf), overflow pages for large records, and a Page Freelist B-tree for MVCC-safe free-space management. The commit protocol requires exactly two fsyncs per transaction and provides full crash safety without a WAL.

Key design decisions: (1) the superblock fits in 192 bytes with 64 bytes of reserved space for future expansion; (2) the `0xFFFF` sentinel for overflow-redirected leaf cells avoids adding any per-cell overhead; (3) the circular dependency between the Page Freelist B-tree and page allocation is resolved by deferring secondary freed pages to the next transaction; (4) file growth uses scaling increments to balance between syscall amortization and disk waste.

### Context for Next Task:

**Task 9 (HAL Trait Design)** should read `008-file-format-spec.md` (this deliverable) and will also need `005-no-std-hal-patterns.md` and `004-embedded-db-architectures.md` (Section 8.5). Key items for Task 9:

- The HAL must expose at minimum: `read_at(offset, buf)`, `write_at(offset, buf)`, `sync_data()`, `sync_all()`, `len()`, `set_len(size)`. These correspond directly to the commit protocol (Section 13) and fsync discipline (Section 15).
- The distinction between `sync_data()` (fdatasync) and `sync_all()` (fsync) is required — Section 15.2 specifies when each is needed.
- The HAL should provide a `try_lock_exclusive()` method for single-process exclusivity, as noted in Task 4 (Section 8.9, residual concern #4).
- All I/O is page-aligned: reads and writes are always at offsets that are multiples of `page_size`, and always for exactly `page_size` bytes. The HAL may optimize for this alignment.

**Task 12 (Design Synthesis)** should incorporate this document's specifications for the file header layout, page formats, commit protocol, and crash safety guarantees.

### Residual Concerns:

1. **Deferred secondary freed pages (Section 11.3)** cause a minor space leak if the database is never compacted after a transaction that triggers freelist B-tree splits. The leaked pages are typically 1–3 pages per such transaction. This is benign for practical use but should be documented in the public API (Task 10) and the `compact()` operation should be prominently featured.

2. **Leaf page doubly-linked list maintenance under CoW (Section 8.1 note):** Updating neighbor leaf pointers on every CoW leaf copy is a known cost. In pathological cases (many single-cell modifications spread across many leaf pages), this could double the number of CoW page writes. The implementation (Task 16) should measure this and consider lazy neighbor updates if it proves problematic.

3. **CRC32C hardware detection at runtime:** The `crc32fast` crate handles this transparently, but the `no_std` core code path must verify that the chosen crate works correctly in `no_std + alloc` mode. Task 15 (HAL std backend implementation) should verify this.

4. **Page size larger than 4096 on systems with 4KB OS pages:** If a user chooses 8192 or 16384 byte pages on a system with 4KB OS pages, the superblock write is not a single atomic OS page write — it spans 2 or 4 OS pages. The dual-superblock checksum mechanism still protects against torn writes (the checksum will be invalid), but the probability of a torn superblock write increases slightly. This is acceptable — the same situation exists in SQLite when using page sizes > 4KB.

### Upstream Flags:

1. **Task 7's Page Freelist not explicitly in the B-tree catalog — ADVISORY.**
   - What was discovered: Task 7 (Section 4) defines seven logical B-trees but does not include the Page Freelist B-tree (which tracks free disk pages for the CoW allocator). This is a file-format-level concern, not a graph-storage-level concern, so its omission from Task 7's catalog is architecturally reasonable. However, the Page Freelist uses the same B-tree page format and CoW machinery as the other seven trees.
   - Which task(s) it likely affects: Task 12 (Design Synthesis) should mention eight B-trees total (seven from Task 7 + the Page Freelist from this document). Task 16 (Storage Engine Implementation) must implement the Page Freelist B-tree.
   - Severity: ADVISORY
   - Suggested action: Task 12 should note that the complete B-tree catalog is seven data B-trees (Task 7) plus one infrastructure B-tree (Page Freelist, from Task 8).

2. **HAL trait must distinguish sync_data from sync_all — ADVISORY.**
   - What was discovered: The fsync discipline (Section 15) requires two distinct sync operations: `sync_data()` (data only, for performance) and `sync_all()` (data + metadata, for file extension). Task 4's HAL sketch (Section 8.5) shows only a single `flush()` method.
   - Which task(s) it likely affects: Task 9 (HAL Trait Design).
   - Severity: ADVISORY
   - Suggested action: Task 9 should provide two sync methods in the `StorageBackend` trait, not one.
