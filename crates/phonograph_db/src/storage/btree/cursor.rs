//! Leaf-level cursor for B-tree range iteration.
//!
//! The cursor navigates from root to the target leaf, then uses
//! a stack-based approach to find the next leaf when the current
//! one is exhausted. This avoids relying on `next_leaf` pointers
//! which may reference stale pages in a CoW B-tree.

use alloc::{format, vec::Vec};
use crate::error::StorageError;
use crate::backend::{self, ReadAt, WriteAt};

use crate::storage::buffer_pool::BufferPool;
use crate::storage::page::interior::InteriorPage;
use crate::storage::page::leaf::{LeafCellValue, LeafPage};
use crate::storage::page::{PageId, PageType};

use super::BTreeConfig;

/// A saved position in the tree for navigating between leaves.
#[derive(Clone)]
struct StackEntry {
    /// Page ID of the interior page.
    page_id: PageId,
    /// The child index we descended into. Next child is index + 1.
    child_index: usize,
    /// Total number of children: cells.len() + 1 (including right_child).
    num_children: usize,
}

/// A cursor for iterating over a range of keys in a B-tree.
///
/// Created positioned at the first key `>= start_key`. Uses a
/// stack of interior page positions to efficiently find the next
/// leaf without relying on leaf link pointers.
pub struct BTreeCursor {
    /// Stack of interior page positions from root to leaf's parent.
    stack: Vec<StackEntry>,
    /// Cached cells from the current leaf page.
    cached_cells: Vec<(Vec<u8>, LeafCellValue)>,
    /// Index of the next cell to return.
    current_cell: usize,
    /// Whether the cursor is exhausted.
    exhausted: bool,
    /// End key (exclusive), or `None` for open-ended scan.
    end_key: Option<Vec<u8>>,
}

impl BTreeCursor {
    /// Creates a cursor positioned at the first key `>= start_key`.
    ///
    /// If `end_key` is `Some`, the cursor stops before keys `>= end_key`.
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure or buffer pool exhaustion.
    pub fn new<B: ReadAt + WriteAt + backend::Durability>(
        root: PageId,
        start_key: &[u8],
        end_key: Option<&[u8]>,
        pool: &mut BufferPool,
        backend: &mut B,
        config: &BTreeConfig,
    ) -> Result<Self, StorageError> {
        if root.is_null() {
            return Ok(Self {
                stack: Vec::new(),
                cached_cells: Vec::new(),
                current_cell: 0,
                exhausted: true,
                end_key: end_key.map(|k| k.to_vec()),
            });
        }

        let mut stack = Vec::new();
        let mut page_id = root;

        // Navigate from root to the leaf containing start_key
        loop {
            let frame = pool.fetch_page(page_id, backend)?;
            let page_data = pool.get_page_data(frame);
            let page_type = PageType::try_from(page_data[8]).map_err(|v| StorageError {
                message: format!("unknown page type: {v:#04x}"),
                #[cfg(feature = "std")]
                source: None,
            })?;

            match page_type {
                PageType::Interior => {
                    let interior = InteriorPage::parse(page_data, config.page_size)?;
                    let cells = interior.cells();
                    let num_children = cells.len() + 1;
                    let pos = cells.partition_point(|c| c.key.as_slice() <= start_key);
                    let child = if pos < cells.len() {
                        cells[pos].left_child
                    } else {
                        interior.right_child
                    };
                    pool.unpin_page(frame, false);

                    stack.push(StackEntry {
                        page_id,
                        child_index: pos,
                        num_children,
                    });
                    page_id = child;
                }
                PageType::Leaf => {
                    let leaf = LeafPage::parse(page_data, config.page_size)?;
                    let cells: Vec<_> = leaf
                        .cells()
                        .iter()
                        .map(|c| (c.key.clone(), c.value.clone()))
                        .collect();
                    pool.unpin_page(frame, false);

                    let start_idx = cells
                        .partition_point(|c| c.0.as_slice() < start_key);

                    return Ok(Self {
                        stack,
                        cached_cells: cells,
                        current_cell: start_idx,
                        exhausted: false,
                        end_key: end_key.map(|k| k.to_vec()),
                    });
                }
                _ => {
                    pool.unpin_page(frame, false);
                    return Err(StorageError {
                        message: format!(
                            "unexpected page type {page_type} during cursor init"
                        ),
                        #[cfg(feature = "std")]
                        source: None,
                    });
                }
            }
        }
    }

    /// Advances the cursor and returns the next `(key, value)` pair.
    ///
    /// Returns `None` when exhausted.
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure or buffer pool exhaustion.
    #[allow(clippy::type_complexity)]
    pub fn next<B: ReadAt + WriteAt + backend::Durability>(
        &mut self,
        pool: &mut BufferPool,
        backend: &mut B,
        config: &BTreeConfig,
    ) -> Result<Option<(Vec<u8>, Vec<u8>)>, StorageError> {
        loop {
            if self.exhausted {
                return Ok(None);
            }

            if self.current_cell < self.cached_cells.len() {
                let (key, value) = &self.cached_cells[self.current_cell];

                if let Some(end) = &self.end_key {
                    if key.as_slice() >= end.as_slice() {
                        self.exhausted = true;
                        return Ok(None);
                    }
                }

                self.current_cell += 1;

                let value_bytes = match value {
                    LeafCellValue::Inline(v) => v.clone(),
                    LeafCellValue::Overflow {
                        overflow_page_id,
                        total_overflow_len,
                    } => {
                        crate::storage::page::overflow::OverflowPage::read_chain(
                            backend,
                            *overflow_page_id,
                            *total_overflow_len,
                            config.page_size,
                        )?
                    }
                };

                return Ok(Some((key.clone(), value_bytes)));
            }

            // Current leaf exhausted. Navigate to the next leaf using the stack.
            if !self.advance_to_next_leaf(pool, backend, config)? {
                self.exhausted = true;
                return Ok(None);
            }
        }
    }

    /// Navigates to the next leaf page using the interior page stack.
    ///
    /// Returns `true` if a new leaf was loaded, `false` if the tree is exhausted.
    fn advance_to_next_leaf<B: ReadAt + WriteAt + backend::Durability>(
        &mut self,
        pool: &mut BufferPool,
        backend: &mut B,
        config: &BTreeConfig,
    ) -> Result<bool, StorageError> {
        // Walk up the stack to find a parent with a right sibling
        while let Some(entry) = self.stack.last_mut() {
            let next_child_idx = entry.child_index + 1;
            if next_child_idx < entry.num_children {
                // This parent has a next child — descend into it
                entry.child_index = next_child_idx;

                let frame = pool.fetch_page(entry.page_id, backend)?;
                let page_data = pool.get_page_data(frame);
                let interior = InteriorPage::parse(page_data, config.page_size)?;
                let cells = interior.cells();
                let child = if next_child_idx < cells.len() {
                    cells[next_child_idx].left_child
                } else {
                    interior.right_child
                };
                pool.unpin_page(frame, false);

                // Descend to the leftmost leaf of this subtree
                return self.descend_to_leftmost_leaf(child, pool, backend, config);
            }
            // No more children at this level — pop and try the parent
            self.stack.pop();
        }

        // Stack empty — no more leaves
        Ok(false)
    }

    /// Descends from a given page to its leftmost leaf, pushing interior
    /// pages onto the stack.
    fn descend_to_leftmost_leaf<B: ReadAt + WriteAt + backend::Durability>(
        &mut self,
        mut page_id: PageId,
        pool: &mut BufferPool,
        backend: &mut B,
        config: &BTreeConfig,
    ) -> Result<bool, StorageError> {
        loop {
            let frame = pool.fetch_page(page_id, backend)?;
            let page_data = pool.get_page_data(frame);
            let page_type = PageType::try_from(page_data[8]).map_err(|v| StorageError {
                message: format!("unknown page type: {v:#04x}"),
                #[cfg(feature = "std")]
                source: None,
            })?;

            match page_type {
                PageType::Interior => {
                    let interior = InteriorPage::parse(page_data, config.page_size)?;
                    let cells = interior.cells();
                    let num_children = cells.len() + 1;
                    let child = if !cells.is_empty() {
                        cells[0].left_child
                    } else {
                        interior.right_child
                    };
                    pool.unpin_page(frame, false);

                    self.stack.push(StackEntry {
                        page_id,
                        child_index: 0,
                        num_children,
                    });
                    page_id = child;
                }
                PageType::Leaf => {
                    let leaf = LeafPage::parse(page_data, config.page_size)?;
                    self.cached_cells = leaf
                        .cells()
                        .iter()
                        .map(|c| (c.key.clone(), c.value.clone()))
                        .collect();
                    pool.unpin_page(frame, false);
                    self.current_cell = 0;
                    return Ok(!self.cached_cells.is_empty());
                }
                _ => {
                    pool.unpin_page(frame, false);
                    return Err(StorageError {
                        message: format!(
                            "unexpected page type {page_type} during leaf descent"
                        ),
                        #[cfg(feature = "std")]
                        source: None,
                    });
                }
            }
        }
    }
}
