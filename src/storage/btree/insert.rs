//! B-tree CoW insert with split propagation.
//!
//! Inserts a key-value pair, splitting leaf and interior pages as needed.
//! All modifications produce new page copies (CoW); old pages are freed.

use crate::error::StorageError;
use crate::backend::{self, ReadAt, WriteAt};

use crate::storage::allocator::PageAllocator;
use crate::storage::buffer_pool::BufferPool;
use crate::storage::page::interior::{InteriorCell, InteriorPage};
use crate::storage::page::leaf::{LeafCell, LeafCellValue, LeafPage};
use crate::storage::page::{PageId, PageType};

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

/// Tracks the traversal path from root to leaf.
struct PathEntry {
    page_id: PageId,
    /// Index of the child pointer that was followed (cell index, or `cells.len()` for right_child).
    child_index: usize,
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
            let cells = [LeafCell {
                key: key.to_vec(),
                value: LeafCellValue::Inline(value.to_vec()),
            }];
            let page_data = LeafPage::build(
                new_root_id,
                txn_id,
                &cells,
                PageId::NULL,
                PageId::NULL,
                self.config.page_size,
            );
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
                    // Find child index
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
                    let mut leaf = LeafPage::parse(page_data, self.config.page_size)?;
                    let old_next = leaf.next_leaf;
                    let old_prev = leaf.prev_leaf;
                    pool.unpin_page(frame, false);

                    // Check for duplicate key and replace
                    if let Some(existing) = leaf.delete_cell(key) {
                        let _ = existing; // old value discarded
                    }

                    let new_cell = LeafCell {
                        key: key.to_vec(),
                        value: LeafCellValue::Inline(value.to_vec()),
                    };

                    // Try to insert into the leaf
                    if leaf.insert_cell(new_cell.clone(), self.config.page_size) {
                        // Fits — CoW the leaf and path
                        let new_leaf_id = allocator.allocate_page();
                        allocated.push(new_leaf_id);
                        freed.push(current);

                        let leaf_data = LeafPage::build(
                            new_leaf_id,
                            txn_id,
                            leaf.cells(),
                            old_next,
                            old_prev,
                            self.config.page_size,
                        );
                        let frame = pool.new_page(new_leaf_id, backend)?;
                        pool.get_page_data_mut(frame)[..self.config.page_size]
                            .copy_from_slice(&leaf_data);
                        pool.unpin_page(frame, true);

                        // Note: neighbor leaf pages are NOT updated here.
                        // Their prev/next pointers still reference the old page,
                        // which is correct for range scans within the current snapshot
                        // since old page data remains valid on disk.

                        // CoW the path from leaf parent to root
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
                    // Re-add the cell to get the full list, then split
                    leaf.insert_cell(new_cell, usize::MAX); // force insert for splitting
                    let (left_cells, right_cells, split_key) = leaf.split();

                    let left_id = allocator.allocate_page();
                    let right_id = allocator.allocate_page();
                    allocated.push(left_id);
                    allocated.push(right_id);
                    freed.push(current);

                    // Build split pages with correct leaf links
                    let left_data = LeafPage::build(
                        left_id,
                        txn_id,
                        &left_cells,
                        right_id,
                        old_prev,
                        self.config.page_size,
                    );
                    let right_data = LeafPage::build(
                        right_id,
                        txn_id,
                        &right_cells,
                        old_next,
                        left_id,
                        self.config.page_size,
                    );

                    let lf = pool.new_page(left_id, backend)?;
                    pool.get_page_data_mut(lf)[..self.config.page_size]
                        .copy_from_slice(&left_data);
                    pool.unpin_page(lf, true);

                    let rf = pool.new_page(right_id, backend)?;
                    pool.get_page_data_mut(rf)[..self.config.page_size]
                        .copy_from_slice(&right_data);
                    pool.unpin_page(rf, true);

                    // Note: neighbor leaf pages are NOT CoW-updated here.
                    // The left page's prev_leaf points to old_prev (valid old page).
                    // The right page's next_leaf points to old_next (valid old page).
                    // Cursor range scans work because they follow these pointers
                    // to read old pages that still have correct data.

                    // Promote split key up
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

                    return Ok(CowResult {
                        new_root,
                        freed_pages: freed,
                        new_pages: allocated,
                    });
                }
                _ => {
                    pool.unpin_page(frame, false);
                    return Err(StorageError {
                        message: format!("unexpected page type {page_type} during insert"),
                        source: None,
                    });
                }
            }
        }
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
            let f = pool.new_page(new_id, backend)?;
            pool.get_page_data_mut(f)[..self.config.page_size]
                .copy_from_slice(&page_bytes);
            pool.unpin_page(f, true);

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
            let f = pool.new_page(root_id, backend)?;
            pool.get_page_data_mut(f)[..self.config.page_size]
                .copy_from_slice(&page_data);
            pool.unpin_page(f, true);
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
                let f = pool.new_page(new_id, backend)?;
                pool.get_page_data_mut(f)[..self.config.page_size]
                    .copy_from_slice(&page_bytes);
                pool.unpin_page(f, true);

                // CoW the remaining path above
                let mut current_child = new_id;
                for j in (0..i).rev() {
                    let pe = &path[j];
                    let pf = pool.fetch_page(pe.page_id, backend)?;
                    let pd = pool.get_page_data(pf);
                    let int = InteriorPage::parse(pd, self.config.page_size)?;
                    pool.unpin_page(pf, false);

                    freed.push(pe.page_id);

                    let mut c: Vec<InteriorCell> = int.cells().to_vec();
                    let rc;
                    if pe.child_index == c.len() {
                        rc = current_child;
                    } else {
                        c[pe.child_index].left_child = current_child;
                        rc = int.right_child;
                    }

                    let nid = allocator.allocate_page();
                    allocated.push(nid);
                    let pb = InteriorPage::build(
                        nid, txn_id, &c, rc, self.config.page_size,
                    );
                    let ff = pool.new_page(nid, backend)?;
                    pool.get_page_data_mut(ff)[..self.config.page_size]
                        .copy_from_slice(&pb);
                    pool.unpin_page(ff, true);
                    current_child = nid;
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

            let lf = pool.new_page(left_id, backend)?;
            pool.get_page_data_mut(lf)[..self.config.page_size]
                .copy_from_slice(&left_data);
            pool.unpin_page(lf, true);

            let rf = pool.new_page(right_id_new, backend)?;
            pool.get_page_data_mut(rf)[..self.config.page_size]
                .copy_from_slice(&right_data);
            pool.unpin_page(rf, true);

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
                let f = pool.new_page(root_id, backend)?;
                pool.get_page_data_mut(f)[..self.config.page_size]
                    .copy_from_slice(&root_data);
                pool.unpin_page(f, true);
                return Ok(root_id);
            }
            // Otherwise continue up the path
        }

        unreachable!("propagate_split should return from within the loop")
    }

}
