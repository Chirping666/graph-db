//! B-tree interior (branch) page format.
//!
//! Interior pages store separator keys and child page pointers,
//! guiding B-tree traversal from the root toward the correct leaf.
//! Layout follows `008-file-format-spec.md` §7.

use alloc::{format, vec, vec::Vec};
use crate::error::StorageError;

use super::header::CommonPageHeader;
use super::{COMMON_HEADER_SIZE, PageId, PageType};

/// Byte offset where the interior page subheader begins.
const SUBHEADER_OFFSET: usize = COMMON_HEADER_SIZE; // 24

/// Size of the interior page subheader: cell_count(2) + right_child(8) + free_start(2) + padding(2).
const SUBHEADER_SIZE: usize = 14;

/// Byte offset where the cell pointer array begins.
const CELL_PTRS_OFFSET: usize = SUBHEADER_OFFSET + SUBHEADER_SIZE; // 38

/// A single cell in an interior page: a separator key with a left-child pointer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InteriorCell {
    /// Page ID of the child subtree for keys less than this separator.
    pub left_child: PageId,
    /// The separator key bytes (big-endian encoded).
    pub key: Vec<u8>,
}

impl InteriorCell {
    /// On-disk size: left_child(8) + key_len(2) + key.
    fn serialized_size(&self) -> usize {
        10 + self.key.len()
    }
}

/// A parsed B-tree interior page.
#[derive(Clone, Debug)]
pub struct InteriorPage {
    /// Common 24-byte page header.
    pub header: CommonPageHeader,
    /// Number of separator keys.
    pub cell_count: u16,
    /// Page ID of the rightmost child (keys >= all separators).
    pub right_child: PageId,
    /// Offset of the first free byte in the cell content area.
    pub free_start: u16,
    /// Parsed cells in key-sorted order.
    cells: Vec<InteriorCell>,
}

impl InteriorPage {
    /// Parses an interior page from raw page bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the page header is malformed, the page type
    /// is not `Interior`, or cell data is out of bounds.
    pub fn parse(page_data: &[u8], page_size: usize) -> Result<Self, StorageError> {
        if page_data.len() < page_size || page_size < CELL_PTRS_OFFSET {
            return Err(StorageError {
                message: format!("interior page too small: {} bytes", page_data.len()),
                #[cfg(feature = "std")]
                source: None,
            });
        }

        let header = CommonPageHeader::deserialize(page_data)?;
        if header.page_type != PageType::Interior {
            return Err(StorageError {
                message: format!("expected Interior page, got {:?}", header.page_type),
                #[cfg(feature = "std")]
                source: None,
            });
        }

        let cell_count = u16::from_le_bytes([page_data[24], page_data[25]]);
        let right_child_bytes: [u8; 8] =
            page_data[26..34].try_into().map_err(|_| StorageError {
                message: "interior page: invalid right_child field length".into(),
                #[cfg(feature = "std")]
                source: None,
            })?;
        let right_child = PageId(u64::from_le_bytes(right_child_bytes));
        let free_start = u16::from_le_bytes([page_data[34], page_data[35]]);
        let padding = u16::from_le_bytes([page_data[36], page_data[37]]);
        if padding != 0 {
            return Err(StorageError {
                message: format!("interior page subheader padding non-zero: {padding:#06x}"),
                #[cfg(feature = "std")]
                source: None,
            });
        }

        let mut cells = Vec::with_capacity(cell_count as usize);
        for i in 0..cell_count as usize {
            let ptr_offset = CELL_PTRS_OFFSET + i * 2;
            if ptr_offset + 2 > page_size {
                return Err(StorageError {
                    message: format!("cell pointer {i} out of bounds"),
                    #[cfg(feature = "std")]
                    source: None,
                });
            }
            let cell_offset =
                u16::from_le_bytes([page_data[ptr_offset], page_data[ptr_offset + 1]]) as usize;

            if cell_offset + 10 > page_size {
                return Err(StorageError {
                    message: format!("cell {i} at offset {cell_offset} out of bounds"),
                    #[cfg(feature = "std")]
                    source: None,
                });
            }

            let left_child_bytes: [u8; 8] = page_data[cell_offset..cell_offset + 8]
                .try_into()
                .map_err(|_| StorageError {
                    message: format!("interior page: invalid left_child length at cell {i}"),
                    #[cfg(feature = "std")]
                    source: None,
                })?;
            let left_child = PageId(u64::from_le_bytes(left_child_bytes));
            let key_len = u16::from_le_bytes([
                page_data[cell_offset + 8],
                page_data[cell_offset + 9],
            ]) as usize;

            if cell_offset + 10 + key_len > page_size {
                return Err(StorageError {
                    message: format!("cell {i} key extends beyond page"),
                    #[cfg(feature = "std")]
                    source: None,
                });
            }

            let key = page_data[cell_offset + 10..cell_offset + 10 + key_len].to_vec();
            cells.push(InteriorCell { left_child, key });
        }

        Ok(Self {
            header,
            cell_count,
            right_child,
            free_start,
            cells,
        })
    }

    /// Builds a complete interior page image with the correct header and checksum.
    ///
    /// Returns a `Vec<u8>` of exactly `page_size` bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the cells do not fit in the page.
    pub fn build(
        page_id: PageId,
        txn_id: u64,
        cells: &[InteriorCell],
        right_child: PageId,
        page_size: usize,
    ) -> Result<Vec<u8>, StorageError> {
        let mut page = vec![0u8; page_size];

        // Write subheader
        let cell_count = cells.len() as u16;
        page[24..26].copy_from_slice(&cell_count.to_le_bytes());
        page[26..34].copy_from_slice(&right_child.0.to_le_bytes());
        // free_start and padding will be set below

        // Write cells backward from end of page; write cell pointers forward from offset 38.
        let mut write_pos = page_size;
        for (i, cell) in cells.iter().enumerate() {
            let cell_size = cell.serialized_size();
            write_pos -= cell_size;

            // Write cell data
            page[write_pos..write_pos + 8].copy_from_slice(&cell.left_child.0.to_le_bytes());
            let key_len = cell.key.len() as u16;
            page[write_pos + 8..write_pos + 10].copy_from_slice(&key_len.to_le_bytes());
            page[write_pos + 10..write_pos + 10 + cell.key.len()]
                .copy_from_slice(&cell.key);

            // Write cell pointer
            let ptr_offset = CELL_PTRS_OFFSET + i * 2;
            page[ptr_offset..ptr_offset + 2]
                .copy_from_slice(&(write_pos as u16).to_le_bytes());
        }

        let free_start = write_pos as u16;
        page[34..36].copy_from_slice(&free_start.to_le_bytes());
        // padding at 36..38 already zero

        // Sanity check: cell pointers must not overlap cell content area
        let ptrs_end = CELL_PTRS_OFFSET + cells.len() * 2;
        if ptrs_end > write_pos {
            return Err(StorageError {
                message: format!(
                    "interior page overflow: ptrs_end={ptrs_end}, content_start={write_pos}"
                ),
                #[cfg(feature = "std")]
                source: None,
            });
        }

        // Write header
        let header = CommonPageHeader {
            page_id,
            page_type: PageType::Interior,
            flags: 0, // is_leaf = 0
            txn_id,
            checksum: 0,
        };
        header.serialize(&mut page);
        let checksum = CommonPageHeader::compute_checksum(&page);
        page[20..24].copy_from_slice(&checksum.to_le_bytes());

        Ok(page)
    }

    /// Binary-searches for the child page to follow for the given `key`.
    ///
    /// Returns the correct child `PageId` for traversal:
    /// - If `key < cells[0].key`: returns `cells[0].left_child`
    /// - If `cells[i].key <= key < cells[i+1].key`: returns `cells[i+1].left_child`
    /// - If `key >= cells[last].key`: returns `right_child`
    pub fn search(&self, key: &[u8]) -> PageId {
        if self.cells.is_empty() {
            return self.right_child;
        }

        // Binary search: find the first cell whose key is greater than `key`.
        let pos = self.cells.partition_point(|cell| cell.key.as_slice() <= key);

        if pos == self.cells.len() {
            // key >= all separator keys → rightmost child
            self.right_child
        } else {
            // key < cells[pos].key → follow cells[pos].left_child
            self.cells[pos].left_child
        }
    }

    /// Returns `true` if a new cell with the given key length can fit in this page.
    pub fn has_space_for(&self, key_len: usize, page_size: usize) -> bool {
        let cell_size = 10 + key_len; // left_child(8) + key_len(2) + key
        // Compute total space needed if this cell were added
        let existing_cell_bytes: usize = self.cells.iter().map(|c| c.serialized_size()).sum();
        let total_ptrs = (self.cells.len() + 1) * 2;
        let total_used = CELL_PTRS_OFFSET + total_ptrs + existing_cell_bytes + cell_size;
        total_used <= page_size
    }

    /// Splits the page at the median key.
    ///
    /// Returns `(left_cells, median_key, right_cells, right_right_child)`.
    /// The median key is promoted to the parent interior page.
    /// - Left cells keep the original left children.
    /// - Right cells keep their left children; `right_right_child` is `self.right_child`.
    pub fn split(
        &self,
    ) -> (Vec<InteriorCell>, Vec<u8>, Vec<InteriorCell>, PageId) {
        let mid = self.cells.len() / 2;
        let left = self.cells[..mid].to_vec();
        let median = self.cells[mid].key.clone();
        // The median cell's left_child becomes the right_child of the left page.
        // Not needed in return — the caller uses left_right_child = cells[mid].left_child
        // Actually: the left page's right_child = median_cell.left_child is wrong.
        // Correct split:
        //   left page: cells[0..mid], right_child = cells[mid].left_child
        //   median key: cells[mid].key (promoted up)
        //   right page: cells[mid+1..], right_child = original right_child
        let _left_right_child = self.cells[mid].left_child;
        let right = self.cells[mid + 1..].to_vec();
        // Return format: (left_cells, median_key, right_cells, right_page_right_child)
        // The caller must set left_page.right_child = cells[mid].left_child
        // We pack left_right_child info: the caller should know cells[mid].left_child
        // Let's return a 5-tuple via a struct or just document it.
        // For simplicity, return the left_right_child as well.
        (left, median, right, self.right_child)
    }

    /// Returns the left page's right_child after a split (the median cell's left_child).
    ///
    /// Must be called with the same cells as `split()`.
    pub fn split_left_right_child(&self) -> PageId {
        let mid = self.cells.len() / 2;
        self.cells[mid].left_child
    }

    /// Returns the cells in key order.
    pub fn cells(&self) -> &[InteriorCell] {
        &self.cells
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::page::DEFAULT_PAGE_SIZE;

    fn make_cell(left_child: u64, key: &[u8]) -> InteriorCell {
        InteriorCell {
            left_child: PageId(left_child),
            key: key.to_vec(),
        }
    }

    #[test]
    fn build_parse_round_trip() {
        let cells = vec![
            make_cell(10, &[0x00, 0x01]),
            make_cell(11, &[0x00, 0x05]),
            make_cell(12, &[0x00, 0x0A]),
        ];
        let right_child = PageId(13);
        let page_data =
            InteriorPage::build(PageId(2), 1, &cells, right_child, DEFAULT_PAGE_SIZE).unwrap();

        assert_eq!(page_data.len(), DEFAULT_PAGE_SIZE);
        CommonPageHeader::validate_checksum(&page_data).unwrap();

        let parsed = InteriorPage::parse(&page_data, DEFAULT_PAGE_SIZE).unwrap();
        assert_eq!(parsed.cell_count, 3);
        assert_eq!(parsed.right_child, right_child);
        assert_eq!(parsed.cells().len(), 3);
        for (i, cell) in parsed.cells().iter().enumerate() {
            assert_eq!(cell.left_child, cells[i].left_child);
            assert_eq!(cell.key, cells[i].key);
        }
    }

    #[test]
    fn search_less_than_first() {
        let cells = vec![
            make_cell(10, &[0x00, 0x05]),
            make_cell(11, &[0x00, 0x0A]),
        ];
        let page_data =
            InteriorPage::build(PageId(2), 1, &cells, PageId(12), DEFAULT_PAGE_SIZE).unwrap();
        let page = InteriorPage::parse(&page_data, DEFAULT_PAGE_SIZE).unwrap();

        // Key less than first separator → first cell's left_child
        assert_eq!(page.search(&[0x00, 0x01]), PageId(10));
    }

    #[test]
    fn search_between_separators() {
        let cells = vec![
            make_cell(10, &[0x00, 0x05]),
            make_cell(11, &[0x00, 0x0A]),
        ];
        let page_data =
            InteriorPage::build(PageId(2), 1, &cells, PageId(12), DEFAULT_PAGE_SIZE).unwrap();
        let page = InteriorPage::parse(&page_data, DEFAULT_PAGE_SIZE).unwrap();

        // Key between separators → second cell's left_child
        assert_eq!(page.search(&[0x00, 0x07]), PageId(11));
    }

    #[test]
    fn search_greater_than_all() {
        let cells = vec![
            make_cell(10, &[0x00, 0x05]),
            make_cell(11, &[0x00, 0x0A]),
        ];
        let page_data =
            InteriorPage::build(PageId(2), 1, &cells, PageId(12), DEFAULT_PAGE_SIZE).unwrap();
        let page = InteriorPage::parse(&page_data, DEFAULT_PAGE_SIZE).unwrap();

        // Key >= all separators → right_child
        assert_eq!(page.search(&[0x00, 0xFF]), PageId(12));
    }

    #[test]
    fn search_equal_to_separator() {
        let cells = vec![
            make_cell(10, &[0x00, 0x05]),
            make_cell(11, &[0x00, 0x0A]),
        ];
        let page_data =
            InteriorPage::build(PageId(2), 1, &cells, PageId(12), DEFAULT_PAGE_SIZE).unwrap();
        let page = InteriorPage::parse(&page_data, DEFAULT_PAGE_SIZE).unwrap();

        // Key == separator → key >= cells[0].key, so go to cells[1].left_child
        assert_eq!(page.search(&[0x00, 0x05]), PageId(11));
        // Key == last separator → go to right_child
        assert_eq!(page.search(&[0x00, 0x0A]), PageId(12));
    }

    #[test]
    fn has_space_for_full_page() {
        // Fill a page with enough cells to make it full
        let mut cells = Vec::new();
        // Each cell: 10 + 8 = 18 bytes + 2 byte pointer = 20 bytes effective
        // Available: 4096 - 38 = 4058 bytes. ~4058/20 = ~202 cells
        for i in 0..200u64 {
            cells.push(make_cell(i, &i.to_be_bytes()));
        }
        let page_data =
            InteriorPage::build(PageId(2), 1, &cells, PageId(999), DEFAULT_PAGE_SIZE).unwrap();
        let page = InteriorPage::parse(&page_data, DEFAULT_PAGE_SIZE).unwrap();

        // Should still have some space
        let has_room = page.has_space_for(8, DEFAULT_PAGE_SIZE);
        // After 200 cells: ptrs = 38 + 200*2 = 438; content = 200*18 = 3600; free_start = 4096 - 3600 = 496
        // Space between ptrs end and free_start: 496 - 438 = 58 bytes. 18+2 = 20 fits.
        assert!(has_room);
    }

    #[test]
    fn split_balanced() {
        let cells: Vec<InteriorCell> = (0..6u64)
            .map(|i| make_cell(i + 10, &i.to_be_bytes()))
            .collect();
        let page_data =
            InteriorPage::build(PageId(2), 1, &cells, PageId(99), DEFAULT_PAGE_SIZE).unwrap();
        let page = InteriorPage::parse(&page_data, DEFAULT_PAGE_SIZE).unwrap();

        let (left, median, right, right_right_child) = page.split();
        let left_right_child = page.split_left_right_child();

        // 6 cells → mid=3 → left=[0,1,2], median=cells[3].key, right=[4,5]
        assert_eq!(left.len(), 3);
        assert_eq!(right.len(), 2);
        assert_eq!(median, cells[3].key);
        assert_eq!(left_right_child, cells[3].left_child);
        assert_eq!(right_right_child, PageId(99));
    }

    #[test]
    fn byte_level_layout() {
        let cells = vec![make_cell(10, &[0xAB, 0xCD])];
        let page_data =
            InteriorPage::build(PageId(5), 42, &cells, PageId(20), DEFAULT_PAGE_SIZE).unwrap();

        // Check page_id at offset 0 (u64 LE)
        assert_eq!(
            u64::from_le_bytes(page_data[0..8].try_into().unwrap()),
            5
        );
        // page_type at offset 8
        assert_eq!(page_data[8], 0x01); // Interior
        // flags at offset 9
        assert_eq!(page_data[9], 0x00); // is_leaf = 0
        // txn_id at offset 12
        assert_eq!(
            u64::from_le_bytes(page_data[12..20].try_into().unwrap()),
            42
        );
        // cell_count at offset 24 (u16 LE)
        assert_eq!(
            u16::from_le_bytes([page_data[24], page_data[25]]),
            1
        );
        // right_child at offset 26 (u64 LE)
        assert_eq!(
            u64::from_le_bytes(page_data[26..34].try_into().unwrap()),
            20
        );
    }
}
