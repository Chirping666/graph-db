# CLAUDE.md — Task 24: Implement Persistent Storage Engine & File Format

**Project:** Embedded Graph Database with Extensible Schema & Pluggable Inference  
**Implementation Task:** 24 (preparation task: 16)  
**Module:** `src/storage/`  
**Status:** Pending  
**Depends on:** Task 22 (core types), Task 23 (HAL traits + std backend)  
**Preparation depends on:** Task 12 (`012-design-document.md`), Task 15 (`tasks/23-hal-std-backend/`)

---

## Orientation

This is Task 24, the implementation of the persistent storage engine. It is the most complex and critical implementation task in the project — the core of the database. The storage engine sits between the HAL layer (which provides raw byte I/O) and the database engine layer (which provides transactions and the public API).

The storage engine implements:
- **Page management:** common page headers, page types (interior, leaf, overflow, free), slotted page layout, checksums
- **Buffer pool:** page frame caching, clock eviction, pin/unpin, dirty page tracking
- **Page allocator:** free-space management via the Page Freelist B-tree, MVCC-safe page reclamation, file growth
- **CoW B+ tree operations:** search, insert, delete, range scan, split, merge, CoW path copying
- **Record serialization:** NodeRecord, EdgeRecord, property serialization, key encoding (big-endian keys, little-endian values)
- **File identity header and superblock:** magic bytes, format versioning, dual-superblock design
- **Commit protocol:** dual-superblock atomic commit with 2-fsync ordering
- **Crash recovery:** superblock validation, active superblock selection (identical to normal startup)

Within the project's hierarchy, this is one task in a 4-phase, 29-task project. Sibling implementation preparation tasks are 14 (core types), 15 (HAL + std backend), 17 (query engine), 18 (inference hooks), 19 (in-memory backend), 20 (integration testing), 21 (docs/publish). Task 25 (query engine) depends on this task's output.

---

## Required Reading

Before writing any code, read these documents in order:

1. **`012-design-document.md`** — The single source of truth. Key sections for this task:
   - §6 — Graph Storage Model (B-tree catalog, record formats, key encoding, property storage, schema mapping, ID allocation)
   - §7 — Single-File Format (file structure, dual superblock, page header, page types, free-space management, versioning)
   - §9 — Buffer Pool (structure, operations, clock eviction, configuration, CoW interaction)
   - §10 — Concurrency Model (single-writer MVCC, snapshot lifecycle, write locking)
   - §11 — Transaction Lifecycle (read/write transaction sequences, commit protocol, read-your-own-writes)
   - §12 — Crash Safety and Recovery (guarantees, recovery procedure, fsync discipline)
   - §16 — Cross-Cutting Concerns (serialization strategy, concurrency guarantees, error handling)
   - §17 — Design Decision Log (especially G1–G14, F1–F14)
   - §19 — Consolidated B-Tree Catalog and Schema Store Key Map

2. **`008-file-format-spec.md`** — Authoritative byte-level reference for:
   - File identity header (§3)
   - Dual-superblock layout (§4)
   - Common page header (§5)
   - B-tree interior page format (§7)
   - B-tree leaf page format (§8)
   - Overflow page format (§9)
   - Free page format (§10)
   - Free-space management (§11)
   - Page allocation and file growth (§12)
   - Full commit protocol sequence (§13)
   - fsync discipline (§15)

3. **`007-graph-storage-model.md`** — Authoritative reference for:
   - B-tree catalog and key formats (§4, §6)
   - Record formats: NodeRecord, EdgeRecord (§5)
   - Property storage: inline vs. overflow (§7)
   - Value serialization format (§7.4)
   - Schema-to-storage mapping (§9)
   - Buffer pool design (§10)
   - CoW B-tree interaction (§10.6)
   - Commit protocol integration (§12)
   - ID allocation and recycling (§14)

4. **`CLAUDE.md` (project root)** — Project-wide rules, especially:
   - Rule 1: No external database crate dependencies
   - Rule 2: `storage/` requires `std` feature
   - Rule 4: Documentation on every public item
   - Rule 5: Test coverage expectations
   - Rule 7: Code style and conventions

5. **Existing code from Tasks 22 and 23:**
   - `src/types/` — NodeId, EdgeId, TypeId, PropertyKeyId, Value, Node, Edge, PropertyMap, etc.
   - `src/hal/` — ReadAt, WriteAt, hal::Sync, StorageBackend traits
   - `src/hal_std/` — FileBackend implementation
   - `src/error/` — Error types, StorageError

---

## Objective

Implement the complete persistent storage engine in `src/storage/`, providing all the infrastructure that the database engine layer (`src/db/`, Task 25+) will build upon.

After this task, the following must be true:
- A caller can create a new database file with the correct identity header and initial superblocks
- A caller can open an existing database file, validate its superblocks, and select the active one
- The buffer pool caches pages and evicts them using the clock algorithm
- CoW B+ tree operations (search, insert, delete, range scan) work correctly on any of the 8 logical B-trees
- Records (NodeRecord, EdgeRecord, schema entries) can be serialized and deserialized
- The commit protocol writes new pages, fsyncs data, writes the new superblock, fsyncs the superblock
- Crash recovery works: after simulating a crash mid-commit, reopening the database sees only committed data
- The Page Freelist manages free pages with MVCC-safe reclamation
- Page allocation uses reclaimable free pages first, then file extension with scaling growth increments

---

## Module Layout

```
src/storage/
├── mod.rs              // Module-level re-exports, StorageEngine struct
├── page/
│   ├── mod.rs          // PageId, PageType, page constants
│   ├── header.rs       // CommonPageHeader: serialize/deserialize, checksum
│   ├── interior.rs     // InteriorPage: parse, build, search, split
│   ├── leaf.rs         // LeafPage: parse, build, insert cell, split, linked list
│   ├── overflow.rs     // OverflowPage: read chain, write chain
│   └── free.rs         // FreePage: header-only page
├── btree/
│   ├── mod.rs          // BTree struct, BTreeConfig
│   ├── search.rs       // Point lookup, range scan
│   ├── insert.rs       // CoW insert with split
│   ├── delete.rs       // CoW delete with merge/rebalance
│   ├── cursor.rs       // Leaf-level cursor for range iteration
│   └── cow.rs          // CoW path copy logic, freed page tracking
├── buffer_pool.rs      // BufferPool, PageFrame, clock eviction
├── allocator.rs        // PageAllocator, file growth, freelist integration
├── format.rs           // FileIdentityHeader, Superblock, database creation/opening
├── serialization.rs    // NodeRecord, EdgeRecord, property serialization, key encoding
└── snapshot.rs         // Snapshot struct (set of root page IDs), reference counting
```

All modules in `src/storage/` are gated behind `#[cfg(feature = "std")]`.

---

## Key Design Decisions to Follow

These decisions are settled in the design documents. Do not re-open them during implementation.

| Decision | Choice | Reference |
|----------|--------|-----------|
| Storage primitive | Unified CoW B+ trees (no slot stores, no WAL) | 012 §6.1, G1 |
| Number of B-trees | 8 (7 data + 1 page freelist) | 012 §6.2, §19.1 |
| Key encoding | Big-endian for all B-tree keys | 012 §6.2, G4 |
| Value encoding | Little-endian for all record values | 012 §6.2, G5 |
| Inline property threshold | ≤ 256 bytes inline, overflow beyond | 012 §6.4, G6 |
| Page size | 4096 bytes (configurable, power of two) | 008 §17 |
| Page header | 24 bytes with CRC32C | 012 §7.3, F2 |
| Superblock | 192 bytes, dual-slot design | 012 §7.2, F12 |
| Buffer pool eviction | Clock algorithm | 012 §9.3, G9 |
| Concurrency model | Single-writer MVCC via CoW snapshots | 012 §10.1, G7–G8 |
| Commit protocol | 2-fsync: data pages, then superblock | 012 §12.3, F1 |
| Free page tracking | Page Freelist B-tree with `(freed_txn_id, page_id)` keys | 012 §7.5, F8 |
| File growth | Scaling increments (8 → 64 → 256 → 1024 pages) | 008 §12.3, F10 |
| Property serialization | Custom binary, no serde | 012 §16.5, G13 |
| Checksum algorithm | CRC32C via `crc32fast` | 012 §7.3, F2 |

---

## Error Handling

Storage engine errors should use the project's `error::StorageError` type. Within the storage module, you may define internal error types that are converted to `StorageError` at module boundaries.

All page reads must validate checksums. Checksum mismatches produce `StorageErrorKind::MediaCorruption`. Buffer pool exhaustion produces `StorageErrorKind::StorageFull` (or a descriptive error). Out-of-bounds reads/writes produce `StorageErrorKind::OutOfBounds`.

---

## Testing Strategy

This task requires extensive testing because it is the correctness foundation for the entire database.

**Unit tests (within each module):**
- Page header serialization round-trip (every page type)
- Interior page: build, search, split, merge
- Leaf page: build, insert, search, split, merge, linked list maintenance
- Overflow page: write chain, read chain, single page, multi-page
- Buffer pool: fetch, unpin, eviction order, dirty page flush, pin count enforcement, pool exhaustion
- Page allocator: allocate from freelist, allocate from file extension, growth increment calculation
- B-tree: insert/search/delete single key, range scan, split propagation, multi-level tree
- Superblock: serialize/deserialize, checksum validation, active superblock selection
- Record serialization: NodeRecord round-trip, EdgeRecord round-trip, property serialization, key encoding
- Key encoding: verify that big-endian encoded keys sort correctly via byte comparison

**Integration tests:**
- Create a new database file, write superblocks, verify file structure
- Insert records into a B-tree via the buffer pool, commit, reopen, verify records persist
- Crash recovery: write data pages but skip the superblock fsync, reopen, verify old state is intact
- MVCC page reclamation: commit multiple transactions, verify old pages are freed after snapshots are released
- Large record overflow: insert a record exceeding 256 bytes of properties, verify overflow chain works
- File growth: exhaust the initial pages, verify file extension occurs with correct growth increments
- Concurrent buffer pool access: multiple readers can fetch the same page simultaneously (via `ReadAt`'s `&self`)

---

## Performance Notes

- The buffer pool is the hot path. Minimize allocations and locks in `fetch_page`. The page table lookup (`HashMap<PageId, usize>`) should be fast; consider using a simpler hash for `PageId` (it's just a `u64`).
- CoW path copies allocate new pages for every modified interior node from leaf to root. A typical B-tree of height 3–4 means 3–4 new pages per single-key insert. Batch mutations within a transaction amortize this because modified interior nodes are shared.
- The clock eviction sweep is O(1) amortized but worst-case O(n) on a single call (full sweep with all frames referenced). This is acceptable.
- CRC32C computation should use `crc32fast` which auto-detects hardware acceleration (SSE 4.2 on x86).

---

## Residual Concerns from Design Phase

These are known issues documented in the design. Be aware of them but do not attempt to solve them beyond what is specified:

1. **Leaf page doubly-linked list maintenance under CoW** (008 residual #2): Updating neighbor leaf pointers on every CoW leaf copy is a known cost. Measure it; if it proves problematic, consider lazy neighbor updates, but the default implementation should maintain the linked list correctly.

2. **Deferred secondary freed pages** (012 §18.1 item 7, 008 §11.3): Pages freed during the Page Freelist B-tree's own CoW operations are deferred to the next transaction. This means 1–3 pages may be temporarily leaked. Implement this deferral correctly; do not attempt to resolve it in a single transaction.

3. **`crc32fast` in `no_std + alloc`** (012 residual #5): Verify that `crc32fast` works under `std` (which is what `storage/` requires). The `no_std` concern is for other modules.

---

## Definition of Done

All of the following must be true before this task is complete:

- [ ] `src/storage/mod.rs` exists with module structure and public re-exports
- [ ] `src/storage/page/` implements all 4 page types with correct byte layouts per `008-file-format-spec.md` §§5–10
- [ ] `src/storage/buffer_pool.rs` implements the buffer pool with clock eviction per `012-design-document.md` §9
- [ ] `src/storage/allocator.rs` implements page allocation with freelist integration and file growth per `008-file-format-spec.md` §§11–12
- [ ] `src/storage/btree/` implements CoW B+ tree search, insert, delete, and range scan
- [ ] `src/storage/format.rs` implements file identity header and dual-superblock per `008-file-format-spec.md` §§3–4
- [ ] `src/storage/serialization.rs` implements NodeRecord, EdgeRecord, property serialization, and key encoding per `007-graph-storage-model.md` §§5–7
- [ ] `src/storage/snapshot.rs` implements snapshot struct with root page IDs and reference counting
- [ ] The commit protocol correctly implements the 2-fsync sequence per `008-file-format-spec.md` §13
- [ ] Crash recovery works: simulated crash mid-commit produces no data corruption on reopen
- [ ] `cargo check` succeeds (full build with std)
- [ ] `cargo check --no-default-features --features alloc` succeeds (storage module is not compiled, but nothing breaks)
- [ ] `cargo test` passes — all storage engine tests green
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` — zero warnings
- [ ] `cargo doc --no-deps` — zero warnings; every `pub` item in `src/storage/` has a doc comment
- [ ] Tests cover: page header round-trips, B-tree insert/search/delete, range scan, split/merge, buffer pool eviction, crash recovery, overflow pages, file creation/opening, superblock validation, record serialization round-trips, key encoding sort order, MVCC page reclamation, file growth
- [ ] `ConstraintValidator` and `InferenceRule` Send+Sync assertions still pass (no regressions)

---

## Out of Scope

- `db/` module (Database struct, transactions, WriteBuffer, schema cache) — Task 17/25
- `hal_mem/` (MemoryBackend) — Task 19/27
- Query and traversal engine — Task 17/25
- Inference engine — Task 18/26
- Integration testing across subsystems — Task 20/28
- Documentation and publish preparation — Task 21/29
