//! File identity header and dual-superblock implementation.
//!
//! Handles database file creation, opening, superblock validation,
//! and active superblock selection. Layout follows
//! `008-file-format-spec.md` §§3–4.

use alloc::{format, vec, vec::Vec};

use crate::error::StorageError;
use crate::backend::{ReadAt, StorageBackend};

use super::map_backend_err;
use super::page::leaf::LeafPage;
use super::page::{IDENTITY_HEADER_SIZE, PageId, SUPERBLOCK_USED_SIZE};

// ---- File Identity Header ----

/// The 14-byte magic sequence: `"EmbedGraph\r\n\x1A\n"`.
pub const MAGIC: &[u8; 14] = b"EmbedGraph\r\n\x1a\n";

/// Current format major version.
pub const FORMAT_MAJOR: u16 = 1;

/// Current format minor version.
pub const FORMAT_MINOR: u16 = 0;

/// The 32-byte file identity header, present at the start of both superblock pages.
///
/// This region is written once at database creation and never changes.
///
/// # Layout (per `008-file-format-spec.md` §3)
///
/// | Offset | Size | Field              | Encoding |
/// |--------|------|--------------------|----------|
/// | 0      | 14   | magic              | bytes    |
/// | 14     | 2    | format_major       | u16 BE   |
/// | 16     | 2    | format_minor       | u16 BE   |
/// | 18     | 4    | application_id     | u32 LE   |
/// | 22     | 2    | page_size_raw      | u16 LE   |
/// | 24     | 8    | creation_timestamp | u64 LE (microseconds since Unix epoch) |
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileIdentityHeader {
    /// Magic bytes (must be [`MAGIC`]).
    pub magic: [u8; 14],
    /// Major format version.
    pub format_major: u16,
    /// Minor format version.
    pub format_minor: u16,
    /// Application identifier (0 = default).
    pub application_id: u32,
    /// Raw page size encoding. Use [`page_size()`](Self::page_size) to decode.
    pub page_size_raw: u16,
    /// Creation timestamp in microseconds since Unix epoch.
    pub creation_timestamp: u64,
}

impl FileIdentityHeader {
    /// Creates a new identity header for the given page size and application ID.
    ///
    /// # Errors
    ///
    /// Returns an error if `page_size` is not a valid page size.
    pub fn new(page_size: usize, application_id: u32) -> Result<Self, StorageError> {
        let page_size_raw = encode_page_size(page_size)?;
        #[cfg(feature = "std")]
        let creation_timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64;
        #[cfg(not(feature = "std"))]
        let creation_timestamp: u64 = 0;

        Ok(Self {
            magic: *MAGIC,
            format_major: FORMAT_MAJOR,
            format_minor: FORMAT_MINOR,
            application_id,
            page_size_raw,
            creation_timestamp,
        })
    }

    /// Serializes this header into the first 32 bytes of `buf`.
    ///
    /// # Panics
    ///
    /// Panics if `buf.len() < 32`.
    pub fn serialize(&self, buf: &mut [u8]) {
        assert!(buf.len() >= IDENTITY_HEADER_SIZE);
        buf[0..14].copy_from_slice(&self.magic);
        buf[14..16].copy_from_slice(&self.format_major.to_be_bytes());
        buf[16..18].copy_from_slice(&self.format_minor.to_be_bytes());
        buf[18..22].copy_from_slice(&self.application_id.to_le_bytes());
        buf[22..24].copy_from_slice(&self.page_size_raw.to_le_bytes());
        buf[24..32].copy_from_slice(&self.creation_timestamp.to_le_bytes());
    }

    /// Deserializes a header from the first 32 bytes of `buf`.
    ///
    /// # Errors
    ///
    /// Returns `MediaCorruption` if the magic bytes do not match.
    pub fn deserialize(buf: &[u8]) -> Result<Self, StorageError> {
        if buf.len() < IDENTITY_HEADER_SIZE {
            return Err(StorageError {
                message: format!(
                    "identity header too short: {} bytes, need {IDENTITY_HEADER_SIZE}",
                    buf.len()
                ),
                #[cfg(feature = "std")]
                source: None,
            });
        }

        let mut magic = [0u8; 14];
        magic.copy_from_slice(&buf[0..14]);
        if magic != *MAGIC {
            return Err(StorageError {
                message: "invalid file identity header: magic bytes do not match".into(),
                #[cfg(feature = "std")]
                source: None,
            });
        }

        Ok(Self {
            magic,
            format_major: u16::from_be_bytes([buf[14], buf[15]]),
            format_minor: u16::from_be_bytes([buf[16], buf[17]]),
            application_id: u32::from_le_bytes(buf[18..22].try_into().unwrap()),
            page_size_raw: u16::from_le_bytes([buf[22], buf[23]]),
            creation_timestamp: u64::from_le_bytes(buf[24..32].try_into().unwrap()),
        })
    }

    /// Returns the page size in bytes decoded from `page_size_raw`.
    ///
    /// # Errors
    ///
    /// Returns an error if the raw value is invalid.
    pub fn page_size(&self) -> Result<usize, StorageError> {
        decode_page_size(self.page_size_raw)
    }

    /// Validates that this header's format version is compatible with this reader.
    ///
    /// # Errors
    ///
    /// Returns an error if `format_major` exceeds the supported version.
    pub fn validate_compatible(&self) -> Result<(), StorageError> {
        if self.format_major > FORMAT_MAJOR {
            return Err(StorageError {
                message: format!(
                    "unsupported format version: {}.{} (this reader supports up to {FORMAT_MAJOR}.{FORMAT_MINOR})",
                    self.format_major, self.format_minor
                ),
                #[cfg(feature = "std")]
                source: None,
            });
        }
        Ok(())
    }
}

/// Encodes a page size into the `page_size_raw` u16 field.
///
/// # Errors
///
/// Returns an error if `page_size` is not a valid power of 2 in `{4096, 8192, 16384, 32768, 65536}`.
fn encode_page_size(page_size: usize) -> Result<u16, StorageError> {
    match page_size {
        4096 | 8192 | 16384 | 32768 => Ok(page_size as u16),
        65536 => Ok(1), // SQLite convention
        _ => Err(StorageError {
            message: format!("invalid page size: {page_size}"),
            #[cfg(feature = "std")]
            source: None,
        }),
    }
}

/// Decodes a `page_size_raw` u16 field into a page size in bytes.
///
/// # Errors
///
/// Returns an error if the value is not a recognized encoding.
fn decode_page_size(raw: u16) -> Result<usize, StorageError> {
    match raw {
        1 => Ok(65536),
        4096 | 8192 | 16384 | 32768 => Ok(raw as usize),
        _ => Err(StorageError {
            message: format!("invalid page_size_raw value: {raw}"),
            #[cfg(feature = "std")]
            source: None,
        }),
    }
}

// ---- Superblock ----

/// Offset of the checksum field within the superblock page.
const SB_CHECKSUM_OFFSET: usize = 184;

/// The dual-superblock structure containing all mutable database state.
///
/// # Layout (per `008-file-format-spec.md` §4)
///
/// Bytes 0–31: File Identity Header (immutable).
/// Bytes 32–183: Mutable fields (transaction_id, total_pages, feature_flags,
///   8 B-tree root pointers, reserved space).
/// Bytes 184–191: xxHash3 checksum (u64 LE) over bytes 0–183.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Superblock {
    /// Monotonically increasing commit counter.
    pub transaction_id: u64,
    /// Total number of pages in the file (including superblock pages).
    pub total_pages: u64,
    /// Optional capability flags.
    pub feature_flags: u64,
    /// Node Store B-tree root page ID (0 = empty).
    pub root_node_store: PageId,
    /// Edge Store B-tree root page ID.
    pub root_edge_store: PageId,
    /// Outgoing Adjacency Index root.
    pub root_outgoing_adj: PageId,
    /// Incoming Adjacency Index root.
    pub root_incoming_adj: PageId,
    /// Type Index root.
    pub root_type_index: PageId,
    /// Schema Store root.
    pub root_schema_store: PageId,
    /// ID Freelist root.
    pub root_id_freelist: PageId,
    /// Page Freelist root.
    pub root_page_freelist: PageId,
    /// xxHash3 checksum over bytes 0–183 of the superblock page.
    pub checksum: u64,
}

impl Superblock {
    /// Serializes mutable fields into the page buffer starting at `SB_MUTABLE_OFFSET`.
    ///
    /// The identity header (bytes 0–31) must already be written.
    /// The checksum is written as-is; call [`compute_checksum`](Self::compute_checksum) first.
    ///
    /// # Panics
    ///
    /// Panics if `buf.len() < SUPERBLOCK_USED_SIZE`.
    pub fn serialize(&self, buf: &mut [u8]) {
        assert!(buf.len() >= SUPERBLOCK_USED_SIZE);

        buf[32..40].copy_from_slice(&self.transaction_id.to_le_bytes());
        buf[40..48].copy_from_slice(&self.total_pages.to_le_bytes());
        buf[48..56].copy_from_slice(&self.feature_flags.to_le_bytes());
        buf[56..64].copy_from_slice(&self.root_node_store.0.to_le_bytes());
        buf[64..72].copy_from_slice(&self.root_edge_store.0.to_le_bytes());
        buf[72..80].copy_from_slice(&self.root_outgoing_adj.0.to_le_bytes());
        buf[80..88].copy_from_slice(&self.root_incoming_adj.0.to_le_bytes());
        buf[88..96].copy_from_slice(&self.root_type_index.0.to_le_bytes());
        buf[96..104].copy_from_slice(&self.root_schema_store.0.to_le_bytes());
        buf[104..112].copy_from_slice(&self.root_id_freelist.0.to_le_bytes());
        buf[112..120].copy_from_slice(&self.root_page_freelist.0.to_le_bytes());
        // Reserved roots: bytes 120–151 must be zero (already zero in a fresh buffer)
        // Reserved fields: bytes 152–183 must be zero
        // Checksum at 184–191
        buf[SB_CHECKSUM_OFFSET..SB_CHECKSUM_OFFSET + 8]
            .copy_from_slice(&self.checksum.to_le_bytes());
    }

    /// Deserializes mutable fields from a superblock page buffer.
    ///
    /// # Errors
    ///
    /// Returns an error if the buffer is too short.
    pub fn deserialize(buf: &[u8]) -> Result<Self, StorageError> {
        if buf.len() < SUPERBLOCK_USED_SIZE {
            return Err(StorageError {
                message: format!(
                    "superblock buffer too short: {} bytes, need {SUPERBLOCK_USED_SIZE}",
                    buf.len()
                ),
                #[cfg(feature = "std")]
                source: None,
            });
        }

        Ok(Self {
            transaction_id: u64::from_le_bytes(buf[32..40].try_into().unwrap()),
            total_pages: u64::from_le_bytes(buf[40..48].try_into().unwrap()),
            feature_flags: u64::from_le_bytes(buf[48..56].try_into().unwrap()),
            root_node_store: PageId(u64::from_le_bytes(buf[56..64].try_into().unwrap())),
            root_edge_store: PageId(u64::from_le_bytes(buf[64..72].try_into().unwrap())),
            root_outgoing_adj: PageId(u64::from_le_bytes(buf[72..80].try_into().unwrap())),
            root_incoming_adj: PageId(u64::from_le_bytes(buf[80..88].try_into().unwrap())),
            root_type_index: PageId(u64::from_le_bytes(buf[88..96].try_into().unwrap())),
            root_schema_store: PageId(u64::from_le_bytes(buf[96..104].try_into().unwrap())),
            root_id_freelist: PageId(u64::from_le_bytes(buf[104..112].try_into().unwrap())),
            root_page_freelist: PageId(u64::from_le_bytes(buf[112..120].try_into().unwrap())),
            checksum: u64::from_le_bytes(
                buf[SB_CHECKSUM_OFFSET..SB_CHECKSUM_OFFSET + 8]
                    .try_into()
                    .unwrap(),
            ),
        })
    }

    /// Computes the xxHash3 checksum over bytes 0–183 of a superblock page,
    /// with the checksum field (bytes 184–191) treated as zeros.
    pub fn compute_checksum(page_buf: &[u8]) -> u64 {
        // Hash bytes 0–183 (the data before the checksum field)
        xxhash_rust::xxh3::xxh3_64(&page_buf[..SB_CHECKSUM_OFFSET])
    }

    /// Validates the superblock's magic bytes and checksum.
    ///
    /// # Errors
    ///
    /// Returns an error if magic or checksum is invalid.
    pub fn validate(page_buf: &[u8]) -> Result<(), StorageError> {
        if page_buf.len() < SUPERBLOCK_USED_SIZE {
            return Err(StorageError {
                message: "superblock page too short for validation".into(),
                #[cfg(feature = "std")]
                source: None,
            });
        }
        // Validate identity header magic
        if &page_buf[0..14] != MAGIC.as_slice() {
            return Err(StorageError {
                message: "superblock magic bytes do not match".into(),
                #[cfg(feature = "std")]
                source: None,
            });
        }
        // Validate checksum
        let stored = u64::from_le_bytes(
            page_buf[SB_CHECKSUM_OFFSET..SB_CHECKSUM_OFFSET + 8]
                .try_into()
                .unwrap(),
        );
        let computed = Self::compute_checksum(page_buf);
        if stored != computed {
            return Err(StorageError {
                message: format!(
                    "superblock checksum mismatch: stored={stored:#018x}, computed={computed:#018x}"
                ),
                #[cfg(feature = "std")]
                source: None,
            });
        }
        Ok(())
    }

    /// Returns all 8 root page IDs as an array in catalog order.
    pub fn root_page_ids(&self) -> [PageId; 8] {
        [
            self.root_node_store,
            self.root_edge_store,
            self.root_outgoing_adj,
            self.root_incoming_adj,
            self.root_type_index,
            self.root_schema_store,
            self.root_id_freelist,
            self.root_page_freelist,
        ]
    }

    /// Creates the initial superblock for a new database.
    ///
    /// Per `008-file-format-spec.md` §2:
    /// - `transaction_id = 1` (creation transaction)
    /// - `total_pages = 3` (page 0 = SB-A, page 1 = SB-B, page 2 = Schema Store root)
    /// - Schema Store root = `PageId(2)`, all other roots = `PageId(0)` (empty)
    pub fn initial() -> Self {
        Self {
            transaction_id: 1,
            total_pages: 3,
            feature_flags: 0,
            root_node_store: PageId::NULL,
            root_edge_store: PageId::NULL,
            root_outgoing_adj: PageId::NULL,
            root_incoming_adj: PageId::NULL,
            root_type_index: PageId::NULL,
            root_schema_store: PageId(2),
            root_id_freelist: PageId::NULL,
            root_page_freelist: PageId::NULL,
            checksum: 0, // computed when written
        }
    }
}

/// Writes a complete superblock page (identity header + mutable fields + checksum).
///
/// Returns the page buffer.
fn build_superblock_page(
    identity_header: &FileIdentityHeader,
    superblock: &Superblock,
    page_size: usize,
) -> Vec<u8> {
    let mut page = vec![0u8; page_size];
    identity_header.serialize(&mut page);
    superblock.serialize(&mut page);
    // Compute and write checksum
    let checksum = Superblock::compute_checksum(&page);
    page[SB_CHECKSUM_OFFSET..SB_CHECKSUM_OFFSET + 8]
        .copy_from_slice(&checksum.to_le_bytes());
    page
}

/// Selects the active superblock from a database file.
///
/// Reads both superblock pages, validates each, and returns the one with
/// the higher `transaction_id`.
///
/// # Returns
///
/// `(superblock, slot_index)` where `slot_index` is 0 or 1.
///
/// # Errors
///
/// - `MediaCorruption` if neither superblock is valid.
/// - I/O errors from the backend.
pub fn select_active_superblock<B: ReadAt>(
    backend: &B,
    page_size: usize,
) -> Result<(Superblock, u8), StorageError> {
    let mut buf_a = vec![0u8; page_size];
    let mut buf_b = vec![0u8; page_size];

    backend
        .read_at(0, &mut buf_a)
        .map_err(map_backend_err)?;
    backend
        .read_at(page_size as u64, &mut buf_b)
        .map_err(map_backend_err)?;

    let valid_a = Superblock::validate(&buf_a).is_ok();
    let valid_b = Superblock::validate(&buf_b).is_ok();

    match (valid_a, valid_b) {
        (true, true) => {
            let sb_a = Superblock::deserialize(&buf_a)?;
            let sb_b = Superblock::deserialize(&buf_b)?;
            if sb_a.transaction_id >= sb_b.transaction_id {
                Ok((sb_a, 0))
            } else {
                Ok((sb_b, 1))
            }
        }
        (true, false) => {
            let sb_a = Superblock::deserialize(&buf_a)?;
            Ok((sb_a, 0))
        }
        (false, true) => {
            let sb_b = Superblock::deserialize(&buf_b)?;
            Ok((sb_b, 1))
        }
        (false, false) => Err(StorageError {
            message: "both superblocks are invalid — database is corrupt".into(),
            #[cfg(feature = "std")]
            source: None,
        }),
    }
}

/// Creates a new database file with identity header, dual superblocks,
/// and an initial empty Schema Store root page.
///
/// # Errors
///
/// Returns an error on I/O failure or invalid page size.
pub fn create_database_file<B: StorageBackend>(
    backend: &mut B,
    page_size: usize,
    application_id: u32,
) -> Result<Superblock, StorageError> {
    let identity = FileIdentityHeader::new(page_size, application_id)?;
    let superblock = Superblock::initial();

    // Build superblock pages
    let sb_page_a = build_superblock_page(&identity, &superblock, page_size);
    // Superblock B: transaction_id = 0 (older, will not be selected)
    let sb_b = Superblock {
        transaction_id: 0,
        ..superblock.clone()
    };
    let sb_page_b = build_superblock_page(&identity, &sb_b, page_size);

    // Build initial Schema Store root page (empty leaf)
    let schema_root_page = LeafPage::build(
        PageId(2),
        1, // txn_id = 1 (creation transaction)
        &[],
        PageId::NULL,
        PageId::NULL,
        page_size,
    );

    // Write to backend
    let total_size = page_size as u64 * 3;
    backend.set_len(total_size).map_err(map_backend_err)?;
    backend.write_at(0, &sb_page_a).map_err(map_backend_err)?;
    backend
        .write_at(page_size as u64, &sb_page_b)
        .map_err(map_backend_err)?;
    backend
        .write_at(page_size as u64 * 2, &schema_root_page)
        .map_err(map_backend_err)?;
    backend.sync_all().map_err(map_backend_err)?;

    Ok(superblock)
}

/// Opens an existing database file and returns the active superblock.
///
/// Validates the identity header and selects the active superblock.
///
/// # Errors
///
/// Returns an error if the identity header is invalid, the page size
/// doesn't match, or both superblocks are corrupt.
pub fn open_database_file<B: ReadAt>(
    backend: &B,
    page_size: usize,
) -> Result<Superblock, StorageError> {
    // Read and validate identity header
    let mut hdr_buf = [0u8; IDENTITY_HEADER_SIZE];
    backend.read_at(0, &mut hdr_buf).map_err(map_backend_err)?;
    let identity = FileIdentityHeader::deserialize(&hdr_buf)?;
    identity.validate_compatible()?;

    let file_page_size = identity.page_size()?;
    if file_page_size != page_size {
        return Err(StorageError {
            message: format!(
                "page size mismatch: file has {file_page_size}, expected {page_size}"
            ),
            #[cfg(feature = "std")]
            source: None,
        });
    }

    let (superblock, _slot) = select_active_superblock(backend, page_size)?;
    Ok(superblock)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::WriteAt;
    use crate::storage::page::DEFAULT_PAGE_SIZE;
    use crate::storage::test_utils::TestBackend;

    #[test]
    fn identity_header_round_trip() {
        let hdr = FileIdentityHeader::new(4096, 0x12345678).unwrap();
        let mut buf = [0u8; IDENTITY_HEADER_SIZE];
        hdr.serialize(&mut buf);
        let rt = FileIdentityHeader::deserialize(&buf).unwrap();
        assert_eq!(hdr.magic, rt.magic);
        assert_eq!(hdr.format_major, rt.format_major);
        assert_eq!(hdr.format_minor, rt.format_minor);
        assert_eq!(hdr.application_id, rt.application_id);
        assert_eq!(hdr.page_size_raw, rt.page_size_raw);
        assert_eq!(hdr.creation_timestamp, rt.creation_timestamp);
    }

    #[test]
    fn identity_header_magic_validation() {
        let mut buf = [0u8; IDENTITY_HEADER_SIZE];
        buf[0..5].copy_from_slice(b"WRONG");
        let err = FileIdentityHeader::deserialize(&buf).unwrap_err();
        assert!(err.message.contains("magic"));
    }

    #[test]
    fn identity_header_version_validation() {
        let mut hdr = FileIdentityHeader::new(4096, 0).unwrap();
        hdr.format_major = 2; // unsupported
        assert!(hdr.validate_compatible().is_err());

        hdr.format_major = 1;
        assert!(hdr.validate_compatible().is_ok());
    }

    #[test]
    fn page_size_encoding_round_trip() {
        for size in [4096, 8192, 16384, 32768, 65536] {
            let raw = encode_page_size(size).unwrap();
            let decoded = decode_page_size(raw).unwrap();
            assert_eq!(decoded, size);
        }
    }

    #[test]
    fn page_size_encoding_invalid() {
        assert!(encode_page_size(1024).is_err());
        assert!(encode_page_size(0).is_err());
        assert!(encode_page_size(5000).is_err());
    }

    #[test]
    fn superblock_round_trip() {
        let identity = FileIdentityHeader::new(4096, 0).unwrap();
        let sb = Superblock {
            transaction_id: 42,
            total_pages: 100,
            feature_flags: 0,
            root_node_store: PageId(10),
            root_edge_store: PageId(11),
            root_outgoing_adj: PageId(12),
            root_incoming_adj: PageId(13),
            root_type_index: PageId(14),
            root_schema_store: PageId(15),
            root_id_freelist: PageId(16),
            root_page_freelist: PageId(17),
            checksum: 0,
        };
        let page = build_superblock_page(&identity, &sb, DEFAULT_PAGE_SIZE);
        Superblock::validate(&page).unwrap();

        let rt = Superblock::deserialize(&page).unwrap();
        assert_eq!(rt.transaction_id, 42);
        assert_eq!(rt.total_pages, 100);
        assert_eq!(rt.root_node_store, PageId(10));
        assert_eq!(rt.root_page_freelist, PageId(17));
    }

    #[test]
    fn superblock_checksum_failure() {
        let identity = FileIdentityHeader::new(4096, 0).unwrap();
        let sb = Superblock::initial();
        let mut page = build_superblock_page(&identity, &sb, DEFAULT_PAGE_SIZE);
        // Corrupt a byte
        page[50] ^= 0xFF;
        let err = Superblock::validate(&page).unwrap_err();
        assert!(err.message.contains("checksum mismatch"));
    }

    #[test]
    fn superblock_root_page_ids() {
        let sb = Superblock {
            root_node_store: PageId(2),
            root_edge_store: PageId(3),
            root_outgoing_adj: PageId(4),
            root_incoming_adj: PageId(5),
            root_type_index: PageId(6),
            root_schema_store: PageId(7),
            root_id_freelist: PageId(8),
            root_page_freelist: PageId(9),
            ..Superblock::initial()
        };
        let ids = sb.root_page_ids();
        assert_eq!(ids[0], PageId(2));
        assert_eq!(ids[5], PageId(7));
        assert_eq!(ids[7], PageId(9));
    }

    #[test]
    fn select_both_valid_higher_txn_wins() {
        let identity = FileIdentityHeader::new(4096, 0).unwrap();

        let sb_a = Superblock {
            transaction_id: 5,
            ..Superblock::initial()
        };
        let sb_b = Superblock {
            transaction_id: 10,
            ..Superblock::initial()
        };

        let page_a = build_superblock_page(&identity, &sb_a, DEFAULT_PAGE_SIZE);
        let page_b = build_superblock_page(&identity, &sb_b, DEFAULT_PAGE_SIZE);

        let mut backend = TestBackend::new();
        backend.set_len(DEFAULT_PAGE_SIZE as u64 * 2).unwrap();
        backend.write_at(0, &page_a).unwrap();
        backend
            .write_at(DEFAULT_PAGE_SIZE as u64, &page_b)
            .unwrap();

        let (sb, slot) = select_active_superblock(&backend, DEFAULT_PAGE_SIZE).unwrap();
        assert_eq!(sb.transaction_id, 10);
        assert_eq!(slot, 1);
    }

    #[test]
    fn select_one_valid_fallback() {
        let identity = FileIdentityHeader::new(4096, 0).unwrap();
        let sb_a = Superblock::initial();
        let page_a = build_superblock_page(&identity, &sb_a, DEFAULT_PAGE_SIZE);

        let mut backend = TestBackend::new();
        backend.set_len(DEFAULT_PAGE_SIZE as u64 * 2).unwrap();
        backend.write_at(0, &page_a).unwrap();
        // Page B is all zeros — invalid

        let (sb, slot) = select_active_superblock(&backend, DEFAULT_PAGE_SIZE).unwrap();
        assert_eq!(sb.transaction_id, 1);
        assert_eq!(slot, 0);
    }

    #[test]
    fn select_both_invalid_error() {
        let mut backend = TestBackend::new();
        backend.set_len(DEFAULT_PAGE_SIZE as u64 * 2).unwrap();
        // Both pages are all zeros — invalid

        let result = select_active_superblock(&backend, DEFAULT_PAGE_SIZE);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("corrupt"));
    }

    #[test]
    fn select_equal_txn_ids() {
        let identity = FileIdentityHeader::new(4096, 0).unwrap();
        let sb = Superblock::initial();
        let page = build_superblock_page(&identity, &sb, DEFAULT_PAGE_SIZE);

        let mut backend = TestBackend::new();
        backend.set_len(DEFAULT_PAGE_SIZE as u64 * 2).unwrap();
        backend.write_at(0, &page).unwrap();
        backend
            .write_at(DEFAULT_PAGE_SIZE as u64, &page)
            .unwrap();

        let (_, slot) = select_active_superblock(&backend, DEFAULT_PAGE_SIZE).unwrap();
        assert_eq!(slot, 0); // tie-break: slot 0
    }

    #[test]
    fn create_and_open_database() {
        let mut backend = TestBackend::new();
        let sb = create_database_file(&mut backend, DEFAULT_PAGE_SIZE, 0).unwrap();
        assert_eq!(sb.transaction_id, 1);
        assert_eq!(sb.total_pages, 3);
        assert_eq!(sb.root_schema_store, PageId(2));
        assert!(sb.root_node_store.is_null());

        // Verify file size
        assert_eq!(backend.data().len(), DEFAULT_PAGE_SIZE * 3);

        // Open the created database
        let opened = open_database_file(&backend, DEFAULT_PAGE_SIZE).unwrap();
        assert_eq!(opened.transaction_id, sb.transaction_id);
        assert_eq!(opened.total_pages, sb.total_pages);
        assert_eq!(opened.root_schema_store, sb.root_schema_store);
    }

    #[test]
    fn open_page_size_mismatch() {
        let mut backend = TestBackend::new();
        create_database_file(&mut backend, 4096, 0).unwrap();

        let result = open_database_file(&backend, 8192);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("page size mismatch"));
    }
}
