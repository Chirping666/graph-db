//! B-tree point lookup.
//!
//! Traverses from root to leaf, returning the value for a given key.

use crate::error::StorageError;
use crate::backend::{self, ReadAt, WriteAt};

use crate::storage::buffer_pool::BufferPool;
use crate::storage::page::interior::InteriorPage;
use crate::storage::page::leaf::{LeafCellValue, LeafPage};
use crate::storage::page::{PageId, PageType};

use super::BTree;

impl BTree {
    /// Looks up a key in the B-tree.
    ///
    /// Returns the value bytes if found, `None` otherwise.
    /// For key-only B-trees, returns `Some(vec![])` when the key exists.
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure, checksum mismatch, or buffer pool exhaustion.
    pub fn search<B: ReadAt + WriteAt + backend::Durability>(
        &self,
        root: PageId,
        key: &[u8],
        pool: &mut BufferPool,
        backend: &mut B,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        if root.is_null() {
            return Ok(None);
        }
        self.search_recursive(root, key, pool, backend)
    }

    fn search_recursive<B: ReadAt + WriteAt + backend::Durability>(
        &self,
        page_id: PageId,
        key: &[u8],
        pool: &mut BufferPool,
        backend: &mut B,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        let frame = pool.fetch_page(page_id, backend)?;
        let page_data = pool.get_page_data(frame);
        let page_type = PageType::try_from(page_data[8]).map_err(|v| StorageError {
            message: format!("unknown page type: {v:#04x}"),
            source: None,
        })?;

        match page_type {
            PageType::Interior => {
                let interior = InteriorPage::parse(page_data, self.config.page_size)?;
                let child = interior.search(key);
                pool.unpin_page(frame, false);
                self.search_recursive(child, key, pool, backend)
            }
            PageType::Leaf => {
                let leaf = LeafPage::parse(page_data, self.config.page_size)?;
                let result = match leaf.search(key) {
                    Some(cell) => match &cell.value {
                        LeafCellValue::Inline(v) => Some(v.clone()),
                        LeafCellValue::Overflow { .. } => {
                            // For now, return a marker; overflow reading is handled at higher level
                            // We need the page_id from the cell
                            match &cell.value {
                                LeafCellValue::Overflow {
                                    overflow_page_id,
                                    total_overflow_len,
                                } => {
                                    pool.unpin_page(frame, false);
                                    let data =
                                        crate::storage::page::overflow::OverflowPage::read_chain(
                                            backend,
                                            *overflow_page_id,
                                            *total_overflow_len,
                                            self.config.page_size,
                                        )?;
                                    return Ok(Some(data));
                                }
                                _ => unreachable!(),
                            }
                        }
                    },
                    None => None,
                };
                pool.unpin_page(frame, false);
                Ok(result)
            }
            _ => {
                pool.unpin_page(frame, false);
                Err(StorageError {
                    message: format!("unexpected page type {page_type} during search"),
                    source: None,
                })
            }
        }
    }
}
