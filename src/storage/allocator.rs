//! Page allocator with freelist integration and file growth.
//!
//! Manages the allocation and deallocation of data pages, preferring
//! reclaimable free pages over file extension. Growth uses scaling
//! increments per `008-file-format-spec.md` §12.

use crate::error::StorageError;
use crate::hal::WriteAt;

use super::map_hal_err;
use super::page::PageId;

/// Page allocator managing page allocation, deallocation, and file growth.
///
/// Tracks the next page ID for file extension, freed pages awaiting
/// insertion into the Page Freelist B-tree, and deferred secondary
/// freed pages from the previous transaction.
pub struct PageAllocator {
    /// Next page ID for file extension (one past current last page).
    next_page_id: u64,
    /// Total pages currently in the file.
    total_pages: u64,
    /// Pages freed in this transaction (to be inserted into freelist at commit).
    freed_pages: Vec<(u64, PageId)>,
    /// Secondary freed pages deferred from a previous transaction.
    deferred_freed: Vec<(u64, PageId)>,
    /// Pages allocated in this transaction (for rollback tracking).
    allocated_pages: Vec<PageId>,
    /// Page size for offset calculations.
    page_size: usize,
}

impl PageAllocator {
    /// Creates a new allocator for a file with `total_pages` existing pages.
    pub fn new(total_pages: u64, page_size: usize) -> Self {
        Self {
            next_page_id: total_pages,
            total_pages,
            freed_pages: Vec::new(),
            deferred_freed: Vec::new(),
            allocated_pages: Vec::new(),
            page_size,
        }
    }

    /// Allocates a page by incrementing the next_page_id counter.
    ///
    /// The returned page ID may be beyond the current file size — the
    /// caller must extend the file before writing.
    pub fn allocate_page(&mut self) -> PageId {
        let page_id = PageId(self.next_page_id);
        self.next_page_id += 1;
        if self.next_page_id > self.total_pages {
            self.total_pages = self.next_page_id;
        }
        self.allocated_pages.push(page_id);
        page_id
    }

    /// Records a page as freed in the current transaction.
    pub fn free_page(&mut self, page_id: PageId, txn_id: u64) {
        self.freed_pages.push((txn_id, page_id));
    }

    /// Returns the current total number of pages in the file.
    pub fn total_pages(&self) -> u64 {
        self.total_pages
    }

    /// Returns the pages freed in this transaction.
    pub fn freed_pages(&self) -> &[(u64, PageId)] {
        &self.freed_pages
    }

    /// Returns the pages allocated in this transaction.
    pub fn allocated_pages(&self) -> &[PageId] {
        &self.allocated_pages
    }

    /// Sets the deferred freed pages (from a previous transaction's
    /// Page Freelist B-tree CoW operations).
    pub fn set_deferred_freed(&mut self, deferred: Vec<(u64, PageId)>) {
        self.deferred_freed = deferred;
    }

    /// Takes and returns the deferred freed pages, leaving the internal list empty.
    pub fn take_deferred_freed(&mut self) -> Vec<(u64, PageId)> {
        std::mem::take(&mut self.deferred_freed)
    }

    /// Resets the per-transaction tracking (freed_pages, allocated_pages).
    ///
    /// Called after a successful commit to prepare for the next transaction.
    pub fn reset_transaction(&mut self) {
        self.freed_pages.clear();
        self.allocated_pages.clear();
    }

    /// Computes the file growth increment based on the current total pages.
    ///
    /// Per `008-file-format-spec.md` §12.3:
    /// - `< 64` pages → grow by 8
    /// - `< 1024` pages → grow by 64
    /// - `< 16384` pages → grow by 256
    /// - `>= 16384` pages → grow by 1024
    pub fn compute_growth_increment(current_total: u64) -> u64 {
        if current_total < 64 {
            8
        } else if current_total < 1024 {
            64
        } else if current_total < 16384 {
            256
        } else {
            1024
        }
    }

    /// Extends the file by the growth increment, returning the newly available page IDs.
    ///
    /// Calls `backend.set_len()` to grow the file.
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure.
    pub fn extend_file<B: WriteAt>(
        &mut self,
        backend: &mut B,
    ) -> Result<Vec<PageId>, StorageError> {
        let increment = Self::compute_growth_increment(self.total_pages);
        let old_total = self.total_pages;
        let new_total = old_total + increment;

        let new_size = new_total * self.page_size as u64;
        backend.set_len(new_size).map_err(map_hal_err)?;

        self.total_pages = new_total;
        // Don't advance next_page_id here — that happens on allocate_page

        let new_pages: Vec<PageId> = (old_total..new_total).map(PageId).collect();
        Ok(new_pages)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hal::WriteAt;
    use crate::storage::page::DEFAULT_PAGE_SIZE;
    use crate::storage::test_utils::TestBackend;

    #[test]
    fn allocate_consecutive_pages() {
        let mut alloc = PageAllocator::new(3, DEFAULT_PAGE_SIZE);
        let p1 = alloc.allocate_page();
        let p2 = alloc.allocate_page();
        let p3 = alloc.allocate_page();
        assert_eq!(p1, PageId(3));
        assert_eq!(p2, PageId(4));
        assert_eq!(p3, PageId(5));
        assert_eq!(alloc.total_pages(), 6);
        assert_eq!(alloc.allocated_pages().len(), 3);
    }

    #[test]
    fn growth_increments() {
        assert_eq!(PageAllocator::compute_growth_increment(3), 8);
        assert_eq!(PageAllocator::compute_growth_increment(63), 8);
        assert_eq!(PageAllocator::compute_growth_increment(64), 64);
        assert_eq!(PageAllocator::compute_growth_increment(1023), 64);
        assert_eq!(PageAllocator::compute_growth_increment(1024), 256);
        assert_eq!(PageAllocator::compute_growth_increment(16383), 256);
        assert_eq!(PageAllocator::compute_growth_increment(16384), 1024);
        assert_eq!(PageAllocator::compute_growth_increment(100000), 1024);
    }

    #[test]
    fn free_page_tracking() {
        let mut alloc = PageAllocator::new(10, DEFAULT_PAGE_SIZE);
        alloc.free_page(PageId(5), 1);
        alloc.free_page(PageId(6), 1);
        assert_eq!(alloc.freed_pages().len(), 2);
        assert_eq!(alloc.freed_pages()[0], (1, PageId(5)));
    }

    #[test]
    fn extend_file_grows_backend() {
        let mut backend = TestBackend::new();
        backend
            .set_len(3 * DEFAULT_PAGE_SIZE as u64)
            .unwrap();

        let mut alloc = PageAllocator::new(3, DEFAULT_PAGE_SIZE);
        let new_pages = alloc.extend_file(&mut backend).unwrap();

        // Growth from 3 pages → increment of 8 → 11 total
        assert_eq!(new_pages.len(), 8);
        assert_eq!(new_pages[0], PageId(3));
        assert_eq!(new_pages[7], PageId(10));
        assert_eq!(alloc.total_pages(), 11);

        // Backend should have grown
        assert_eq!(backend.data().len(), 11 * DEFAULT_PAGE_SIZE);
    }

    #[test]
    fn deferred_freed_pages() {
        let mut alloc = PageAllocator::new(10, DEFAULT_PAGE_SIZE);
        alloc.set_deferred_freed(vec![(1, PageId(5)), (2, PageId(6))]);

        let deferred = alloc.take_deferred_freed();
        assert_eq!(deferred.len(), 2);
        assert!(alloc.take_deferred_freed().is_empty()); // taken
    }

    #[test]
    fn reset_transaction() {
        let mut alloc = PageAllocator::new(3, DEFAULT_PAGE_SIZE);
        alloc.allocate_page();
        alloc.free_page(PageId(2), 1);
        assert!(!alloc.allocated_pages().is_empty());
        assert!(!alloc.freed_pages().is_empty());

        alloc.reset_transaction();
        assert!(alloc.allocated_pages().is_empty());
        assert!(alloc.freed_pages().is_empty());
    }
}
