//! CoW B+ tree operations.
//!
//! Provides search, insert, delete, and range scan over the eight
//! logical B-trees stored in the database file. All mutations use
//! copy-on-write (CoW) to preserve snapshot isolation.
//!
//! Operations are stateless: the root page ID is passed in and a
//! new root (for mutations) or result (for reads) is returned.

use crate::error::StorageError;
use crate::storage::buffer_pool::BufferPool;
use crate::storage::page::PageId;

pub mod cow;
pub mod cursor;
pub mod delete;
pub mod insert;
pub mod search;

/// Configuration for B-tree operations.
#[derive(Clone, Debug)]
pub struct BTreeConfig {
    /// Page size in bytes.
    pub page_size: usize,
}

/// A logical B+ tree.
///
/// Operations take a root `PageId` and return results or a new root.
/// This struct carries only configuration — no mutable state.
pub struct BTree {
    /// Configuration for this B-tree.
    pub config: BTreeConfig,
}

/// Tracks one step of the traversal path from root to leaf.
pub(crate) struct PathEntry {
    /// The page ID of the interior node at this level.
    pub(crate) page_id: PageId,
    /// Index of the child pointer that was followed (cell index, or `cells.len()` for right_child).
    pub(crate) child_index: usize,
}

impl BTree {
    /// Creates a new `BTree` with the given configuration.
    pub fn new(config: BTreeConfig) -> Self {
        Self { config }
    }

    /// Traverses from `root` to the leaf that should contain `key`,
    /// recording the path of interior nodes visited.
    ///
    /// Returns `(path, leaf_page_id)` where path contains one entry per
    /// interior level traversed. The caller is responsible for fetching
    /// and parsing the leaf page at `leaf_page_id`.
    pub(crate) fn traverse_to_leaf<B: crate::backend::ReadAt + crate::backend::WriteAt + crate::backend::Durability>(
        &self,
        root: PageId,
        key: &[u8],
        pool: &mut BufferPool,
        backend: &mut B,
    ) -> Result<(Vec<PathEntry>, PageId), StorageError> {
        use crate::storage::page::interior::InteriorPage;
        use crate::storage::page::PageType;

        let mut path: Vec<PathEntry> = Vec::new();
        let mut current = root;

        loop {
            let frame = pool.fetch_page(current, backend)?;
            let page_data = pool.get_page_data(frame);
            let page_type = PageType::try_from(page_data[8]).map_err(|v| StorageError {
                message: format!("unknown page type: {v:#04x}"),
                source: None,
            })?;

            match page_type {
                PageType::Interior => {
                    let interior = InteriorPage::parse(page_data, self.config.page_size)?;
                    let cells = interior.cells();
                    let pos = cells.partition_point(|c| c.key.as_slice() <= key);
                    let child = if pos == cells.len() {
                        interior.right_child
                    } else {
                        cells[pos].left_child
                    };
                    pool.unpin_page(frame, false);
                    path.push(PathEntry {
                        page_id: current,
                        child_index: pos,
                    });
                    current = child;
                }
                PageType::Leaf => {
                    pool.unpin_page(frame, false);
                    return Ok((path, current));
                }
                _ => {
                    pool.unpin_page(frame, false);
                    return Err(StorageError {
                        message: format!("expected Interior or Leaf, got {:?}", page_type),
                        source: None,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::WriteAt;
    use crate::storage::allocator::PageAllocator;
    use crate::storage::buffer_pool::{BufferPool, MIN_BUFFER_POOL_FRAMES};
    use crate::storage::page::{DEFAULT_PAGE_SIZE, PageId};
    use crate::storage::test_utils::TestBackend;

    fn setup() -> (TestBackend, BufferPool, PageAllocator, BTree) {
        let mut backend = TestBackend::new();
        // Pre-size for some pages
        backend.set_len(DEFAULT_PAGE_SIZE as u64 * 1000).unwrap();
        let pool = BufferPool::new(MIN_BUFFER_POOL_FRAMES, DEFAULT_PAGE_SIZE);
        let allocator = PageAllocator::new(2, DEFAULT_PAGE_SIZE); // start after superblock pages
        let btree = BTree::new(BTreeConfig {
            page_size: DEFAULT_PAGE_SIZE,
        });
        (backend, pool, allocator, btree)
    }

    fn make_key(n: u64) -> [u8; 8] {
        n.to_be_bytes()
    }

    fn make_value(n: u64) -> Vec<u8> {
        format!("value-{n}").into_bytes()
    }

    #[test]
    fn empty_tree_search_returns_none() {
        let (mut backend, mut pool, _, btree) = setup();
        let result = btree.search(PageId::NULL, &make_key(1), &mut pool, &mut backend).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn empty_tree_range_scan_returns_empty() {
        let (mut backend, mut pool, _, btree) = setup();
        let mut cursor = cursor::BTreeCursor::new(
            PageId::NULL, &make_key(0), None, &mut pool, &mut backend, &btree.config,
        ).unwrap();
        let next = cursor.next(&mut pool, &mut backend, &btree.config).unwrap();
        assert!(next.is_none());
    }

    #[test]
    fn single_insert_and_search() {
        let (mut backend, mut pool, mut alloc, btree) = setup();
        let key = make_key(42);
        let value = make_value(42);

        let result = btree.insert(PageId::NULL, &key, &value, &mut pool, &mut alloc, &mut backend, 1).unwrap();
        assert!(!result.new_root.is_null());

        let found = btree.search(result.new_root, &key, &mut pool, &mut backend).unwrap();
        assert_eq!(found, Some(value));

        // Search for non-existent key
        let not_found = btree.search(result.new_root, &make_key(99), &mut pool, &mut backend).unwrap();
        assert!(not_found.is_none());
    }

    #[test]
    fn multiple_inserts_no_splits() {
        let (mut backend, mut pool, mut alloc, btree) = setup();
        let mut root = PageId::NULL;

        for i in 0..10u64 {
            let r = btree.insert(root, &make_key(i), &make_value(i), &mut pool, &mut alloc, &mut backend, 1).unwrap();
            root = r.new_root;
        }

        // Search for all keys
        for i in 0..10u64 {
            let found = btree.search(root, &make_key(i), &mut pool, &mut backend).unwrap();
            assert_eq!(found, Some(make_value(i)), "key {i} not found");
        }
    }

    #[test]
    fn range_scan_returns_sorted_keys() {
        let (mut backend, mut pool, mut alloc, btree) = setup();
        let mut root = PageId::NULL;

        // Insert in reverse order
        for i in (0..10u64).rev() {
            let r = btree.insert(root, &make_key(i), &make_value(i), &mut pool, &mut alloc, &mut backend, 1).unwrap();
            root = r.new_root;
        }

        // Range scan should return all in order
        let mut cursor = cursor::BTreeCursor::new(
            root, &make_key(0), None, &mut pool, &mut backend, &btree.config,
        ).unwrap();

        let mut results = Vec::new();
        while let Some((k, v)) = cursor.next(&mut pool, &mut backend, &btree.config).unwrap() {
            results.push((k, v));
        }
        assert_eq!(results.len(), 10);
        for i in 0..10u64 {
            assert_eq!(results[i as usize].0, make_key(i).to_vec());
        }
    }

    #[test]
    fn insert_causing_leaf_split() {
        let (mut backend, mut pool, mut alloc, btree) = setup();
        let mut root = PageId::NULL;

        // Insert enough keys to overflow a single leaf page.
        // Each cell: 4 + 8 (key) + ~10 (value) = ~22 bytes + 2 ptr = ~24 bytes
        // Available leaf: 4096 - 44 = 4052. ~4052/24 = ~168 cells. Use 200 to be sure.
        for i in 0..200u64 {
            let r = btree.insert(root, &make_key(i), &make_value(i), &mut pool, &mut alloc, &mut backend, 1).unwrap();
            root = r.new_root;
        }

        // All keys should be searchable
        for i in 0..200u64 {
            let found = btree.search(root, &make_key(i), &mut pool, &mut backend).unwrap();
            assert!(found.is_some(), "key {i} not found after split");
        }
    }

    #[test]
    fn insert_causing_multi_level_split() {
        let (mut backend, mut pool, mut alloc, btree) = setup();
        let mut root = PageId::NULL;

        // Insert enough keys to require multi-level splits (interior pages too).
        // ~200 keys per leaf, ~200 children per interior → need ~200*200 = 40000 for 3 levels
        // Use 1000 keys — enough for at least 2 levels.
        for i in 0..1000u64 {
            let r = btree.insert(root, &make_key(i), &make_value(i), &mut pool, &mut alloc, &mut backend, 1).unwrap();
            root = r.new_root;
        }

        // All keys searchable
        for i in 0..1000u64 {
            let found = btree.search(root, &make_key(i), &mut pool, &mut backend).unwrap();
            assert!(found.is_some(), "key {i} not found in multi-level tree");
        }

        // Range scan returns all in order
        let mut cursor = cursor::BTreeCursor::new(
            root, &make_key(0), None, &mut pool, &mut backend, &btree.config,
        ).unwrap();
        let mut count = 0u64;
        let mut prev_key: Option<Vec<u8>> = None;
        while let Some((k, _)) = cursor.next(&mut pool, &mut backend, &btree.config).unwrap() {
            if let Some(pk) = &prev_key {
                assert!(k > *pk, "keys not in order");
            }
            prev_key = Some(k);
            count += 1;
        }
        assert_eq!(count, 1000);
    }

    #[test]
    fn delete_from_single_leaf() {
        let (mut backend, mut pool, mut alloc, btree) = setup();
        let mut root = PageId::NULL;

        for i in 0..5u64 {
            let r = btree.insert(root, &make_key(i), &make_value(i), &mut pool, &mut alloc, &mut backend, 1).unwrap();
            root = r.new_root;
        }

        // Delete key 2
        let del = btree.delete(root, &make_key(2), &mut pool, &mut alloc, &mut backend, 2).unwrap();
        assert!(del.is_some());
        root = del.unwrap().new_root;

        // Key 2 should be gone
        assert!(btree.search(root, &make_key(2), &mut pool, &mut backend).unwrap().is_none());
        // Others still present
        for i in [0, 1, 3, 4] {
            assert!(btree.search(root, &make_key(i), &mut pool, &mut backend).unwrap().is_some());
        }
    }

    #[test]
    fn delete_nonexistent_key_returns_none() {
        let (mut backend, mut pool, mut alloc, btree) = setup();
        let r = btree.insert(PageId::NULL, &make_key(1), &make_value(1), &mut pool, &mut alloc, &mut backend, 1).unwrap();
        let del = btree.delete(r.new_root, &make_key(99), &mut pool, &mut alloc, &mut backend, 2).unwrap();
        assert!(del.is_none());
    }

    #[test]
    fn range_scan_with_bounds() {
        let (mut backend, mut pool, mut alloc, btree) = setup();
        let mut root = PageId::NULL;

        for i in 0..100u64 {
            let r = btree.insert(root, &make_key(i), &make_value(i), &mut pool, &mut alloc, &mut backend, 1).unwrap();
            root = r.new_root;
        }

        // Range [20, 40) exclusive end
        let mut cursor = cursor::BTreeCursor::new(
            root, &make_key(20), Some(&make_key(40)), &mut pool, &mut backend, &btree.config,
        ).unwrap();

        let mut results = Vec::new();
        while let Some((k, _)) = cursor.next(&mut pool, &mut backend, &btree.config).unwrap() {
            results.push(k);
        }
        assert_eq!(results.len(), 20); // keys 20..39
        assert_eq!(results[0], make_key(20).to_vec());
        assert_eq!(results[19], make_key(39).to_vec());
    }

    #[test]
    fn range_scan_open_end() {
        let (mut backend, mut pool, mut alloc, btree) = setup();
        let mut root = PageId::NULL;

        for i in 0..100u64 {
            let r = btree.insert(root, &make_key(i), &make_value(i), &mut pool, &mut alloc, &mut backend, 1).unwrap();
            root = r.new_root;
        }

        // Scan from key 90 to end
        let mut cursor = cursor::BTreeCursor::new(
            root, &make_key(90), None, &mut pool, &mut backend, &btree.config,
        ).unwrap();

        let mut count = 0;
        while cursor.next(&mut pool, &mut backend, &btree.config).unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, 10); // keys 90..99
    }

    #[test]
    fn key_only_btree() {
        let (mut backend, mut pool, mut alloc, btree) = setup();
        let mut root = PageId::NULL;

        // Insert keys with empty values (index B-tree pattern)
        for i in 0..20u64 {
            let r = btree.insert(root, &make_key(i), &[], &mut pool, &mut alloc, &mut backend, 1).unwrap();
            root = r.new_root;
        }

        let found = btree.search(root, &make_key(10), &mut pool, &mut backend).unwrap();
        assert_eq!(found, Some(vec![]));
    }

    #[test]
    fn duplicate_key_update() {
        let (mut backend, mut pool, mut alloc, btree) = setup();
        let key = make_key(42);

        let r1 = btree.insert(PageId::NULL, &key, b"v1", &mut pool, &mut alloc, &mut backend, 1).unwrap();
        let r2 = btree.insert(r1.new_root, &key, b"v2", &mut pool, &mut alloc, &mut backend, 2).unwrap();

        let found = btree.search(r2.new_root, &key, &mut pool, &mut backend).unwrap();
        assert_eq!(found, Some(b"v2".to_vec()));
    }

    #[test]
    fn cow_old_root_unchanged() {
        let (mut backend, mut pool, mut alloc, btree) = setup();
        let key1 = make_key(1);
        let key2 = make_key(2);

        let r1 = btree.insert(PageId::NULL, &key1, b"v1", &mut pool, &mut alloc, &mut backend, 1).unwrap();
        let old_root = r1.new_root;

        // Flush to backend so we can read the old root later
        pool.flush_all_dirty(&mut backend).unwrap();

        let r2 = btree.insert(old_root, &key2, b"v2", &mut pool, &mut alloc, &mut backend, 2).unwrap();
        pool.flush_all_dirty(&mut backend).unwrap();

        // Old root should still only contain key1
        let found = btree.search(old_root, &key1, &mut pool, &mut backend).unwrap();
        assert_eq!(found, Some(b"v1".to_vec()));
        let found2 = btree.search(old_root, &key2, &mut pool, &mut backend).unwrap();
        assert!(found2.is_none());

        // New root should contain both
        assert!(btree.search(r2.new_root, &key1, &mut pool, &mut backend).unwrap().is_some());
        assert!(btree.search(r2.new_root, &key2, &mut pool, &mut backend).unwrap().is_some());
    }

    #[test]
    fn freed_pages_tracked() {
        let (mut backend, mut pool, mut alloc, btree) = setup();
        let r1 = btree.insert(PageId::NULL, &make_key(1), b"v1", &mut pool, &mut alloc, &mut backend, 1).unwrap();
        assert!(r1.freed_pages.is_empty()); // first insert into empty tree

        let r2 = btree.insert(r1.new_root, &make_key(2), b"v2", &mut pool, &mut alloc, &mut backend, 2).unwrap();
        // The old root leaf was freed (CoW'd)
        assert!(!r2.freed_pages.is_empty());
        assert!(r2.freed_pages.contains(&r1.new_root));
    }

    #[test]
    fn sort_order_random_insert() {
        let (mut backend, mut pool, mut alloc, btree) = setup();
        let mut root = PageId::NULL;

        // Insert in a shuffled order
        let keys: Vec<u64> = vec![50, 10, 90, 30, 70, 20, 80, 40, 60, 0, 100, 5, 95, 15, 85];
        for &k in &keys {
            let r = btree.insert(root, &make_key(k), &make_value(k), &mut pool, &mut alloc, &mut backend, 1).unwrap();
            root = r.new_root;
        }

        // Range scan should return sorted
        let mut cursor = cursor::BTreeCursor::new(
            root, &[0u8; 8], None, &mut pool, &mut backend, &btree.config,
        ).unwrap();

        let mut prev: Option<Vec<u8>> = None;
        let mut count = 0;
        while let Some((k, _)) = cursor.next(&mut pool, &mut backend, &btree.config).unwrap() {
            if let Some(p) = &prev {
                assert!(k > *p, "sort order violation");
            }
            prev = Some(k);
            count += 1;
        }
        assert_eq!(count, keys.len());
    }

    #[test]
    fn range_scan_across_leaf_boundaries() {
        let (mut backend, mut pool, mut alloc, btree) = setup();
        let mut root = PageId::NULL;

        // Insert 200 keys — enough to span multiple leaves
        for i in 0..200u64 {
            let r = btree.insert(root, &make_key(i), &make_value(i), &mut pool, &mut alloc, &mut backend, 1).unwrap();
            root = r.new_root;
        }

        // Full range scan
        let mut cursor = cursor::BTreeCursor::new(
            root, &make_key(0), None, &mut pool, &mut backend, &btree.config,
        ).unwrap();

        let mut count = 0;
        while cursor.next(&mut pool, &mut backend, &btree.config).unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, 200);
    }

    #[test]
    fn delete_after_splits() {
        let (mut backend, mut pool, mut alloc, btree) = setup();
        let mut root = PageId::NULL;

        for i in 0..200u64 {
            let r = btree.insert(root, &make_key(i), &make_value(i), &mut pool, &mut alloc, &mut backend, 1).unwrap();
            root = r.new_root;
        }

        // Delete every other key
        for i in (0..200u64).step_by(2) {
            if let Some(r) = btree.delete(root, &make_key(i), &mut pool, &mut alloc, &mut backend, 2).unwrap() {
                root = r.new_root;
            }
        }

        // Odd keys should still be present
        for i in (1..200u64).step_by(2) {
            assert!(btree.search(root, &make_key(i), &mut pool, &mut backend).unwrap().is_some(), "key {i} missing");
        }
        // Even keys should be gone
        for i in (0..200u64).step_by(2) {
            assert!(btree.search(root, &make_key(i), &mut pool, &mut backend).unwrap().is_none(), "key {i} still present");
        }
    }

    #[test]
    #[ignore = "stress test — takes a few seconds"]
    fn stress_test_10k_keys() {
        let (mut backend, mut pool, mut alloc, btree) = setup();
        // Extend backend for many pages
        backend.set_len(DEFAULT_PAGE_SIZE as u64 * 50000).unwrap();
        let mut root = PageId::NULL;

        // Insert 10,000 keys
        for i in 0..10_000u64 {
            let r = btree.insert(root, &make_key(i), &make_value(i), &mut pool, &mut alloc, &mut backend, 1).unwrap();
            root = r.new_root;
        }

        // Verify all searchable
        for i in 0..10_000u64 {
            assert!(btree.search(root, &make_key(i), &mut pool, &mut backend).unwrap().is_some(), "key {i} not found");
        }

        // Delete 5,000 even keys
        for i in (0..10_000u64).step_by(2) {
            if let Some(r) = btree.delete(root, &make_key(i), &mut pool, &mut alloc, &mut backend, 2).unwrap() {
                root = r.new_root;
            }
        }

        // Verify remaining 5,000 odd keys
        for i in (1..10_000u64).step_by(2) {
            assert!(btree.search(root, &make_key(i), &mut pool, &mut backend).unwrap().is_some(), "key {i} missing after delete");
        }

        // Range scan returns exactly 5,000
        let mut cursor = cursor::BTreeCursor::new(
            root, &[0u8; 8], None, &mut pool, &mut backend, &btree.config,
        ).unwrap();
        let mut count = 0;
        while cursor.next(&mut pool, &mut backend, &btree.config).unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, 5000);
    }
}
