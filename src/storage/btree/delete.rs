//! B-tree CoW delete with borrow/rebalance.
//!
//! Removes a key from the B-tree. In this v1 implementation, underfull
//! leaves are left as-is (no merge). This trades some space utilization
//! for implementation simplicity in a CoW B-tree where write amplification
//! is already inherent.
//!
//! TODO: Implement leaf merge for better space utilization in v2.

use crate::error::StorageError;
use crate::hal::{self, ReadAt, WriteAt};

use crate::storage::allocator::PageAllocator;
use crate::storage::buffer_pool::BufferPool;
use crate::storage::page::interior::{InteriorCell, InteriorPage};
use crate::storage::page::leaf::LeafPage;
use crate::storage::page::{PageId, PageType};

use super::cow::CowResult;
use super::BTree;

/// Tracks the traversal path from root to leaf.
struct PathEntry {
    page_id: PageId,
    child_index: usize,
}

#[allow(clippy::too_many_arguments)]
impl BTree {
    /// Deletes a key from the B-tree.
    ///
    /// Returns `Ok(Some(CowResult))` with the new root if the key was found
    /// and deleted, or `Ok(None)` if the key was not found.
    ///
    /// # Design Note
    ///
    /// This v1 implementation does not merge underfull leaves. Deleted keys
    /// are simply removed, and the leaf may become sparse. Space is reclaimed
    /// when the leaf is naturally rewritten or by a future `compact()` operation.
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure, checksum mismatch, or buffer pool exhaustion.
    pub fn delete<B: ReadAt + WriteAt + hal::Sync>(
        &self,
        root: PageId,
        key: &[u8],
        pool: &mut BufferPool,
        allocator: &mut PageAllocator,
        backend: &mut B,
        txn_id: u64,
    ) -> Result<Option<CowResult>, StorageError> {
        if root.is_null() {
            return Ok(None);
        }

        let mut freed = Vec::new();
        let mut allocated = Vec::new();

        // Traverse to the leaf
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
                    let mut leaf = LeafPage::parse(page_data, self.config.page_size)?;
                    let old_next = leaf.next_leaf;
                    let old_prev = leaf.prev_leaf;
                    pool.unpin_page(frame, false);

                    // Try to delete the key
                    if leaf.delete_cell(key).is_none() {
                        return Ok(None); // key not found
                    }

                    // If the leaf is now empty and it's the root (no path), return null root
                    if leaf.cell_count() == 0 && path.is_empty() {
                        freed.push(current);
                        return Ok(Some(CowResult {
                            new_root: PageId::NULL,
                            freed_pages: freed,
                            new_pages: allocated,
                        }));
                    }

                    // CoW the leaf
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
                    let f = pool.new_page(new_leaf_id, backend)?;
                    pool.get_page_data_mut(f)[..self.config.page_size]
                        .copy_from_slice(&leaf_data);
                    pool.unpin_page(f, true);

                    // Note: neighbor leaf pages are not CoW-updated.
                    // Old pages on disk remain valid for range scan traversal.

                    // CoW the interior path
                    let new_root =
                        self.cow_delete_path(&path, new_leaf_id, pool, allocator, backend, txn_id, &mut freed, &mut allocated)?;

                    return Ok(Some(CowResult {
                        new_root,
                        freed_pages: freed,
                        new_pages: allocated,
                    }));
                }
                _ => {
                    pool.unpin_page(frame, false);
                    return Err(StorageError {
                        message: format!("unexpected page type {page_type} during delete"),
                        source: None,
                    });
                }
            }
        }
    }

    /// CoW-copies the interior path with the updated child pointer after deletion.
    fn cow_delete_path<B: ReadAt + WriteAt + hal::Sync>(
        &self,
        path: &[PathEntry],
        new_child: PageId,
        pool: &mut BufferPool,
        allocator: &mut PageAllocator,
        backend: &mut B,
        txn_id: u64,
        freed: &mut Vec<PageId>,
        allocated: &mut Vec<PageId>,
    ) -> Result<PageId, StorageError> {
        if path.is_empty() {
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

            let mut cells: Vec<InteriorCell> = interior.cells().to_vec();
            let right_child;

            if entry.child_index == cells.len() {
                right_child = current_child;
            } else {
                cells[entry.child_index].left_child = current_child;
                right_child = interior.right_child;
            }

            let new_id = allocator.allocate_page();
            allocated.push(new_id);

            let page_bytes = InteriorPage::build(
                new_id, txn_id, &cells, right_child, self.config.page_size,
            );
            let f = pool.new_page(new_id, backend)?;
            pool.get_page_data_mut(f)[..self.config.page_size]
                .copy_from_slice(&page_bytes);
            pool.unpin_page(f, true);

            current_child = new_id;
        }

        Ok(current_child)
    }

}
