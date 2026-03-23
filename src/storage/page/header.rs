//! Common 24-byte page header shared by all data page types.
//!
//! Every data page (pages 2+) begins with this header. It provides
//! page identification, type discrimination, transaction tracking,
//! and CRC32C integrity checking.

use crate::error::StorageError;

use super::{COMMON_HEADER_SIZE, PageId, PageType};

/// The 24-byte common header present at the start of every data page.
///
/// # Layout (per `008-file-format-spec.md` §5)
///
/// | Offset | Size | Field     | Encoding |
/// |--------|------|-----------|----------|
/// | 0      | 8    | page_id   | u64 LE   |
/// | 8      | 1    | page_type | u8       |
/// | 9      | 1    | flags     | u8       |
/// | 10     | 2    | _padding  | must be 0|
/// | 12     | 8    | txn_id    | u64 LE   |
/// | 20     | 4    | checksum  | u32 LE (CRC32C) |
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommonPageHeader {
    /// The page's own page number (self-referential for corruption detection).
    pub page_id: PageId,
    /// The type of data stored in this page.
    pub page_type: PageType,
    /// Page-type-specific flags. Bit 0: `is_leaf` for B-tree pages.
    pub flags: u8,
    /// Transaction ID that created this page version.
    pub txn_id: u64,
    /// CRC32C checksum of the entire page (with checksum field zeroed).
    pub checksum: u32,
}

impl CommonPageHeader {
    /// Serializes this header into the first 24 bytes of `buf`.
    ///
    /// The checksum field is written as-is; the caller is responsible
    /// for computing it via [`Self::compute_checksum`] before serializing.
    ///
    /// # Panics
    ///
    /// Panics if `buf.len() < 24`.
    pub fn serialize(&self, buf: &mut [u8]) {
        assert!(buf.len() >= COMMON_HEADER_SIZE);
        buf[0..8].copy_from_slice(&self.page_id.0.to_le_bytes());
        buf[8] = self.page_type as u8;
        buf[9] = self.flags;
        buf[10] = 0; // padding
        buf[11] = 0; // padding
        buf[12..20].copy_from_slice(&self.txn_id.to_le_bytes());
        buf[20..24].copy_from_slice(&self.checksum.to_le_bytes());
    }

    /// Deserializes a header from the first 24 bytes of `buf`.
    ///
    /// # Errors
    ///
    /// - Returns an error if the padding bytes (offset 10–11) are non-zero.
    /// - Returns an error if `page_type` is not a recognized value.
    /// - Returns an error if `buf.len() < 24`.
    pub fn deserialize(buf: &[u8]) -> Result<Self, StorageError> {
        if buf.len() < COMMON_HEADER_SIZE {
            return Err(StorageError {
                message: format!(
                    "page header too short: {} bytes, need {}",
                    buf.len(),
                    COMMON_HEADER_SIZE
                ),
                source: None,
            });
        }

        let padding = u16::from_le_bytes([buf[10], buf[11]]);
        if padding != 0 {
            return Err(StorageError {
                message: format!("page header padding bytes are non-zero: {padding:#06x}"),
                source: None,
            });
        }

        let page_type_byte = buf[8];
        let page_type = PageType::try_from(page_type_byte).map_err(|v| StorageError {
            message: format!("unknown page type: {v:#04x}"),
            source: None,
        })?;

        Ok(Self {
            page_id: PageId(u64::from_le_bytes(buf[0..8].try_into().unwrap())),
            page_type,
            flags: buf[9],
            txn_id: u64::from_le_bytes(buf[12..20].try_into().unwrap()),
            checksum: u32::from_le_bytes(buf[20..24].try_into().unwrap()),
        })
    }

    /// Computes the CRC32C checksum over an entire page, treating the
    /// checksum field (bytes 20–23) as zeros.
    ///
    /// The checksum covers the full `page_data` slice (which should be
    /// exactly `page_size` bytes).
    pub fn compute_checksum(page_data: &[u8]) -> u32 {
        let mut hasher = crc32fast::Hasher::new();
        // Hash bytes 0–19 (before checksum field)
        hasher.update(&page_data[..20]);
        // Hash 4 zero bytes in place of the checksum field
        hasher.update(&[0u8; 4]);
        // Hash bytes 24 to end (after checksum field)
        if page_data.len() > COMMON_HEADER_SIZE {
            hasher.update(&page_data[COMMON_HEADER_SIZE..]);
        }
        hasher.finalize()
    }

    /// Validates the CRC32C checksum of an entire page.
    ///
    /// # Errors
    ///
    /// Returns a `StorageError` with `MediaCorruption` semantics if the
    /// stored checksum does not match the computed checksum.
    pub fn validate_checksum(page_data: &[u8]) -> Result<(), StorageError> {
        if page_data.len() < COMMON_HEADER_SIZE {
            return Err(StorageError {
                message: format!(
                    "page too short for checksum validation: {} bytes",
                    page_data.len()
                ),
                source: None,
            });
        }
        let stored = u32::from_le_bytes(page_data[20..24].try_into().unwrap());
        let computed = Self::compute_checksum(page_data);
        if stored != computed {
            return Err(StorageError {
                message: format!(
                    "CRC32C checksum mismatch: stored={stored:#010x}, computed={computed:#010x}"
                ),
                source: None,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::page::DEFAULT_PAGE_SIZE;

    #[test]
    fn serialize_deserialize_round_trip() {
        let header = CommonPageHeader {
            page_id: PageId(42),
            page_type: PageType::Leaf,
            flags: 0x01,
            txn_id: 100,
            checksum: 0xDEADBEEF,
        };
        let mut buf = [0u8; COMMON_HEADER_SIZE];
        header.serialize(&mut buf);
        let deserialized = CommonPageHeader::deserialize(&buf).unwrap();
        assert_eq!(header, deserialized);
    }

    #[test]
    fn checksum_deterministic() {
        let mut page = vec![0u8; DEFAULT_PAGE_SIZE];
        page[0] = 42;
        page[100] = 0xFF;
        let c1 = CommonPageHeader::compute_checksum(&page);
        let c2 = CommonPageHeader::compute_checksum(&page);
        assert_eq!(c1, c2);
        assert_ne!(c1, 0); // extremely unlikely to be zero for non-trivial data
    }

    #[test]
    fn checksum_validation_success() {
        let mut page = vec![0u8; DEFAULT_PAGE_SIZE];
        // Write a valid header
        let header = CommonPageHeader {
            page_id: PageId(2),
            page_type: PageType::Interior,
            flags: 0,
            txn_id: 1,
            checksum: 0, // placeholder
        };
        header.serialize(&mut page);
        // Compute and write the real checksum
        let checksum = CommonPageHeader::compute_checksum(&page);
        page[20..24].copy_from_slice(&checksum.to_le_bytes());

        assert!(CommonPageHeader::validate_checksum(&page).is_ok());
    }

    #[test]
    fn checksum_validation_failure() {
        let mut page = vec![0u8; DEFAULT_PAGE_SIZE];
        let header = CommonPageHeader {
            page_id: PageId(2),
            page_type: PageType::Interior,
            flags: 0,
            txn_id: 1,
            checksum: 0,
        };
        header.serialize(&mut page);
        let checksum = CommonPageHeader::compute_checksum(&page);
        page[20..24].copy_from_slice(&checksum.to_le_bytes());

        // Flip a bit in the payload
        page[50] ^= 0x01;

        let err = CommonPageHeader::validate_checksum(&page).unwrap_err();
        assert!(err.message.contains("checksum mismatch"));
    }

    #[test]
    fn reject_nonzero_padding() {
        let mut buf = [0u8; COMMON_HEADER_SIZE];
        let header = CommonPageHeader {
            page_id: PageId(1),
            page_type: PageType::Leaf,
            flags: 0,
            txn_id: 0,
            checksum: 0,
        };
        header.serialize(&mut buf);
        buf[10] = 0xFF; // corrupt padding
        let err = CommonPageHeader::deserialize(&buf).unwrap_err();
        assert!(err.message.contains("padding"));
    }

    #[test]
    fn reject_unknown_page_type() {
        let mut buf = [0u8; COMMON_HEADER_SIZE];
        buf[8] = 0x00; // invalid page type
        let err = CommonPageHeader::deserialize(&buf).unwrap_err();
        assert!(err.message.contains("unknown page type"));

        buf[8] = 0x05; // also invalid
        let err = CommonPageHeader::deserialize(&buf).unwrap_err();
        assert!(err.message.contains("unknown page type"));
    }

    #[test]
    fn all_page_types_round_trip() {
        for pt in [
            PageType::Interior,
            PageType::Leaf,
            PageType::Overflow,
            PageType::Free,
        ] {
            let header = CommonPageHeader {
                page_id: PageId(10),
                page_type: pt,
                flags: 0,
                txn_id: 5,
                checksum: 123,
            };
            let mut buf = [0u8; COMMON_HEADER_SIZE];
            header.serialize(&mut buf);
            let rt = CommonPageHeader::deserialize(&buf).unwrap();
            assert_eq!(header, rt);
        }
    }
}
