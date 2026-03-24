//! CoW path copy logic and freed page tracking.
//!
//! When a leaf is modified, every page from the leaf to the root must
//! be copied to new pages. The old pages are recorded as freed.

use crate::storage::page::PageId;

/// The result of a CoW B-tree mutation.
///
/// Contains the new root page ID after the mutation, plus the sets
/// of freed (old) and newly allocated pages.
#[derive(Clone, Debug)]
pub struct CowResult {
    /// The new root page ID after the mutation.
    pub new_root: PageId,
    /// Pages that were replaced (old versions) and can be freed.
    pub freed_pages: Vec<PageId>,
    /// Pages that were newly allocated for the mutation.
    pub new_pages: Vec<PageId>,
}
