//! Free page format.
//!
//! A free page has only the common 24-byte header with `page_type = Free`.
//! The rest of the page is zero-filled.
//! Layout follows `008-file-format-spec.md` §10.

use crate::error::StorageError;

use super::header::CommonPageHeader;
use super::{PageId, PageType};

/// A parsed free page. Contains only the common header.
#[derive(Clone, Debug)]
pub struct FreePage {
    /// Common 24-byte page header.
    pub header: CommonPageHeader,
}

impl FreePage {
    /// Builds a free page image: header with `page_type = Free`, rest zero-filled.
    ///
    /// Returns a `Vec<u8>` of exactly `page_size` bytes.
    pub fn build(page_id: PageId, txn_id: u64, page_size: usize) -> Vec<u8> {
        let mut page = vec![0u8; page_size];

        let header = CommonPageHeader {
            page_id,
            page_type: PageType::Free,
            flags: 0,
            txn_id,
            checksum: 0,
        };
        header.serialize(&mut page);
        let checksum = CommonPageHeader::compute_checksum(&page);
        page[20..24].copy_from_slice(&checksum.to_le_bytes());

        page
    }

    /// Parses a free page from raw page bytes.
    ///
    /// # Errors
    ///
    /// Returns an error if the page type is not `Free`.
    pub fn parse(page_data: &[u8]) -> Result<Self, StorageError> {
        let header = CommonPageHeader::deserialize(page_data)?;
        if header.page_type != PageType::Free {
            return Err(StorageError {
                message: format!("expected Free page, got {:?}", header.page_type),
                source: None,
            });
        }
        Ok(Self { header })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::page::DEFAULT_PAGE_SIZE;
    use crate::storage::page::header::CommonPageHeader;

    #[test]
    fn build_parse_round_trip() {
        let page_data = FreePage::build(PageId(42), 5, DEFAULT_PAGE_SIZE);
        assert_eq!(page_data.len(), DEFAULT_PAGE_SIZE);

        CommonPageHeader::validate_checksum(&page_data).unwrap();

        let parsed = FreePage::parse(&page_data).unwrap();
        assert_eq!(parsed.header.page_id, PageId(42));
        assert_eq!(parsed.header.page_type, PageType::Free);
        assert_eq!(parsed.header.txn_id, 5);
    }

    #[test]
    fn rest_is_zeros() {
        let page_data = FreePage::build(PageId(42), 5, DEFAULT_PAGE_SIZE);
        // Everything after the 24-byte header should be zero
        for &b in &page_data[24..] {
            assert_eq!(b, 0);
        }
    }

    #[test]
    fn reject_non_free_page() {
        let mut page_data = FreePage::build(PageId(42), 5, DEFAULT_PAGE_SIZE);
        page_data[8] = 0x01; // change to Interior
        // Fix checksum so the header parse doesn't fail on checksum
        let checksum = CommonPageHeader::compute_checksum(&page_data);
        page_data[20..24].copy_from_slice(&checksum.to_le_bytes());

        let err = FreePage::parse(&page_data).unwrap_err();
        assert!(err.message.contains("expected Free"));
    }
}
