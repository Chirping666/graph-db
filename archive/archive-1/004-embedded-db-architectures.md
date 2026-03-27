# 004 — Embedded Database Architectures

**Project:** Embedded Graph Database with Extensible Schema & Pluggable Inference
**Task:** 4 — Research: Embedded Database Architectures
**Status:** Complete
**Intended audience:** The designer of this project and all downstream Claude instances. A reader with programming experience but general database knowledge (from `001-db-internals-fundamentals.md`) should be able to read this document and understand how each embedded database makes its architectural choices, and what those choices imply for this project. Readers already familiar with SQLite or LMDB internals may skip directly to Section 6 (File Format Patterns Deep-Dive) and Section 8 (Lessons for This Project).

---

## Table of Contents

1. [What "Embedded" Means — and Why It Changes Everything](#1-what-embedded-means--and-why-it-changes-everything)
2. [SQLite — The Gold Standard](#2-sqlite--the-gold-standard)
3. [LMDB — The CoW Pioneer](#3-lmdb--the-cow-pioneer)
4. [redb — The Modern Rust CoW Database](#4-redb--the-modern-rust-cow-database)
5. [DuckDB — The Analytical Embedded Database](#5-duckdb--the-analytical-embedded-database)
6. [File Format Design Patterns Deep-Dive](#6-file-format-design-patterns-deep-dive)
7. [Concurrency Models Compared](#7-concurrency-models-compared)
8. [Lessons Applicable to This Project](#8-lessons-applicable-to-this-project)
9. [Cross-Reference: Connecting to Tasks 1, 2, and 3](#9-cross-reference-connecting-to-tasks-1-2-and-3)

---

## 1. What "Embedded" Means — and Why It Changes Everything

An **embedded database** is one that runs within the address space of the application that uses it. There is no separate database server process. The database library is linked directly into the application binary. This has profound consequences for every design decision:

### No network layer
All data access is in-process function calls, not network protocol round-trips. Latency is microseconds, not milliseconds. This means the database cannot amortize per-operation overhead the way a server database can (no "client sends a batch of 100 queries; server processes them in one syscall"). Each operation must be cheap on its own.

### Single-process ownership (usually)
A server database like PostgreSQL is designed for hundreds of simultaneous client connections from different processes, potentially on different machines. An embedded database is designed primarily for single-process access (though some, like LMDB, support multi-process access to the same file via memory-mapped I/O). This simplifies concurrency: instead of a complex lock manager serving arbitrary clients, the database can use simpler in-process synchronization (mutexes, RwLocks, atomics).

### The application is the query planner
In a server database, the SQL parser and query planner are server-side components. In an embedded database, the query planner runs in the same process as everything else. For databases with a query language (SQLite, DuckDB), this means the query planning logic lives in the library. For databases without a query language (redb, LMDB, this project), the application code *is* the query planner — the developer makes traversal decisions directly.

### Resource ownership
An embedded database shares memory with the application. The buffer pool competes with application heap allocations. This makes memory limits a first-class concern: the database must not silently consume all available RAM, and it must degrade gracefully under memory pressure.

### Single-file constraint
Server databases typically manage a directory of files (data files, WAL files, log files, temporary files). Embedded databases are under strong pressure to operate as a single file — users do not want a hidden directory of database files appearing alongside their application data. This constraint shapes every file format decision.

### No_std considerations
An embedded database intended for constrained environments (`no_std + alloc`) cannot rely on OS services like threads, file I/O, or even allocation via the system allocator. The database must be abstracted over a hardware abstraction layer (HAL). This is unusual — neither SQLite, LMDB, redb, nor DuckDB support `no_std`. But embedded-hal-style Rust crates demonstrate the pattern is viable.

---

## 2. SQLite — The Gold Standard

### 2.1 What SQLite is

SQLite is the most widely deployed database in the world (billions of instances). It is a C library providing a full SQL database within a single `.db` file. Despite its name, it is not "lite" in capability — it implements most of SQL92, ACID transactions, triggers, views, full-text search extensions, and a surprisingly sophisticated query planner. It is used in every iOS device, every Android device, most web browsers, and countless embedded systems.

SQLite is an excellent case study because:
- Its file format is a published, stable standard (version 3 format has been stable since 2004)
- Its source code is a single ~250,000-line amalgamation file, extensively documented
- It has survived two decades of production use across every imaginable deployment scenario
- Its design decisions are well-documented in published papers and the SQLite documentation

### 2.2 Architecture overview

```
┌─────────────────────────────────────────────────────────────┐
│                     SQLite Architecture                      │
├──────────────────────────┬──────────────────────────────────┤
│   SQL Interface Layer    │  sqlite3_exec, sqlite3_prepare   │
├──────────────────────────┴──────────────────────────────────┤
│   SQL Compiler           │  Tokenizer → Parser → Code Gen  │
│                          │  (produces VDBE bytecode)        │
├─────────────────────────────────────────────────────────────┤
│   Virtual Machine (VDBE) │  Bytecode interpreter           │
├─────────────────────────────────────────────────────────────┤
│   B-Tree Layer           │  Table B-trees + Index B-trees  │
├─────────────────────────────────────────────────────────────┤
│   Pager                  │  Page cache + WAL management    │
│                          │  (the heart of SQLite)          │
├─────────────────────────────────────────────────────────────┤
│   OS Interface (VFS)     │  Abstracts file I/O per OS      │
└─────────────────────────────────────────────────────────────┘
```

The key insight: the **Pager** is the component that manages the single `.db` file. Everything above it (B-trees, VDBE, SQL compiler) treats data as an abstract collection of numbered pages. Everything below it (VFS) abstracts over operating system file I/O. The Pager sits in the middle, responsible for:
- Reading and writing pages to/from the file
- The page cache (equivalent to a buffer pool, though small by default — 2MB)
- WAL management (in WAL mode) or rollback journal management (in journal mode)
- Crash recovery
- Concurrency control (file locking)

### 2.3 File format

The SQLite file format is documented in full at https://www.sqlite.org/fileformat2.html. Key aspects:

**Page size:** Fixed for the lifetime of the database, chosen at creation time. Historically 1024 bytes; SQLite defaults to 4096 bytes today. Must be a power of 2 between 512 and 65536. The first page of the file begins with a 100-byte **database header**.

**Database header (first 100 bytes of page 1):**
```
Offset  Size  Description
0       16    Magic string: "SQLite format 3\000"
16      2     Page size in bytes (or 1 meaning 65536)
18      1     File format write version (1=legacy, 2=WAL)
19      1     File format read version
20      1     Reserved space per page (for extensions)
21      1     Max embedded payload fraction (must be 64)
22      1     Min embedded payload fraction (must be 32)
23      1     Leaf payload fraction (must be 32)
24      4     File change counter (incremented on every write transaction)
28      4     Size of the database file in pages
32      4     Page number of first freelist trunk page
36      4     Total number of freelist pages
40      4     Schema cookie (incremented on every schema change)
44      4     Schema format number (1–4)
48      4     Default page cache size
52      4     Largest root B-tree page number (for auto-vacuum)
56      4     Text encoding (1=UTF-8, 2=UTF-16le, 3=UTF-16be)
60      4     User version (available for application use)
64      4     Incremental vacuum mode (0=none, >0=auto-vacuum)
68      4     Application ID (for file type identification)
72      20    Reserved for expansion (must be zeros)
92      4     Version-valid-for number
96      4     SQLite version number
```

This header is a masterclass in embedded database format design:
- **Magic string** identifies the file type unambiguously
- **File format version** separates read vs. write compatibility (a newer reader can always read old formats; older readers may refuse to write to newer formats)
- **Schema cookie** detects schema changes without reading the full schema
- **Application ID** allows applications to claim the file format (prevents SQLite from opening an arbitrary binary file and corrupting it)
- **Reserved space per page** allows future extensions to add per-page metadata without breaking the format
- **Zero-padding at offset 72–92** reserves space for future header fields

**Page types:**
- **Interior B-tree page:** Stores separator keys and child page pointers
- **Leaf B-tree page:** Stores the actual row data
- **Overflow page:** Holds data that doesn't fit in a B-tree page (cells > ~1/4 of the page size)
- **Pointer map page** (auto-vacuum mode): Maps page numbers to their parent page for efficient vacuum
- **Freelist trunk page:** The head of the freelist chain
- **Freelist leaf page:** Pages on the freelist

**Free-space management:**
SQLite uses a **freelist**: deleted pages are added to a linked list of free pages. The list head pointer is in the database header. Each freelist trunk page can hold up to `(page_size / 4) - 2` pointers to freelist leaf pages. On allocation, SQLite takes from the freelist if available; otherwise appends to the end of the file.

**B-tree structure:**
SQLite stores each table as a separate B-tree, with the B-tree root page number stored in the `sqlite_master` table (itself a B-tree). Each row in a table is a **cell** in the leaf pages of that table's B-tree, encoded with a compact **record format** (variable-length integers, type-tagged values). Indexes are stored as separate B-trees whose keys include the indexed column values plus the row ID.

### 2.4 Concurrency: WAL mode vs. journal mode

SQLite has two main concurrency modes:

**Rollback journal mode (legacy):**
- Before modifying a page, copy the original page to a journal file (`database.db-journal`)
- Modify the page in the database file
- On commit: delete the journal file (making the transaction permanent)
- On crash recovery: if journal exists at startup, copy journal pages back to the database file (undo the uncommitted changes)
- Concurrency: **exclusive write lock on the whole database file during writes**. Readers are blocked during writes. Writers block all readers. This is called the "big lock" approach.

**WAL mode (default in modern SQLite):**
- Writes go to a separate WAL file (`database.db-wal`) instead of modifying the main database
- Readers read from the main database file, checking the WAL for any newer versions of pages
- The WAL is appended to; the main database file is only updated during a **checkpoint** (which is often automatic)
- Concurrency: **multiple simultaneous readers** (each reads from a consistent WAL position); **one writer at a time** (but does not block readers)
- WAL file is a separate file, which is a minor violation of the "single file" ideal (mitigated: many applications treat `.db`, `.db-wal`, and `.db-shm` as a unit)

**WAL shared memory file (`-shm`):**
SQLite WAL mode uses a shared memory file (`.db-shm`) to coordinate between multiple processes or threads reading the same WAL. It stores the WAL index — a mapping from (page_number → WAL frame offset) — so that readers can find the most recent version of each page in the WAL without scanning from the beginning. The `-shm` file is reconstructable from the WAL; losing it is not data loss.

### 2.5 Crash recovery

**Journal mode recovery:** At startup, if the journal file exists, SQLite reads it and copies original pages back to the database file, then deletes the journal. Straightforward.

**WAL mode recovery:** At startup, if the WAL file exists but is incomplete (crash during WAL write), SQLite determines the last valid committed frame (each WAL frame has a checksum; the first frame with an invalid checksum marks the end of valid data). Frames beyond this point are ignored. The WAL is then checkpointed. This is robust against torn writes because each WAL frame has an independent checksum.

**Torn write protection:** SQLite detects torn writes in the main database file using page checksums (in WAL mode, checksums are in the WAL frame header). In journal mode, the journal itself acts as the protection.

### 2.6 VFS (Virtual File System) abstraction

SQLite abstracts all file I/O through a **VFS** (Virtual File System) interface. The VFS defines:
- `xOpen`, `xDelete`, `xAccess`, `xFullPathname` — file and directory operations
- `xRead`, `xWrite`, `xTruncate`, `xSync` — file content operations
- `xLock`, `xUnlock`, `xCheckReservedLock` — file locking operations
- `xSleep`, `xCurrentTime`, `xGetLastError` — OS services

This allows SQLite to run on Windows, Linux, macOS, iOS, Android, VxWorks, and custom embedded OS environments by swapping the VFS implementation. An in-memory VFS is also provided.

The VFS is a close conceptual ancestor of this project's HAL trait system (Task 9).

### 2.7 Startup and shutdown

**Startup:**
1. Open the database file
2. Read the 100-byte header; validate magic, version, page size
3. Read the freelist head page (if any)
4. If WAL mode: open or create the WAL file; reconstruct the WAL index
5. The first read transaction begins lazily (on first query)
6. Schema is loaded on first access (the `sqlite_master` B-tree is traversed)

**Shutdown:**
1. Commit or rollback any open transactions
2. Flush all dirty pages from the page cache to disk
3. If WAL mode: optionally checkpoint (fold the WAL into the main database file)
4. Close file handles
5. Release memory

SQLite does not require a dedicated shutdown procedure if the process is killed — crash recovery handles incomplete writes correctly. This is a design goal: the database must be safe even against `kill -9`.

### 2.8 In-memory vs. on-disk boundary

SQLite's page cache (managed by the Pager) is the boundary. By default it is 2MB (512 × 4KB pages). The developer can increase it. When the cache is full and a new page is needed, the Pager evicts a page (least recently used among unpinned pages); if the evicted page is dirty, it is written to disk (or the WAL) first.

SQLite also supports a fully **in-memory database** (`sqlite3_open(":memory:")`) where all pages live in a hash table in memory, never touching disk. This shares the same code paths as the on-disk mode — the VFS layer simply discards write calls and returns an empty page on reads.

---

## 3. LMDB — The CoW Pioneer

### 3.1 What LMDB is

**LMDB (Lightning Memory-Mapped Database)** is a C library developed by Howard Chu for the OpenLDAP project. It is a key-value store — no SQL, no query language, just get/put/delete on sorted byte-string keys. Its distinguishing characteristics:

- **Memory-mapped I/O:** The entire database file is mapped into the process's virtual address space with `mmap()`. Reads are pure pointer dereferences — no `read()` syscall, no buffer copying. The OS page cache serves as the buffer pool.
- **Copy-on-write B+ tree:** All writes produce new page versions; the old tree remains valid as a read-only snapshot.
- **No WAL, no journal:** Crash safety comes entirely from the CoW + atomic root-pointer-swap mechanism.
- **MVCC via CoW:** Each read transaction holds a reference to a specific root page, seeing a consistent snapshot regardless of concurrent writes.
- **Single writer, multiple readers:** One write transaction at a time (enforced by a mutex); unlimited concurrent read transactions (readers never block and are never blocked).

LMDB is the direct architectural ancestor of redb (Section 4). Understanding LMDB is essential for understanding why redb made the choices it did.

### 3.2 Memory-mapped I/O — how it works

`mmap()` maps a file into virtual address space. The OS manages paging: when the process reads an address in the mapped region, the OS loads the corresponding page from the file into physical memory if it isn't already there (a page fault). The OS page cache is shared between processes mapping the same file.

**Benefits:**
- **Zero-copy reads:** Reading a page is a memory access, not a `read()` syscall. Extremely fast for random access on hot data (already in page cache).
- **Shared buffer pool:** Multiple processes mapping the same file share the OS page cache automatically. No coordination needed.
- **Simple implementation:** The storage engine does not need to implement a buffer pool — the OS provides one.

**Drawbacks:**
- **No buffer pool control:** The OS decides which pages to evict from the page cache. The application cannot pin pages or control eviction policy.
- **File size must be declared upfront (or grown carefully):** `mmap()` requires a region size. LMDB requires the user to declare a maximum database size at open time. Growing the database requires re-mapping.
- **Write complexity:** Writing to the mmap'd region in a CoW design means allocating new pages at specific offsets, which requires managing the free-page list carefully.
- **Multi-process safety** on some filesystems requires careful locking for the root-pointer update.
- **64-bit address space required** for large databases (a 32-bit process cannot map a file larger than ~2–3GB).

### 3.3 LMDB file format

LMDB uses a **fixed 4096-byte page size** (not configurable). The file consists of two sections:

**Meta pages (pages 0 and 1):** LMDB maintains two meta pages and alternates between them on every commit. Each meta page contains:
```
Magic number (LMDB_MAGIC)
Format version
Flags (fixed-size keys, no sub-databases, etc.)
Address (mmap base address — for pointer fixup on re-open)
Map size (current declared maximum database size)
Last page number used
Transaction ID of last committed transaction
Root page number of the main B-tree
Root page number of the free-space B-tree
```

The alternation between meta pages 0 and 1 is the **atomic commit mechanism**: on commit, LMDB writes a completely new meta page to the *non-current* meta page slot, then updates an atomic counter indicating which slot is now current. If the process crashes after writing the new meta page but before updating the counter — or even during the meta page write — the old meta page is still valid. There is no partially-written state that could corrupt the database.

**B+ tree pages:** Standard B+ tree structure with interior (branch) and leaf pages. Each page has a 16-byte header:
```
pgno: u64   — page number
pad: u16    — alignment
flags: u16  — page type (branch, leaf, overflow, etc.)
lower: u16  — lower bound of free space (slot array end)
upper: u16  — upper bound of free space (payload start)
```
The slotted-page layout (slot array + payload growing from opposite ends) is identical in concept to SQLite's B-tree pages.

**Free-space management:** LMDB maintains a second B-tree (`free_db`) that maps transaction IDs to lists of freed page numbers. Pages freed in transaction T cannot be reused until no read transaction older than T is still active. This is the MVCC garbage collection mechanism — an elegant use of the existing B-tree machinery.

### 3.4 LMDB concurrency model

- **One writer:** A system mutex (or file lock for multi-process) ensures only one write transaction at a time. Writers never conflict with each other — no deadlock possible.
- **Unlimited readers:** Read transactions hold a reference to a specific meta-page (and thus a specific root page). They see a consistent snapshot from when they started. No reader-writer coordination beyond the root-pointer read.
- **Reader table:** LMDB maintains a shared memory region (typically a `.lock` file) containing a **reader table** — an array of slots, one per reader, recording the transaction ID each reader has locked to. The writer uses this to determine the oldest active reader, which governs how far back the free-space list must be kept.

**MVCC story:** Every committed write transaction increments the transaction ID. Each read transaction captures the current transaction ID at start time. The free-page manager only recycles pages from transaction T when all readers with a start ID ≤ T have finished. This is exact MVCC without version chains in the main data — the entire tree snapshot is the "version."

### 3.5 Crash recovery

**Normal case:** On startup, LMDB reads both meta pages and selects the one with the higher transaction ID that has a valid checksum. If both are valid, the newer one wins. If one is corrupted (torn write during meta page write), the other is still valid. The database is always in a consistent state from the perspective of whichever meta page is chosen.

**Free-space recovery:** The free-space B-tree may contain pages freed by a transaction that was in progress when the crash occurred. On startup, LMDB identifies the last committed transaction (from the meta page) and discards any free-list entries from transactions newer than that. The discarded pages are "leaked" (not in the free list, not in the live tree) — LMDB does not recover them. Over time, crashed partial transactions can cause mild space leaks, which `mdb_env_copy` (a compaction tool) can fix.

**No fsync on reads, fsync on meta page writes:** LMDB fsyncs the data pages first, then fsyncs (or uses `msync`) the meta page write. The ordering guarantees that the meta page points to fully-written data.

### 3.6 Startup and shutdown

**Startup:** Open the file, `mmap()` the region up to `mapsize`, read both meta pages, select the valid one with the higher transaction ID, open the free-space B-tree. Typically takes milliseconds for any database size — no recovery log to replay.

**Shutdown:** Close any open transactions, `munmap()` the region, close file handles. No special flush required — the OS page cache will eventually flush dirty pages (but LMDB's CoW design means pages are only dirty in the OS cache when explicitly written during a commit, after fsync).

### 3.7 Key lessons from LMDB

1. **CoW + dual-meta-page atomic swap is a complete crash-safety solution** — no WAL, no journal, no recovery log replay. Startup is O(1) regardless of database size.
2. **mmap simplifies the buffer pool** but surrenders control. For this project's `no_std` target and HAL abstraction, mmap is not available — we need an explicit buffer pool. But the CoW tree structure can be preserved.
3. **Free-space management is itself a B-tree** — an elegant, uniform solution. One structural mechanism handles both data and metadata.
4. **The reader table (per-reader transaction ID tracking)** is the key to safe MVCC garbage collection. This concept must appear in any MVCC design.
5. **Alternating meta pages** is the canonical atomic commit mechanism for embedded databases. This should be the direct model for this project's file header design.

---

## 4. redb — The Modern Rust CoW Database

### 4.1 What redb is

**redb** is a pure-Rust embedded key-value store that explicitly targets the same use case as LMDB but without `unsafe` code in its core, without `mmap` dependency, and with a Rust-idiomatic API. It is actively maintained (as of 2025), published to crates.io, and is the closest existing analog to what this project intends to build. Its source code is the most directly applicable reference for this project.

redb stores data in a single file. It provides typed key-value tables (typed via Rust generics and a serialization trait), ACID transactions, and MVCC via CoW B-trees.

### 4.2 Architecture overview

```
┌────────────────────────────────────────────────────────┐
│                    redb Architecture                    │
├────────────────────────────────────────────────────────┤
│  Public API Layer                                      │
│  Database, ReadTransaction, WriteTransaction, Table    │
├────────────────────────────────────────────────────────┤
│  Table Manager                                         │
│  Named table registry; maps table names → root pages  │
├────────────────────────────────────────────────────────┤
│  B-tree Layer (copy-on-write)                          │
│  BtreeMut (write), Btree (read), node serialization   │
├────────────────────────────────────────────────────────┤
│  Transaction Manager                                   │
│  In-progress transaction tracking; MVCC lifecycle     │
├────────────────────────────────────────────────────────┤
│  Allocator (page-level free-space management)          │
│  Freed-page tracking per transaction ID               │
├────────────────────────────────────────────────────────┤
│  Page Manager / Storage Layer                          │
│  Page cache, file I/O (no mmap), dirty page tracking  │
├────────────────────────────────────────────────────────┤
│  File backend (std::fs)                                │
│  Standard file I/O with explicit pread/pwrite/fsync   │
└────────────────────────────────────────────────────────┘
```

### 4.3 File format

redb uses a **fixed 4096-byte page size**. The file begins with a **superblock region** that serves a similar role to LMDB's alternating meta pages. Key differences from LMDB:

**Superblock (two copies, as in LMDB):**
redb also maintains two copies of the database header and alternates between them on commit. Each copy is stored in the first few pages of the file. The copy with the higher transaction ID (verified by a checksum) is the active one.

The superblock records:
- Magic number and format version
- Page size
- Database UUID (for detecting cross-database page corruption)
- Current transaction ID
- Root page of the main allocator B-tree (tracks free and used regions)
- Root page of the table registry B-tree (maps table names to their root pages)
- Checksum

**Page layout:** redb pages have a compact header followed by B-tree node content. Interior nodes store (key, child_page_id) pairs. Leaf nodes store (key, value) pairs, with large values stored in overflow pages.

**Allocator design:** redb's allocator tracks free regions in a B-tree of its own (separate from the user data B-trees). The allocator B-tree maps page numbers to their free/allocated state. On commit, the allocator's B-tree is itself CoW-updated, and the new root is written to the superblock.

**Freed page tracking (MVCC):** redb maintains a list of pages freed in each committed transaction. These pages cannot be reused until all read transactions that predate the freeing transaction have completed. The freed-page lists are stored in the allocator's B-tree, keyed by transaction ID.

### 4.4 redb concurrency model

- **One write transaction at a time:** A `Mutex` ensures serialized writes. Acquiring the write lock returns a `WriteTransaction` handle.
- **Unlimited concurrent read transactions:** A `ReadTransaction` captures the current committed transaction ID (the "snapshot ID") and holds it for its lifetime. Reads see only data committed before the snapshot ID.
- **MVCC without version chains:** Because redb uses CoW B-trees, "versions" are entire tree snapshots (identified by their root page). A read transaction holds a root page reference; that snapshot is immutable.
- **Reader tracking:** redb maintains an in-memory set of active reader transaction IDs. The writer consults this set to determine the oldest active reader, which determines when freed pages can be reclaimed.

**No deadlock possible:** With one writer and readers-never-blocking, there is no lock acquisition ordering to violate.

### 4.5 Crash recovery

redb's crash recovery is essentially identical to LMDB's:

1. Open the file; read both superblock copies
2. Find the copy with the highest valid transaction ID (checksum verification)
3. Use that as the current state; the other copy is ignored
4. Any pages written after the last valid commit are either:
   - Unreachable (not pointed to by the valid superblock) — they are garbage, reclaimed by the allocator on next write
   - Part of the valid tree — already committed

**Torn write protection:** A write to a 4096-byte page that is itself larger than the filesystem's atomic write unit (typically 512 bytes or 4096 bytes) could be torn. redb addresses this via:
- The superblock's checksum detecting a corrupted superblock write
- Page-level checksums detecting a corrupted data page write (redb validates checksums on read)
- The invariant that the old superblock is never overwritten until the new one is fully written and fsynced

**fsync discipline:**
1. Write all modified data pages to the OS buffer (via `pwrite()`)
2. `fsync()` the data pages to stable storage
3. Write the new superblock copy (to the non-current slot)
4. `fsync()` the superblock
5. Atomically update which superblock slot is current (via a single-page write + fsync)

This precise fsync ordering ensures that no crash can result in a state where the superblock points to data that hasn't been written to disk.

### 4.6 Startup

1. Open the file (create if it doesn't exist)
2. If new: initialize both superblock copies; fsync; return
3. If existing: read both superblock copies; validate checksums; take the newer valid one
4. Initialize the in-memory page cache (empty at startup)
5. Initialize the allocator from the root allocator B-tree page (read lazily)
6. Initialize the table registry from the root table B-tree page (read lazily)
7. Ready to serve transactions — typically takes < 1ms regardless of database size

No log replay. No recovery pass. This is the key advantage of the CoW design.

### 4.7 What makes redb especially relevant to this project

1. **Pure Rust, safe code:** The safety story is aligned with this project's goals.
2. **No mmap:** Uses `pread`/`pwrite` for all I/O, which is directly compatible with a HAL trait abstraction (reading/writing at specific byte offsets is a minimal, implementable interface).
3. **CoW B-tree + dual-superblock:** Exactly the crash safety mechanism to adopt.
4. **Typed tables:** redb's `TableDefinition<K, V>` with generic keys and values is a model for how the schema/type system can layer over a storage core.
5. **Source code is readable and well-organized:** The redb source at https://github.com/cberner/redb is a direct implementation reference.

**Key divergence from this project:** redb is a key-value store — it has no graph traversal, no type hierarchy, no constraint or inference system. This project adds these layers on top of the storage foundation that redb demonstrates is viable.

---

## 5. DuckDB — The Analytical Embedded Database

### 5.1 What DuckDB is

**DuckDB** is an embedded analytical database. It provides full SQL (including window functions, CTEs, joins across multiple tables), columnar storage, and vectorized query execution. It is designed for OLAP (Online Analytical Processing) workloads — complex queries over large datasets — not OLTP (Online Transaction Processing) workloads.

DuckDB is an interesting contrast case: it makes the opposite tradeoffs from LMDB/redb in many dimensions, which helps clarify which choices are essential for this project and which are domain-specific.

### 5.2 Architecture overview

```
┌─────────────────────────────────────────────────────────────┐
│                    DuckDB Architecture                       │
├─────────────────────────────────────────────────────────────┤
│  SQL Layer: Parser (libpg_query), Binder, Planner, Optimizer │
├─────────────────────────────────────────────────────────────┤
│  Execution Engine: Vectorized pipeline execution            │
│  (operates on 2048-row batches, not single rows)            │
├─────────────────────────────────────────────────────────────┤
│  Transaction Manager: MVCC with snapshot isolation          │
├─────────────────────────────────────────────────────────────┤
│  Storage Manager: Columnar block storage                    │
│  Row groups (122,880 rows each), compressed columns         │
├─────────────────────────────────────────────────────────────┤
│  Buffer Manager: Fixed-size block cache (256KB blocks)      │
├─────────────────────────────────────────────────────────────┤
│  Checkpointing: WAL for durability; periodic checkpoints    │
├─────────────────────────────────────────────────────────────┤
│  File I/O: std filesystem, optional mmap                    │
└─────────────────────────────────────────────────────────────┘
```

### 5.3 File format

DuckDB uses a significantly more complex file format than SQLite or redb, because it needs to store compressed columnar data efficiently.

**Block size:** DuckDB uses 256KB blocks by default (much larger than SQLite's 4KB pages). This is optimized for analytical workloads where sequential read of large column chunks is common.

**Database file structure:**
```
[File header — magic + version + main block pointer]
[Block 0: Free list metadata]
[Block 1: Catalog block (schema: tables, indexes, types)]
[Data blocks: Row group data (compressed columns)]
[WAL: append-only log (separate region or separate file)]
```

**Row groups:** DuckDB organizes table data into **row groups** of up to 122,880 rows. Within each row group, each column is stored separately as a sequence of compressed blocks. Compression is applied per column chunk (dictionary encoding, RLE, bit-packing, etc.).

**MVCC design:** DuckDB uses a version chain approach (conceptually similar to PostgreSQL). Each row version points to the next older version. Transactions see the version that was committed before their start timestamp. Unlike LMDB/redb, DuckDB does not use CoW B-trees — it uses in-place updates with explicit version chains.

**WAL in DuckDB:**
DuckDB maintains a WAL (Write-Ahead Log) for durability. Unlike SQLite's WAL, DuckDB's WAL is primarily for crash recovery during a checkpoint cycle. The WAL is periodically checkpointed (folded into the main database file). Between checkpoints, the WAL records all changes.

### 5.4 DuckDB concurrency model

- **Optimistic concurrency control (OCC):** DuckDB uses MVCC with snapshot isolation and optimistic write conflict detection. Multiple writers can proceed concurrently and check for conflicts at commit time.
- **Version chains:** Each modified tuple has a version chain; readers traverse the chain to find the appropriate version for their snapshot timestamp.
- **Multi-version management:** Versions older than the oldest active transaction are garbage-collected.

DuckDB's OCC approach is better suited for OLAP workloads (many readers, infrequent writes that don't conflict) than for OLTP workloads (frequent, potentially conflicting writes on the same rows).

### 5.5 Key architectural contrast with LMDB/redb

| Aspect | LMDB / redb | DuckDB |
|--------|-------------|--------|
| Primary workload | OLTP (transactional) | OLAP (analytical) |
| Storage layout | Row-based (one B-tree entry per row) | Columnar (column chunks per row group) |
| Block/page size | 4 KB | 256 KB |
| Write approach | CoW (new pages, atomic root swap) | In-place with WAL |
| Concurrency | Single writer + MVCC readers | OCC, multiple writers |
| Buffer pool | Implicit (OS page cache via mmap) or explicit | Explicit 256KB block cache |
| Startup time | Instant (no log replay) | Fast (WAL checkpoint replay if needed) |
| Write throughput | Moderate (CoW write amplification) | Higher (OCC, no B-tree CoW amplification) |
| Read throughput | High for random access | High for sequential column scans |

### 5.6 What DuckDB teaches this project

DuckDB's architectural choices are largely **inapplicable** to a graph database foundation that targets OLTP-style transactional access — which is what this project needs. However, several lessons apply:

1. **Block size matters for workload:** DuckDB's 256KB blocks are chosen for sequential analytical reads. This project's graph traversal is random-access heavy — 4KB or 8KB pages are appropriate, not 256KB.

2. **The WAL + checkpoint pattern** is the dominant industry approach for in-place update databases. If this project ever moves away from CoW, WAL + checkpoint is the fallback.

3. **Buffer manager design matters even at small scale:** DuckDB's buffer manager is sophisticated (pin counting, eviction policies, background I/O). Even a small embedded database benefits from a carefully designed buffer manager.

4. **Schema storage as a separate "catalog"** is a pattern shared with DuckDB. The catalog (schema/type registry) is logically separate from the data, even if physically in the same file.

---

## 6. File Format Design Patterns Deep-Dive

This section synthesizes lessons from all four systems above into a structured analysis of file format design patterns. This section is the most directly applicable to Task 8 (Single-File Format Spec).

### 6.1 The magic number and version header

Every durable file format must begin with an **unambiguous identifier** that:
1. Identifies the file type (prevents opening a non-database file as a database)
2. Identifies the application that owns the file (prevents corruption by wrong software versions)
3. Declares the format version (enables migration and forward compatibility)

**Pattern:**
```
[0..16]  Magic bytes: human-readable + control characters
         Example: b"MyDB\x0D\x0A\x1A\x0A\x00"
         The \r\n\x1A\n sequence (borrowed from PNG) detects text-mode corruption:
           \r\n detects Windows line-ending conversion
           \x1A detects EOF truncation on DOS systems
           \n detects Unix line-ending normalization
[16..18] Format major version (breaking changes)
[18..20] Format minor version (additive changes, backward compatible)
[20..22] Application ID (u16 or u32, registered by the application)
```

SQLite uses exactly this pattern. The magic bytes, combined with the application ID, ensure that attempting to open a PNG file, a ZIP file, or a different application's database file fails cleanly with a "wrong format" error rather than silently corrupting the file.

**For this project:** The file should begin with a magic string like `b"EmbedGraph\x0D\x0A\x1A\x0A"` (14 bytes) followed by version and application ID fields. The application ID field allows downstream users to claim the format for their specific use.

### 6.2 The dual-header / alternating-superblock pattern

This is the most important crash-safety pattern for CoW embedded databases. Both LMDB and redb use it; it deserves detailed treatment.

**The problem:** On a commit, the database must atomically transition from "old consistent state" to "new consistent state." This transition is represented by updating a root pointer (or a set of root pointers) in the file header. But writing the header itself is not atomic — if the write is torn, the header is corrupted and neither the old nor new state is recoverable.

**The solution — alternating superblocks:**
```
File layout (pages 0 and 1):
  Page 0: Superblock A
  Page 1: Superblock B
  (pages 2+: B-tree data, slot stores, etc.)

Commit procedure:
  1. Write all new B-tree pages to their target locations (pages 2+)
  2. fsync() — ensure data pages reach stable storage
  3. Compute new superblock content (new root pointers, new txn ID, checksum)
  4. Write new superblock to the INACTIVE slot (the one NOT currently in use)
  5. fsync() — ensure the new superblock reaches stable storage
  6. Atomically update the "which superblock is active" indicator
  7. fsync() — ensure the indicator update reaches stable storage

Recovery:
  1. Read both superblocks
  2. Validate each checksum
  3. The valid one with the higher transaction ID is the current state
  4. If both are valid: newer one wins
  5. If one is invalid (torn write): the other is the valid state
  6. If both are invalid: database is corrupt (extremely unlikely — requires two successive torn writes at the worst moment)
```

**The "which is active" indicator** can be:
- The transaction ID in the superblock (higher = more recent)
- A separate single-byte flag in the file header (simpler but requires its own atomic write)
- An implicit rule: the superblock at even-numbered total commits is in slot A; odd in slot B (derived from the transaction ID modulo 2)

**Best practice:** Use the transaction ID as the tie-breaker (higher transaction ID = more recent valid superblock). No separate indicator needed; just read both, verify checksums, pick the higher valid one.

**Superblock contents** (for this project — see Section 8.2 for the full design):
```
magic: [u8; 14]          — "EmbedGraph\x0D\x0A\x1A\x0A"
format_version: u16      — breaking format changes
application_id: u32      — application-specific identifier
transaction_id: u64      — monotonically increasing commit counter
page_size: u16           — fixed at creation, never changes
db_flags: u32            — configuration flags
node_store_root: u64     — root page of node slot store
edge_store_root: u64     — root page of edge slot store
property_store_root: u64 — root page of property store
schema_btree_root: u64   — root page of schema/type B-tree
type_index_root: u64     — root page of type→node index B-tree
edge_index_root: u64     — root page of edge type B-tree index
freelist_root: u64       — root page of free-page B-tree
total_pages: u64         — current size of the database in pages
checksum: u64            — xxHash3 or CRC32 of all preceding bytes
```

### 6.3 Page header conventions

Every page in the file should begin with a compact header. The key fields:

```
page_id: u64             — the page's own ID (for corruption detection)
page_type: u8            — identifies the page's role
flags: u8                — page-specific flags
checksum: u32            — CRC32 or xxHash32 of page contents
lsn_or_txn_id: u64      — for WAL-based designs: the LSN of the last write
                         — for CoW designs: the transaction ID that wrote this page
```

**Why store the page's own ID?** SQLite calls this the "page number self-check." If a page ends up at the wrong offset (e.g., due to a partial copy operation gone wrong), the stored page ID will not match the expected ID derived from the file offset. This detects a class of corruption that checksums alone cannot catch.

**Checksum placement:** Store the checksum in the page header, covering the entire page content except the checksum field itself. Compute on write; verify on read. If the checksum fails, the page is corrupt — the database can attempt recovery (use the WAL/CoW backup) rather than silently returning corrupt data.

**Checksum algorithm choice:**
- **CRC32C (hardware-accelerated):** Available on x86 via SSE4.2, fast, simple. Standard in many databases. The `crc32fast` crate provides this.
- **xxHash3:** Faster than CRC32 on large data (SIMD-optimized), excellent distribution, not a cryptographic hash. The `xxhash-rust` crate.
- **For `no_std` compatibility:** Both have `no_std` implementations, but the hardware-accelerated variants require feature flags.

**Recommendation:** xxHash3 for performance; fall back to a pure-Rust CRC32 for `no_std` environments without the target CPU feature.

### 6.4 Free-space management strategies

Four strategies appear across the systems studied:

**Strategy A: Linked freelist (SQLite)**
A list of free pages chained through the pages themselves (each free page stores a pointer to the next free page). Simple, no extra structure. Pro: O(1) to allocate a single page. Con: Fragmentation over time; no way to efficiently find a contiguous run of N free pages.

**Strategy B: Free-space B-tree (LMDB, redb)**
A dedicated B-tree mapping page numbers (or ranges) to their free/allocated status, tagged with the transaction ID that freed them. Pro: Supports MVCC-safe reclamation (only reclaim pages freed before the oldest active reader). Con: The B-tree itself consumes pages and requires maintenance; but it shares the CoW B-tree infrastructure.

**Strategy C: Bitmap (PostgreSQL FSM)**
A bitmap or coarse-grained occupancy map (a few bits per page indicating free space available). Pro: Fast scan for a free page. Con: Fixed overhead; coarse-grained (only tracks approximate space, not exact).

**Strategy D: Append-only with explicit compaction**
Never reclaim freed space during normal operation — just append new pages to the end of the file. Periodically run a compaction operation that rewrites the entire database into a compact file. Pro: Extremely simple allocator. Con: File grows without bound until compaction; compaction requires extra disk space and time.

**For this project:** Strategy B (free-space B-tree) is the correct choice. It integrates naturally with the CoW B-tree design (one more B-tree to manage, using the same machinery as everything else), it supports MVCC-safe reclamation, and it handles fragmentation well. The overhead is acceptable for an embedded database.

### 6.5 Overflow pages

When a record (a property block, a B-tree leaf value) exceeds a threshold (typically 1/4 to 1/3 of the page size), it must spill into overflow pages. The standard design:

```
Main page: stores the first portion of the record + count of overflow pages
Overflow page 0: next N bytes of the record + pointer to overflow page 1
Overflow page 1: next N bytes + pointer to overflow page 2
...
Overflow page k: final bytes + null pointer
```

Each overflow page has its own header (page type = overflow, checksum, page ID). The chain length is bounded by the record size; for typical records, overflow chains are 1–3 pages long.

**Threshold choice:** SQLite uses "the cell must fit in a leaf page with at least 4 cells" — approximately 25% of the page size. For a 4096-byte page with a 24-byte header and 8-byte slot entries, the threshold is roughly (4096 - 24) / 4 - 8 ≈ 1010 bytes. Anything larger goes to overflow.

For a property graph, most property blocks (a few string properties per node) will fit comfortably within this threshold. Nodes with very large properties (e.g., a long text document stored as a node property) will use overflow pages.

### 6.6 Extensibility provisions

How does a file format stay viable over years of evolution? Three mechanisms:

**A. Version-gated fields:** The format version number gates which fields are interpreted. Readers that understand version 1 ignore fields at offsets that exist only in version 2+. The reserved zero-padding in the SQLite header demonstrates this — 20 bytes at offset 72 are explicitly reserved for future use.

**B. Feature flags in the header:** A flags word in the superblock can indicate which optional features are active (e.g., compression enabled, secondary indexes present, named-graph support enabled). Readers that don't understand a flag can either reject the file or operate in reduced-capability mode.

**C. Typed page tags:** Every page has a type byte. New page types can be added without breaking readers — old readers simply skip unknown page types (if they know they don't need to interpret them). Unknown page types that appear in a B-tree traversal path are a different problem (they indicate a format version the reader doesn't support).

**For this project:**
- Reserve at least 32 bytes of the superblock for future header fields (initialize to zero)
- Include a `feature_flags: u64` field in the superblock for optional capability bits
- Include an `application_id: u32` field so downstream users can claim the format for their specific ontology layer
- Use typed page headers with explicit version gating

### 6.7 File sizing and growth

**Initial allocation:** Create the file with a few pages (header pages, initial B-tree roots). Do not pre-allocate a large file — grow on demand.

**Growth strategy:** When the allocator finds no free pages, it extends the file:
1. Compute new size: current total pages + growth increment (e.g., 16 pages at a time, up to larger increments for large databases)
2. `ftruncate()` or `SetEndOfFile()` to the new size (this extends the file with zeros, which is a valid "all free pages" state)
3. Add the new page range to the free-space B-tree
4. Allocate from the newly added range

**Why not pre-allocate?** Pre-allocation causes unnecessary disk usage for small databases (the most common case in embedded use). Grow on demand is the right default; the application can provide a "reserve N bytes" API if pre-allocation is desirable.

---

## 7. Concurrency Models Compared

This section provides a focused comparison of how each studied system handles multi-threaded and multi-process concurrent access. This feeds directly into Task 7 (Graph Storage Model) and Task 16 (Storage Engine Implementation).

### 7.1 SQLite: File-level locking

**Thread model:** SQLite is thread-safe in "multi-thread" or "serialized" mode (configurable). In serialized mode, a global mutex serializes all database access.

**Process model (journal mode):** SQLite uses POSIX advisory locks on the database file to coordinate between processes:
- Shared lock: read transaction in progress; prevents other processes from taking write lock
- Reserved lock: write transaction started, not yet committed; other readers can proceed
- Pending lock: write transaction about to commit; new readers must wait
- Exclusive lock: write transaction committing; all other access blocked

**Process model (WAL mode):** In WAL mode, multiple processes can read concurrently (each reads from the main database file plus the WAL). Writers acquire a write lock on the WAL file only. Readers never block. Checkpointing acquires an exclusive lock to fold the WAL into the database.

**Analysis:**
- Journal mode: write-heavy workloads block all readers, even for short read queries
- WAL mode: the right model for most modern workloads; one writer, unlimited readers
- File locking is fragile on some network filesystems (NFS, SMB) — SQLite documentation warns against using it on network-mounted files
- The "database-level lock" granularity means no concurrent write transactions ever (even to different tables) — acceptable for embedded use

### 7.2 LMDB: OS mutex + reader table

**Thread model:** A `pthread_mutex` (or equivalent) serializes write transactions. Readers use no locking — they capture the root page pointer atomically (a single 64-bit load, which is atomic on all modern architectures).

**Process model:** LMDB uses a shared memory file (`.lock` file, `mmap`'d) containing:
- A mutex in shared memory (via a `pthread_mutex` with `PTHREAD_PROCESS_SHARED` attribute)
- A reader table: an array of per-reader slots, each containing the transaction ID the reader has locked to

This allows LMDB to work correctly across multiple processes opening the same database file.

**Analysis:**
- One writer at a time: simple, no deadlock
- Readers truly never block: the root page pointer swap is a single atomic write, invisible to readers
- The reader table enables precise MVCC garbage collection (free pages when no reader is old enough to need them)
- The `PTHREAD_PROCESS_SHARED` mutex requires platform support (not universally available on `no_std`)

### 7.3 redb: In-process Rust synchronization

**Thread model:** redb uses `std::sync::Mutex` for write serialization and in-process reference counting for reader tracking. There is no multi-process support — redb is strictly single-process.

**Reader tracking:**
```rust
// Conceptual (not actual redb code, but captures the design)
struct TransactionTracker {
    next_id: AtomicU64,
    oldest_live_reader: Mutex<Option<u64>>,
    // maps live read txn ID → reader count
    active_readers: Mutex<BTreeMap<u64, usize>>,
}
```

When a read transaction starts: increment the reader count for the current committed snapshot ID. When it ends: decrement. The garbage collector can reclaim pages freed in transaction T once `min(active_readers.keys())` > T.

**Analysis:**
- Strictly single-process: simpler than LMDB's cross-process design, but appropriate for an embedded library
- In-process Mutex and reference counting are safe and efficient in Rust
- No file-level locks needed (file is owned exclusively by one process; the OS provides this via regular file opening)

### 7.4 DuckDB: Multi-version optimistic concurrency

**Thread model:** DuckDB supports multiple concurrent write transactions (with conflict detection at commit time). This is enabled by its MVCC version chain model (each row has a version history) and its optimistic concurrency control approach.

**Snapshot isolation:** Each transaction starts with a snapshot timestamp. Reads see only committed data from before the snapshot. Concurrent writers proceed optimistically; at commit, write-write conflicts cause one transaction to abort.

**Analysis:**
- OCC is appropriate for analytics workloads with rare write conflicts
- For OLTP workloads (like this project's graph mutations), write-write conflicts on the same edge or node are possible, and OCC abort+retry adds latency
- The version chain model (per-row version history) is more complex than CoW (per-tree-snapshot) but allows finer-grained concurrency

### 7.5 Concurrency model comparison table

| System | Writer concurrency | Reader concurrency | Reader/writer isolation | Deadlock possible? | Multi-process? |
|--------|-------------------|-------------------|------------------------|-------------------|----------------|
| SQLite (journal) | One at a time (file lock) | Blocked during write | None (readers wait) | No | Yes |
| SQLite (WAL) | One at a time (WAL lock) | Unlimited concurrent | Full snapshot isolation | No | Yes |
| LMDB | One at a time (mutex) | Unlimited concurrent | Full snapshot isolation | No | Yes (mmap + shared mutex) |
| redb | One at a time (Mutex) | Unlimited concurrent | Full snapshot isolation | No | No (single process) |
| DuckDB | Multiple (OCC) | Multiple | Snapshot isolation | Possible (abort on conflict) | Limited |

### 7.6 The "single writer + MVCC readers" model

The convergence of SQLite (WAL mode), LMDB, and redb on the **single writer + unlimited MVCC readers** model is not coincidental. This model:

1. **Eliminates deadlock** — with one writer at a time, there are no competing write lock acquisitions that could form a cycle
2. **Eliminates reader-writer blocking** — readers hold a snapshot pointer; they never wait for the writer to finish
3. **Simplifies recovery** — no need to undo partial writes by multiple concurrent writers; only one write transaction was ever in progress
4. **Matches embedded workloads** — most embedded database use cases have moderate write rates and high read rates; maximizing read concurrency is more valuable than maximizing write concurrency
5. **Is trivially safe** — the correctness argument fits in a paragraph; complex OCC or 2PL designs have much larger correctness proofs

**For graph databases specifically:** Graph traversal is inherently read-heavy. A typical ontology query involves traversing dozens or hundreds of edges and reading properties at each step — all reads. Writes (adding nodes, edges, asserting new properties) are less frequent. The single-writer model is well-matched.

**Recommendation for this project:** Single writer + unlimited MVCC readers, implemented via:
- A `Mutex` (or `RwLock` with write-mode) for writer serialization
- An `Arc<AtomicU64>` or similar for snapshot ID capture by readers
- A `BTreeMap<u64, usize>` (under a Mutex) for reader reference counting
- CoW B-trees for the schema, type index, and edge index structures
- Freed-page tracking keyed by transaction ID for MVCC-safe reclamation

---

## 8. Lessons Applicable to This Project

This section distills all of the above into direct, actionable lessons for Tasks 7, 8, 9, and 16.

### 8.1 Adopt the CoW + dual-superblock crash safety model

**Evidence:** Both LMDB and redb use this model; it has proven correct in production at scale. SQLite's WAL mode achieves similar guarantees with more complexity. DuckDB's WAL+checkpoint approach is appropriate for OLAP but adds unnecessary complexity for a primarily OLTP-style graph database.

**For this project:**
- Two superblock pages at the beginning of the file (pages 0 and 1)
- All writes go to new pages; no in-place update of committed data
- Commit = write new superblock to the inactive slot + fsync + swap active slot
- Recovery = read both superblocks, pick the valid one with higher transaction ID
- No WAL needed; no log replay on startup

**Risk:** CoW write amplification — a single edge insertion requires copying the pages along the path from the root to the modified leaf in each affected B-tree (O(log N) pages per tree). For a database with a few B-trees (schema, type index, edge index), a single edge insertion might dirty 12–20 pages. This is manageable — each B-tree is typically 2–4 levels deep for databases in the millions of records range.

**Mitigation:** Group multiple writes into a single transaction. An application that batches 100 insertions into one transaction pays the O(log N) write amplification once, not 100 times.

### 8.2 Design the superblock for extensibility from the start

**Evidence:** SQLite's header has a 20-byte reserved section that has prevented format-breaking changes for 20 years. redb's superblock is versioned and checksum-protected.

**For this project, the superblock should include:**
```rust
struct Superblock {
    // Immutable identity (never change after creation)
    magic: [u8; 14],            // b"EmbedGraph\x0D\x0A\x1A\x0A"
    format_version_major: u16,  // Breaking changes
    format_version_minor: u16,  // Additive changes
    application_id: u32,        // Downstream application identifier (0 = unset)
    page_size: u16,             // Fixed at creation (power of 2, 4096 recommended)
    _reserved_identity: [u8; 8], // Must be zero; reserved for future immutable fields

    // Mutable state (updated on each commit)
    transaction_id: u64,        // Monotonically increasing
    feature_flags: u64,         // Bit flags for optional features
    total_pages: u64,           // Current file size in pages
    
    // Root pointers (updated on each commit; all point to CoW B-tree roots or slot store roots)
    schema_btree_root: u64,     // Schema/type registry B-tree
    freelist_btree_root: u64,   // Free-page B-tree (MVCC-aware)
    node_store_head: u64,       // First page of node slot store
    edge_store_head: u64,       // First page of edge slot store
    property_store_root: u64,   // Property block store root
    type_index_root: u64,       // (node_type_id, node_id) → exists index
    edge_src_index_root: u64,   // (src_node_id, edge_type_id, edge_id) B-tree
    edge_tgt_index_root: u64,   // (tgt_node_id, edge_type_id, edge_id) B-tree
    
    // Extensibility
    _reserved_roots: [u64; 4],  // Future root pointers; initialize to 0
    _reserved_flags: [u8; 16],  // Future flags; initialize to 0
    
    // Must be last
    checksum: u64,              // xxHash3 of all preceding bytes
}
// Total: ~160 bytes, fits comfortably in one 4096-byte page with room to spare
```

Each superblock fits in one 4096-byte page, with ~3900 bytes of remaining space for additional future fields. This is explicit breathing room, not waste.

### 8.3 Separate the buffer pool from the mmap concern

**Evidence:** LMDB uses `mmap` and surrenders buffer pool control to the OS. redb uses `pread`/`pwrite` and manages an explicit page cache. For `no_std` + HAL compatibility, mmap is unavailable — an explicit buffer pool is required.

**For this project:**
- The buffer pool is an in-process fixed-size cache of page frames
- Each frame holds: a page copy, the page ID, a dirty flag, a pin count, and an LRU sequence number
- The HAL provides `read_at(offset, buf)` and `write_at(offset, buf)` — no mmap
- The HAL's `flush()` maps to `fsync()` in the std backend
- The buffer pool size is configurable at database open time (default: 4MB = 1024 × 4KB pages)

**Buffer pool API (internal, not public):**
```rust
trait BufferPool {
    fn pin_page(&self, page_id: PageId) -> Result<PageGuard, Error>;
    fn alloc_page(&self) -> Result<PageGuard, Error>;
    fn flush_dirty_pages(&self) -> Result<(), Error>;
    fn evict_all(&self) -> Result<(), Error>;
}

struct PageGuard<'a> { /* holds pin count; Drop unpins */ }
impl PageGuard<'_> {
    fn read(&self) -> &[u8];
    fn write(&mut self) -> &mut [u8];  // marks page dirty
}
```

### 8.4 Use typed page headers with checksums everywhere

**Evidence:** SQLite's page checksum (in WAL mode), LMDB's page checksum, and redb's page checksum all detect torn writes and corruption. The cost is negligible (one CRC32 computation per page read/write).

**For this project:**
```rust
// 24-byte page header, present on every page
struct PageHeader {
    page_id: u64,      // Self-referential ID (corruption detection)
    page_type: u8,     // PageType enum (B-tree interior, leaf, overflow, slot, free, etc.)
    flags: u8,         // Page-type-specific flags
    _pad: u16,         // Alignment
    txn_id: u64,       // Transaction ID that last wrote this page
    checksum: u32,     // CRC32C of the entire page (header fields except checksum + payload)
}
```

Computed on every write; verified on every read (first access after page is loaded into buffer pool). Invalid checksum → `StorageError::PageCorrupt`.

### 8.5 Design the VFS / HAL abstraction based on the SQLite VFS precedent

**Evidence:** SQLite's VFS has been the gold standard for portable storage I/O abstraction for 20 years. It successfully abstracts over Windows, Linux, macOS, iOS, Android, and custom embedded platforms. redb's move from mmap to explicit `pread`/`pwrite` shows this is feasible in pure Rust.

**Minimum HAL trait surface for this project:**
```rust
// The minimum required to implement the full storage engine
trait StorageBackend {
    type Error: core::fmt::Debug;
    
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Self::Error>;
    fn write_at(&self, offset: u64, buf: &[u8]) -> Result<(), Self::Error>;
    fn flush(&self) -> Result<(), Self::Error>;   // maps to fsync
    fn len(&self) -> Result<u64, Self::Error>;    // current file size
    fn set_len(&self, size: u64) -> Result<(), Self::Error>; // extend/truncate
}
```

All five methods are implementable on any platform with any I/O mechanism:
- In-memory: buffer + Vec
- std fs: `File::read_at`, `File::write_at` (or pread/pwrite), `File::sync_all`, `File::metadata().len()`, `File::set_len()`
- Embedded flash: flash driver `read`, `write`, `erase` (with adaptation)
- Testing: `Vec<u8>` in-memory mock

This will be the primary input to Task 9 (HAL Trait Design), which formalizes and extends this sketch.

### 8.6 Use the freelist B-tree with MVCC-aware reclamation

**Evidence:** Both LMDB and redb track freed pages per transaction ID. Pages freed in transaction T are only reclaimed when no reader started at or before transaction T is still active.

**For this project, the freelist B-tree maps:**
```
Key:   (freeing_txn_id: u64, page_id: u64)   — composite key, sorted
Value: ()                                     — existence is the data
```

The writer, on commit, adds entries for all pages freed in that transaction. The garbage collector (run during write transactions, lazily) scans the freelist for entries with `freeing_txn_id ≤ oldest_active_reader_txn_id` and reclaims those pages.

**Reader tracking:**
```rust
struct ReaderTracker {
    active: Mutex<BTreeMap<u64 /* txn_id */, usize /* refcount */>>,
}
impl ReaderTracker {
    fn oldest_active(&self) -> Option<u64> {
        self.active.lock().keys().next().copied()
    }
}
```

### 8.7 Page size: 4096 bytes is the right default

**Evidence:** SQLite uses 4096 bytes as its default. redb uses 4096 bytes. LMDB uses 4096 bytes (matching the OS page size). DuckDB uses 256KB (anomalous — driven by columnar analytics, not row-based access).

**For this project:**
- 4096 bytes default, matching the OS page size and CPU cache line arithmetic
- Allow the page size to be configured at database creation time (4096, 8192, 16384 are reasonable choices)
- Store the page size in the superblock (first two copies); never change it after creation
- Why allow configuration? Future users with large property blobs might prefer 8192-byte pages to reduce overflow chain length

### 8.8 Startup must be fast and not require log replay

**Evidence:** SQLite WAL mode, LMDB, and redb all achieve sub-millisecond startup by eliminating recovery log replay. DuckDB's WAL-based design requires checkpoint replay on startup, but this is fast for small WALs.

**For this project:** The CoW + dual-superblock model eliminates log replay by design. Startup is:
1. Open file
2. Read superblocks (2 pages)
3. Validate checksums, select active superblock
4. Populate buffer pool header metadata
5. First transaction starts immediately

Target startup time: < 1ms for any database size. This should be a hard requirement documented in the public API.

### 8.9 Do not implement multi-process access in the initial version

**Evidence:** LMDB supports multi-process access via `mmap` + shared memory mutex. redb explicitly does not support multi-process access. SQLite supports it via file locking, which is fragile on network filesystems.

**For this project:** The target use case is a single process (typical for an embedded database). Multi-process access requires:
- File locking (fragile on some filesystems)
- Cross-process reader table (shared memory or file-based)
- Coordination protocol for WAL/superblock updates

This adds significant complexity for a use case that is not required by the project goals. Single-process access is the only initially supported mode. A `file_already_open` error should be returned if the same file is opened twice (detectable via a file lock taken at open time). Multi-process support is a future extension, gated behind a feature flag.

### 8.10 The in-memory backend shares code paths, not special-cased branches

**Evidence:** SQLite's in-memory database uses the same Pager, B-tree, and VDBE code paths as an on-disk database. The VFS layer provides the abstraction. redb could be extended to an in-memory backend similarly (replace the file backend with a `Vec<u8>`).

**For this project:** The in-memory backend should be a `StorageBackend` implementation backed by `Vec<u8>` (or `BTreeMap<PageId, Box<[u8]>>` for sparse backing). The entire storage engine code above the HAL runs identically. The in-memory backend:
- Satisfies `read_at` from memory
- Satisfies `write_at` to memory
- Satisfies `flush` as a no-op
- Optionally: `snapshot_to_disk(path)` writes the Vec to a file for persistence; `load_from_disk(path)` reads it back

This is Task 19's domain, but the architectural implication is: **do not write any code above the HAL that special-cases `InMemory` vs. `OnDisk`**. The HAL abstraction must be complete.

---

## 9. Cross-Reference: Connecting to Tasks 1, 2, and 3

This section briefly maps the findings from this document to the prior research, to give Task 7 and Task 8 a unified view.

### 9.1 From Task 1 (DB Internals Fundamentals)

Task 1 tentatively recommended CoW B-trees. This document confirms that recommendation:
- LMDB and redb use CoW B-trees successfully in production
- The dual-superblock atomic commit pattern is proven correct
- The read-your-writes and MVCC properties follow naturally

Task 1 noted that fsync discipline is critical. The commit sequence in Section 8.2 of this document makes the fsync ordering explicit:
```
write data pages → fsync → write new superblock → fsync → swap active indicator → fsync
```
Each fsync in this sequence has a specific invariant it protects.

### 9.2 From Task 2 (Graph Storage Strategies)

Task 2 recommended a hybrid: fixed-size slot stores for node/edge records + CoW B-tree secondary indexes. This document adds:

- **Slot stores and CoW:** The slot stores (node record array, edge record array) can be managed as CoW pages like any other page. A slot page, when modified, is copied to a new location; the free-space B-tree tracks the new mapping. The superblock's `node_store_head` and `edge_store_head` pointers are updated at commit.

- **However:** Task 2 noted the open question of WAL for slot stores vs. CoW for slot stores. This document confirms: CoW for slot stores is the right answer because it keeps the design uniform (no WAL at all in the engine) and simplifies crash recovery.

- **Multi-record atomicity:** An edge insertion modifies 3 records (source node, target node, edge record). In CoW terms, this means 3 slot pages are dirtied in one transaction and written as new pages on commit. This is manageable and natural — transactions already batch writes.

### 9.3 From Task 3 (Ontology Models Survey)

Task 3 identified a schema/type registry and a property key registry as foundation requirements (B1, B5). This document confirms:
- The schema B-tree root pointer lives in the superblock, updated on each commit that changes the schema
- The property key registry (small, frequently accessed) should be fully loaded into memory at startup and cached there — it fits in a few pages
- Schema changes are full transactions (commit the schema B-tree update along with any concurrent data changes, or in a dedicated schema-change transaction)

Task 3 required multiple type labels per node (A1). Task 2 suggested a secondary type index B-tree (`(type_id, node_id) → exists`). This document confirms that B-tree root pointer belongs in the superblock (`type_index_root`), updated on every commit that adds/removes a type label from a node.

---

## Completion Report: Task 4 — Embedded Database Architectures

### Status: COMPLETE

### Done Criterion:
The criterion requires: (1) an architectural summary of each system studied, (2) a focused deep-dive on file format design patterns, (3) a section on concurrency models, (4) lessons applicable to this project.

Systems studied: SQLite (Section 2), LMDB (Section 3), redb (Section 4), DuckDB (Section 5) — four systems, exceeding the minimum ✓

File format design patterns deep-dive: Section 6, covering magic numbers/versioning, dual-superblock pattern, page header conventions, free-space management strategies, overflow pages, extensibility provisions, and file sizing — comprehensive ✓

Concurrency models: Section 7, covering all four systems' models, a comparison table, and analysis of the single-writer + MVCC readers convergence ✓

Lessons applicable to this project: Section 8, 10 lessons directly actionable for Tasks 7, 8, 9, and 16 ✓

### Deliverables:
- `004-embedded-db-architectures.md` — this document

### Summary:
Studied four embedded databases (SQLite, LMDB, redb, DuckDB) in architectural depth. Produced a comprehensive file format design patterns section that can serve as a direct input to Task 8 (Single-File Format). Confirmed the CoW + dual-superblock crash safety model as the correct choice, with LMDB and redb as direct precedents. Identified redb as the most directly applicable reference implementation for this project (pure Rust, no mmap, similar structural goals). Established 10 concrete design lessons.

The central finding: **SQLite, LMDB, and redb all converge on single-writer + MVCC readers as the correct concurrency model for embedded databases**. The differences are in how they achieve it (file locking vs. process mutex vs. in-process Mutex) and what crash-safety mechanism they use (WAL/journal vs. CoW + dual-superblock). For this project, the redb model (in-process Mutex + CoW + dual-superblock, no mmap, explicit pread/pwrite) is the correct architectural foundation.

### Context for Next Task:
This document is a required dependency for **Task 8 (Single-File Format Spec)**. Task 8 will also depend on Task 1 (`001-db-internals-fundamentals.md`) and Task 7 (`007-graph-storage-model.md`).

For Task 8, the most important sections of this document are:
- Section 6 (File Format Design Patterns Deep-Dive) — the primary design input
- Section 8.2 (Superblock design sketch) — a starting point for the actual spec
- Section 8.3 (Buffer pool / HAL separation) — affects the page I/O interface spec
- Section 8.4 (Page headers with checksums) — the page header spec

For **Task 9 (HAL Trait Design)**, this document provides:
- Section 8.5 (HAL trait sketch, directly inspired by SQLite VFS)
- Section 2.6 (SQLite VFS detailed description)
- Section 8.10 (in-memory backend shares code paths) — the key architectural principle

For **Task 7 (Graph Storage Model)**:
- Section 7.6 (single-writer + MVCC readers model recommendation)
- Section 8.6 (freelist B-tree with MVCC-aware reclamation)
- Section 9.2 (integration of CoW approach with Task 2's slot store recommendation)

### Residual Concerns:
1. **The superblock sketch in Section 8.2** is a design starting point, not a final spec. Task 8 must refine field sizes, alignment, and the exact set of root pointers based on the final storage model from Task 7. In particular, the number of B-tree root pointers depends on how many B-trees the design uses (schema, freelist, type index, edge source index, edge target index, and potentially more for property indexes).

2. **Page header size (24 bytes proposed)** consumes 24/4096 ≈ 0.6% of each page. This is acceptable but Task 8 should verify it against the actual page content needs (how many B-tree cells fit in the remaining 4072 bytes?).

3. **Checksum algorithm (`no_std` compatibility):** The xxHash3 recommendation is conditional on having a `no_std`-compatible crate available. This should be verified during Task 9/15 implementation. CRC32C via the `crc32fast` crate is the fallback.

4. **File locking for single-process exclusivity:** The recommendation in Section 8.9 to take an exclusive file lock at open time (to prevent two processes opening the same file) requires a file-locking HAL method. The HAL sketch in Section 8.5 does not include this — Task 9 should add it.

5. **redb source code version pinning:** The redb crate is actively developed and its internal architecture may evolve. Task 8 and 9 implementers should pin to a specific redb version when studying the source code, and note which version was studied.

### Upstream Flags:
None. All findings are scoped to Tasks 7, 8, 9, and 16. The tentative recommendations from Task 1 (CoW B-tree) and Task 2 (hybrid index-free adjacency + CoW B-tree indexes) are confirmed by this research, not contradicted.
