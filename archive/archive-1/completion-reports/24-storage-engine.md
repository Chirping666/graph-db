# Completion Report: Task 24 — Implement Persistent Storage Engine & File Format

**Status:** COMPLETE
**Date:** 2026-03-24
**Task:** 24 (Storage Engine)
**Modules:** `src/storage/`, updates to `src/lib.rs` and `Cargo.toml`

---

## Done Criterion Assessment

| # | Criterion | Status | Evidence |
|---|-----------|--------|----------|
| 1 | `src/storage/mod.rs` exists with module structure and public re-exports | PASS | Module root with 7 submodules, `StorageEngine`, `StorageEngineConfig` |
| 2 | `src/storage/page/` implements all 4 page types per 008 §§5-10 | PASS | `header.rs`, `interior.rs`, `leaf.rs`, `overflow.rs`, `free.rs` |
| 3 | `src/storage/buffer_pool.rs` implements buffer pool with clock eviction | PASS | `BufferPool`, `PageFrame`, clock algorithm, fetch/unpin/flush/new_page |
| 4 | `src/storage/allocator.rs` implements page allocation with growth | PASS | `PageAllocator`, scaling increments, `extend_file` |
| 5 | `src/storage/btree/` implements CoW B+ tree search, insert, delete, range scan | PASS | `search.rs`, `insert.rs`, `delete.rs`, `cursor.rs`, `cow.rs` |
| 6 | `src/storage/format.rs` implements identity header and dual-superblock | PASS | `FileIdentityHeader`, `Superblock`, `create_database_file`, `open_database_file` |
| 7 | `src/storage/serialization.rs` implements records, properties, key encoding | PASS | All 8 key types, `NodeRecord`, `EdgeRecord`, property serialization, schema store |
| 8 | `src/storage/snapshot.rs` implements snapshot with root page IDs | PASS | `Snapshot`, `SnapshotRoots`, `From<&Superblock>` |
| 9 | Commit protocol implements 2-fsync sequence per 008 §13 | PASS | `StorageEngine::commit()` with data flush → fsync → superblock write → fsync |
| 10 | Crash recovery: simulated crash produces no corruption on reopen | PASS | 4 crash recovery tests pass |
| 11 | `cargo check` succeeds | PASS | |
| 12 | `cargo check --no-default-features --features alloc` succeeds | PASS | |
| 13 | `cargo test` passes — all tests green | PASS | 234 tests pass (231 lib + 3 integration), 1 ignored (stress test) |
| 14 | `cargo clippy --all-targets --all-features -- -D warnings` — zero warnings | PASS | |
| 15 | `cargo doc --no-deps` — zero warnings | PASS | |
| 16 | Tests cover all required scenarios | PASS | See test summary below |
| 17 | Send+Sync assertions still pass (no regressions) | PASS | All 115 pre-existing tests pass |

---

## Test Summary

```
cargo test
  234 passed (231 lib + 3 integration), 0 failed, 1 ignored

  Pre-existing (Tasks 22-23):             115 tests
  Phase 1 — Page types:                    38 tests
  Phase 2 — File format:                   14 tests
  Phase 3 — Snapshot:                       3 tests
  Phase 4 — Buffer pool:                    9 tests
  Phase 5 — Serialization:                 19 tests
  Phase 6 — Allocator:                      6 tests
  Phase 7 — B-tree operations:             18 tests (+ 1 ignored stress test)
  Phase 8 — StorageEngine lifecycle:        5 tests
  Phase 9 — Crash recovery:                 4 tests
  Phase 10 — Integration (FileBackend):     3 tests
```

---

## Deliverables

| File | Description |
|------|-------------|
| `Cargo.toml` | Added `crc32fast`, `xxhash-rust` dependencies |
| `src/lib.rs` | Added `pub mod storage` (std-gated) |
| `src/storage/mod.rs` | Module root, `StorageEngine`, `StorageEngineConfig`, `map_hal_err`, `TestBackend` |
| `src/storage/page/mod.rs` | `PageId`, `PageType`, page size constants |
| `src/storage/page/header.rs` | `CommonPageHeader` (24-byte, CRC32C) |
| `src/storage/page/interior.rs` | `InteriorPage`, `InteriorCell` (slotted page layout) |
| `src/storage/page/leaf.rs` | `LeafPage`, `LeafCell`, `LeafCellValue` (overflow sentinel) |
| `src/storage/page/overflow.rs` | `OverflowPage` (chain read/write) |
| `src/storage/page/free.rs` | `FreePage` |
| `src/storage/btree/mod.rs` | `BTree`, `BTreeConfig`, comprehensive test suite |
| `src/storage/btree/search.rs` | Point lookup (root → leaf traversal) |
| `src/storage/btree/insert.rs` | CoW insert with leaf/interior split propagation |
| `src/storage/btree/delete.rs` | CoW delete (no merge in v1) |
| `src/storage/btree/cursor.rs` | Stack-based range scan cursor |
| `src/storage/btree/cow.rs` | `CowResult` (new_root, freed_pages, new_pages) |
| `src/storage/buffer_pool.rs` | `BufferPool` with clock eviction |
| `src/storage/allocator.rs` | `PageAllocator` with scaling growth increments |
| `src/storage/format.rs` | `FileIdentityHeader`, `Superblock`, create/open/select |
| `src/storage/serialization.rs` | All key encodings, `NodeRecord`, `EdgeRecord`, property serialization |
| `src/storage/snapshot.rs` | `Snapshot`, `SnapshotRoots` |
| `tests/storage_integration.rs` | 3 integration tests with FileBackend + tempfile |

---

## Notable Decisions

1. **Spec corrections**: The implementation follows `008-file-format-spec.md` over the checklist where they conflict:
   - Identity header magic: `"EmbedGraph\r\n\x1A\n"` (14 bytes)
   - Superblock checksum: xxHash3 (u64), not CRC32C
   - Superblock layout: identity header at bytes 0-31, no separate magic
   - Page size encoding: `page_size_raw: u16 LE` with SQLite convention
   - Initial B-tree roots: only Schema Store gets page 2; others start at PageId(0)

2. **B-tree delete: no merge (v1)**: Underfull leaves are left as-is after deletion. This trades some space utilization for simplicity in a CoW B-tree where write amplification is already inherent. TODO: implement merge in v2.

3. **Cursor uses stack-based navigation, not leaf links**: The `BTreeCursor` maintains a stack of interior page positions and navigates to the next leaf by walking up the stack. This avoids relying on `next_leaf`/`prev_leaf` pointers which become stale in a CoW B-tree when pages are replaced without updating neighbors. This is more correct but slightly slower (O(log N) per leaf transition vs O(1)).

4. **Leaf link updates skipped**: When a leaf is CoW-copied or split, neighbor leaves' prev/next pointers are NOT updated. This is safe because the cursor doesn't use them, and old pages remain readable on disk in the CoW model.

5. **Buffer pool `fetch_page` takes `&mut B: StorageBackend`**: Allows flushing dirty victims on eviction. No separate read-only path — the buffer pool always requires write access.

6. **TestBackend for unit tests**: A minimal in-memory `StorageBackend` implementation in `#[cfg(test)]` replaces the not-yet-implemented `MemoryBackend` (Task 27).

---

## Context for Next Task (Task 25: Query Engine)

The `StorageEngine<B>` provides the interface that the database engine layer builds upon:

- `StorageEngine::create(backend, config)` / `open(backend, config)` — lifecycle
- `engine.current_snapshot()` — returns `Snapshot` with all B-tree root page IDs
- `engine.search(root, key)` — point lookup in any B-tree
- `engine.insert(root, key, value, txn_id)` → `CowResult` — CoW insert
- `engine.delete(root, key, txn_id)` → `Option<CowResult>` — CoW delete
- `engine.commit(new_roots, freed_pages)` → `Snapshot` — 2-fsync commit
- `BTreeCursor::new(root, start, end, pool, backend, config)` — range scan
- All serialization functions for key encoding and record formats

The database engine (Task 25) will:
- Manage the write mutex and read transaction snapshots
- Build `ChangeSet` and run `ConstraintValidator`s at commit time
- Use `WriteBuffer` for read-your-own-writes overlay
- Maintain the `SchemaCache` from the Schema Store B-tree

---

## Residual Concerns

1. **Page Freelist not actively used**: Freed pages are tracked in `CowResult` but not yet inserted into the Page Freelist B-tree during commit. The `commit()` method accepts freed pages but defers freelist insertion to the db layer (Task 25). This means page reclamation is not yet functional.

2. **No overflow handling in insert**: The insert path always uses `LeafCellValue::Inline`. Values exceeding the overflow threshold need to be stored in overflow pages. This should be handled by the db layer when serializing records.

3. **Cursor re-reads interior pages per leaf transition**: The stack-based cursor approach is correct but re-fetches interior pages from the buffer pool each time it advances to a new leaf. For hot buffer pools this is fine (cache hit), but could be optimized by caching interior page data in the cursor.
