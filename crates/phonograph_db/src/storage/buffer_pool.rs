//! Buffer pool with clock eviction.
//!
//! Caches data pages in memory and manages their lifecycle. The clock
//! algorithm provides O(1) amortized eviction with low overhead.
//! See `012-design-document.md` §9.

use alloc::{vec, vec::Vec};
use hashbrown::HashMap;

use crate::error::StorageError;
use crate::backend::{self, ReadAt, WriteAt};

use super::map_backend_err;
use super::page::header::CommonPageHeader;
use super::page::PageId;

/// Minimum buffer pool capacity in frames.
pub const MIN_BUFFER_POOL_FRAMES: usize = 64;

/// Default buffer pool capacity in frames.
pub const DEFAULT_BUFFER_POOL_FRAMES: usize = 1024;

/// A single frame in the buffer pool, holding one page's worth of data.
struct PageFrame {
    /// Which page this frame holds (`PageId(0)` if empty).
    page_id: PageId,
    /// Raw page bytes (`page_size` length).
    data: Vec<u8>,
    /// Whether this frame has been modified since last flush.
    dirty: bool,
    /// Number of active references (pins) to this frame.
    pin_count: u32,
    /// Clock eviction reference bit (set on access, cleared by sweep).
    reference_bit: bool,
}

/// A page cache with clock-based eviction.
///
/// The buffer pool stores a fixed number of page frames. When a page
/// is requested, it is either served from cache (hit) or read from
/// the backend (miss), potentially evicting a victim frame.
///
/// # Design
///
/// - `fetch_page` reads a page into a frame, pins it, and returns its index.
/// - `unpin_page` releases the pin (and optionally marks dirty).
/// - `flush_page` / `flush_all_dirty` writes dirty frames to disk.
/// - `new_page` allocates a frame for a newly created page (CoW).
/// - Eviction uses the clock algorithm: O(1) amortized.
pub struct BufferPool {
    frames: Vec<PageFrame>,
    /// Maps page ID → frame index for O(1) lookup.
    page_table: HashMap<u64, usize>,
    /// Current position of the clock hand.
    clock_hand: usize,
    /// Total number of frames.
    capacity: usize,
    /// Page size in bytes.
    page_size: usize,
}

impl BufferPool {
    /// Creates a new buffer pool with the given capacity and page size.
    ///
    /// # Panics
    ///
    /// Panics if `capacity < MIN_BUFFER_POOL_FRAMES`.
    pub fn new(capacity: usize, page_size: usize) -> Self {
        assert!(
            capacity >= MIN_BUFFER_POOL_FRAMES,
            "buffer pool capacity {capacity} < minimum {MIN_BUFFER_POOL_FRAMES}"
        );

        let frames = (0..capacity)
            .map(|_| PageFrame {
                page_id: PageId::NULL,
                data: vec![0u8; page_size],
                dirty: false,
                pin_count: 0,
                reference_bit: false,
            })
            .collect();

        Self {
            frames,
            page_table: HashMap::with_capacity(capacity),
            clock_hand: 0,
            capacity,
            page_size,
        }
    }

    /// Returns the buffer pool capacity in frames.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Returns the page size in bytes.
    pub fn page_size(&self) -> usize {
        self.page_size
    }

    /// Fetches a page into the buffer pool, returning its frame index.
    ///
    /// On a cache hit, the existing frame is pinned and returned.
    /// On a miss, a victim frame is evicted (flushed if dirty) and the
    /// requested page is read from the backend.
    ///
    /// The caller must call [`unpin_page`](Self::unpin_page) when done.
    ///
    /// # Errors
    ///
    /// - I/O error reading from backend.
    /// - `MediaCorruption` if the page's CRC32C checksum is invalid.
    /// - Buffer pool exhausted (all frames pinned).
    pub fn fetch_page<B: ReadAt + WriteAt + backend::Durability>(
        &mut self,
        page_id: PageId,
        backend: &mut B,
    ) -> Result<usize, StorageError> {
        // Cache hit
        if let Some(&frame_idx) = self.page_table.get(&page_id.0) {
            self.frames[frame_idx].reference_bit = true;
            self.frames[frame_idx].pin_count += 1;
            return Ok(frame_idx);
        }

        // Cache miss: find a victim
        let frame_idx = self.find_victim()?;

        // Flush dirty victim if needed
        if self.frames[frame_idx].dirty {
            self.flush_frame(frame_idx, backend)?;
        }

        // Remove old page from page table
        let old_page_id = self.frames[frame_idx].page_id;
        if !old_page_id.is_null() {
            self.page_table.remove(&old_page_id.0);
        }

        // Read new page from disk
        let offset = page_id.0 * self.page_size as u64;
        backend
            .read_at(offset, &mut self.frames[frame_idx].data)
            .map_err(map_backend_err)?;

        // Validate checksum
        CommonPageHeader::validate_checksum(&self.frames[frame_idx].data)?;

        // Set up frame
        self.frames[frame_idx].page_id = page_id;
        self.frames[frame_idx].dirty = false;
        self.frames[frame_idx].pin_count = 1;
        self.frames[frame_idx].reference_bit = true;
        self.page_table.insert(page_id.0, frame_idx);

        Ok(frame_idx)
    }

    /// Finds a victim frame for eviction using the clock algorithm.
    ///
    /// # Errors
    ///
    /// Returns an error if all frames are pinned (pool exhausted).
    fn find_victim(&mut self) -> Result<usize, StorageError> {
        let start = self.clock_hand;
        let mut swept = 0;

        loop {
            let frame = &mut self.frames[self.clock_hand];

            if frame.pin_count == 0 {
                if !frame.reference_bit {
                    // Found a victim
                    let victim = self.clock_hand;
                    self.clock_hand = (self.clock_hand + 1) % self.capacity;
                    return Ok(victim);
                }
                // Clear reference bit, give it a second chance
                frame.reference_bit = false;
            }
            // Pinned or reference_bit was just cleared: advance

            self.clock_hand = (self.clock_hand + 1) % self.capacity;
            swept += 1;

            // If we've swept through all frames twice (once to clear bits,
            // once to confirm still pinned), the pool is exhausted.
            if swept >= self.capacity * 2 {
                return Err(StorageError {
                    message: "buffer pool exhausted: all frames are pinned".into(),
                    #[cfg(feature = "std")]
                    source: None,
                });
            }
            // Safety check: prevent infinite loop if we return to start after 2 full sweeps
            if swept > self.capacity * 2 && self.clock_hand == start {
                break;
            }
        }

        Err(StorageError {
            message: "buffer pool exhausted: all frames are pinned".into(),
            #[cfg(feature = "std")]
            source: None,
        })
    }

    /// Unpins a page frame, decrementing its pin count.
    ///
    /// If `dirty` is true, the frame is marked as dirty (needing flush).
    ///
    /// # Panics
    ///
    /// Panics if `frame_index` is out of bounds or pin_count is already 0.
    pub fn unpin_page(&mut self, frame_index: usize, dirty: bool) {
        let frame = &mut self.frames[frame_index];
        assert!(
            frame.pin_count > 0,
            "unpin_page: frame {} pin_count is already 0",
            frame_index
        );
        frame.pin_count -= 1;
        if dirty {
            frame.dirty = true;
        }
    }

    /// Flushes a single dirty frame to disk.
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure.
    pub fn flush_page<B: WriteAt + backend::Durability>(
        &mut self,
        frame_index: usize,
        backend: &mut B,
    ) -> Result<(), StorageError> {
        self.flush_frame(frame_index, backend)
    }

    /// Internal: flush a frame to disk.
    fn flush_frame<B: WriteAt>(
        &mut self,
        frame_index: usize,
        backend: &mut B,
    ) -> Result<(), StorageError> {
        let frame = &mut self.frames[frame_index];
        if !frame.dirty {
            return Ok(());
        }
        let offset = frame.page_id.0 * self.page_size as u64;
        backend
            .write_at(offset, &frame.data)
            .map_err(map_backend_err)?;
        frame.dirty = false;
        Ok(())
    }

    /// Looks up a page in the page table, returning the frame index if cached.
    pub fn page_table_get(&self, page_id: PageId) -> Option<&usize> {
        self.page_table.get(&page_id.0)
    }

    /// Flushes all dirty frames to the backend.
    ///
    /// # Errors
    ///
    /// Returns an error on the first I/O failure.
    pub fn flush_all_dirty<B: WriteAt + backend::Durability>(
        &mut self,
        backend: &mut B,
    ) -> Result<(), StorageError> {
        for i in 0..self.capacity {
            if self.frames[i].dirty {
                self.flush_frame(i, backend)?;
            }
        }
        Ok(())
    }

    /// Returns a read-only reference to the page data in a frame.
    ///
    /// # Panics
    ///
    /// Panics if `frame_index` is out of bounds.
    pub fn get_page_data(&self, frame_index: usize) -> &[u8] {
        &self.frames[frame_index].data
    }

    /// Returns a mutable reference to the page data in a frame.
    ///
    /// The caller should mark the frame dirty via `unpin_page(_, true)`
    /// after modifying the data.
    ///
    /// # Panics
    ///
    /// Panics if `frame_index` is out of bounds.
    pub fn get_page_data_mut(&mut self, frame_index: usize) -> &mut [u8] {
        &mut self.frames[frame_index].data
    }

    /// Allocates a frame for a newly created page (e.g., CoW copy).
    ///
    /// The frame is zero-initialized and marked dirty. Pin count is set to 1.
    /// The caller writes the page content afterward.
    ///
    /// # Errors
    ///
    /// Returns an error if the pool is exhausted (all frames pinned).
    pub fn new_page<B: ReadAt + WriteAt + backend::Durability>(
        &mut self,
        page_id: PageId,
        backend: &mut B,
    ) -> Result<usize, StorageError> {
        let frame_idx = self.find_victim()?;

        // Flush dirty victim
        if self.frames[frame_idx].dirty {
            self.flush_frame(frame_idx, backend)?;
        }

        // Remove old page from page table
        let old_page_id = self.frames[frame_idx].page_id;
        if !old_page_id.is_null() {
            self.page_table.remove(&old_page_id.0);
        }

        // Zero-initialize
        self.frames[frame_idx].data.fill(0);
        self.frames[frame_idx].page_id = page_id;
        self.frames[frame_idx].dirty = true;
        self.frames[frame_idx].pin_count = 1;
        self.frames[frame_idx].reference_bit = true;
        self.page_table.insert(page_id.0, frame_idx);

        Ok(frame_idx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::WriteAt;
    use crate::storage::page::header::CommonPageHeader;
    use crate::storage::page::{DEFAULT_PAGE_SIZE, PageType};
    use crate::storage::test_utils::TestBackend;

    /// Writes a valid page to the backend at the given page_id offset.
    fn write_test_page(backend: &mut TestBackend, page_id: PageId, marker: u8) {
        let mut page = vec![0u8; DEFAULT_PAGE_SIZE];
        let header = CommonPageHeader {
            page_id,
            page_type: PageType::Leaf,
            flags: 0x01,
            txn_id: 1,
            checksum: 0,
        };
        header.serialize(&mut page);
        page[50] = marker; // distinguishing byte
        let checksum = CommonPageHeader::compute_checksum(&page);
        page[20..24].copy_from_slice(&checksum.to_le_bytes());

        let offset = page_id.0 * DEFAULT_PAGE_SIZE as u64;
        let current_len = backend.data().len();
        let needed = offset as usize + DEFAULT_PAGE_SIZE;
        if needed > current_len {
            backend.set_len(needed as u64).unwrap();
        }
        backend.write_at(offset, &page).unwrap();
    }

    #[test]
    fn fetch_page_cache_miss_reads_from_backend() {
        let mut backend = TestBackend::new();
        write_test_page(&mut backend, PageId(2), 0xAB);

        let mut pool = BufferPool::new(MIN_BUFFER_POOL_FRAMES, DEFAULT_PAGE_SIZE);
        let idx = pool.fetch_page(PageId(2), &mut backend).unwrap();
        assert_eq!(pool.get_page_data(idx)[50], 0xAB);
        pool.unpin_page(idx, false);
    }

    #[test]
    fn fetch_page_cache_hit() {
        let mut backend = TestBackend::new();
        write_test_page(&mut backend, PageId(2), 0xAB);

        let mut pool = BufferPool::new(MIN_BUFFER_POOL_FRAMES, DEFAULT_PAGE_SIZE);
        let idx1 = pool.fetch_page(PageId(2), &mut backend).unwrap();
        pool.unpin_page(idx1, false);

        // Second fetch should be a cache hit (same frame index)
        let idx2 = pool.fetch_page(PageId(2), &mut backend).unwrap();
        assert_eq!(idx1, idx2);
        pool.unpin_page(idx2, false);
    }

    #[test]
    fn unpin_decrements_pin_count() {
        let mut backend = TestBackend::new();
        write_test_page(&mut backend, PageId(2), 0);

        let mut pool = BufferPool::new(MIN_BUFFER_POOL_FRAMES, DEFAULT_PAGE_SIZE);
        let idx = pool.fetch_page(PageId(2), &mut backend).unwrap();
        // Pin twice
        let idx2 = pool.fetch_page(PageId(2), &mut backend).unwrap();
        assert_eq!(idx, idx2);
        assert_eq!(pool.frames[idx].pin_count, 2);
        pool.unpin_page(idx, false);
        assert_eq!(pool.frames[idx].pin_count, 1);
        pool.unpin_page(idx, false);
        assert_eq!(pool.frames[idx].pin_count, 0);
    }

    #[test]
    fn eviction_respects_reference_bit() {
        let mut backend = TestBackend::new();
        // Write enough pages to fill a small pool and trigger eviction
        for i in 2..2 + MIN_BUFFER_POOL_FRAMES as u64 + 1 {
            write_test_page(&mut backend, PageId(i), i as u8);
        }

        let mut pool = BufferPool::new(MIN_BUFFER_POOL_FRAMES, DEFAULT_PAGE_SIZE);

        // Fill the pool
        let mut indices = Vec::new();
        for i in 2..2 + MIN_BUFFER_POOL_FRAMES as u64 {
            let idx = pool.fetch_page(PageId(i), &mut backend).unwrap();
            indices.push(idx);
            pool.unpin_page(idx, false);
        }

        // All frames have reference_bit=true (just accessed). Clear one manually.
        pool.frames[indices[0]].reference_bit = false;

        // Fetch one more page — should evict the one with reference_bit=false
        let extra_id = PageId(2 + MIN_BUFFER_POOL_FRAMES as u64);
        let idx = pool.fetch_page(extra_id, &mut backend).unwrap();
        assert_eq!(idx, indices[0]); // evicted the frame we cleared
        pool.unpin_page(idx, false);
    }

    #[test]
    fn dirty_page_flushed_on_eviction() {
        let mut backend = TestBackend::new();
        for i in 2..2 + MIN_BUFFER_POOL_FRAMES as u64 + 1 {
            write_test_page(&mut backend, PageId(i), i as u8);
        }

        let mut pool = BufferPool::new(MIN_BUFFER_POOL_FRAMES, DEFAULT_PAGE_SIZE);

        // Fetch and dirty the first page
        let idx = pool.fetch_page(PageId(2), &mut backend).unwrap();
        pool.get_page_data_mut(idx)[50] = 0xFF;
        // Recompute checksum after modification
        let checksum = CommonPageHeader::compute_checksum(pool.get_page_data(idx));
        pool.get_page_data_mut(idx)[20..24].copy_from_slice(&checksum.to_le_bytes());
        pool.unpin_page(idx, true);
        pool.frames[idx].reference_bit = false;

        // Fill remaining pool
        for i in 3..2 + MIN_BUFFER_POOL_FRAMES as u64 {
            let fidx = pool.fetch_page(PageId(i), &mut backend).unwrap();
            pool.unpin_page(fidx, false);
        }

        // Fetch one more to trigger eviction of the dirty page
        let extra_id = PageId(2 + MIN_BUFFER_POOL_FRAMES as u64);
        pool.fetch_page(extra_id, &mut backend).unwrap();

        // Verify the dirty page was written to backend
        let data = backend.data();
        let offset = 2 * DEFAULT_PAGE_SIZE;
        assert_eq!(data[offset + 50], 0xFF);
    }

    #[test]
    fn pin_count_prevents_eviction() {
        let mut backend = TestBackend::new();
        for i in 2..2 + MIN_BUFFER_POOL_FRAMES as u64 + 1 {
            write_test_page(&mut backend, PageId(i), i as u8);
        }

        let mut pool = BufferPool::new(MIN_BUFFER_POOL_FRAMES, DEFAULT_PAGE_SIZE);

        // Pin ALL frames
        for i in 2..2 + MIN_BUFFER_POOL_FRAMES as u64 {
            pool.fetch_page(PageId(i), &mut backend).unwrap();
            // Don't unpin!
        }

        // Try to fetch one more — should fail (pool exhausted)
        let result = pool.fetch_page(
            PageId(2 + MIN_BUFFER_POOL_FRAMES as u64),
            &mut backend,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("exhausted"));
    }

    #[test]
    fn new_page_allocates_dirty_frame() {
        let mut backend = TestBackend::new();
        backend.set_len(DEFAULT_PAGE_SIZE as u64 * 100).unwrap();

        let mut pool = BufferPool::new(MIN_BUFFER_POOL_FRAMES, DEFAULT_PAGE_SIZE);
        let idx = pool.new_page(PageId(50), &mut backend).unwrap();

        // Frame should be dirty and zero-initialized
        assert!(pool.frames[idx].dirty);
        assert_eq!(pool.frames[idx].pin_count, 1);
        assert!(pool.get_page_data(idx).iter().all(|&b| b == 0));

        pool.unpin_page(idx, true);
    }

    #[test]
    fn flush_all_dirty() {
        let mut backend = TestBackend::new();
        write_test_page(&mut backend, PageId(2), 0);
        write_test_page(&mut backend, PageId(3), 0);

        let mut pool = BufferPool::new(MIN_BUFFER_POOL_FRAMES, DEFAULT_PAGE_SIZE);

        let idx1 = pool.fetch_page(PageId(2), &mut backend).unwrap();
        pool.get_page_data_mut(idx1)[50] = 0xAA;
        let cs1 = CommonPageHeader::compute_checksum(pool.get_page_data(idx1));
        pool.get_page_data_mut(idx1)[20..24].copy_from_slice(&cs1.to_le_bytes());
        pool.unpin_page(idx1, true);

        let idx2 = pool.fetch_page(PageId(3), &mut backend).unwrap();
        pool.get_page_data_mut(idx2)[50] = 0xBB;
        let cs2 = CommonPageHeader::compute_checksum(pool.get_page_data(idx2));
        pool.get_page_data_mut(idx2)[20..24].copy_from_slice(&cs2.to_le_bytes());
        pool.unpin_page(idx2, true);

        pool.flush_all_dirty(&mut backend).unwrap();

        // Verify both pages written
        let data = backend.data();
        assert_eq!(data[2 * DEFAULT_PAGE_SIZE + 50], 0xAA);
        assert_eq!(data[3 * DEFAULT_PAGE_SIZE + 50], 0xBB);

        // Dirty flags should be cleared
        assert!(!pool.frames[idx1].dirty);
        assert!(!pool.frames[idx2].dirty);
    }

    #[test]
    fn clock_algorithm_second_chance() {
        let mut backend = TestBackend::new();
        for i in 2..2 + MIN_BUFFER_POOL_FRAMES as u64 + 2 {
            write_test_page(&mut backend, PageId(i), i as u8);
        }

        let mut pool = BufferPool::new(MIN_BUFFER_POOL_FRAMES, DEFAULT_PAGE_SIZE);

        // Fill pool and unpin all
        for i in 2..2 + MIN_BUFFER_POOL_FRAMES as u64 {
            let idx = pool.fetch_page(PageId(i), &mut backend).unwrap();
            pool.unpin_page(idx, false);
        }

        // All have reference_bit=true. Fetching a new page should:
        // 1. Sweep clearing reference bits
        // 2. On second sweep, evict the first frame (reference_bit now false)
        let idx = pool
            .fetch_page(
                PageId(2 + MIN_BUFFER_POOL_FRAMES as u64),
                &mut backend,
            )
            .unwrap();
        pool.unpin_page(idx, false);

        // The eviction happened; the new page is in the pool
        assert!(pool.page_table.contains_key(&(2 + MIN_BUFFER_POOL_FRAMES as u64)));
    }
}
