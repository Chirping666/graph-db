//! B-tree CoW insert with split propagation.
//!
//! Inserts a key-value pair, splitting leaf and interior pages as needed.
//! All modifications produce new page copies (CoW); old pages are freed.

use alloc::vec::Vec;
use crate::error::StorageError;
use crate::backend::{self, ReadAt, WriteAt};

use crate::storage::allocator::PageAllocator;
use crate::storage::buffer_pool::BufferPool;
use crate::storage::page::interior::{InteriorCell, InteriorPage};
use crate::storage::page::leaf::{LeafCell, LeafCellValue, LeafPage};
use crate::storage::page::overflow::OverflowPage;
use crate::storage::page::PageId;

use super::cow::CowResult;
use super::BTree;

/// An entry to promote into a parent interior page after a split.
struct PromotedKey {
    /// The separator key promoted from the child split.
    key: Vec<u8>,
    /// The left child of this separator (the new left page).
    left_child: PageId,
    /// The right child of this separator (the new right page).
    right_child: PageId,
}

use super::PathEntry;

/// Total leaf page header size: common(24) + subheader(20) = 44.
const LEAF_HEADER_SIZE: usize = 44;

/// Maximum inline cell size for a leaf page.
///
/// Per `008-file-format-spec.md` §8.2, a cell triggers overflow when
/// `total_cell_size > (page_size - 44) / 4`.
fn overflow_cell_threshold(page_size: usize) -> usize {
    (page_size - LEAF_HEADER_SIZE) / 4
}

#[allow(clippy::too_many_arguments)]
impl BTree {
    /// Inserts a key-value pair into the B-tree.
    ///
    /// If the key already exists, its value is replaced. Returns a [`CowResult`]
    /// with the new root page ID and the sets of freed and new pages.
    ///
    /// If `root` is `PageId::NULL` (empty tree), a new leaf root is created.
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure, checksum mismatch, or buffer pool exhaustion.
    pub fn insert<B: ReadAt + WriteAt + backend::Durability>(
        &self,
        root: PageId,
        key: &[u8],
        value: &[u8],
        pool: &mut BufferPool,
        allocator: &mut PageAllocator,
        backend: &mut B,
        txn_id: u64,
    ) -> Result<CowResult, StorageError> {
        let mut freed = Vec::new();
        let mut allocated = Vec::new();

        // Empty tree: create a new leaf root
        if root.is_null() {
            let new_root_id = allocator.allocate_page();
            let cell = self.create_leaf_cell(key, value, pool, allocator, backend, txn_id, &mut allocated)?;
            let cells = [cell];
            let page_data = LeafPage::build(
                new_root_id,
                txn_id,
                &cells,
                PageId::NULL,
                PageId::NULL,
                self.config.page_size,
            )?;
            let frame = pool.new_page(new_root_id, backend)?;
            pool.get_page_data_mut(frame)[..self.config.page_size]
                .copy_from_slice(&page_data);
            pool.unpin_page(frame, true);
            allocated.push(new_root_id);

            return Ok(CowResult {
                new_root: new_root_id,
                freed_pages: freed,
                new_pages: allocated,
            });
        }

        // Traverse to the leaf, recording the path
        let (path, leaf_page_id) = self.traverse_to_leaf(root, key, pool, backend)?;

        let frame = pool.fetch_page(leaf_page_id, backend)?;
        let page_data = pool.get_page_data(frame);
        let mut leaf = LeafPage::parse(page_data, self.config.page_size)?;
        let old_next = leaf.next_leaf;
        let old_prev = leaf.prev_leaf;
        pool.unpin_page(frame, false);

        // Check for duplicate key and replace
        if let Some(existing) = leaf.delete_cell(key) {
            if let LeafCellValue::Overflow { overflow_page_id, .. } = &existing.value {
                self.free_overflow_chain(*overflow_page_id, pool, backend, &mut freed)?;
            }
        }

        let new_cell = self.create_leaf_cell(key, value, pool, allocator, backend, txn_id, &mut allocated)?;

        // Try to insert into the leaf
        if leaf.insert_cell(new_cell.clone(), self.config.page_size) {
            // Fits — CoW the leaf and path
            let new_leaf_id = allocator.allocate_page();
            allocated.push(new_leaf_id);
            freed.push(leaf_page_id);

            let leaf_data = LeafPage::build(
                new_leaf_id,
                txn_id,
                leaf.cells(),
                old_next,
                old_prev,
                self.config.page_size,
            )?;
            let frame = pool.new_page(new_leaf_id, backend)?;
            pool.get_page_data_mut(frame)[..self.config.page_size]
                .copy_from_slice(&leaf_data);
            pool.unpin_page(frame, true);

            let new_root = self.cow_path(
                &path,
                new_leaf_id,
                None,
                pool,
                allocator,
                backend,
                txn_id,
                &mut freed,
                &mut allocated,
            )?;

            return Ok(CowResult {
                new_root,
                freed_pages: freed,
                new_pages: allocated,
            });
        }

        // Leaf is full — must split
        leaf.insert_cell(new_cell, usize::MAX); // force insert for splitting
        let (left_cells, right_cells, split_key) = leaf.split();

        let left_id = allocator.allocate_page();
        let right_id = allocator.allocate_page();
        allocated.push(left_id);
        allocated.push(right_id);
        freed.push(leaf_page_id);

        let left_data = LeafPage::build(
            left_id, txn_id, &left_cells, right_id, old_prev, self.config.page_size,
        )?;
        let right_data = LeafPage::build(
            right_id, txn_id, &right_cells, old_next, left_id, self.config.page_size,
        )?;

        let left_frame = pool.new_page(left_id, backend)?;
        pool.get_page_data_mut(left_frame)[..self.config.page_size]
            .copy_from_slice(&left_data);
        pool.unpin_page(left_frame, true);

        let right_frame = pool.new_page(right_id, backend)?;
        pool.get_page_data_mut(right_frame)[..self.config.page_size]
            .copy_from_slice(&right_data);
        pool.unpin_page(right_frame, true);

        let promoted = PromotedKey {
            key: split_key,
            left_child: left_id,
            right_child: right_id,
        };

        let new_root = self.propagate_split(
            &path,
            promoted,
            pool,
            allocator,
            backend,
            txn_id,
            &mut freed,
            &mut allocated,
        )?;

        Ok(CowResult {
            new_root,
            freed_pages: freed,
            new_pages: allocated,
        })
    }

    /// CoW-copies the interior path from leaf parent up to root, replacing the child pointer.
    fn cow_path<B: ReadAt + WriteAt + backend::Durability>(
        &self,
        path: &[PathEntry],
        new_child: PageId,
        promoted: Option<PromotedKey>,
        pool: &mut BufferPool,
        allocator: &mut PageAllocator,
        backend: &mut B,
        txn_id: u64,
        freed: &mut Vec<PageId>,
        allocated: &mut Vec<PageId>,
    ) -> Result<PageId, StorageError> {
        if path.is_empty() {
            // The leaf is the root — no interior pages to copy
            return Ok(new_child);
        }

        let mut current_child = new_child;

        for i in (0..path.len()).rev() {
            let entry = &path[i];
            let frame = pool.fetch_page(entry.page_id, backend)?;
            let page_data = pool.get_page_data(frame);
            let interior = InteriorPage::parse(page_data, self.config.page_size)?;
            pool.unpin_page(frame, false);

            freed.push(entry.page_id);

            // Rebuild cells with updated child pointer
            let mut cells: Vec<InteriorCell> = interior.cells().to_vec();
            let right_child;

            if entry.child_index == cells.len() {
                // Was following right_child
                right_child = current_child;
            } else {
                cells[entry.child_index].left_child = current_child;
                right_child = interior.right_child;
            }

            let new_id = allocator.allocate_page();
            allocated.push(new_id);

            let page_bytes = InteriorPage::build(
                new_id,
                txn_id,
                &cells,
                right_child,
                self.config.page_size,
            );
            let frame = pool.new_page(new_id, backend)?;
            pool.get_page_data_mut(frame)[..self.config.page_size]
                .copy_from_slice(&page_bytes);
            pool.unpin_page(frame, true);

            current_child = new_id;
        }

        let _ = promoted; // handled in propagate_split instead
        Ok(current_child)
    }

    /// Propagates a split up through the interior path, creating a new root if needed.
    fn propagate_split<B: ReadAt + WriteAt + backend::Durability>(
        &self,
        path: &[PathEntry],
        mut promoted: PromotedKey,
        pool: &mut BufferPool,
        allocator: &mut PageAllocator,
        backend: &mut B,
        txn_id: u64,
        freed: &mut Vec<PageId>,
        allocated: &mut Vec<PageId>,
    ) -> Result<PageId, StorageError> {
        if path.is_empty() {
            // Root was a leaf that split — create new interior root
            let root_id = allocator.allocate_page();
            allocated.push(root_id);
            let cells = [InteriorCell {
                left_child: promoted.left_child,
                key: promoted.key,
            }];
            let page_data = InteriorPage::build(
                root_id,
                txn_id,
                &cells,
                promoted.right_child,
                self.config.page_size,
            );
            let frame = pool.new_page(root_id, backend)?;
            pool.get_page_data_mut(frame)[..self.config.page_size]
                .copy_from_slice(&page_data);
            pool.unpin_page(frame, true);
            return Ok(root_id);
        }

        // Walk path from leaf parent up to root
        for i in (0..path.len()).rev() {
            let entry = &path[i];
            let frame = pool.fetch_page(entry.page_id, backend)?;
            let page_data = pool.get_page_data(frame);
            let interior = InteriorPage::parse(page_data, self.config.page_size)?;
            pool.unpin_page(frame, false);

            freed.push(entry.page_id);

            // Insert the promoted key into this interior page
            let mut cells: Vec<InteriorCell> = interior.cells().to_vec();
            let mut right_child = interior.right_child;

            // The promoted key replaces the child at child_index
            // Insert new separator: left_child = promoted.left_child, key = promoted.key
            // The slot at child_index pointed to the old (now split) child
            let new_cell = InteriorCell {
                left_child: promoted.left_child,
                key: promoted.key.clone(),
            };

            if entry.child_index < cells.len() {
                // The old child was cells[child_index].left_child
                // Replace it: insert new_cell before child_index, update child_index's left_child to right
                cells[entry.child_index].left_child = promoted.right_child;
                cells.insert(entry.child_index, new_cell);
            } else {
                // The old child was right_child
                cells.push(new_cell);
                right_child = promoted.right_child;
            }

            // Check if this interior page needs to split
            let total_cell_bytes: usize =
                cells.iter().map(|c| 10 + c.key.len() + 2).sum();
            let header_overhead = 38; // 24 (common) + 14 (subheader)
            if header_overhead + total_cell_bytes <= self.config.page_size {
                // Fits — write the new page and CoW the rest of the path
                let new_id = allocator.allocate_page();
                allocated.push(new_id);
                let page_bytes = InteriorPage::build(
                    new_id,
                    txn_id,
                    &cells,
                    right_child,
                    self.config.page_size,
                );
                let frame = pool.new_page(new_id, backend)?;
                pool.get_page_data_mut(frame)[..self.config.page_size]
                    .copy_from_slice(&page_bytes);
                pool.unpin_page(frame, true);

                // CoW the remaining path above
                let mut current_child = new_id;
                for j in (0..i).rev() {
                    let parent_entry = &path[j];
                    let parent_frame = pool.fetch_page(parent_entry.page_id, backend)?;
                    let parent_data = pool.get_page_data(parent_frame);
                    let parent_page = InteriorPage::parse(parent_data, self.config.page_size)?;
                    pool.unpin_page(parent_frame, false);

                    freed.push(parent_entry.page_id);

                    let mut parent_cells: Vec<InteriorCell> = parent_page.cells().to_vec();
                    let parent_right_child;
                    if parent_entry.child_index == parent_cells.len() {
                        parent_right_child = current_child;
                    } else {
                        parent_cells[parent_entry.child_index].left_child = current_child;
                        parent_right_child = parent_page.right_child;
                    }

                    let new_parent_id = allocator.allocate_page();
                    allocated.push(new_parent_id);
                    let parent_bytes = InteriorPage::build(
                        new_parent_id, txn_id, &parent_cells, parent_right_child, self.config.page_size,
                    );
                    let new_frame = pool.new_page(new_parent_id, backend)?;
                    pool.get_page_data_mut(new_frame)[..self.config.page_size]
                        .copy_from_slice(&parent_bytes);
                    pool.unpin_page(new_frame, true);
                    current_child = new_parent_id;
                }

                return Ok(current_child);
            }

            // Interior page splits
            let mid = cells.len() / 2;
            let left_cells = cells[..mid].to_vec();
            let median_key = cells[mid].key.clone();
            let split_left_right_child = cells[mid].left_child;
            let right_cells = cells[mid + 1..].to_vec();

            let left_id = allocator.allocate_page();
            let right_id_new = allocator.allocate_page();
            allocated.push(left_id);
            allocated.push(right_id_new);

            let left_data = InteriorPage::build(
                left_id,
                txn_id,
                &left_cells,
                split_left_right_child,
                self.config.page_size,
            );
            let right_data = InteriorPage::build(
                right_id_new,
                txn_id,
                &right_cells,
                right_child,
                self.config.page_size,
            );

            let left_frame = pool.new_page(left_id, backend)?;
            pool.get_page_data_mut(left_frame)[..self.config.page_size]
                .copy_from_slice(&left_data);
            pool.unpin_page(left_frame, true);

            let right_frame = pool.new_page(right_id_new, backend)?;
            pool.get_page_data_mut(right_frame)[..self.config.page_size]
                .copy_from_slice(&right_data);
            pool.unpin_page(right_frame, true);

            promoted = PromotedKey {
                key: median_key,
                left_child: left_id,
                right_child: right_id_new,
            };

            // If this was the root, create a new root
            if i == 0 {
                let root_id = allocator.allocate_page();
                allocated.push(root_id);
                let root_cells = [InteriorCell {
                    left_child: promoted.left_child,
                    key: promoted.key,
                }];
                let root_data = InteriorPage::build(
                    root_id,
                    txn_id,
                    &root_cells,
                    promoted.right_child,
                    self.config.page_size,
                );
                let frame = pool.new_page(root_id, backend)?;
                pool.get_page_data_mut(frame)[..self.config.page_size]
                    .copy_from_slice(&root_data);
                pool.unpin_page(frame, true);
                return Ok(root_id);
            }
            // Otherwise continue up the path
        }

        Err(StorageError {
            message: "propagate_split exhausted path without producing a root".into(),
            #[cfg(feature = "std")]
            source: None,
        })
    }

    /// Creates a leaf cell, dispatching to overflow pages if the value
    /// exceeds the inline cell threshold.
    fn create_leaf_cell<B: ReadAt + WriteAt + backend::Durability>(
        &self,
        key: &[u8],
        value: &[u8],
        pool: &mut BufferPool,
        allocator: &mut PageAllocator,
        backend: &mut B,
        txn_id: u64,
        allocated: &mut Vec<PageId>,
    ) -> Result<LeafCell, StorageError> {
        let inline_size = 4 + key.len() + value.len();
        let threshold = overflow_cell_threshold(self.config.page_size);

        if inline_size <= threshold {
            return Ok(LeafCell {
                key: key.to_vec(),
                value: LeafCellValue::Inline(value.to_vec()),
            });
        }

        // Dispatch to overflow pages
        let max_payload = OverflowPage::max_payload(self.config.page_size);
        let num_pages = value.len().div_ceil(max_payload);

        let mut page_ids = Vec::with_capacity(num_pages);
        for _ in 0..num_pages {
            let pid = allocator.allocate_page();
            page_ids.push(pid);
            allocated.push(pid);
        }

        let chain_pages = OverflowPage::build_chain(&page_ids, txn_id, value, self.config.page_size)?;

        for (i, page_data) in chain_pages.iter().enumerate() {
            let frame = pool.new_page(page_ids[i], backend)?;
            pool.get_page_data_mut(frame)[..self.config.page_size]
                .copy_from_slice(page_data);
            pool.unpin_page(frame, true);
        }

        Ok(LeafCell {
            key: key.to_vec(),
            value: LeafCellValue::Overflow {
                overflow_page_id: page_ids[0],
                total_overflow_len: value.len() as u32,
            },
        })
    }

    /// Walks an overflow page chain and adds all page IDs to the freed list.
    fn free_overflow_chain<B: ReadAt + WriteAt + backend::Durability>(
        &self,
        first_page: PageId,
        pool: &mut BufferPool,
        backend: &mut B,
        freed: &mut Vec<PageId>,
    ) -> Result<(), StorageError> {
        let mut current = first_page;
        while !current.is_null() {
            freed.push(current);
            let frame = pool.fetch_page(current, backend)?;
            let page_data = pool.get_page_data(frame);
            let overflow = OverflowPage::parse(page_data, self.config.page_size)?;
            let next = overflow.next_page;
            pool.unpin_page(frame, false);
            current = next;
        }
        Ok(())
    }
}
