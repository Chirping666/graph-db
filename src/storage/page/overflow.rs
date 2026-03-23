//! Overflow page format for large record values.
//!
//! When a leaf cell's value exceeds the overflow threshold, it is
//! stored in a chain of overflow pages. Each page carries a payload
//! and a pointer to the next page in the chain.
//! Layout follows `008-file-format-spec.md` §9.

use crate::error::StorageError;
use crate::hal::ReadAt;
use crate::storage::map_hal_err;

use super::header::CommonPageHeader;
use super::{PageId, PageType};

/// Total header size for overflow pages: common(24) + next_page(8) + data_length(4) = 36.
const OVERFLOW_HEADER_SIZE: usize = 36;

/// A parsed overflow page.
#[derive(Clone, Debug)]
pub struct OverflowPage {
    /// Common 24-byte page header.
    pub header: CommonPageHeader,
    /// Page ID of the next overflow page in the chain (0 = last page).
    pub next_page: PageId,
    /// Number of payload bytes stored in this page.
    pub data_length: u32,
    /// The payload data.
    pub data: Vec<u8>,
}

impl OverflowPage {
    /// Parses an overflow page from raw page bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the header is malformed or page type is not `Overflow`.
    pub fn parse(page_data: &[u8], page_size: usize) -> Result<Self, StorageError> {
        if page_data.len() < page_size || page_size < OVERFLOW_HEADER_SIZE {
            return Err(StorageError {
                message: format!("overflow page too small: {} bytes", page_data.len()),
                source: None,
            });
        }

        let header = CommonPageHeader::deserialize(page_data)?;
        if header.page_type != PageType::Overflow {
            return Err(StorageError {
                message: format!("expected Overflow page, got {:?}", header.page_type),
                source: None,
            });
        }

        let next_page = PageId(u64::from_le_bytes(
            page_data[24..32].try_into().unwrap(),
        ));
        let data_length = u32::from_le_bytes(
            page_data[32..36].try_into().unwrap(),
        );

        let max = Self::max_payload(page_size);
        if data_length as usize > max {
            return Err(StorageError {
                message: format!(
                    "overflow data_length {data_length} exceeds max payload {max}"
                ),
                source: None,
            });
        }

        let data = page_data[OVERFLOW_HEADER_SIZE..OVERFLOW_HEADER_SIZE + data_length as usize]
            .to_vec();

        Ok(Self {
            header,
            next_page,
            data_length,
            data,
        })
    }

    /// Builds a single overflow page image with the correct header and checksum.
    ///
    /// Returns a `Vec<u8>` of exactly `page_size` bytes.
    ///
    /// # Panics
    ///
    /// Panics if `data.len()` exceeds [`max_payload`](Self::max_payload).
    pub fn build(
        page_id: PageId,
        txn_id: u64,
        next_page: PageId,
        data: &[u8],
        page_size: usize,
    ) -> Vec<u8> {
        let max = Self::max_payload(page_size);
        assert!(
            data.len() <= max,
            "overflow data length {} exceeds max payload {max}",
            data.len()
        );

        let mut page = vec![0u8; page_size];

        // Subheader
        page[24..32].copy_from_slice(&next_page.0.to_le_bytes());
        let data_length = data.len() as u32;
        page[32..36].copy_from_slice(&data_length.to_le_bytes());
        page[OVERFLOW_HEADER_SIZE..OVERFLOW_HEADER_SIZE + data.len()].copy_from_slice(data);

        // Header
        let header = CommonPageHeader {
            page_id,
            page_type: PageType::Overflow,
            flags: 0,
            txn_id,
            checksum: 0,
        };
        header.serialize(&mut page);
        let checksum = CommonPageHeader::compute_checksum(&page);
        page[20..24].copy_from_slice(&checksum.to_le_bytes());

        page
    }

    /// Returns the maximum payload bytes per overflow page.
    pub fn max_payload(page_size: usize) -> usize {
        page_size - OVERFLOW_HEADER_SIZE
    }

    /// Builds a chain of overflow pages from a data buffer.
    ///
    /// Each `page_ids[i]` is assigned to one page in the chain, linked
    /// sequentially. Returns one page image per entry.
    ///
    /// # Panics
    ///
    /// Panics if `page_ids` is empty or insufficient for the data.
    pub fn build_chain(
        page_ids: &[PageId],
        txn_id: u64,
        data: &[u8],
        page_size: usize,
    ) -> Vec<Vec<u8>> {
        let max = Self::max_payload(page_size);
        let needed = data.len().div_ceil(max);
        assert!(
            page_ids.len() >= needed,
            "need {needed} overflow pages but only {} IDs provided",
            page_ids.len()
        );

        let mut pages = Vec::with_capacity(needed);
        let mut offset = 0;

        for i in 0..needed {
            let end = (offset + max).min(data.len());
            let chunk = &data[offset..end];
            let next_page = if i + 1 < needed {
                page_ids[i + 1]
            } else {
                PageId::NULL
            };
            pages.push(Self::build(page_ids[i], txn_id, next_page, chunk, page_size));
            offset = end;
        }

        pages
    }

    /// Reads an overflow chain from the backend, reconstructing the full data.
    ///
    /// Follows `next_page` pointers starting from `first_page` until the
    /// chain ends (`next_page == 0`). Validates that the total reconstructed
    /// length matches `expected_total`.
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure, checksum mismatch, or length mismatch.
    pub fn read_chain<B: ReadAt>(
        backend: &B,
        first_page: PageId,
        expected_total: u32,
        page_size: usize,
    ) -> Result<Vec<u8>, StorageError> {
        let mut result = Vec::with_capacity(expected_total as usize);
        let mut current = first_page;
        let mut buf = vec![0u8; page_size];

        while !current.is_null() {
            backend
                .read_at(current.0 * page_size as u64, &mut buf)
                .map_err(map_hal_err)?;
            CommonPageHeader::validate_checksum(&buf)?;
            let page = Self::parse(&buf, page_size)?;
            result.extend_from_slice(&page.data);
            current = page.next_page;
        }

        if result.len() != expected_total as usize {
            return Err(StorageError {
                message: format!(
                    "overflow chain length mismatch: got {}, expected {expected_total}",
                    result.len()
                ),
                source: None,
            });
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::page::DEFAULT_PAGE_SIZE;
    use crate::hal::WriteAt;
    use crate::storage::test_utils::TestBackend;

    #[test]
    fn single_page_round_trip() {
        let data = b"hello overflow world";
        let page_data = OverflowPage::build(
            PageId(10), 1, PageId::NULL, data, DEFAULT_PAGE_SIZE,
        );

        assert_eq!(page_data.len(), DEFAULT_PAGE_SIZE);
        CommonPageHeader::validate_checksum(&page_data).unwrap();

        let parsed = OverflowPage::parse(&page_data, DEFAULT_PAGE_SIZE).unwrap();
        assert_eq!(parsed.next_page, PageId::NULL);
        assert_eq!(parsed.data_length, data.len() as u32);
        assert_eq!(parsed.data, data);
    }

    #[test]
    fn chain_of_three() {
        let page_ids = [PageId(10), PageId(11), PageId(12)];
        // Create data that spans 3 pages
        let max = OverflowPage::max_payload(DEFAULT_PAGE_SIZE);
        let total_len = max * 2 + 100;
        let data: Vec<u8> = (0..total_len).map(|i| (i % 256) as u8).collect();

        let pages = OverflowPage::build_chain(&page_ids, 1, &data, DEFAULT_PAGE_SIZE);
        assert_eq!(pages.len(), 3);

        // Verify linking
        let p0 = OverflowPage::parse(&pages[0], DEFAULT_PAGE_SIZE).unwrap();
        let p1 = OverflowPage::parse(&pages[1], DEFAULT_PAGE_SIZE).unwrap();
        let p2 = OverflowPage::parse(&pages[2], DEFAULT_PAGE_SIZE).unwrap();

        assert_eq!(p0.next_page, PageId(11));
        assert_eq!(p1.next_page, PageId(12));
        assert_eq!(p2.next_page, PageId::NULL);
    }

    #[test]
    fn read_chain_reconstructs_data() {
        let page_ids = [PageId(10), PageId(11), PageId(12)];
        let max = OverflowPage::max_payload(DEFAULT_PAGE_SIZE);
        let total_len = max * 2 + 100;
        let data: Vec<u8> = (0..total_len).map(|i| (i % 256) as u8).collect();

        let pages = OverflowPage::build_chain(&page_ids, 1, &data, DEFAULT_PAGE_SIZE);

        // Write pages to a TestBackend
        let mut backend = TestBackend::new();
        for (i, page) in pages.iter().enumerate() {
            let offset = page_ids[i].0 * DEFAULT_PAGE_SIZE as u64;
            backend.set_len(offset + DEFAULT_PAGE_SIZE as u64).unwrap();
            backend.write_at(offset, page).unwrap();
        }

        let reconstructed = OverflowPage::read_chain(
            &backend,
            PageId(10),
            total_len as u32,
            DEFAULT_PAGE_SIZE,
        )
        .unwrap();
        assert_eq!(reconstructed, data);
    }

    #[test]
    fn read_chain_wrong_total() {
        let data = b"some data";
        let page_data = OverflowPage::build(
            PageId(10), 1, PageId::NULL, data, DEFAULT_PAGE_SIZE,
        );

        let mut backend = TestBackend::new();
        let offset = 10 * DEFAULT_PAGE_SIZE as u64;
        backend.set_len(offset + DEFAULT_PAGE_SIZE as u64).unwrap();
        backend.write_at(offset, &page_data).unwrap();

        let result = OverflowPage::read_chain(&backend, PageId(10), 999, DEFAULT_PAGE_SIZE);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("length mismatch"));
    }

    #[test]
    fn max_payload_4096() {
        assert_eq!(OverflowPage::max_payload(4096), 4060);
    }
}
