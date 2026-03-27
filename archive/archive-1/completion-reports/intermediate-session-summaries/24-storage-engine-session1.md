# Task 24 Session 1 — Storage Engine Phases 0–6

**Date:** 2026-03-23
**Scope:** Phases 0–6 (scaffolding, pages, file format, snapshot, buffer pool, serialization, allocator)
**Status:** COMPLETE for session scope. Phases 7–11 remain for next session.

---

## Verification

| Check | Result |
|-------|--------|
| `cargo test` | 204 passed, 0 failed, 0 ignored |
| `cargo check` | OK |
| `cargo check --no-default-features --features alloc` | OK (storage not compiled, no regressions) |
| `cargo clippy --all-targets --all-features -- -D warnings` | 0 warnings |
| `cargo doc --no-deps` | 0 warnings |
| Existing Send+Sync assertions | Pass (no regressions) |

---

## Phases Completed

### Phase 0: Module Scaffolding (checklist 0.1–0.2)
- Created `src/storage/` with 15 source files across `page/`, `btree/`, and top-level modules
- Added `#[cfg(feature = "std")] pub mod storage;` to `src/lib.rs`
- Added `crc32fast = "1"` and `xxhash-rust = { version = "0.8", features = ["xxh3"] }` to `Cargo.toml`
- Created `TestBackend` in `src/storage/mod.rs` `#[cfg(test)]` — minimal in-memory `StorageBackend` implementation for unit tests (MemoryBackend is Task 27)
- Added `map_hal_err` helper for HAL error conversion

### Phase 1: Page Fundamentals (checklist 1.1–1.10) — 38 tests
- **`page/mod.rs`**: `PageId(u64)` with `NULL` sentinel, `PageType` enum (`Interior=0x01`, `Leaf=0x02`, `Overflow=0x03`, `Free=0x04`), constants (`DEFAULT_PAGE_SIZE=4096`, `COMMON_HEADER_SIZE=24`, `IDENTITY_HEADER_SIZE=32`, `SUPERBLOCK_USED_SIZE=192`)
- **`page/header.rs`**: `CommonPageHeader` — 24-byte serialize/deserialize, CRC32C checksum via `crc32fast` (checksum covers entire page with bytes 20–23 zeroed)
- **`page/interior.rs`**: `InteriorPage` with slotted page layout — parse, build, binary search, has_space_for, split at median. Cell format: `left_child(8) + key_len(2) + key`
- **`page/leaf.rs`**: `LeafPage` with doubly-linked list (next/prev leaf), `LeafCellValue::Inline` / `LeafCellValue::Overflow` (sentinel `0xFFFF`). Operations: parse, build, search, search_range, insert_cell, delete_cell, has_space_for, split
- **`page/overflow.rs`**: `OverflowPage` — single page and chain operations. `build_chain` splits data across pages; `read_chain` reconstructs from backend with checksum validation. Max payload = `page_size - 36`
- **`page/free.rs`**: `FreePage` — header-only with `page_type = Free`, rest zero-filled

### Phase 2: File Format (checklist 2.1–2.6) — 14 tests
- **`FileIdentityHeader`**: 32 bytes per `008-file-format-spec.md` §3
  - Magic: `"EmbedGraph\r\n\x1A\n"` (14 bytes, PNG convention) — **corrected from checklist's `b"GRAPHDB\0"`**
  - Format version: `format_major=1`, `format_minor=0` (u16 BE)
  - Page size: `page_size_raw` (u16 LE) with SQLite convention (value 1 = 65536) — **corrected from checklist's `page_size_log2: u8`**
  - Application ID: u32 LE, creation timestamp: u64 LE (microseconds)
- **`Superblock`**: 192 bytes per `008-file-format-spec.md` §4
  - Identity header at bytes 0–31 (immutable), mutable fields at 32+ — **corrected from checklist's separate `b"GRAPHDB_SUPER\0"` magic**
  - 8 B-tree root pointers, reserved roots (4 x u64), reserved fields (32 bytes)
  - Checksum: xxHash3 (u64) at offset 184 over bytes 0–183 — **corrected from checklist's CRC32C (u32)**
  - `initial()`: txn_id=1, total_pages=3, schema_store root=PageId(2), all others=PageId(0)
- **`select_active_superblock`**: reads both pages, validates magic + checksum, picks higher txn_id
- **`create_database_file`**: writes identity header + 2 superblocks + empty leaf for Schema Store root, fsyncs
- **`open_database_file`**: validates identity header, checks page size match, selects active superblock

### Phase 3: Snapshot (checklist 3.1–3.2) — 3 tests
- `Snapshot` and `SnapshotRoots` structs
- `From<&Superblock>` conversion
- `root_for_tree(0..7)` accessor mapping to catalog order

### Phase 4: Buffer Pool (checklist 4.1–4.6) — 9 tests
- `PageFrame`: page_id, `Vec<u8>` data, dirty, pin_count, reference_bit
- `BufferPool`: HashMap page table, clock eviction, min 64 frames
- `fetch_page`: cache hit (pin + reference bit) or miss (clock evict, flush dirty, read + validate checksum)
- `find_victim`: clock algorithm — unpinned + no reference bit = evict, else clear bit and advance. Pool exhaustion error after 2 full sweeps.
- `unpin_page(frame, dirty)`, `flush_page`, `flush_all_dirty`
- `new_page`: zero-init dirty frame for CoW allocations
- `get_page_data` / `get_page_data_mut`

### Phase 5: Serialization (checklist 5.1–5.6) — 19 tests
- **Key encoding (big-endian)**: `encode_node_key` (8B), `encode_edge_key` (8B), `encode_outgoing_adj_key` (20B), `encode_incoming_adj_key` (20B), `encode_type_index_key` (13B), `encode_page_freelist_key` (16B), `encode_id_freelist_key` (9B). All with corresponding decode functions.
- **Schema Store keys**: 6 prefix discriminators (0x01–0x06) for type definitions, property keys, counters, hierarchy, extensions, provenance
- **Value serialization**: 10 type tags (0x00–0x09) covering all `Value` variants including recursive `List`
- **PropertyMap**: `[entry_count: u16 LE] [entries...]` with `[key_id: u32 LE] [value_tag + payload]`
- **NodeRecord**: flags(1) + type_count(1) + primary_type(4 LE) + property_size(4 LE) + overflow_page_id(8 LE) + extra_types + inline_props. `from_node` / `to_node` conversions.
- **EdgeRecord**: same pattern + source(8 LE) + target(8 LE). `from_edge` / `to_edge` conversions.
- **Schema Store values**: `serialize_type_definition`, `serialize_property_key_name`, `serialize_counter`, `serialize_provenance` with round-trip deserializers

### Phase 6: Page Allocator (checklist 6.1–6.2) — 6 tests
- `PageAllocator`: tracks next_page_id, freed_pages, deferred_freed, allocated_pages
- `allocate_page`: increments counter, tracks allocation
- `free_page`: records (txn_id, page_id) for freelist insertion at commit
- `extend_file`: grows file by scaling increment via `set_len`
- Growth increments: `<64 → 8`, `<1024 → 64`, `<16384 → 256`, `>=16384 → 1024`
- `set_deferred_freed` / `take_deferred_freed` for Page Freelist B-tree CoW secondary frees
- `reset_transaction` clears per-txn tracking

---

## Files Created/Modified

| File | Action |
|------|--------|
| `Cargo.toml` | Added `crc32fast`, `xxhash-rust` dependencies |
| `src/lib.rs` | Added `#[cfg(feature = "std")] pub mod storage` |
| `src/storage/mod.rs` | Module root, `map_hal_err`, `TestBackend` |
| `src/storage/page/mod.rs` | `PageId`, `PageType`, constants |
| `src/storage/page/header.rs` | `CommonPageHeader` with CRC32C |
| `src/storage/page/interior.rs` | `InteriorPage`, `InteriorCell` |
| `src/storage/page/leaf.rs` | `LeafPage`, `LeafCell`, `LeafCellValue` |
| `src/storage/page/overflow.rs` | `OverflowPage` with chain operations |
| `src/storage/page/free.rs` | `FreePage` |
| `src/storage/btree/mod.rs` | Placeholder with submodule declarations |
| `src/storage/btree/{search,insert,delete,cursor,cow}.rs` | Placeholders |
| `src/storage/buffer_pool.rs` | `BufferPool`, `PageFrame`, clock eviction |
| `src/storage/allocator.rs` | `PageAllocator`, growth increments |
| `src/storage/format.rs` | `FileIdentityHeader`, `Superblock`, create/open |
| `src/storage/serialization.rs` | All key encodings, record serialization |
| `src/storage/snapshot.rs` | `Snapshot`, `SnapshotRoots` |

---

## Spec Corrections Applied

The checklist (`tasks/24-storage-engine/checklist.md`) had several inaccuracies relative to the authoritative spec (`008-file-format-spec.md`). The implementation follows the spec:

1. **Identity header magic**: Spec: `"EmbedGraph\r\n\x1A\n"` (14 bytes). Checklist: `b"GRAPHDB\0"` (8 bytes).
2. **Page size encoding**: Spec: `page_size_raw: u16 LE` with SQLite convention. Checklist: `page_size_log2: u8`.
3. **Superblock checksum**: Spec: xxHash3 (u64, 8 bytes) at offset 184. Checklist: CRC32C (u32, 4 bytes).
4. **Superblock layout**: Spec: identity header at bytes 0–31, no separate "superblock magic". Checklist: separate `b"GRAPHDB_SUPER\0"` magic.
5. **Initial B-tree roots**: Spec: only Schema Store gets page 2, other 7 trees start with root=0. Checklist suggested either sharing page 2 or individual pages.

---

## Context for Next Session (Phases 7–11)

### What's ready for the B-tree layer
- `InteriorPage` and `LeafPage` provide parse/build/search/split operations
- `BufferPool` provides fetch_page/unpin_page/new_page/flush
- `PageAllocator` provides allocate_page/free_page/extend_file
- `Snapshot` and `SnapshotRoots` capture B-tree root pointers
- All key encodings and record serialization are implemented

### What needs to be built
- **Phase 7**: `BTree` struct, search, cursor (range scan), CoW path copy, insert (with split), delete (borrow-only, no merge in v1)
- **Phase 8**: `StorageEngine<B>` tying everything together, 2-fsync commit protocol, MVCC page reclamation
- **Phase 9**: Crash recovery tests (4 scenarios)
- **Phase 10**: Integration tests with `FileBackend` + `tempfile`
- **Phase 11**: Final verification + completion report

### Design decisions for next session
- **B-tree delete**: borrow-only (skip merge) for v1 — documented in plan
- **Buffer pool `fetch_page`**: takes `&mut B: StorageBackend` to support dirty victim flushing
- **Leaf link maintenance**: when a leaf splits, neighbor pages need CoW updates to their prev/next pointers
- **Page Freelist circular dependency**: secondary freed pages from freelist B-tree CoW are deferred to next transaction
