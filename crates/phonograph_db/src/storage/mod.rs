//! Persistent storage engine internals.
//!
//! This module contains the core storage engine for the embedded graph database,
//! including page management, buffer pool, CoW B+ tree operations, record
//! serialization, and the dual-superblock file format.
//!
//! All modules in `storage/` require the `std` feature and are gated behind
//! `#[cfg(feature = "std")]` at the crate root.

use alloc::{format, vec, vec::Vec};
pub mod page;
pub mod btree;
pub mod buffer_pool;
pub mod allocator;
pub mod format;
pub mod serialization;
pub mod snapshot;

/// Converts a backend error into a crate-level [`StorageError`](crate::error::StorageError).
pub(crate) fn map_backend_err<E: crate::backend::BackendError>(e: E) -> crate::error::StorageError {
    crate::error::StorageError {
        message: alloc::format!("{e}"),
        #[cfg(feature = "std")]
        source: None,
    }
}

extern crate alloc;

use crate::error::StorageError;
use crate::backend::StorageBackend;

use self::allocator::PageAllocator;
use self::btree::{BTree, BTreeConfig};
use self::buffer_pool::BufferPool;
use self::format::{FileIdentityHeader, Superblock};
use self::page::{PageId, DEFAULT_PAGE_SIZE, IDENTITY_HEADER_SIZE};
use self::snapshot::{Snapshot, SnapshotRoots};

/// Configuration for creating or opening a [`StorageEngine`].
#[derive(Clone, Debug)]
pub struct StorageEngineConfig {
    /// Page size in bytes (default: 4096, must be power of 2, 4096–65536).
    pub page_size: usize,
    /// Number of buffer pool frames (default: 1024, min: 64).
    pub buffer_pool_frames: usize,
    /// Application identifier written to the file header.
    pub application_id: u32,
}

impl Default for StorageEngineConfig {
    fn default() -> Self {
        Self {
            page_size: DEFAULT_PAGE_SIZE,
            buffer_pool_frames: buffer_pool::DEFAULT_BUFFER_POOL_FRAMES,
            application_id: 0,
        }
    }
}

/// The persistent storage engine tying together the buffer pool,
/// page allocator, B-tree operations, and dual-superblock format.
///
/// Generic over the storage backend `B` (e.g., `FileBackend` or a test backend).
pub struct StorageEngine<B: StorageBackend> {
    /// The underlying storage backend.
    backend: B,
    /// In-memory page cache.
    buffer_pool: BufferPool,
    /// Page allocator tracking free and allocated pages.
    allocator: PageAllocator,
    /// The currently active superblock.
    active_superblock: Superblock,
    /// Which superblock slot (0 or 1) is active.
    active_slot: u8,
    /// Page size in bytes.
    page_size: usize,
    /// B-tree operations.
    btree: BTree,
}

impl<B: StorageBackend> StorageEngine<B> {
    /// Creates a new database file and returns a `StorageEngine` ready for use.
    ///
    /// Writes the file identity header, dual superblocks, and initial Schema Store
    /// root page. The backend should be empty or will be overwritten.
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure or invalid configuration.
    pub fn create(mut backend: B, config: StorageEngineConfig) -> Result<Self, StorageError> {
        let superblock =
            format::create_database_file(&mut backend, config.page_size, config.application_id)?;
        let allocator = PageAllocator::new(superblock.total_pages, config.page_size);
        let buffer_pool = BufferPool::new(config.buffer_pool_frames, config.page_size);
        let btree = BTree::new(BTreeConfig {
            page_size: config.page_size,
        });

        Ok(Self {
            backend,
            buffer_pool,
            allocator,
            active_superblock: superblock,
            active_slot: 0,
            page_size: config.page_size,
            btree,
        })
    }

    /// Opens an existing database file and returns a `StorageEngine`.
    ///
    /// Validates the file identity header and selects the active superblock.
    ///
    /// # Errors
    ///
    /// Returns an error if the file is corrupt, the page size doesn't match,
    /// or I/O fails.
    pub fn open(backend: B, config: StorageEngineConfig) -> Result<Self, StorageError> {
        let (superblock, slot) =
            format::select_active_superblock(&backend, config.page_size)?;

        // Validate identity header
        let mut hdr_buf = [0u8; IDENTITY_HEADER_SIZE];
        backend.read_at(0, &mut hdr_buf).map_err(map_backend_err)?;
        let identity = FileIdentityHeader::deserialize(&hdr_buf)?;
        identity.validate_compatible()?;
        let file_page_size = identity.page_size()?;
        if file_page_size != config.page_size {
            return Err(StorageError {
                message: format!(
                    "page size mismatch: file has {file_page_size}, config has {}",
                    config.page_size
                ),
                #[cfg(feature = "std")]
                source: None,
            });
        }

        let allocator = PageAllocator::new(superblock.total_pages, config.page_size);
        let buffer_pool = BufferPool::new(config.buffer_pool_frames, config.page_size);
        let btree = BTree::new(BTreeConfig {
            page_size: config.page_size,
        });

        Ok(Self {
            backend,
            buffer_pool,
            allocator,
            active_superblock: superblock,
            active_slot: slot,
            page_size: config.page_size,
            btree,
        })
    }

    /// Returns the current snapshot (set of B-tree root page IDs).
    pub fn current_snapshot(&self) -> Snapshot {
        Snapshot::from(&self.active_superblock)
    }

    /// Returns the page size in bytes.
    pub fn page_size(&self) -> usize {
        self.page_size
    }

    /// Returns a reference to the B-tree operations.
    pub fn btree(&self) -> &BTree {
        &self.btree
    }

    /// Returns a mutable reference to the buffer pool.
    pub fn buffer_pool_mut(&mut self) -> &mut BufferPool {
        &mut self.buffer_pool
    }

    /// Returns a reference to the backend.
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Returns a mutable reference to the backend.
    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Returns a mutable reference to the page allocator.
    pub fn allocator_mut(&mut self) -> &mut PageAllocator {
        &mut self.allocator
    }

    /// Returns the current transaction ID.
    pub fn transaction_id(&self) -> u64 {
        self.active_superblock.transaction_id
    }

    /// Searches for a key in a B-tree identified by its root page.
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure or checksum mismatch.
    pub fn search(&mut self, root: PageId, key: &[u8]) -> Result<Option<Vec<u8>>, StorageError> {
        self.btree
            .search(root, key, &mut self.buffer_pool, &mut self.backend)
    }

    /// Performs a range scan on a B-tree, collecting all key-value pairs
    /// in the range `[start_key, end_key)`.
    ///
    /// If `end_key` is `None`, scans to the end of the tree.
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure or checksum mismatch.
    #[allow(clippy::type_complexity)]
    pub fn range_scan(
        &mut self,
        root: PageId,
        start_key: &[u8],
        end_key: Option<&[u8]>,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>, StorageError> {
        let config = self.btree.config.clone();
        let mut cursor = btree::cursor::BTreeCursor::new(
            root,
            start_key,
            end_key,
            &mut self.buffer_pool,
            &mut self.backend,
            &config,
        )?;
        let mut results = Vec::new();
        while let Some(entry) =
            cursor.next(&mut self.buffer_pool, &mut self.backend, &config)?
        {
            results.push(entry);
        }
        Ok(results)
    }

    /// Inserts a key-value pair into a B-tree identified by its root page.
    ///
    /// Returns the new root page ID and sets of freed/allocated pages.
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure or buffer pool exhaustion.
    pub fn insert(
        &mut self,
        root: PageId,
        key: &[u8],
        value: &[u8],
        txn_id: u64,
    ) -> Result<btree::cow::CowResult, StorageError> {
        self.btree.insert(
            root,
            key,
            value,
            &mut self.buffer_pool,
            &mut self.allocator,
            &mut self.backend,
            txn_id,
        )
    }

    /// Deletes a key from a B-tree identified by its root page.
    ///
    /// Returns `Some(CowResult)` if the key was found and deleted, `None` otherwise.
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure or buffer pool exhaustion.
    pub fn delete(
        &mut self,
        root: PageId,
        key: &[u8],
        txn_id: u64,
    ) -> Result<Option<btree::cow::CowResult>, StorageError> {
        self.btree.delete(
            root,
            key,
            &mut self.buffer_pool,
            &mut self.allocator,
            &mut self.backend,
            txn_id,
        )
    }

    /// Commits the current set of B-tree mutations to disk.
    ///
    /// Implements the 2-fsync commit protocol per `008-file-format-spec.md` §13:
    /// 1. Flush all dirty data pages
    /// 2. First fsync (sync_all if file extended, else sync_data)
    /// 3. Write new superblock to inactive slot
    /// 4. Second fsync (sync_data)
    /// 5. Update internal state
    ///
    /// # Errors
    ///
    /// Returns an error on I/O failure.
    pub fn commit(
        &mut self,
        new_roots: SnapshotRoots,
        freed_pages: Vec<(u64, PageId)>,
    ) -> Result<Snapshot, StorageError> {
        let new_txn_id = self.active_superblock.transaction_id + 1;
        let new_total = self.allocator.total_pages();

        // Phase 1: Flush all dirty data pages
        self.buffer_pool.flush_all_dirty(&mut self.backend)?;

        // Phase 2: First fsync
        let file_extended = new_total > self.active_superblock.total_pages;
        if file_extended {
            // Extend file first
            self.backend
                .set_len(new_total * self.page_size as u64)
                .map_err(map_backend_err)?;
            self.backend.sync_all().map_err(map_backend_err)?;
        } else {
            self.backend.sync_data().map_err(map_backend_err)?;
        }

        // Phase 3: Write new superblock to inactive slot
        let inactive_slot = 1 - self.active_slot;
        let new_superblock = Superblock {
            transaction_id: new_txn_id,
            total_pages: new_total,
            feature_flags: self.active_superblock.feature_flags,
            root_node_store: new_roots.node_store,
            root_edge_store: new_roots.edge_store,
            root_outgoing_adj: new_roots.outgoing_adj,
            root_incoming_adj: new_roots.incoming_adj,
            root_type_index: new_roots.type_index,
            root_schema_store: new_roots.schema_store,
            root_id_freelist: new_roots.id_freelist,
            root_page_freelist: new_roots.page_freelist,
            checksum: 0, // computed below
        };

        // Build superblock page with identity header
        let mut sb_page = vec![0u8; self.page_size];
        // Read identity header from page 0
        self.backend
            .read_at(0, &mut sb_page[..IDENTITY_HEADER_SIZE])
            .map_err(map_backend_err)?;
        new_superblock.serialize(&mut sb_page);
        let checksum = Superblock::compute_checksum(&sb_page);
        sb_page[184..192].copy_from_slice(&checksum.to_le_bytes());

        // Write to inactive slot
        let sb_offset = inactive_slot as u64 * self.page_size as u64;
        self.backend
            .write_at(sb_offset, &sb_page)
            .map_err(map_backend_err)?;

        // Phase 4: Second fsync
        self.backend.sync_data().map_err(map_backend_err)?;

        // Phase 5: Update internal state
        self.active_superblock = Superblock {
            checksum,
            ..new_superblock
        };
        self.active_slot = inactive_slot;
        self.allocator.reset_transaction();

        // Store freed pages for future MVCC reclamation (deferred to freelist insert)
        let _ = freed_pages; // In a full implementation, these would be inserted into the
                             // Page Freelist B-tree. For now, they are tracked but not yet
                             // inserted into the freelist. The db layer (Task 25) will handle this.

        let snapshot = Snapshot::from(&self.active_superblock);
        Ok(snapshot)
    }
}

#[cfg(test)]
mod engine_tests {
    use super::*;
    use crate::storage::test_utils::TestBackend;

    fn default_config() -> StorageEngineConfig {
        StorageEngineConfig {
            page_size: DEFAULT_PAGE_SIZE,
            buffer_pool_frames: buffer_pool::MIN_BUFFER_POOL_FRAMES,
            application_id: 0,
        }
    }

    #[test]
    fn create_database() {
        let backend = TestBackend::new();
        let engine = StorageEngine::create(backend, default_config()).unwrap();
        let snap = engine.current_snapshot();
        assert_eq!(snap.transaction_id, 1);
        assert_eq!(snap.total_pages, 3);
        assert_eq!(snap.roots.schema_store, PageId(2));
        assert!(snap.roots.node_store.is_null());
    }

    #[test]
    fn create_then_open() {
        let mut backend = TestBackend::new();
        {
            let engine = StorageEngine::create(backend, default_config()).unwrap();
            backend = engine.backend;
        }

        let engine = StorageEngine::open(backend, default_config()).unwrap();
        let snap = engine.current_snapshot();
        assert_eq!(snap.transaction_id, 1);
        assert_eq!(snap.roots.schema_store, PageId(2));
    }

    #[test]
    fn insert_and_commit() {
        let backend = TestBackend::new();
        let mut engine = StorageEngine::create(backend, default_config()).unwrap();
        let snap = engine.current_snapshot();

        // Insert a key into the node store
        let key = 42u64.to_be_bytes();
        let value = b"test-node";
        let result = engine.insert(snap.roots.node_store, &key, value, 2).unwrap();

        // Build new roots
        let new_roots = SnapshotRoots {
            node_store: result.new_root,
            ..snap.roots
        };
        let freed = result
            .freed_pages
            .into_iter()
            .map(|p| (2u64, p))
            .collect();

        let new_snap = engine.commit(new_roots, freed).unwrap();
        assert_eq!(new_snap.transaction_id, 2);
        assert!(!new_snap.roots.node_store.is_null());

        // Search for the key
        let found = engine.search(new_snap.roots.node_store, &key).unwrap();
        assert_eq!(found, Some(value.to_vec()));
    }

    #[test]
    fn multiple_commits() {
        let backend = TestBackend::new();
        let mut engine = StorageEngine::create(backend, default_config()).unwrap();
        let mut snap = engine.current_snapshot();

        for i in 1..=5u64 {
            let key = i.to_be_bytes();
            let value = format!("val-{i}").into_bytes();
            let txn_id = snap.transaction_id + 1;
            let result = engine
                .insert(snap.roots.node_store, &key, &value, txn_id)
                .unwrap();

            let new_roots = SnapshotRoots {
                node_store: result.new_root,
                ..snap.roots
            };
            let freed = result
                .freed_pages
                .into_iter()
                .map(|p| (txn_id, p))
                .collect();

            snap = engine.commit(new_roots, freed).unwrap();
        }

        assert_eq!(snap.transaction_id, 6); // 1 (initial) + 5 commits

        // All keys searchable
        for i in 1..=5u64 {
            let found = engine.search(snap.roots.node_store, &i.to_be_bytes()).unwrap();
            assert!(found.is_some(), "key {i} not found");
        }
    }

    #[test]
    fn superblock_alternation() {
        let backend = TestBackend::new();
        let mut engine = StorageEngine::create(backend, default_config()).unwrap();
        assert_eq!(engine.active_slot, 0);

        let snap = engine.current_snapshot();
        let key = 1u64.to_be_bytes();
        let result = engine.insert(snap.roots.node_store, &key, b"v1", 2).unwrap();
        let new_roots = SnapshotRoots {
            node_store: result.new_root,
            ..snap.roots
        };
        let snap2 = engine.commit(new_roots, vec![]).unwrap();
        assert_eq!(engine.active_slot, 1);

        let result2 = engine.insert(snap2.roots.node_store, &2u64.to_be_bytes(), b"v2", 3).unwrap();
        let new_roots2 = SnapshotRoots {
            node_store: result2.new_root,
            ..snap2.roots
        };
        engine.commit(new_roots2, vec![]).unwrap();
        assert_eq!(engine.active_slot, 0); // alternated back
    }
}

#[cfg(test)]
mod crash_recovery_tests {
    use super::*;
    use crate::backend::{Durability, ReadAt, WriteAt};
    use crate::storage::test_utils::TestBackend;

    fn default_config() -> StorageEngineConfig {
        StorageEngineConfig {
            page_size: DEFAULT_PAGE_SIZE,
            buffer_pool_frames: buffer_pool::MIN_BUFFER_POOL_FRAMES,
            application_id: 0,
        }
    }

    /// Helper: create a database and commit some initial data.
    fn create_with_data() -> (TestBackend, Snapshot) {
        let backend = TestBackend::new();
        let mut engine = StorageEngine::create(backend, default_config()).unwrap();
        let snap = engine.current_snapshot();

        // Insert 5 keys and commit
        let mut root = snap.roots.node_store;
        let mut all_freed = Vec::new();
        for i in 1..=5u64 {
            let r = engine.insert(root, &i.to_be_bytes(), &format!("v{i}").into_bytes(), 2).unwrap();
            all_freed.extend(r.freed_pages.into_iter().map(|p| (2u64, p)));
            root = r.new_root;
        }
        let new_roots = SnapshotRoots {
            node_store: root,
            ..snap.roots
        };
        let committed_snap = engine.commit(new_roots, all_freed).unwrap();
        (engine.backend, committed_snap)
    }

    #[test]
    fn crash_interrupted_data_write_no_superblock() {
        // 1. Create database with data (txn 2 committed)
        let (mut backend, committed_snap) = create_with_data();

        // 2. Start new mutations: insert new keys, flush data pages — but do NOT commit
        let mut engine = StorageEngine::open(backend, default_config()).unwrap();
        let snap = engine.current_snapshot();
        let _r = engine.insert(snap.roots.node_store, &100u64.to_be_bytes(), b"new", 3).unwrap();
        engine.buffer_pool.flush_all_dirty(&mut engine.backend).unwrap();
        // 3. "Crash" — do NOT write superblock. Drop engine without commit.
        backend = engine.backend;

        // 4. Reopen — should see only committed data
        let engine2 = StorageEngine::open(backend, default_config()).unwrap();
        let snap2 = engine2.current_snapshot();
        assert_eq!(snap2.transaction_id, committed_snap.transaction_id);

        // Key 100 should NOT be visible
        let mut engine2 = engine2;
        let found = engine2.search(snap2.roots.node_store, &100u64.to_be_bytes()).unwrap();
        assert!(found.is_none());

        // Original keys should still be there
        for i in 1..=5u64 {
            let found = engine2.search(snap2.roots.node_store, &i.to_be_bytes()).unwrap();
            assert!(found.is_some(), "committed key {i} missing after crash");
        }
    }

    #[test]
    fn crash_corrupted_superblock_fallback() {
        // 1. Create and commit 2 transactions so both slots are written
        let (mut backend, _snap1) = create_with_data();
        let mut engine = StorageEngine::open(backend, default_config()).unwrap();
        let snap = engine.current_snapshot();

        // Commit again so both superblock slots have valid data
        let r = engine.insert(snap.roots.node_store, &10u64.to_be_bytes(), b"ten", 3).unwrap();
        let new_roots = SnapshotRoots {
            node_store: r.new_root,
            ..snap.roots
        };
        let snap2 = engine.commit(new_roots, vec![]).unwrap();
        let active_slot = engine.active_slot;
        backend = engine.backend;

        // 2. Corrupt the active superblock's checksum
        let corrupt_offset = active_slot as u64 * DEFAULT_PAGE_SIZE as u64 + 184;
        let mut corrupt_bytes = [0u8; 8];
        backend.read_at(corrupt_offset, &mut corrupt_bytes).unwrap();
        corrupt_bytes[0] ^= 0xFF;
        backend.write_at(corrupt_offset, &corrupt_bytes).unwrap();

        // 3. Reopen — should fall back to the other valid superblock
        let (fallback_sb, fallback_slot) =
            format::select_active_superblock(&backend, DEFAULT_PAGE_SIZE).unwrap();

        // The fallback slot is the other one
        assert_ne!(fallback_slot, active_slot);
        // It has an older transaction_id
        assert!(fallback_sb.transaction_id < snap2.transaction_id);
    }

    #[test]
    fn crash_after_first_fsync_before_superblock() {
        // Same as interrupted data write — new data pages exist but superblock not updated
        let (mut backend, committed_snap) = create_with_data();
        let mut engine = StorageEngine::open(backend, default_config()).unwrap();
        let snap = engine.current_snapshot();

        let _r = engine.insert(snap.roots.node_store, &200u64.to_be_bytes(), b"new", 3).unwrap();
        // Flush + fsync (phase 1-2 of commit)
        engine.buffer_pool.flush_all_dirty(&mut engine.backend).unwrap();
        Durability::sync_data(&mut engine.backend).map_err(map_backend_err).unwrap();
        // "Crash" before superblock write
        backend = engine.backend;

        let engine2 = StorageEngine::open(backend, default_config()).unwrap();
        assert_eq!(engine2.current_snapshot().transaction_id, committed_snap.transaction_id);
    }

    #[test]
    fn recovery_with_one_valid_superblock() {
        // Create and do 2 commits so both superblock slots are written
        let (mut backend, _) = create_with_data();
        let mut engine = StorageEngine::open(backend, default_config()).unwrap();
        let snap = engine.current_snapshot();
        let r = engine.insert(snap.roots.node_store, &99u64.to_be_bytes(), b"x", 3).unwrap();
        let new_roots = SnapshotRoots { node_store: r.new_root, ..snap.roots };
        engine.commit(new_roots, vec![]).unwrap();
        backend = engine.backend;

        // Corrupt superblock slot 0's checksum
        let mut checksum_buf = [0u8; 8];
        backend.read_at(184, &mut checksum_buf).unwrap();
        checksum_buf[0] ^= 0xFF;
        backend.write_at(184, &checksum_buf).unwrap();

        // select_active_superblock should succeed using slot 1
        let (sb, slot) = format::select_active_superblock(&backend, DEFAULT_PAGE_SIZE).unwrap();
        assert_eq!(slot, 1);
        assert!(sb.transaction_id >= 1);
    }
}

#[cfg(test)]
pub(crate) mod test_utils {
    //! Test-only in-memory backend for storage tests.

    use crate::sync::Mutex;

    use crate::backend::error::{StorageErrorKind, BackendErrorType};
    use crate::backend::{ReadAt, WriteAt};

    /// A simple in-memory storage backend error.
    #[derive(Debug)]
    pub struct TestError {
        pub kind: StorageErrorKind,
        pub message: String,
    }

    impl core::fmt::Display for TestError {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            write!(f, "{}: {}", self.kind, self.message)
        }
    }

    impl std::error::Error for TestError {}

    impl crate::backend::error::BackendError for TestError {
        fn kind(&self) -> StorageErrorKind {
            self.kind
        }
    }

    /// A test-only in-memory storage backend.
    ///
    /// Uses a `Vec<u8>` as the backing store. Thread-safe via `Mutex`.
    pub struct TestBackend {
        data: Mutex<Vec<u8>>,
    }

    impl TestBackend {
        /// Creates a new empty `TestBackend`.
        pub fn new() -> Self {
            Self {
                data: Mutex::new(Vec::new()),
            }
        }

        /// Returns a copy of the current backing data.
        pub fn data(&self) -> Vec<u8> {
            self.data.lock().clone()
        }
    }

    impl BackendErrorType for TestBackend {
        type Error = TestError;
    }

    impl ReadAt for TestBackend {
        fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), TestError> {
            let data = self.data.lock();
            let start = offset as usize;
            let end = start + buf.len();
            if end > data.len() {
                return Err(TestError {
                    kind: StorageErrorKind::OutOfBounds,
                    message: format!(
                        "read_at: offset={offset}, len={}, file_size={}",
                        buf.len(),
                        data.len()
                    ),
                });
            }
            buf.copy_from_slice(&data[start..end]);
            Ok(())
        }

        fn len(&self) -> Result<u64, TestError> {
            Ok(self.data.lock().len() as u64)
        }
    }

    impl WriteAt for TestBackend {
        fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<(), TestError> {
            let mut data = self.data.lock();
            let start = offset as usize;
            let end = start + buf.len();
            if end > data.len() {
                data.resize(end, 0);
            }
            data[start..end].copy_from_slice(buf);
            Ok(())
        }

        fn set_len(&mut self, new_size: u64) -> Result<(), TestError> {
            let mut data = self.data.lock();
            data.resize(new_size as usize, 0);
            Ok(())
        }
    }

    impl crate::backend::Durability for TestBackend {
        fn sync_data(&mut self) -> Result<(), TestError> {
            Ok(())
        }

        fn sync_all(&mut self) -> Result<(), TestError> {
            Ok(())
        }
    }
}
