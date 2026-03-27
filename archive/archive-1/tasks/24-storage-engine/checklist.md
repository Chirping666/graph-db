# Checklist: Task 24 — Implement Persistent Storage Engine & File Format

**Parent:** Task 16 (this checklist)  
**Implements:** All modules in `src/storage/` — page management, buffer pool, CoW B+ tree, record serialization, file format, commit protocol, crash recovery, free-space management.

Execute items in order. After each item, run the verification command(s) listed. Do not proceed until verification passes.

---

## Phase 0: Module Scaffolding

### 0.1 — Create storage module structure

Create the directory and file structure:

```
src/storage/
├── mod.rs
├── page/
│   ├── mod.rs
│   ├── header.rs
│   ├── interior.rs
│   ├── leaf.rs
│   ├── overflow.rs
│   └── free.rs
├── btree/
│   ├── mod.rs
│   ├── search.rs
│   ├── insert.rs
│   ├── delete.rs
│   ├── cursor.rs
│   └── cow.rs
├── buffer_pool.rs
├── allocator.rs
├── format.rs
├── serialization.rs
└── snapshot.rs
```

In `src/storage/mod.rs`:
- Add `#![cfg_attr(not(feature = "std"), allow(dead_code))]` if needed.
- Declare all submodules as `pub mod`.
- Add a module-level doc comment (`//!`) explaining that this module contains the persistent storage engine internals, gated behind the `std` feature.

In `src/lib.rs`:
- Add `#[cfg(feature = "std")] pub mod storage;`

Each file should contain a placeholder `//!` module doc comment.

**Verify:**
- `cargo check` succeeds.
- `cargo check --no-default-features --features alloc` succeeds (storage module is not compiled).

### 0.2 — Add crc32fast dependency

In `Cargo.toml`, add:
```toml
[dependencies]
crc32fast = "1"
```

This dependency is needed by the page header checksum implementation. Verify it compiles under the `std` feature.

**Verify:** `cargo check` succeeds.

---

## Phase 1: Page Fundamentals (`src/storage/page/`)

### 1.1 — Define PageId and page constants

In `src/storage/page/mod.rs`, define:

```rust
/// A page identifier. Page 0 is the identity header + superblock A,
/// page 1 is superblock B, pages 2+ are data pages.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PageId(pub u64);
```

Add:
- `PageId::NULL` constant as `PageId(0)` (note: page 0 exists in the file but `PageId(0)` is used as a sentinel for "no page" in buffer pool frames).
- `impl Display for PageId`.

Define constants:
- `pub const DEFAULT_PAGE_SIZE: usize = 4096;`
- `pub const MIN_PAGE_SIZE: usize = 4096;`
- `pub const COMMON_HEADER_SIZE: usize = 24;`
- `pub const IDENTITY_HEADER_SIZE: usize = 32;`
- `pub const SUPERBLOCK_SIZE: usize = 192;` (160 bytes + reserved + alignment as per 008 §4)

Define the page type discriminant:
```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum PageType {
    Interior = 0x01,
    Leaf = 0x02,
    Overflow = 0x03,
    Free = 0x04,
}
```

Implement `TryFrom<u8>` for `PageType` (returns error for unknown values including 0x00).

**Verify:** `cargo check`

### 1.2 — Implement CommonPageHeader

In `src/storage/page/header.rs`, implement the 24-byte common page header per `008-file-format-spec.md` §5:

```rust
pub struct CommonPageHeader {
    pub page_id: PageId,        // offset 0, u64 LE
    pub page_type: PageType,    // offset 8, u8
    pub flags: u8,              // offset 9, u8
    // offset 10: 2 bytes padding (must be zero)
    pub txn_id: u64,            // offset 12, u64 LE
    pub checksum: u32,          // offset 20, u32 LE (CRC32C)
}
```

Implement:
- `pub fn serialize(&self, buf: &mut [u8])` — writes 24 bytes to buf. The checksum field is written as-is (caller computes it).
- `pub fn deserialize(buf: &[u8]) -> Result<Self, StorageError>` — reads 24 bytes from buf. Validates padding bytes are zero.
- `pub fn compute_checksum(page_data: &[u8]) -> u32` — computes CRC32C over all page bytes with the checksum field (bytes 20–23) zeroed. Uses `crc32fast`.
- `pub fn validate_checksum(page_data: &[u8]) -> Result<(), StorageError>` — computes checksum and compares to stored value. Returns `StorageErrorKind::MediaCorruption` on mismatch.

**⚠ Pitfall:** The checksum is computed over the *entire page* (all `page_size` bytes), not just the header. When computing, temporarily treat bytes 20–23 as zeros.

**Verify:** `cargo check`

### 1.3 — Unit tests for CommonPageHeader

Test:
- Serialize a header, deserialize it, verify all fields match.
- Compute checksum of a page filled with known data, verify it is deterministic.
- Validate checksum succeeds for correct data.
- Validate checksum fails (returns `MediaCorruption`) for data with a flipped bit.
- Reject deserialization when padding bytes are non-zero.
- Reject deserialization when `page_type` byte is 0x00 or unknown.

**Verify:** `cargo test -- storage::page::header` passes.

### 1.4 — Implement InteriorPage

In `src/storage/page/interior.rs`, implement the B-tree interior page format per `008-file-format-spec.md` §7:

```rust
pub struct InteriorPage {
    pub header: CommonPageHeader,
    pub cell_count: u16,
    pub right_child: PageId,
    pub free_start: u16,
    cells: Vec<InteriorCell>,  // parsed cells in key order
}

pub struct InteriorCell {
    pub left_child: PageId,    // u64 LE
    pub key: Vec<u8>,          // variable-length key bytes
}
```

Implement:
- `pub fn parse(page_data: &[u8], page_size: usize) -> Result<Self, StorageError>` — parses from raw page bytes. Reads the cell pointer array, then reads each cell from the content area.
- `pub fn build(page_id: PageId, txn_id: u64, cells: &[InteriorCell], right_child: PageId, page_size: usize) -> Vec<u8>` — constructs a complete page image (all `page_size` bytes) with correct header, cell pointer array, and cell content area. Computes and writes the CRC32C checksum.
- `pub fn search(&self, key: &[u8]) -> PageId` — binary search on cells to find the correct child page ID for the given key. Returns `right_child` if key >= all separator keys.
- `pub fn has_space_for(&self, key_len: usize, page_size: usize) -> bool` — checks if a new cell can fit.
- `pub fn split(&self, page_size: usize) -> (Vec<InteriorCell>, Vec<u8>, Vec<InteriorCell>, PageId)` — splits the page at the median. Returns (left cells, median key, right cells, right's right_child). The median key is promoted to the parent.

**⚠ Pitfall — slotted page layout:** Cell pointer offsets are from page start. Cells grow backward from the end of the page. The cell pointer array grows forward from offset 38. Verify that `free_start` tracking is correct during builds and inserts.

**Verify:** `cargo check`

### 1.5 — Unit tests for InteriorPage

Test:
- Build an interior page with 3 cells, parse it back, verify all cells and right_child match.
- Search for a key less than all separators → returns first cell's left_child.
- Search for a key between two separators → returns correct child.
- Search for a key greater than all separators → returns right_child.
- Search with a key equal to a separator → returns the correct child (key ≤ convention).
- `has_space_for` returns false when the page is full.
- Split an interior page with an even number of cells → median key is promoted, left and right sets are balanced.
- Verify byte-level layout matches `008-file-format-spec.md` §7 (check specific offsets).

**Verify:** `cargo test -- storage::page::interior` passes.

### 1.6 — Implement LeafPage

In `src/storage/page/leaf.rs`, implement the B-tree leaf page format per `008-file-format-spec.md` §8:

```rust
pub struct LeafPage {
    pub header: CommonPageHeader,
    pub cell_count: u16,
    pub free_start: u16,
    pub next_leaf: PageId,     // 0 = none
    pub prev_leaf: PageId,     // 0 = none
    cells: Vec<LeafCell>,      // parsed cells in key order
}

pub struct LeafCell {
    pub key: Vec<u8>,
    pub value: LeafCellValue,
}

pub enum LeafCellValue {
    Inline(Vec<u8>),
    Overflow {
        overflow_page_id: PageId,
        total_overflow_len: u32,
    },
}
```

Implement:
- `pub fn parse(page_data: &[u8], page_size: usize) -> Result<Self, StorageError>` — reads header, cell pointers, cells. Detects overflow-redirected cells by `value_len == 0xFFFF`.
- `pub fn build(page_id: PageId, txn_id: u64, cells: &[LeafCell], next_leaf: PageId, prev_leaf: PageId, page_size: usize) -> Vec<u8>` — constructs a complete page image with checksum.
- `pub fn search(&self, key: &[u8]) -> Option<&LeafCell>` — binary search for exact key match.
- `pub fn search_range(&self, start_key: &[u8], end_key: &[u8]) -> Vec<&LeafCell>` — returns all cells with keys in `[start_key, end_key]`.
- `pub fn insert_cell(&mut self, cell: LeafCell) -> Result<(), ()>` — inserts a cell in sorted position. Returns `Err(())` if the page is full.
- `pub fn delete_cell(&mut self, key: &[u8]) -> Option<LeafCell>` — removes and returns the cell with the given key.
- `pub fn has_space_for(&self, key_len: usize, value_len: usize, page_size: usize) -> bool`
- `pub fn split(&self, page_size: usize) -> (Vec<LeafCell>, Vec<LeafCell>, Vec<u8>)` — splits at median. Returns (left cells, right cells, split key). The split key is a copy of the first key in the right set.
- `pub fn cell_count(&self) -> usize`
- `pub fn cells(&self) -> &[LeafCell]`

**⚠ Pitfall — overflow sentinel:** `value_len == 0xFFFF` in the raw byte format signals an overflow-redirected cell. When parsing, read the 12-byte overflow pointer (8-byte PageId + 4-byte total_overflow_len) instead of a normal value.

**⚠ Pitfall — key ordering:** All key comparisons are byte-level lexicographic (memcmp). This is correct because all B-tree keys use big-endian encoding per design decision G4.

**Verify:** `cargo check`

### 1.7 — Unit tests for LeafPage

Test:
- Build a leaf page with 5 cells, parse it back, verify all cells match.
- Search for an existing key → returns the correct cell.
- Search for a non-existent key → returns None.
- Range search returns correct subset.
- Insert a cell into a non-full page → cell appears in correct sorted position.
- Insert into a full page → returns Err.
- Delete a cell → cell is removed, other cells remain.
- Split a leaf page → left and right are balanced; split key is first key of right set.
- Overflow cell: build a leaf cell with `value_len = 0xFFFF` and overflow pointer, parse it back, verify the overflow fields are correct.
- Verify `next_leaf` and `prev_leaf` are correctly serialized and parsed.
- Verify byte-level layout matches `008-file-format-spec.md` §8 (check offsets 24–44).
- Key-only cell (e.g., for adjacency index): value_len = 0, empty value. Round-trips correctly.

**Verify:** `cargo test -- storage::page::leaf` passes.

### 1.8 — Implement OverflowPage

In `src/storage/page/overflow.rs`, implement the overflow page format per `008-file-format-spec.md` §9:

```rust
pub struct OverflowPage {
    pub header: CommonPageHeader,
    pub next_page: PageId,      // 0 = last page in chain
    pub data_length: u32,
    pub data: Vec<u8>,
}
```

Implement:
- `pub fn parse(page_data: &[u8], page_size: usize) -> Result<Self, StorageError>`
- `pub fn build(page_id: PageId, txn_id: u64, next_page: PageId, data: &[u8], page_size: usize) -> Vec<u8>`
- `pub fn max_payload(page_size: usize) -> usize` — returns `page_size - 36`.
- `pub fn build_chain(page_ids: &[PageId], txn_id: u64, data: &[u8], page_size: usize) -> Vec<Vec<u8>>` — splits `data` across multiple overflow pages, linking them. Returns one page image per PageId.
- `pub fn read_chain<B: StorageBackend>(backend: &B, first_page: PageId, expected_total: u32, page_size: usize) -> Result<Vec<u8>, StorageError>` — follows the chain, concatenating data, validating total length matches `expected_total`.

**Verify:** `cargo check`

### 1.9 — Unit tests for OverflowPage

Test:
- Single overflow page: build, parse, verify data matches.
- Chain of 3 overflow pages: build chain, parse each page, verify linking (next_page pointers).
- `read_chain` on a 3-page chain: reconstructed data matches original.
- `read_chain` with wrong `expected_total` → error.
- `max_payload` at 4096 page size → 4060.

**Verify:** `cargo test -- storage::page::overflow` passes.

### 1.10 — Implement FreePage

In `src/storage/page/free.rs`:

```rust
pub struct FreePage {
    pub header: CommonPageHeader,
}
```

Implement:
- `pub fn build(page_id: PageId, txn_id: u64, page_size: usize) -> Vec<u8>` — writes header with `page_type = Free`, fills rest with zeros.
- `pub fn parse(page_data: &[u8]) -> Result<Self, StorageError>` — validates `page_type == Free`.

**Verify:** `cargo check`

---

## Phase 2: File Format (`src/storage/format.rs`)

### 2.1 — Implement FileIdentityHeader

Implement the 32-byte file identity header per `008-file-format-spec.md` §3:

```rust
pub struct FileIdentityHeader {
    pub magic: [u8; 8],         // b"GRAPHDB\0"
    pub format_major: u16,      // BE
    pub format_minor: u16,      // BE
    pub page_size_log2: u8,     // log2(page_size) - 12
    pub application_id: u32,    // LE
    pub creation_timestamp: u64, // LE, Unix epoch seconds
    // 7 bytes reserved (must be zero)
}
```

Implement:
- `pub const MAGIC: &[u8; 8] = b"GRAPHDB\0";`
- `pub fn new(page_size: usize, application_id: u32) -> Self` — computes `page_size_log2`, sets format version to 1.0, sets creation timestamp to current time.
- `pub fn serialize(&self, buf: &mut [u8])` — writes 32 bytes.
- `pub fn deserialize(buf: &[u8]) -> Result<Self, StorageError>` — validates magic bytes. Returns `MediaCorruption` on magic mismatch.
- `pub fn page_size(&self) -> usize` — computes `1 << (page_size_log2 + 12)`.
- `pub fn validate_compatible(&self) -> Result<(), StorageError>` — checks `format_major == 1`. Returns error for unknown major versions.

**Verify:** `cargo check`

### 2.2 — Implement Superblock

Implement the dual-superblock structure per `008-file-format-spec.md` §4:

```rust
pub struct Superblock {
    pub magic: [u8; 14],           // GRAPHDB_SUPER\0
    pub transaction_id: u64,       // LE
    pub total_pages: u64,          // LE
    pub root_node_store: PageId,
    pub root_edge_store: PageId,
    pub root_outgoing_adj: PageId,
    pub root_incoming_adj: PageId,
    pub root_type_index: PageId,
    pub root_schema_store: PageId,
    pub root_id_freelist: PageId,
    pub root_page_freelist: PageId,
    pub feature_flags: u64,        // LE
    pub checksum: u32,             // LE
    // reserved bytes to fill 192 total
}
```

Implement:
- `pub const SUPERBLOCK_MAGIC: &[u8; 14] = b"GRAPHDB_SUPER\0";`
- `pub fn serialize(&self, buf: &mut [u8])` — writes to buf (up to 192 bytes). Checksum covers bytes 0 through the byte before the checksum field.
- `pub fn deserialize(buf: &[u8]) -> Result<Self, StorageError>` — parses and validates magic.
- `pub fn compute_checksum(buf: &[u8]) -> u32` — CRC32C over superblock bytes (with checksum field zeroed).
- `pub fn validate(&self, raw_bytes: &[u8]) -> Result<(), StorageError>` — validates magic and checksum.
- `pub fn root_page_ids(&self) -> [PageId; 8]` — returns all 8 root page IDs as an array.
- `pub fn initial(page_size: usize) -> Self` — creates the initial superblock for a new database: txn_id=1, total_pages=3 (header page + SB-B page + one initial root page), all root page IDs pointing to PageId(2) (a single empty leaf page that all trees share initially), feature_flags=0.

**⚠ Design note on initial tree roots:** For a brand-new database, all 8 B-trees start empty. The simplest approach is to allocate one empty leaf page per B-tree (pages 2–9, with total_pages=10). Alternatively, all empty trees can share a single empty root page (page 2, total_pages=3) since they diverge on first insert. Choose the simpler implementation; document the choice.

**Verify:** `cargo check`

### 2.3 — Implement active superblock selection

Implement the startup procedure per `008-file-format-spec.md` §4.3:

```rust
pub fn select_active_superblock<B: ReadAt>(
    backend: &B,
    page_size: usize,
) -> Result<(Superblock, u8), StorageError>
```

This function:
1. Reads both superblock pages (page 0 at offset `IDENTITY_HEADER_SIZE`, page 1 at offset `page_size`).
2. Validates magic and checksum for each.
3. If both valid: returns the one with higher `transaction_id`. Ties: return either (slot 0).
4. If one valid: returns that one.
5. If neither valid: returns `MediaCorruption` error.

Returns the active superblock and its slot index (0 or 1).

**Verify:** `cargo check`

### 2.4 — Implement database file creation

```rust
pub fn create_database_file<B: StorageBackend>(
    backend: &mut B,
    page_size: usize,
    application_id: u32,
) -> Result<Superblock, StorageError>
```

This function per `008-file-format-spec.md` §15.3:
1. Writes the file identity header to the start of page 0.
2. Creates an initial superblock.
3. Allocates initial empty leaf page(s) for B-tree roots.
4. Writes superblock A (in page 0, after identity header) and superblock B (page 1).
5. Writes the initial root page(s).
6. Calls `backend.sync_all()` — single fsync at creation.
7. Returns the initial superblock.

**Verify:** `cargo check`

### 2.5 — Implement database file opening

```rust
pub fn open_database_file<B: ReadAt>(
    backend: &B,
    page_size: usize,
) -> Result<Superblock, StorageError>
```

This function:
1. Reads and validates the file identity header.
2. Calls `select_active_superblock`.
3. Returns the active superblock.

Also validate that the header's page size matches the provided `page_size` parameter.

**Verify:** `cargo check`

### 2.6 — Unit tests for file format

Test:
- `FileIdentityHeader`: serialize, deserialize, round-trip. Magic validation failure. Version validation.
- `Superblock`: serialize, deserialize, round-trip. Checksum validation success and failure. Root page ID extraction.
- Active superblock selection: both valid (higher txn_id wins), one valid (fallback), both invalid (error), equal txn_ids.
- Create database file using a `MemoryBackend`, verify the byte layout of pages 0 and 1.
- Open the created database file, verify the returned superblock matches what was written.

**Verify:** `cargo test -- storage::format` passes.

---

## Phase 3: Snapshot (`src/storage/snapshot.rs`)

### 3.1 — Implement Snapshot

```rust
/// A consistent point-in-time view of the database,
/// defined by a set of B-tree root page IDs and a transaction ID.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub transaction_id: u64,
    pub total_pages: u64,
    pub roots: SnapshotRoots,
}

#[derive(Clone, Debug)]
pub struct SnapshotRoots {
    pub node_store: PageId,
    pub edge_store: PageId,
    pub outgoing_adj: PageId,
    pub incoming_adj: PageId,
    pub type_index: PageId,
    pub schema_store: PageId,
    pub id_freelist: PageId,
    pub page_freelist: PageId,
}
```

Implement:
- `impl From<&Superblock> for Snapshot` — extracts roots from superblock.
- Accessor methods to get the root page for a given B-tree by logical name.
- `pub fn root_for_tree(&self, tree_index: usize) -> PageId` — index 0–7 maps to the 8 B-trees in catalog order (per `012-design-document.md` §19.1).

**Verify:** `cargo check`

### 3.2 — Unit tests for Snapshot

Test:
- Create a snapshot from a superblock, verify all root page IDs match.
- `root_for_tree(0)` returns node_store root, `root_for_tree(7)` returns page_freelist root.

**Verify:** `cargo test -- storage::snapshot` passes.

---

## Phase 4: Buffer Pool (`src/storage/buffer_pool.rs`)

### 4.1 — Implement PageFrame and BufferPool structure

Per `012-design-document.md` §9:

```rust
pub struct PageFrame {
    page_id: PageId,        // PageId(0) if empty
    data: Vec<u8>,          // page_size bytes
    dirty: bool,
    pin_count: u32,
    reference_bit: bool,
}

pub struct BufferPool {
    frames: Vec<PageFrame>,
    page_table: HashMap<PageId, usize>,  // PageId → frame index
    clock_hand: usize,
    capacity: usize,
    page_size: usize,
}
```

Implement:
- `pub fn new(capacity: usize, page_size: usize) -> Self` — allocates `capacity` empty frames.
- Accessors: `pub fn capacity(&self) -> usize`, `pub fn page_size(&self) -> usize`.

**⚠ Design note:** The buffer pool stores `Vec<u8>` rather than `[u8; PAGE_SIZE]` because the page size is configurable at runtime. Each frame's `data` is allocated to `page_size` bytes.

**Verify:** `cargo check`

### 4.2 — Implement fetch_page

```rust
pub fn fetch_page<B: ReadAt>(
    &mut self,
    page_id: PageId,
    backend: &B,
) -> Result<usize, StorageError>
```

Per `007-graph-storage-model.md` §10.3:
1. Check `page_table` for `page_id`. If found: set `reference_bit = true`, increment `pin_count`, return frame index.
2. Cache miss: call `find_victim()` to get a frame index. If the victim frame is dirty, flush it to `backend` (requires `&mut` access — see pitfall below). Read the page from disk at `page_id.0 * page_size`. Validate the page checksum. Insert into `page_table`. Set `pin_count = 1`, `reference_bit = true`. Return frame index.

**⚠ Pitfall — backend mutability:** `ReadAt::read_at` takes `&self`, so reading doesn't need `&mut backend`. However, flushing a dirty victim requires `WriteAt::write_at`, which takes `&mut self`. The buffer pool's `fetch_page` will need to accept a backend that implements `StorageBackend` (both `ReadAt` and `WriteAt`). Alternatively, separate the flush path. The simplest approach is to take `&B` where `B: ReadAt` for reads and handle dirty victim flushing separately via a flush method that takes `&mut B`. Design this so that read transactions (which never dirty pages) can use a read-only code path.

**⚠ Revised design — split fetch for read vs. write:**
- `fetch_page_read<B: ReadAt>(&mut self, page_id: PageId, backend: &B) -> Result<usize, StorageError>` — for read paths. If a dirty victim is encountered, it cannot be flushed (no `WriteAt` available). The clock should skip dirty frames or the pool should have been flushed before entering read-only mode.
- `fetch_page<B: StorageBackend>(&mut self, page_id: PageId, backend: &mut B) -> Result<usize, StorageError>` — for write paths. Can flush dirty victims.

Document this design choice in the code.

**Verify:** `cargo check`

### 4.3 — Implement clock eviction

```rust
fn find_victim(&mut self) -> Result<usize, StorageError>
```

Per `007-graph-storage-model.md` §10.4:
1. Starting from `clock_hand`, sweep frames circularly.
2. Frame unpinned + reference_bit false → evict (return frame index, remove from `page_table`).
3. Frame unpinned + reference_bit true → clear reference_bit, advance.
4. Frame pinned → skip, advance.
5. Full cycle with no evictable frame → return error (buffer pool exhausted).

**Verify:** `cargo check`

### 4.4 — Implement unpin_page and flush

```rust
pub fn unpin_page(&mut self, frame_index: usize, dirty: bool)
```

Decrements `pin_count`. If `dirty`, marks the frame as dirty.

```rust
pub fn flush_page<B: WriteAt + hal::Sync>(
    &mut self,
    frame_index: usize,
    backend: &mut B,
) -> Result<(), StorageError>
```

Writes the frame's data to disk at `frame.page_id.0 * page_size`. Clears the dirty flag.

```rust
pub fn flush_all_dirty<B: WriteAt + hal::Sync>(
    &mut self,
    backend: &mut B,
) -> Result<(), StorageError>
```

Flushes all dirty frames. Used at commit time.

```rust
pub fn get_page_data(&self, frame_index: usize) -> &[u8]
pub fn get_page_data_mut(&mut self, frame_index: usize) -> &mut [u8]
```

Returns a reference to the frame's data buffer.

**Verify:** `cargo check`

### 4.5 — Implement new_page (for CoW)

```rust
pub fn new_page(&mut self, page_id: PageId, page_size: usize) -> Result<usize, StorageError>
```

Allocates a frame for a newly created page (CoW copy or file extension). The frame is marked dirty. If the pool is full, eviction is needed. The page data is zero-initialized (the caller writes the content afterward).

**Verify:** `cargo check`

### 4.6 — Unit tests for BufferPool

Test:
- Create a pool with capacity 4 and a `MemoryBackend`. Pre-write page data to the backend.
- `fetch_page` on a page not in pool → reads from backend, returns frame index. Data matches what was written.
- `fetch_page` on the same page again → cache hit, pin_count increments.
- `unpin_page` decrements pin_count.
- Fetch 5 pages in a pool of capacity 4 → eviction occurs. Verify the evicted page is the one with reference_bit=false.
- Dirty page eviction: mark a page dirty, fetch enough pages to trigger eviction of the dirty page, verify the dirty page was written to the backend.
- Pin count enforcement: a pinned page is never evicted.
- Clock algorithm behavior: access pattern that verifies reference_bit clearing.
- Pool exhaustion: pin all frames, attempt to fetch another page → error.
- `new_page`: allocate a new page frame, write to it, flush → data appears in backend.
- `flush_all_dirty`: mark multiple pages dirty, flush all, verify all are written to backend and dirty flags are cleared.

**Verify:** `cargo test -- storage::buffer_pool` passes.

---

## Phase 5: Key Encoding and Record Serialization (`src/storage/serialization.rs`)

### 5.1 — Implement B-tree key encoding

Per `007-graph-storage-model.md` §6 and `012-design-document.md` §6.2, implement key encoding helpers:

```rust
/// Encodes a NodeId as an 8-byte big-endian key.
pub fn encode_node_key(id: NodeId) -> [u8; 8]

/// Encodes an EdgeId as an 8-byte big-endian key.
pub fn encode_edge_key(id: EdgeId) -> [u8; 8]

/// Encodes an outgoing adjacency key: (NodeId, TypeId, EdgeId) = 20 bytes BE.
pub fn encode_outgoing_adj_key(node: NodeId, type_id: TypeId, edge: EdgeId) -> [u8; 20]

/// Encodes an incoming adjacency key: (NodeId, TypeId, EdgeId) = 20 bytes BE.
pub fn encode_incoming_adj_key(node: NodeId, type_id: TypeId, edge: EdgeId) -> [u8; 20]

/// Encodes a type index key: (TypeKindTag, TypeId, EntityId) = 13 bytes BE.
pub fn encode_type_index_key(kind_tag: u8, type_id: TypeId, entity_id: u64) -> [u8; 13]

/// Encodes a page freelist key: (FreedTxnId, PageId) = 16 bytes BE.
pub fn encode_page_freelist_key(freed_txn_id: u64, page_id: PageId) -> [u8; 16]

/// Encodes an ID freelist key: (EntityKindTag, EntityId) = 9 bytes BE.
pub fn encode_id_freelist_key(kind_tag: u8, entity_id: u64) -> [u8; 9]
```

Also implement Schema Store key encoding helpers per `012-design-document.md` §19.2:

```rust
/// Encodes a Schema Store key with prefix 0x01 (type definition).
pub fn encode_schema_type_key(type_id: TypeId) -> Vec<u8>

/// Encodes a Schema Store key with prefix 0x02 (property key).
pub fn encode_schema_property_key(key_id: PropertyKeyId) -> Vec<u8>

/// Encodes a Schema Store key with prefix 0x03 (counter).
pub fn encode_schema_counter_key(counter_name: u8) -> Vec<u8>

/// Encodes a Schema Store key with prefix 0x04 (type hierarchy edge).
pub fn encode_schema_hierarchy_key(child: TypeId, parent: TypeId) -> Vec<u8>

/// Encodes a Schema Store key with prefix 0x05 (extension name).
pub fn encode_schema_extension_key(kind: u8, name: &str) -> Vec<u8>

/// Encodes a Schema Store key with prefix 0x06 (provenance).
pub fn encode_schema_provenance_key(entity_kind: u8, entity_id: u64, sub_id: u32) -> Vec<u8>
```

Corresponding decode functions for each key type.

**Verify:** `cargo check`

### 5.2 — Implement NodeRecord serialization

Per `007-graph-storage-model.md` §5.1:

```rust
pub struct NodeRecord {
    pub flags: u8,              // bit 0: is_anonymous
    pub type_count: u8,
    pub primary_type: TypeId,
    pub property_size: u32,
    pub overflow_page_id: PageId,
    pub extra_types: Vec<TypeId>,
    pub inline_properties: Vec<u8>,
}
```

Implement:
- `pub fn serialize(&self) -> Vec<u8>` — produces the binary record in LE format.
- `pub fn deserialize(data: &[u8]) -> Result<Self, StorageError>`
- `pub fn from_node(node: &Node, serialized_props: &[u8], overflow_page: Option<PageId>) -> Self` — constructs from a `Node` and pre-serialized property bytes.
- `pub fn to_node(&self, node_id: NodeId, properties: PropertyMap) -> Node` — reconstructs the `Node`.

**Verify:** `cargo check`

### 5.3 — Implement EdgeRecord serialization

Per `007-graph-storage-model.md` §5.2:

```rust
pub struct EdgeRecord {
    pub flags: u8,
    pub source: NodeId,
    pub target: NodeId,
    pub type_count: u8,
    pub primary_type: TypeId,
    pub property_size: u32,
    pub overflow_page_id: PageId,
    pub extra_types: Vec<TypeId>,
    pub inline_properties: Vec<u8>,
}
```

Implement `serialize`, `deserialize`, `from_edge`, `to_edge` analogous to NodeRecord.

**Verify:** `cargo check`

### 5.4 — Implement property serialization

Per `007-graph-storage-model.md` §7.4:

```rust
/// Serializes a PropertyMap to binary format.
/// Each property: [PropertyKeyId: 4 LE] [type_tag: 1] [payload].
pub fn serialize_properties(props: &PropertyMap) -> Vec<u8>

/// Deserializes a PropertyMap from binary format.
pub fn deserialize_properties(data: &[u8]) -> Result<PropertyMap, StorageError>

/// Serializes a single Value to binary format.
/// Format: [type_tag: u8] [payload]
pub fn serialize_value(value: &Value) -> Vec<u8>

/// Deserializes a single Value from binary format.
pub fn deserialize_value(data: &[u8]) -> Result<(Value, usize), StorageError>
```

Value type tags:
- `0x00` = Null, `0x01` = Bool, `0x02` = I64, `0x03` = U64, `0x04` = F64
- `0x05` = String, `0x06` = Bytes, `0x07` = NodeRef, `0x08` = LangString, `0x09` = List

String/Bytes encoding: `[len: u32 LE] [data]`.
LangString encoding: `[value_len: u32 LE] [value_data] [lang_len: u32 LE] [lang_data]`.
List encoding: `[count: u32 LE] [element_0] [element_1] ...` (each element is a recursive `serialize_value`).

**⚠ Pitfall:** `deserialize_value` must return the number of bytes consumed so the caller can advance through a buffer of consecutive values.

**Verify:** `cargo check`

### 5.5 — Implement Schema Store value serialization

Implement serialization for schema entries stored in the Schema Store B-tree:

```rust
/// Serializes a TypeDefinition for storage in the Schema Store.
pub fn serialize_type_definition(td: &TypeDefinition) -> Vec<u8>

/// Deserializes a TypeDefinition from Schema Store bytes.
pub fn deserialize_type_definition(data: &[u8]) -> Result<TypeDefinition, StorageError>

/// Serializes a property key name (just the string with length prefix).
pub fn serialize_property_key_name(name: &str) -> Vec<u8>

/// Deserializes a property key name.
pub fn deserialize_property_key_name(data: &[u8]) -> Result<String, StorageError>

/// Serializes a counter value (u64 LE).
pub fn serialize_counter(value: u64) -> [u8; 8]

/// Deserializes a counter value.
pub fn deserialize_counter(data: &[u8]) -> Result<u64, StorageError>

/// Serializes a provenance record.
pub fn serialize_provenance(record: &ProvenanceRecord) -> Vec<u8>

/// Deserializes a provenance record.
pub fn deserialize_provenance(data: &[u8]) -> Result<ProvenanceRecord, StorageError>
```

**Verify:** `cargo check`

### 5.6 — Unit tests for serialization

Test:
- **Key encoding sort order:** Generate 10 random NodeIds, encode each as big-endian keys, sort the byte arrays lexicographically, decode, verify the decoded IDs are in ascending order. Repeat for all composite key types (adjacency, type index, page freelist, ID freelist).
- **NodeRecord round-trip:** Create a NodeRecord with 1 type and 3 properties, serialize, deserialize, verify all fields match.
- **NodeRecord with overflow:** Create with `overflow_page_id != 0`, verify overflow indicator is correct.
- **EdgeRecord round-trip:** Similar to NodeRecord, including source/target.
- **Property serialization:** Round-trip for every Value variant (Null, Bool, I64, U64, F64, String, Bytes, NodeRef, LangString, empty List, non-empty List, nested List).
- **Empty PropertyMap:** serialize → 0 bytes. Deserialize → empty map.
- **Schema Store entries:** Round-trip for TypeDefinition (with supertypes, property declarations, metadata), property key name, counter value, provenance record.
- **Key encoding edge cases:** Encode NodeId(0) and NodeId(u64::MAX), verify byte representations. Verify that NodeId(1) < NodeId(256) in byte order (i.e., big-endian works correctly).

**Verify:** `cargo test -- storage::serialization` passes.

---

## Phase 6: Page Allocator (`src/storage/allocator.rs`)

### 6.1 — Implement PageAllocator

```rust
pub struct PageAllocator {
    /// Next page ID for file extension.
    next_page_id: u64,
    /// Total pages currently in the file.
    total_pages: u64,
    /// Pages freed in this transaction (to be inserted into freelist at commit).
    freed_pages: Vec<(u64, PageId)>,  // (freed_txn_id, page_id)
    /// Secondary freed pages deferred from previous transaction.
    deferred_freed: Vec<(u64, PageId)>,
    /// Pages allocated in this transaction (for rollback tracking).
    allocated_pages: Vec<PageId>,
    /// Page size for offset calculations.
    page_size: usize,
}
```

Implement:
- `pub fn new(total_pages: u64, page_size: usize) -> Self`
- `pub fn allocate_page(&mut self) -> PageId` — allocates from the next_page_id counter. Increments `next_page_id` and tracks in `allocated_pages`. (Freelist-based allocation is deferred to Phase 8 integration with B-tree.)
- `pub fn free_page(&mut self, page_id: PageId, txn_id: u64)` — records in `freed_pages`.
- `pub fn total_pages(&self) -> u64`
- `pub fn freed_pages(&self) -> &[(u64, PageId)]`
- `pub fn set_deferred_freed(&mut self, deferred: Vec<(u64, PageId)>)`
- `pub fn take_deferred_freed(&mut self) -> Vec<(u64, PageId)>`

Growth increment calculation per `008-file-format-spec.md` §12.3:
```rust
pub fn compute_growth_increment(current_total: u64) -> u64 {
    if current_total < 64 { 8 }
    else if current_total < 1024 { 64 }
    else if current_total < 16384 { 256 }
    else { 1024 }
}
```

```rust
pub fn extend_file<B: WriteAt>(
    &mut self,
    backend: &mut B,
) -> Result<Vec<PageId>, StorageError>
```

Extends the file by `compute_growth_increment(total_pages)` pages. Calls `backend.set_len()`. Returns the newly available page IDs.

**Verify:** `cargo check`

### 6.2 — Unit tests for PageAllocator

Test:
- Allocate 3 pages → returns consecutive PageIds.
- Growth increment: verify correct values at boundary sizes (63, 64, 1023, 1024, 16383, 16384).
- Free a page → appears in `freed_pages`.
- `extend_file` with a `MemoryBackend`: verify backend length increased correctly.

**Verify:** `cargo test -- storage::allocator` passes.

---

## Phase 7: CoW B+ Tree Operations (`src/storage/btree/`)

### 7.1 — Define BTree struct and configuration

In `src/storage/btree/mod.rs`:

```rust
pub struct BTreeConfig {
    pub page_size: usize,
}

/// A logical B+ tree identified by its root page.
/// Operations are stateless — the root is passed in and a new root is returned.
pub struct BTree {
    config: BTreeConfig,
}
```

The B-tree operations are stateless functions that take a root page ID and return results or a new root page ID (for mutations). This matches the CoW model: each operation may produce new pages.

**Verify:** `cargo check`

### 7.2 — Implement B-tree search (point lookup)

In `src/storage/btree/search.rs`:

```rust
impl BTree {
    /// Looks up a key in the B-tree. Returns the value bytes if found, None otherwise.
    pub fn search(
        &self,
        root: PageId,
        key: &[u8],
        pool: &mut BufferPool,
        backend: &impl ReadAt,
    ) -> Result<Option<Vec<u8>>, StorageError>
}
```

Algorithm:
1. Fetch the root page from the buffer pool.
2. If interior: binary search for the child pointer, unpin current page, recurse into child.
3. If leaf: binary search for the key. If found, return a clone of the value bytes (or follow overflow chain). Unpin the page.

**⚠ Pitfall — pin discipline:** Every `fetch_page` must be balanced by an `unpin_page`. If the function returns early (error), ensure the page is unpinned. Consider using a RAII guard pattern or careful `?` placement.

**Verify:** `cargo check`

### 7.3 — Implement B-tree range scan

In `src/storage/btree/cursor.rs`:

```rust
pub struct BTreeCursor {
    /// Current leaf page ID (or PageId(0) if exhausted).
    current_leaf: PageId,
    /// Current cell index within the leaf.
    current_cell: usize,
    /// End key (exclusive) for the range, or None for open-ended.
    end_key: Option<Vec<u8>>,
}

impl BTreeCursor {
    /// Creates a cursor positioned at the first key >= start_key.
    pub fn new(
        root: PageId,
        start_key: &[u8],
        end_key: Option<&[u8]>,
        pool: &mut BufferPool,
        backend: &impl ReadAt,
        config: &BTreeConfig,
    ) -> Result<Self, StorageError>

    /// Advances the cursor and returns the next (key, value) pair.
    /// Returns None when the range is exhausted.
    pub fn next(
        &mut self,
        pool: &mut BufferPool,
        backend: &impl ReadAt,
        config: &BTreeConfig,
    ) -> Result<Option<(Vec<u8>, Vec<u8>)>, StorageError>
}
```

The cursor navigates from root to the first matching leaf, then follows `next_leaf` pointers for the rest of the range.

For key-only B-trees (adjacency index, type index, freelist), the value is empty. The cursor returns `(key, vec![])`.

**Verify:** `cargo check`

### 7.4 — Implement CoW path copy

In `src/storage/btree/cow.rs`:

```rust
pub struct CowResult {
    pub new_root: PageId,
    pub freed_pages: Vec<PageId>,
    pub new_pages: Vec<PageId>,
}
```

The CoW path copy is the core mechanism: when a leaf is modified, every page from the leaf to the root is copied to new pages. The old pages are recorded as freed.

```rust
/// Performs a CoW path copy, returning the new root and the set of
/// freed and newly allocated pages.
pub fn cow_path_copy(
    old_root: PageId,
    path: &[(PageId, usize)],  // (page_id, child_index) from root to parent of modified leaf
    new_leaf: PageId,
    allocator: &mut PageAllocator,
    pool: &mut BufferPool,
    backend: &impl ReadAt,
    config: &BTreeConfig,
    txn_id: u64,
) -> Result<CowResult, StorageError>
```

The function walks the path from the leaf's parent up to the root, copying each interior page with the updated child pointer.

**Verify:** `cargo check`

### 7.5 — Implement B-tree insert

In `src/storage/btree/insert.rs`:

```rust
impl BTree {
    /// Inserts a key-value pair into the B-tree.
    /// Returns a CowResult with the new root page ID.
    /// If the key already exists, the old value is replaced.
    pub fn insert(
        &self,
        root: PageId,
        key: &[u8],
        value: &[u8],
        pool: &mut BufferPool,
        allocator: &mut PageAllocator,
        backend: &impl ReadAt,
        txn_id: u64,
    ) -> Result<CowResult, StorageError>
}
```

Algorithm:
1. Traverse from root to the target leaf, recording the path (page IDs and child indices).
2. Insert the cell into the leaf. If the leaf has space, create a CoW copy of the leaf + path.
3. If the leaf is full, split it. The split produces two new leaf pages and a median key to promote.
4. Promote the median key up the path (inserting into parent interior pages), splitting interior pages if necessary.
5. If the root splits, create a new root interior page.
6. Update leaf `prev_leaf`/`next_leaf` pointers for split leaves and their neighbors.

**⚠ Pitfall — leaf link maintenance:** When a leaf splits, the new right leaf's `next_leaf` points to the old leaf's original `next_leaf`. The old leaf's `next_leaf` points to the new right leaf. The neighbor page (if any) must also be CoW-copied to update its `prev_leaf` pointer. Track these in the `CowResult::freed_pages` and `new_pages`.

**Verify:** `cargo check`

### 7.6 — Implement B-tree delete

In `src/storage/btree/delete.rs`:

```rust
impl BTree {
    /// Deletes a key from the B-tree.
    /// Returns CowResult with the new root, or None if the key was not found.
    pub fn delete(
        &self,
        root: PageId,
        key: &[u8],
        pool: &mut BufferPool,
        allocator: &mut PageAllocator,
        backend: &impl ReadAt,
        txn_id: u64,
    ) -> Result<Option<CowResult>, StorageError>
}
```

Algorithm:
1. Traverse from root to the target leaf.
2. Remove the cell from the leaf. If the leaf still meets minimum fill, CoW copy the path.
3. If the leaf is underfull, attempt to borrow from a sibling. If borrowing isn't possible, merge with a sibling.
4. Merging may cascade up the tree (removing a separator key from the parent).
5. If the root becomes empty (single child), the child becomes the new root (tree height decreases).
6. Update leaf links for merged/rebalanced leaves.

**⚠ Design note on minimum fill:** For CoW B-trees, strict minimum fill enforcement is less critical than for in-place B-trees because CoW already has write amplification. A simpler implementation may skip merging entirely in v1 (accepting lower space utilization) and only implement borrowing. Document this choice if taken, and add a TODO for merge support.

**Verify:** `cargo check`

### 7.7 — Unit tests for B-tree operations

This is the most critical test suite in the project. Use a `MemoryBackend` with a `BufferPool`.

Test:
- **Empty tree:** Search returns None. Range scan returns empty.
- **Single insert + search:** Insert one key-value, search for it → found. Search for a non-existent key → None.
- **Multiple inserts (no splits):** Insert 10 keys into a tree that fits in one leaf page. Search for each → all found. Range scan returns them in key order.
- **Insert causing leaf split:** Insert enough keys to overflow a single leaf page. Verify the tree has 2 leaves + 1 interior root. Search for all keys → all found.
- **Insert causing multi-level split:** Insert enough keys to force interior page splits. Verify tree height increases correctly. All keys searchable.
- **Delete from non-split tree:** Insert 5 keys, delete 1, verify the deleted key returns None, remaining keys still present.
- **Delete causing underflow (if merge implemented):** Insert enough keys to fill 2 leaves, delete keys until one leaf is underfull, verify merge or borrow occurs.
- **Range scan:** Insert 100 sequential keys, range scan [20, 40] → returns 21 keys.
- **Range scan across leaf boundaries:** Verify the cursor correctly follows `next_leaf` pointers.
- **Range scan with open end:** Scan from key 50 to end → returns all keys >= 50.
- **Prefix scan (for adjacency queries):** Insert adjacency keys with shared prefix, scan by prefix → returns correct subset.
- **Key-only B-tree:** Insert keys with empty values (for index B-trees). Search returns `Some(vec![])`.
- **Duplicate key update:** Insert key K with value V1, then insert K with value V2. Search returns V2.
- **Large value (overflow):** Insert a key with a value > 256 bytes. The leaf cell uses overflow. Search returns the full value via overflow chain reconstruction.
- **CoW verification:** After insert, the old root page is not modified. Verify by reading the old root from the backend directly and checking it's unchanged.
- **Freed pages:** Insert, then insert again (causing CoW). Verify `CowResult::freed_pages` contains the old pages.
- **Sort order correctness:** Insert keys in random order. Range scan over the entire tree returns keys in sorted order.

**Verify:** `cargo test -- storage::btree` passes.

### 7.8 — B-tree stress test

Insert 10,000 keys with random values, then:
1. Verify all 10,000 keys are searchable.
2. Delete 5,000 random keys, verify the other 5,000 are still searchable.
3. Range scan the entire tree — returns exactly the 5,000 remaining keys in sorted order.

This test should be marked `#[ignore]` if it takes more than a few seconds, with a comment explaining it's a stress test.

**Verify:** `cargo test -- storage::btree::stress` passes (or is `#[ignore]`d with documented reason).

---

## Phase 8: Commit Protocol and Database Lifecycle (`src/storage/mod.rs`)

### 8.1 — Implement StorageEngine

Tie all components together in `src/storage/mod.rs`:

```rust
pub struct StorageEngine<B: StorageBackend> {
    backend: B,
    buffer_pool: BufferPool,
    allocator: PageAllocator,
    active_superblock: Superblock,
    active_slot: u8,           // 0 or 1
    page_size: usize,
    btree: BTree,
}
```

Implement:
- `pub fn create(backend: B, config: StorageEngineConfig) -> Result<Self, StorageError>` — creates a new database file, writes identity header and initial superblocks.
- `pub fn open(backend: B, config: StorageEngineConfig) -> Result<Self, StorageError>` — opens an existing database, validates headers, selects active superblock.
- `pub fn current_snapshot(&self) -> Snapshot` — returns the current snapshot from the active superblock.
- `pub fn page_size(&self) -> usize`

```rust
pub struct StorageEngineConfig {
    pub page_size: usize,          // default: 4096
    pub buffer_pool_frames: usize, // default: 1024, min: 64
    pub application_id: u32,       // default: 0
}
```

**Verify:** `cargo check`

### 8.2 — Implement the commit protocol

```rust
impl<B: StorageBackend> StorageEngine<B> {
    /// Commits a set of B-tree mutations to disk.
    /// Implements the 2-fsync commit protocol per 008 §13.
    pub fn commit(
        &mut self,
        new_roots: SnapshotRoots,
        freed_pages: Vec<(u64, PageId)>,
        new_total_pages: u64,
    ) -> Result<Snapshot, StorageError>
}
```

The commit protocol (per `008-file-format-spec.md` §13):

**Phase 1 — Write new data pages:** All new/modified pages are already in the buffer pool as dirty frames. Flush all dirty pages.

**Phase 2 — Insert freed pages into Page Freelist:** Insert each `(freed_txn_id, page_id)` into the Page Freelist B-tree (which may itself produce new pages and freed pages). Record any secondary freed pages for deferral.

**Phase 3 — First fsync:** If the file was extended (`new_total_pages > active_superblock.total_pages`), call `backend.sync_all()`. Otherwise, call `backend.sync_data()`.

**Phase 4 — Write new superblock:** Determine the inactive slot (opposite of `active_slot`). Construct a new superblock with `transaction_id + 1`, the new root page IDs, and updated `total_pages`. Compute its checksum. Write to the inactive slot's file offset.

**Phase 5 — Second fsync:** Call `backend.sync_data()`.

**Phase 6 — Update internal state:** Set `active_superblock` to the new superblock, flip `active_slot`. Move secondary freed pages to `allocator.deferred_freed`.

Return the new `Snapshot`.

**Verify:** `cargo check`

### 8.3 — Implement MVCC-safe page reclamation

Add to the `StorageEngine`:

```rust
/// Returns pages from the Page Freelist that are safe to reclaim
/// (freed before the given oldest active reader transaction ID).
pub fn reclaimable_pages(
    &mut self,
    oldest_reader_txn: u64,
) -> Result<Vec<PageId>, StorageError>
```

Scans the Page Freelist B-tree for keys where `freed_txn_id < oldest_reader_txn`. These pages can be reused.

Integrate this with `allocator.allocate_page()`:
```rust
/// Allocates a page, preferring reclaimable free pages over file extension.
pub fn allocate_page(
    &mut self,
    oldest_reader_txn: u64,
) -> Result<PageId, StorageError>
```

**Verify:** `cargo check`

### 8.4 — Unit tests for StorageEngine lifecycle

Test with `MemoryBackend`:
- **Create:** Create a new StorageEngine. Verify the backend contains the correct file structure (identity header at offset 0, two superblocks, initial root pages).
- **Open:** Create, then open the same backend. Verify the snapshot matches the initial state.
- **Commit:** Create, insert a key into the Node Store B-tree, commit. Verify the superblock's `transaction_id` incremented. Verify the key is searchable via the new snapshot's root.
- **Multiple commits:** Perform 5 sequential commits. Verify `transaction_id` increments correctly.
- **Superblock alternation:** After each commit, verify the active slot alternates between 0 and 1.

**Verify:** `cargo test -- storage::engine` passes (or equivalent test module name).

---

## Phase 9: Crash Recovery Tests

### 9.1 — Simulated crash: interrupted data page write

1. Create a database with some initial data (insert keys, commit).
2. Begin a new transaction: insert new keys, write new pages to the buffer pool, flush data pages to the backend.
3. **Simulate crash:** Do NOT write the new superblock. Do NOT fsync.
4. Open the database from the same backend bytes.
5. Verify the database sees only the data from step 1. The step 2 data is not visible.

This verifies that uncommitted data pages are invisible because the old superblock is still active.

**Verify:** Test passes.

### 9.2 — Simulated crash: interrupted superblock write

1. Create a database, commit some data.
2. Write new data pages and fsync them.
3. Write a new superblock but corrupt it (e.g., write partial data or flip a checksum bit).
4. Open the database. The startup procedure should select the other (valid) superblock.
5. Verify the database sees the data from step 1 (the last fully committed state).

**Verify:** Test passes.

### 9.3 — Simulated crash: crash after first fsync but before superblock write

1. Create a database, commit data (txn 1).
2. Perform new mutations. Flush new data pages. Call `sync_data()` (first fsync complete).
3. **Simulate crash:** Do not write the new superblock.
4. Open the database. Old superblock is active.
5. Verify the database sees txn 1 data only.

The new data pages exist on disk but are unreachable (no superblock points to them). They are "leaked" space, recoverable by `compact()`.

**Verify:** Test passes.

### 9.4 — Recovery with one valid superblock

1. Create a database and commit 3 transactions.
2. Manually corrupt superblock A (flip bytes in the checksum area).
3. Open the database. Should succeed using superblock B.
4. Verify the data matches the most recent commit whose superblock survived.

**Verify:** Test passes.

---

## Phase 10: Integration Tests

### 10.1 — End-to-end: insert, commit, reopen, read

1. Create a database file (using `FileBackend` with `tempfile`).
2. Insert 50 nodes into the Node Store B-tree.
3. Commit.
4. Close (drop) the `StorageEngine`.
5. Reopen the same file.
6. Verify all 50 nodes are searchable.

This tests the full persistence path.

**Verify:** Test passes.

### 10.2 — Multi-transaction persistence

1. Create a database.
2. Transaction 1: insert nodes 1–10, commit.
3. Transaction 2: insert nodes 11–20, delete node 5, commit.
4. Transaction 3: insert nodes 21–30, commit.
5. Close and reopen.
6. Verify: nodes 1–4, 6–30 are present. Node 5 is absent.

**Verify:** Test passes.

### 10.3 — File growth under sustained inserts

1. Create a database with a small initial size.
2. Insert 1,000 records (enough to trigger multiple file extensions).
3. Verify all records are searchable.
4. Verify the file size grew in increments matching the growth strategy.

**Verify:** Test passes.

### 10.4 — Overflow record round-trip

1. Insert a node with a property bag larger than 256 bytes (triggering overflow pages).
2. Commit.
3. Close and reopen.
4. Read the node back. Verify the property bag is fully reconstructed from the overflow chain.

**Verify:** Test passes.

---

## Phase 11: Final Verification

### 11.1 — Full std build

```
cargo check
```

Must succeed with zero errors.

### 11.2 — no_std verification (no regressions)

```
cargo check --no-default-features --features alloc
```

Must succeed. The storage module is not compiled, but no other module is broken.

### 11.3 — Full test suite

```
cargo test
```

All tests pass, zero failures. No `#[ignore]` without a documented reason.

### 11.4 — Clippy

```
cargo clippy --all-targets --all-features -- -D warnings
```

Zero warnings.

### 11.5 — Documentation

```
cargo doc --no-deps
```

Zero warnings. Every `pub` item in `src/storage/` has a doc comment.

### 11.6 — Compile-time assertions

Verify that existing `ConstraintValidator` and `InferenceRule` Send+Sync assertions (from Task 22) still pass. No regressions.

### 11.7 — Review against design documents

Manually verify:
- Page header layout matches `008-file-format-spec.md` §5 (24 bytes, correct offsets).
- Interior page layout matches `008-file-format-spec.md` §7.
- Leaf page layout matches `008-file-format-spec.md` §8.
- Overflow page layout matches `008-file-format-spec.md` §9.
- All 8 B-tree root page IDs are stored in the superblock per `012-design-document.md` §19.1.
- Schema Store key encoding prefixes match `012-design-document.md` §19.2 (0x01–0x06).
- Commit protocol follows the 2-fsync sequence per `008-file-format-spec.md` §13.
- Key encoding is big-endian; value encoding is little-endian per design decisions G4, G5.
- Buffer pool capacity minimum is 64 frames per `012-design-document.md` §9.4.
- Growth increment strategy matches `008-file-format-spec.md` §12.3.

Document any intentional deviations from the spec in the completion report.

---

## Post-Completion

Produce a completion report following the format in the master project prompt's Instance Rules section. Include the verification evidence from Phase 11. Note:
- Whether B-tree merge (delete underflow) was fully implemented or deferred.
- Whether initial B-tree roots share a single empty page or get individual pages.
- The approach chosen for buffer pool read-path vs write-path `fetch_page`.
- Any performance observations from the stress test.
- Context for Task 17/25 (query engine): which `StorageEngine` methods constitute the interface that the query engine will build on.
