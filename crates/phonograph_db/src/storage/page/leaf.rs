//! B-tree leaf page format.
//!
//! Leaf pages store key-value cells (or key-only cells for index B-trees).
//! They form a doubly-linked list for efficient range scans.
//! Layout follows `008-file-format-spec.md` §8.

use alloc::{format, vec, vec::Vec};
use crate::error::StorageError;

use super::header::CommonPageHeader;
use super::{COMMON_HEADER_SIZE, PageId, PageType};

/// Byte offset where the leaf page subheader begins.
const SUBHEADER_OFFSET: usize = COMMON_HEADER_SIZE; // 24

/// Size of the leaf subheader: cell_count(2) + free_start(2) + next_leaf(8) + prev_leaf(8) = 20.
const SUBHEADER_SIZE: usize = 20;

/// Byte offset where the cell pointer array begins.
const CELL_PTRS_OFFSET: usize = SUBHEADER_OFFSET + SUBHEADER_SIZE; // 44

/// Sentinel value for `value_len` indicating an overflow-redirected cell.
const OVERFLOW_SENTINEL: u16 = 0xFFFF;

/// The value payload of a leaf cell: either inline bytes or an overflow pointer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeafCellValue {
    /// Value stored inline in the leaf cell.
    Inline(Vec<u8>),
    /// Value stored in an overflow page chain.
    Overflow {
        /// Page ID of the first overflow page.
        overflow_page_id: PageId,
        /// Total byte length of the overflow data.
        total_overflow_len: u32,
    },
}

/// A single key-value cell in a leaf page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeafCell {
    /// The key bytes (big-endian encoded).
    pub key: Vec<u8>,
    /// The value: inline bytes or overflow pointer.
    pub value: LeafCellValue,
}

impl LeafCell {
    /// On-disk size of this cell.
    fn serialized_size(&self) -> usize {
        match &self.value {
            LeafCellValue::Inline(v) => 4 + self.key.len() + v.len(),
            LeafCellValue::Overflow { .. } => 4 + self.key.len() + 12,
        }
    }
}

/// A parsed B-tree leaf page.
#[derive(Clone, Debug)]
pub struct LeafPage {
    /// Common 24-byte page header.
    pub header: CommonPageHeader,
    /// Number of key-value cells.
    pub cell_count: u16,
    /// Offset of first free byte in the cell content area.
    pub free_start: u16,
    /// Page ID of the next leaf in sorted order (0 = none).
    pub next_leaf: PageId,
    /// Page ID of the previous leaf in sorted order (0 = none).
    pub prev_leaf: PageId,
    /// Parsed cells in key-sorted order.
    cells: Vec<LeafCell>,
}

impl LeafPage {
    /// Parses a leaf page from raw page bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the header is malformed, page type is not `Leaf`,
    /// or cell data is out of bounds.
    pub fn parse(page_data: &[u8], page_size: usize) -> Result<Self, StorageError> {
        if page_data.len() < page_size || page_size < CELL_PTRS_OFFSET {
            return Err(StorageError {
                message: format!("leaf page too small: {} bytes", page_data.len()),
                #[cfg(feature = "std")]
                source: None,
            });
        }

        let header = CommonPageHeader::deserialize(page_data)?;
        if header.page_type != PageType::Leaf {
            return Err(StorageError {
                message: format!("expected Leaf page, got {:?}", header.page_type),
                #[cfg(feature = "std")]
                source: None,
            });
        }

        let cell_count = u16::from_le_bytes([page_data[24], page_data[25]]);
        let free_start = u16::from_le_bytes([page_data[26], page_data[27]]);
        let next_leaf = PageId(u64::from_le_bytes(
            page_data[28..36].try_into().unwrap(),
        ));
        let prev_leaf = PageId(u64::from_le_bytes(
            page_data[36..44].try_into().unwrap(),
        ));

        let mut cells = Vec::with_capacity(cell_count as usize);
        for i in 0..cell_count as usize {
            let ptr_offset = CELL_PTRS_OFFSET + i * 2;
            if ptr_offset + 2 > page_size {
                return Err(StorageError {
                    message: format!("leaf cell pointer {i} out of bounds"),
                    #[cfg(feature = "std")]
                    source: None,
                });
            }
            let cell_offset =
                u16::from_le_bytes([page_data[ptr_offset], page_data[ptr_offset + 1]]) as usize;

            if cell_offset + 4 > page_size {
                return Err(StorageError {
                    message: format!("leaf cell {i} at offset {cell_offset} out of bounds"),
                    #[cfg(feature = "std")]
                    source: None,
                });
            }

            let key_len = u16::from_le_bytes([
                page_data[cell_offset],
                page_data[cell_offset + 1],
            ]) as usize;
            let value_len_raw = u16::from_le_bytes([
                page_data[cell_offset + 2],
                page_data[cell_offset + 3],
            ]);

            let key_start = cell_offset + 4;
            if key_start + key_len > page_size {
                return Err(StorageError {
                    message: format!("leaf cell {i} key extends beyond page"),
                    #[cfg(feature = "std")]
                    source: None,
                });
            }
            let key = page_data[key_start..key_start + key_len].to_vec();

            let value = if value_len_raw == OVERFLOW_SENTINEL {
                // Overflow-redirected cell
                let ov_start = key_start + key_len;
                if ov_start + 12 > page_size {
                    return Err(StorageError {
                        message: format!("leaf cell {i} overflow pointer extends beyond page"),
                        #[cfg(feature = "std")]
                        source: None,
                    });
                }
                let overflow_page_id = PageId(u64::from_le_bytes(
                    page_data[ov_start..ov_start + 8].try_into().unwrap(),
                ));
                let total_overflow_len = u32::from_le_bytes(
                    page_data[ov_start + 8..ov_start + 12].try_into().unwrap(),
                );
                LeafCellValue::Overflow {
                    overflow_page_id,
                    total_overflow_len,
                }
            } else {
                let value_len = value_len_raw as usize;
                let val_start = key_start + key_len;
                if val_start + value_len > page_size {
                    return Err(StorageError {
                        message: format!("leaf cell {i} value extends beyond page"),
                        #[cfg(feature = "std")]
                        source: None,
                    });
                }
                LeafCellValue::Inline(page_data[val_start..val_start + value_len].to_vec())
            };

            cells.push(LeafCell { key, value });
        }

        Ok(Self {
            header,
            cell_count,
            free_start,
            next_leaf,
            prev_leaf,
            cells,
        })
    }

    /// Builds a complete leaf page image with the correct header and checksum.
    ///
    /// Returns a `Vec<u8>` of exactly `page_size` bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the cells do not fit in the page.
    pub fn build(
        page_id: PageId,
        txn_id: u64,
        cells: &[LeafCell],
        next_leaf: PageId,
        prev_leaf: PageId,
        page_size: usize,
    ) -> Result<Vec<u8>, StorageError> {
        let mut page = vec![0u8; page_size];

        // Write subheader
        let cell_count = cells.len() as u16;
        page[24..26].copy_from_slice(&cell_count.to_le_bytes());
        // free_start will be set below
        page[28..36].copy_from_slice(&next_leaf.0.to_le_bytes());
        page[36..44].copy_from_slice(&prev_leaf.0.to_le_bytes());

        // Write cells backward from end of page
        let mut write_pos = page_size;
        for (i, cell) in cells.iter().enumerate() {
            let cell_size = cell.serialized_size();
            write_pos -= cell_size;

            let key_len = cell.key.len() as u16;
            page[write_pos..write_pos + 2].copy_from_slice(&key_len.to_le_bytes());

            match &cell.value {
                LeafCellValue::Inline(v) => {
                    let value_len = v.len() as u16;
                    page[write_pos + 2..write_pos + 4]
                        .copy_from_slice(&value_len.to_le_bytes());
                    page[write_pos + 4..write_pos + 4 + cell.key.len()]
                        .copy_from_slice(&cell.key);
                    page[write_pos + 4 + cell.key.len()
                        ..write_pos + 4 + cell.key.len() + v.len()]
                        .copy_from_slice(v);
                }
                LeafCellValue::Overflow {
                    overflow_page_id,
                    total_overflow_len,
                } => {
                    page[write_pos + 2..write_pos + 4]
                        .copy_from_slice(&OVERFLOW_SENTINEL.to_le_bytes());
                    page[write_pos + 4..write_pos + 4 + cell.key.len()]
                        .copy_from_slice(&cell.key);
                    let ov_start = write_pos + 4 + cell.key.len();
                    page[ov_start..ov_start + 8]
                        .copy_from_slice(&overflow_page_id.0.to_le_bytes());
                    page[ov_start + 8..ov_start + 12]
                        .copy_from_slice(&total_overflow_len.to_le_bytes());
                }
            }

            // Write cell pointer
            let ptr_offset = CELL_PTRS_OFFSET + i * 2;
            page[ptr_offset..ptr_offset + 2]
                .copy_from_slice(&(write_pos as u16).to_le_bytes());
        }

        let free_start = write_pos as u16;
        page[26..28].copy_from_slice(&free_start.to_le_bytes());

        // Sanity check
        let ptrs_end = CELL_PTRS_OFFSET + cells.len() * 2;
        if ptrs_end > write_pos {
            return Err(StorageError {
                message: format!(
                    "leaf page overflow: ptrs_end={ptrs_end}, content_start={write_pos}"
                ),
                #[cfg(feature = "std")]
                source: None,
            });
        }

        // Write header
        let header = CommonPageHeader {
            page_id,
            page_type: PageType::Leaf,
            flags: 0x01, // is_leaf = 1
            txn_id,
            checksum: 0,
        };
        header.serialize(&mut page);
        let checksum = CommonPageHeader::compute_checksum(&page);
        page[20..24].copy_from_slice(&checksum.to_le_bytes());

        Ok(page)
    }

    /// Binary-searches for an exact key match among the cells.
    ///
    /// Returns a reference to the matching cell, or `None` if not found.
    pub fn search(&self, key: &[u8]) -> Option<&LeafCell> {
        self.cells
            .binary_search_by(|cell| cell.key.as_slice().cmp(key))
            .ok()
            .map(|i| &self.cells[i])
    }

    /// Returns all cells with keys in the inclusive range `[start_key, end_key]`.
    pub fn search_range(&self, start_key: &[u8], end_key: &[u8]) -> Vec<&LeafCell> {
        let start = self.cells.partition_point(|c| c.key.as_slice() < start_key);
        let end = self.cells.partition_point(|c| c.key.as_slice() <= end_key);
        self.cells[start..end].iter().collect()
    }

    /// Inserts a cell in sorted position.
    ///
    /// Returns `true` if the cell was inserted, `false` if the page is full.
    pub fn insert_cell(&mut self, cell: LeafCell, page_size: usize) -> bool {
        if !self.has_space_for(cell.key.len(), cell.serialized_size() - 4 - cell.key.len(), page_size) {
            return false;
        }
        let pos = self.cells.partition_point(|c| c.key.as_slice() < cell.key.as_slice());
        self.cells.insert(pos, cell);
        self.cell_count = self.cells.len() as u16;
        true
    }

    /// Removes and returns the cell with the given key, or `None` if not found.
    pub fn delete_cell(&mut self, key: &[u8]) -> Option<LeafCell> {
        if let Ok(pos) = self.cells.binary_search_by(|c| c.key.as_slice().cmp(key)) {
            let cell = self.cells.remove(pos);
            self.cell_count = self.cells.len() as u16;
            Some(cell)
        } else {
            None
        }
    }

    /// Returns `true` if a new cell with the given key and value lengths can fit.
    pub fn has_space_for(&self, key_len: usize, value_len: usize, page_size: usize) -> bool {
        let cell_size = 4 + key_len + value_len;
        let ptr_size = 2;
        // Compute space used by existing cells
        let existing_cell_bytes: usize = self.cells.iter().map(|c| c.serialized_size()).sum();
        let existing_ptrs = self.cells.len() * 2;
        let total_used = CELL_PTRS_OFFSET + existing_ptrs + ptr_size + existing_cell_bytes + cell_size;
        total_used <= page_size
    }

    /// Splits the page at the median.
    ///
    /// Returns `(left_cells, right_cells, split_key)` where `split_key`
    /// is a copy of the first key in the right set.
    pub fn split(&self) -> (Vec<LeafCell>, Vec<LeafCell>, Vec<u8>) {
        let mid = self.cells.len() / 2;
        let left = self.cells[..mid].to_vec();
        let right = self.cells[mid..].to_vec();
        let split_key = right[0].key.clone();
        (left, right, split_key)
    }

    /// Returns the number of cells in this leaf.
    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }

    /// Returns the cells in key order.
    pub fn cells(&self) -> &[LeafCell] {
        &self.cells
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::page::DEFAULT_PAGE_SIZE;

    fn inline_cell(key: &[u8], value: &[u8]) -> LeafCell {
        LeafCell {
            key: key.to_vec(),
            value: LeafCellValue::Inline(value.to_vec()),
        }
    }

    fn key_only_cell(key: &[u8]) -> LeafCell {
        LeafCell {
            key: key.to_vec(),
            value: LeafCellValue::Inline(vec![]),
        }
    }

    #[test]
    fn build_parse_round_trip() {
        let cells = vec![
            inline_cell(&[0x00, 0x01], b"hello"),
            inline_cell(&[0x00, 0x02], b"world"),
            inline_cell(&[0x00, 0x03], b"foo"),
            inline_cell(&[0x00, 0x04], b"bar"),
            inline_cell(&[0x00, 0x05], b"baz"),
        ];
        let page_data = LeafPage::build(
            PageId(3),
            1,
            &cells,
            PageId(4),
            PageId(2),
            DEFAULT_PAGE_SIZE,
        ).unwrap();

        assert_eq!(page_data.len(), DEFAULT_PAGE_SIZE);
        CommonPageHeader::validate_checksum(&page_data).unwrap();

        let parsed = LeafPage::parse(&page_data, DEFAULT_PAGE_SIZE).unwrap();
        assert_eq!(parsed.cell_count, 5);
        assert_eq!(parsed.next_leaf, PageId(4));
        assert_eq!(parsed.prev_leaf, PageId(2));
        assert_eq!(parsed.cells().len(), 5);
        for (i, cell) in parsed.cells().iter().enumerate() {
            assert_eq!(cell, &cells[i]);
        }
    }

    #[test]
    fn search_found() {
        let cells = vec![
            inline_cell(&[0x00, 0x01], b"a"),
            inline_cell(&[0x00, 0x05], b"b"),
            inline_cell(&[0x00, 0x0A], b"c"),
        ];
        let page_data = LeafPage::build(
            PageId(3), 1, &cells, PageId::NULL, PageId::NULL, DEFAULT_PAGE_SIZE,
        ).unwrap();
        let page = LeafPage::parse(&page_data, DEFAULT_PAGE_SIZE).unwrap();

        let result = page.search(&[0x00, 0x05]);
        assert!(result.is_some());
        assert_eq!(
            result.unwrap().value,
            LeafCellValue::Inline(b"b".to_vec())
        );
    }

    #[test]
    fn search_not_found() {
        let cells = vec![
            inline_cell(&[0x00, 0x01], b"a"),
            inline_cell(&[0x00, 0x05], b"b"),
        ];
        let page_data = LeafPage::build(
            PageId(3), 1, &cells, PageId::NULL, PageId::NULL, DEFAULT_PAGE_SIZE,
        ).unwrap();
        let page = LeafPage::parse(&page_data, DEFAULT_PAGE_SIZE).unwrap();
        assert!(page.search(&[0x00, 0x03]).is_none());
    }

    #[test]
    fn search_range() {
        let cells: Vec<LeafCell> = (0..10u8)
            .map(|i| inline_cell(&[i], &[i + 100]))
            .collect();
        let page_data = LeafPage::build(
            PageId(3), 1, &cells, PageId::NULL, PageId::NULL, DEFAULT_PAGE_SIZE,
        ).unwrap();
        let page = LeafPage::parse(&page_data, DEFAULT_PAGE_SIZE).unwrap();

        let range = page.search_range(&[3], &[7]);
        assert_eq!(range.len(), 5); // keys 3,4,5,6,7
    }

    #[test]
    fn insert_cell_sorted() {
        let cells = vec![
            inline_cell(&[0x01], b"a"),
            inline_cell(&[0x03], b"c"),
        ];
        let page_data = LeafPage::build(
            PageId(3), 1, &cells, PageId::NULL, PageId::NULL, DEFAULT_PAGE_SIZE,
        ).unwrap();
        let mut page = LeafPage::parse(&page_data, DEFAULT_PAGE_SIZE).unwrap();

        assert!(page.insert_cell(inline_cell(&[0x02], b"b"), DEFAULT_PAGE_SIZE));
        assert_eq!(page.cell_count(), 3);
        assert_eq!(page.cells()[0].key, vec![0x01]);
        assert_eq!(page.cells()[1].key, vec![0x02]);
        assert_eq!(page.cells()[2].key, vec![0x03]);
    }

    #[test]
    fn delete_cell_found() {
        let cells = vec![
            inline_cell(&[0x01], b"a"),
            inline_cell(&[0x02], b"b"),
            inline_cell(&[0x03], b"c"),
        ];
        let page_data = LeafPage::build(
            PageId(3), 1, &cells, PageId::NULL, PageId::NULL, DEFAULT_PAGE_SIZE,
        ).unwrap();
        let mut page = LeafPage::parse(&page_data, DEFAULT_PAGE_SIZE).unwrap();

        let deleted = page.delete_cell(&[0x02]);
        assert!(deleted.is_some());
        assert_eq!(page.cell_count(), 2);
        assert!(page.search(&[0x02]).is_none());
    }

    #[test]
    fn split_balanced() {
        let cells: Vec<LeafCell> = (0..6u8)
            .map(|i| inline_cell(&[i], &[i + 100]))
            .collect();
        let page_data = LeafPage::build(
            PageId(3), 1, &cells, PageId::NULL, PageId::NULL, DEFAULT_PAGE_SIZE,
        ).unwrap();
        let page = LeafPage::parse(&page_data, DEFAULT_PAGE_SIZE).unwrap();

        let (left, right, split_key) = page.split();
        assert_eq!(left.len(), 3); // keys 0,1,2
        assert_eq!(right.len(), 3); // keys 3,4,5
        assert_eq!(split_key, vec![3u8]); // first key of right set
    }

    #[test]
    fn overflow_cell_round_trip() {
        let cells = vec![
            inline_cell(&[0x01], b"normal"),
            LeafCell {
                key: vec![0x02],
                value: LeafCellValue::Overflow {
                    overflow_page_id: PageId(100),
                    total_overflow_len: 50000,
                },
            },
            inline_cell(&[0x03], b"also normal"),
        ];
        let page_data = LeafPage::build(
            PageId(3), 1, &cells, PageId::NULL, PageId::NULL, DEFAULT_PAGE_SIZE,
        ).unwrap();
        let parsed = LeafPage::parse(&page_data, DEFAULT_PAGE_SIZE).unwrap();

        assert_eq!(parsed.cells()[1].key, vec![0x02]);
        match &parsed.cells()[1].value {
            LeafCellValue::Overflow {
                overflow_page_id,
                total_overflow_len,
            } => {
                assert_eq!(*overflow_page_id, PageId(100));
                assert_eq!(*total_overflow_len, 50000);
            }
            _ => panic!("expected overflow cell"),
        }
    }

    #[test]
    fn next_prev_leaf_serialized() {
        let page_data = LeafPage::build(
            PageId(5), 1, &[], PageId(6), PageId(4), DEFAULT_PAGE_SIZE,
        ).unwrap();
        let parsed = LeafPage::parse(&page_data, DEFAULT_PAGE_SIZE).unwrap();
        assert_eq!(parsed.next_leaf, PageId(6));
        assert_eq!(parsed.prev_leaf, PageId(4));
    }

    #[test]
    fn byte_level_layout() {
        let cells = vec![inline_cell(&[0xAB], b"X")];
        let page_data = LeafPage::build(
            PageId(3), 7, &cells, PageId(10), PageId(2), DEFAULT_PAGE_SIZE,
        ).unwrap();

        // page_type at offset 8
        assert_eq!(page_data[8], 0x02); // Leaf
        // flags at offset 9
        assert_eq!(page_data[9], 0x01); // is_leaf = 1
        // cell_count at offset 24
        assert_eq!(u16::from_le_bytes([page_data[24], page_data[25]]), 1);
        // next_leaf at offset 28
        assert_eq!(
            u64::from_le_bytes(page_data[28..36].try_into().unwrap()),
            10
        );
        // prev_leaf at offset 36
        assert_eq!(
            u64::from_le_bytes(page_data[36..44].try_into().unwrap()),
            2
        );
    }

    #[test]
    fn key_only_cell_round_trip() {
        let cells = vec![
            key_only_cell(&[0x00, 0x01, 0x02]),
            key_only_cell(&[0x00, 0x01, 0x03]),
        ];
        let page_data = LeafPage::build(
            PageId(3), 1, &cells, PageId::NULL, PageId::NULL, DEFAULT_PAGE_SIZE,
        ).unwrap();
        let parsed = LeafPage::parse(&page_data, DEFAULT_PAGE_SIZE).unwrap();
        for cell in parsed.cells() {
            assert_eq!(cell.value, LeafCellValue::Inline(vec![]));
        }
    }
}
