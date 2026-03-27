//! Page management: types, headers, and page-type-specific layouts.
//!
//! All data in the database file is organized into fixed-size pages.
//! Pages 0 and 1 are superblock pages; pages 2+ are data pages
//! (interior, leaf, overflow, or free).

pub mod header;
pub mod interior;
pub mod leaf;
pub mod overflow;
pub mod free;

use core::fmt;

/// Default page size in bytes.
pub const DEFAULT_PAGE_SIZE: usize = 4096;

/// Minimum supported page size in bytes.
pub const MIN_PAGE_SIZE: usize = 4096;

/// Maximum supported page size in bytes.
pub const MAX_PAGE_SIZE: usize = 65536;

/// Size of the common page header in bytes.
pub const COMMON_HEADER_SIZE: usize = 24;

/// Size of the file identity header in bytes.
pub const IDENTITY_HEADER_SIZE: usize = 32;

/// Size of the used portion of a superblock page in bytes (identity header + mutable fields).
pub const SUPERBLOCK_USED_SIZE: usize = 192;

/// A page identifier. Page 0 and 1 are superblock pages; pages 2+ are data pages.
///
/// `PageId(0)` is used as a null sentinel meaning "no page" (e.g., an empty
/// B-tree root or the end of a leaf linked list).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PageId(pub u64);

impl PageId {
    /// Null sentinel — indicates no page / empty tree.
    pub const NULL: PageId = PageId(0);

    /// Returns `true` if this is the null sentinel.
    pub fn is_null(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for PageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PageId({})", self.0)
    }
}

/// Page type discriminant stored in the common page header.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum PageType {
    /// B-tree interior (branch) node.
    Interior = 0x01,
    /// B-tree leaf node.
    Leaf = 0x02,
    /// Overflow page for large values.
    Overflow = 0x03,
    /// Free (unallocated) page.
    Free = 0x04,
}

impl TryFrom<u8> for PageType {
    type Error = u8;

    fn try_from(value: u8) -> Result<Self, u8> {
        match value {
            0x01 => Ok(PageType::Interior),
            0x02 => Ok(PageType::Leaf),
            0x03 => Ok(PageType::Overflow),
            0x04 => Ok(PageType::Free),
            other => Err(other),
        }
    }
}

impl fmt::Display for PageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PageType::Interior => write!(f, "Interior"),
            PageType::Leaf => write!(f, "Leaf"),
            PageType::Overflow => write!(f, "Overflow"),
            PageType::Free => write!(f, "Free"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_id_null() {
        assert!(PageId::NULL.is_null());
        assert!(PageId(0).is_null());
        assert!(!PageId(1).is_null());
        assert!(!PageId(42).is_null());
    }

    #[test]
    fn page_id_display() {
        assert_eq!(PageId(5).to_string(), "PageId(5)");
    }

    #[test]
    fn page_id_ordering() {
        assert!(PageId(1) < PageId(2));
        assert!(PageId(0) < PageId(u64::MAX));
    }

    #[test]
    fn page_type_from_u8() {
        assert_eq!(PageType::try_from(0x01), Ok(PageType::Interior));
        assert_eq!(PageType::try_from(0x02), Ok(PageType::Leaf));
        assert_eq!(PageType::try_from(0x03), Ok(PageType::Overflow));
        assert_eq!(PageType::try_from(0x04), Ok(PageType::Free));
        assert_eq!(PageType::try_from(0x00), Err(0x00));
        assert_eq!(PageType::try_from(0x05), Err(0x05));
        assert_eq!(PageType::try_from(0xFF), Err(0xFF));
    }
}
