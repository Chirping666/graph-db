# 001 — Database Internals Fundamentals

**Project:** Embedded Graph Database with Extensible Schema & Pluggable Inference
**Task:** 1 — Research: Database Internals Fundamentals
**Status:** Complete
**Intended audience:** The designer of this project and all downstream Claude instances. A reader with programming experience but no prior database internals knowledge should be able to read this document and explain every concept back in their own words.

---

## Table of Contents

1. [Why Storage Engines Exist](#1-why-storage-engines-exist)
2. [Pages: The Unit of Disk I/O](#2-pages-the-unit-of-disk-io)
3. [B-Trees: The Workhorse Index Structure](#3-b-trees-the-workhorse-index-structure)
4. [LSM-Trees: Write-Optimized Alternatives](#4-lsm-trees-write-optimized-alternatives)
5. [The Buffer Pool: Bridging RAM and Disk](#5-the-buffer-pool-bridging-ram-and-disk)
6. [Write-Ahead Logging (WAL): Crash Safety](#6-write-ahead-logging-wal-crash-safety)
7. [Transactions and ACID](#7-transactions-and-acid)
8. [Concurrency Control: Locking](#8-concurrency-control-locking)
9. [MVCC: Multi-Version Concurrency Control](#9-mvcc-multi-version-concurrency-control)
10. [Isolation Levels](#10-isolation-levels)
11. [Crash Recovery](#11-crash-recovery)
12. [Putting It Together: Request Lifecycle](#12-putting-it-together-request-lifecycle)
13. [Summary: Relevance Matrix for This Project](#13-summary-relevance-matrix-for-this-project)

---

## 1. Why Storage Engines Exist

A storage engine is the component of a database responsible for **how data is physically organized, stored, and retrieved**. It sits below the query layer (SQL parser, query planner) and above the raw operating system file I/O.

The core problem a storage engine solves: **disk is slow and sequential; queries want fast, random access**. A hard disk spinning at 7200 RPM takes ~5 ms to seek to a random location. An NVMe SSD is faster (~100 µs), but still orders of magnitude slower than RAM (~100 ns). A storage engine's job is to organize data on disk so that common access patterns are efficient, to keep hot data in RAM, and to ensure that a crash at any moment does not corrupt the database.

Every design decision in a storage engine is a tradeoff between:
- **Read performance** vs. **write performance**
- **Space efficiency** vs. **access speed**
- **Simplicity** vs. **crash safety**
- **Throughput** vs. **latency**

### Relevance to this project

This project builds a storage engine from scratch. Understanding *why* engines make the choices they do is prerequisite to making those choices deliberately. Every subsequent section in this document maps to a concrete design decision in tasks 7, 8, and 9.

---

## 2. Pages: The Unit of Disk I/O

### What a page is

The operating system reads and writes files in blocks (typically 4 KB). Databases define their own logical unit — the **page** — which is typically a multiple of the OS block size (4 KB, 8 KB, or 16 KB are common choices).

All I/O happens in whole pages. If you want to read a single 8-byte integer from a 4 KB page, you still read the entire 4 KB page into memory. This is not waste — it's a feature. Locality means that the data near the thing you want is probably also the data you'll need next, so reading a whole page at once amortizes the seek cost.

### Page anatomy

A typical page has three regions:

```
┌──────────────────────────────────────────────────────┐
│  Page Header (fixed size, ~20–64 bytes)              │
│  - Page ID (unique identifier within the file)       │
│  - Page type (e.g., leaf, interior, overflow, free)  │
│  - Flags (dirty, pinned, etc.)                       │
│  - Checksum (detects corruption on read)             │
│  - LSN (Log Sequence Number, for WAL integration)    │
├──────────────────────────────────────────────────────┤
│  Cell / Slot Array (grows from left)                 │
│  - Offsets pointing to cells in the payload area     │
├──────────────────────────────────────────────────────┤
│  Free Space (middle area, shrinks from both ends)    │
├──────────────────────────────────────────────────────┤
│  Cell Payload (grows from right)                     │
│  - Actual data records stored in this page           │
└──────────────────────────────────────────────────────┘
```

The **slot array / cell directory** pattern (also called "slotted page") decouples the logical order of records (slot index) from their physical position. This allows records to be rearranged or compacted within a page without changing their slot numbers, so external references (pointers from parent pages) remain valid.

### Free-space management

When a page is partially full, a database needs to know which pages have room for new records. Approaches:

- **Free Space Map (FSM):** A separate structure (often its own page or section) that tracks roughly how much free space each page has, often in coarse buckets (0–25%, 25–50%, etc.). PostgreSQL uses this approach.
- **Linked free list:** Free pages are chained together with a pointer in each free page's header pointing to the next. Simple but can fragment over time.
- **Bitmap:** One bit (or a few bits) per page indicating free/full. Fast to scan for the next free page.

### Page IDs and addressing

Each page is identified by a **page ID** (a monotonically assigned integer). The byte offset of page N in the file is simply `N × page_size`. This makes random access O(1): to read page 42, seek to offset `42 × 4096` and read 4096 bytes.

### Overflow pages

When a single record (a node with many properties, for example) is larger than a page, the record is split across **overflow pages**. The primary page stores the beginning of the record plus a pointer to the first overflow page. Overflow pages form a chain. This is rare in practice but must be handled correctly.

### Relevance to this project

Pages are the foundation of everything else. The file format spec (task 8) will define page size, page header layout, and free-space management strategy. The storage engine (task 16) will implement page allocation, reading, and writing. Every other concept in this document assumes pages exist.

**Design questions to decide in task 8:**
- Page size (4 KB is a safe default; 8 KB if records tend to be larger)
- Whether to use a slotted page layout or a fixed-record layout
- How to represent free space (FSM, linked list, or bitmap)
- Checksum algorithm (CRC32 is standard; xxHash is faster)

---

## 3. B-Trees: The Workhorse Index Structure

### The problem B-trees solve

Given a disk organized into pages, how do you efficiently find a record by key? A flat sorted array lets you binary search, but updating it requires rewriting large portions of the file. A hash table gives O(1) lookup but doesn't support range queries. B-trees solve both problems: O(log N) lookup, O(log N) insert/delete, and efficient range scans — all while keeping pages well-utilized.

### How B-trees work

A B-tree is a **balanced, ordered, multi-way tree** where each node corresponds to one disk page. The invariants are:

1. Every leaf node is at the same depth (the tree is perfectly height-balanced).
2. Each node stores between ⌈M/2⌉ and M keys (M is the branching factor, chosen so a node fills one page).
3. Interior nodes store keys as separators and child page pointers; leaf nodes store the actual data (or pointers to data).
4. Keys within a node are stored in sorted order.

```
                   [30 | 60]               ← Interior node (page 1)
                  /     |     \
          [10|20]   [35|45|55]  [70|80]    ← Interior nodes
          /  |  \   / | | | \   / |  \
        [.] [.] [.][.][.][.][.][.][.] [.] ← Leaf nodes (actual data)
```

**Lookup:** Start at the root. At each interior node, binary search among the separator keys to choose the correct child pointer. Follow the pointer to the child page. Repeat until a leaf is reached. Disk reads = tree height ≈ log_M(N). For M=200 and N=1 million records, height ≈ 3. Three disk reads to find any record.

**Insert:** Find the correct leaf (as above). If the leaf has room, insert the key in sorted order. If the leaf is full, **split** it: allocate a new page, move half the keys to it, and push the median key up to the parent. The parent may also need to split (recursive).

**Delete:** Find and remove the key. If the node falls below the minimum fill threshold, either borrow a key from a sibling node or **merge** with a sibling (the inverse of a split).

### B+ trees (the common variant)

Most databases use **B+ trees**, which differ from B-trees in one important way: **only leaf nodes store data**; interior nodes store only keys (separator keys) used for routing. Leaf nodes are linked in a doubly-linked list, enabling efficient range scans without traversing the tree.

```
Interior nodes: keys only (routing)
Leaf nodes: keys + data, linked left-to-right
```

This doubles the branching factor for interior nodes (they hold more keys per page since they don't hold data), reducing tree height and thus disk reads.

### Copy-on-write B-trees (append-only)

Some databases (including LMDB, and redb) use a **copy-on-write (CoW)** variant. Instead of modifying pages in place, updates write a new version of the modified page to a new location. The root page pointer is then atomically updated to point to the new root. Old pages become garbage, collected later.

CoW B-trees have important properties:
- **No WAL required for crash safety.** The database is always in a consistent state — the old root is valid until the new root is atomically committed.
- **Readers never block writers** (and vice versa), because readers hold a reference to an old root and see a consistent snapshot.
- **Increased write amplification** — a single key insert causes O(log N) page writes (the modified leaf plus every ancestor up to the root).

### Relevance to this project

B-trees (or B+ trees) are the natural choice for the primary index structures — for example, an index mapping node IDs to their page locations, or an index on edge source/target IDs. The choice between a standard B+ tree (with WAL for crash safety) and a CoW B-tree (crash-safe by design) is a pivotal decision for task 7 and 8.

**Key tradeoff:**
- Standard B+ tree + WAL: lower write amplification, more implementation complexity (WAL + recovery logic)
- CoW B-tree: simpler crash safety, higher write amplification per operation but potentially simpler overall

Given that redb (a well-regarded Rust embedded database) uses CoW B-trees successfully, this approach merits serious consideration for task 7.

---

## 4. LSM-Trees: Write-Optimized Alternatives

### The problem LSM-trees solve

B-trees are read-optimized: a lookup touches O(log N) pages. But writes are also O(log N) page writes, and each write requires a random disk seek — expensive on spinning disks and still costly on SSDs (due to write amplification and garbage collection pressure).

**LSM-trees (Log-Structured Merge-trees)** sacrifice some read performance for dramatically better write throughput. They are used in RocksDB, LevelDB, Cassandra, and HBase.

### How LSM-trees work

The core idea: **batch all writes into a sorted, sequential, append-only structure**. Random writes become sequential writes, which are much faster.

**Components:**

1. **MemTable (in-memory):** All writes go into a sorted in-memory structure (typically a red-black tree or skiplist). The MemTable absorbs writes at RAM speed.

2. **WAL (Write-Ahead Log):** Every write is also appended to a WAL on disk (sequentially, which is fast) before being acknowledged. This ensures durability even if the process crashes before the MemTable is flushed.

3. **SSTables (Sorted String Tables):** When the MemTable reaches a size threshold, it is flushed to disk as an immutable SSTable — a file of key-value pairs in sorted order. Flushing is a sequential write, which is fast.

4. **Compaction:** Over time, multiple SSTables accumulate. A background process merges and compacts them, eliminating deleted/overwritten entries and maintaining sorted order. Compaction reads and rewrites SSTables — again, sequentially.

**Read path:** Check the MemTable first. If not found, check SSTables from newest to oldest. Bloom filters (probabilistic data structures) are attached to each SSTable to quickly skip SSTables that definitely don't contain the key.

```
Write → MemTable + WAL
        ↓ (when full)
       SSTable L0
        ↓ (compaction)
       SSTable L1, L2, ...  (larger, fewer, fully sorted)
```

### LSM-tree vs. B-tree comparison

| Aspect                  | B-tree                        | LSM-tree                        |
|-------------------------|-------------------------------|----------------------------------|
| Write performance       | O(log N) random writes        | Primarily sequential writes      |
| Read performance        | O(log N) disk reads           | O(levels × log N), Bloom filters |
| Space amplification     | Low (~30%)                    | Higher (during compaction)       |
| Write amplification     | Moderate                      | High (compaction rewrites data)  |
| Range scans             | Excellent (linked leaf nodes) | Moderate (merge across SSTables) |
| Implementation complexity | Moderate                    | High (compaction, Bloom filters) |

### Relevance to this project

LSM-trees are **not the recommended choice** for this project, for these reasons:

1. **Read-heavy workloads are typical for graph databases.** Graph traversals read far more than they write. B-trees are better suited.
2. **Embedded single-file constraint.** LSM-trees work naturally with multiple files (one per SSTable). Fitting them into a single file is awkward.
3. **Implementation complexity.** Compaction adds significant ongoing complexity and background I/O that complicates the concurrency model.
4. **Crash recovery.** CoW B-trees offer a simpler crash-safety story than LSM-trees in an embedded context.

**However, understanding LSM-trees is important because:**
- Some embedded databases (like RocksDB as a WAL-adjacent structure) mix approaches
- The concept of a **MemTable + flush** (buffering writes in memory and flushing as a batch) is applicable to any storage engine, including B-tree engines
- The WAL component of LSM-trees is identical in concept to the WAL used with B-trees (see section 6)

---

## 5. The Buffer Pool: Bridging RAM and Disk

### What a buffer pool is

The buffer pool (also called the page cache or buffer cache) is a region of RAM that the database manages itself, distinct from the OS page cache. It holds **recently accessed or modified pages** in memory to avoid redundant disk reads.

Think of it as a managed cache with the following properties:
- Fixed size (configured at startup, e.g., 64 MB)
- Organized as a hash map from (page_id → in-memory page frame)
- Each frame tracks: the page's content, whether it's **dirty** (modified but not yet written to disk), and a **pin count** (how many active operations are using it)

### How it works

**Page lookup:**
1. Check if the page is in the buffer pool (hash map lookup).
2. If yes (**hit**): return a reference, increment pin count.
3. If no (**miss**): read the page from disk, load it into a free frame, add to hash map, return reference.

**Eviction (when the buffer pool is full):**
When a miss occurs and there are no free frames, an existing page must be evicted:
1. Choose an unpinned frame (pinned pages cannot be evicted — they're in active use).
2. If the frame is dirty, write it to disk first (**flush**).
3. Remove from hash map, load the new page into the frame.

**Replacement policy:** Which page to evict? Common choices:
- **LRU (Least Recently Used):** Evict the page that was accessed least recently. Good general-purpose policy.
- **Clock (approximation of LRU):** A circular buffer with a "recently used" bit. Cheaper than true LRU.
- **LRU-K:** Evict based on the K-th most recent access. Better for sequential scan resistance (prevents a large sequential scan from evicting all hot pages).
- **FIFO:** Simple but poor performance — ignores access frequency.

### Dirty page tracking and write-back

When a page is modified, it is marked **dirty**. Dirty pages must be written to disk before they can be evicted or before a transaction commits (depending on the durability model). The process of writing dirty pages to disk is called **flushing** or **write-back**.

**Relationship with WAL:** In a WAL-based system, a dirty page must not be written to disk until the WAL record for that modification has been durably written (the **WAL-before-data** rule, also called "write-ahead logging protocol"). This ensures that if the database crashes after the page is written but before the WAL record, recovery can still undo the change.

### Pinning

A page that is currently being read or written by an active operation is **pinned** — it cannot be evicted. This is implemented via a reference count (pin count). The buffer pool guarantees that a pinned page's memory address remains stable.

### The OS page cache interaction

The OS also maintains its own page cache for file I/O. Most databases bypass the OS cache by opening files with `O_DIRECT` (Linux) or `FILE_FLAG_NO_BUFFERING` (Windows), ensuring that I/O goes directly between the buffer pool and disk, avoiding double-caching. However, this requires I/O to be aligned to the disk sector size (typically 512 bytes or 4096 bytes).

### Relevance to this project

The buffer pool is the **performance heart** of the storage engine. Its design directly affects:
- How many concurrent readers/writers are supported (frame pinning and eviction policy)
- Durability (dirty page flush ordering relative to WAL)
- Memory footprint (fixed pool size must be configurable)

For this project, a reasonable starting implementation is an LRU or Clock-based buffer pool backed by a `HashMap<PageId, FrameIndex>` and a fixed `Vec<PageFrame>`.

**Design questions for task 7:**
- What is the minimum viable buffer pool for the initial implementation?
- How does the buffer pool interact with the concurrency control layer (pinned pages during a transaction)?
- How is dirty-page tracking integrated with WAL flushing?

---

## 6. Write-Ahead Logging (WAL): Crash Safety

### The problem WAL solves

In-place update databases (standard B-tree engines) modify pages directly. If the process crashes while writing a page (e.g., midway through a 4 KB write), the page on disk contains a mixture of old and new data — a **torn write** — and the database is corrupt.

**Write-Ahead Logging** solves this by recording every change to a separate, append-only log file *before* applying that change to the data pages. "Write-ahead" means: the log record is durably on disk *before* the page is modified.

If a crash occurs:
- If the log record is missing (crash before log write): the change never happened; the page is still in its old state — consistent.
- If the log record is present but the page was not updated (crash after log write, before page write): recovery replays the log record to apply the change — consistent.
- If both are present: already consistent.

### WAL structure

A WAL is a sequential append-only file (or circular buffer). Each **log record** contains:
- **LSN (Log Sequence Number):** A monotonically increasing integer identifying this record's position in the log.
- **Transaction ID:** Which transaction produced this change.
- **Record type:** `BEGIN`, `COMMIT`, `ABORT`, `UPDATE`, `INSERT`, `DELETE`.
- **Page ID and offset:** Which page and which bytes were changed.
- **Before-image (UNDO data):** The old value of the bytes (used for rolling back aborted transactions).
- **After-image (REDO data):** The new value of the bytes (used for replaying during recovery).

A **checkpoint** is a special log record that indicates: "at this point, all dirty pages from before this LSN have been flushed to disk." During recovery, the database can start replaying from the most recent checkpoint rather than from the beginning of the log.

### The WAL protocol (ARIES simplified)

The canonical WAL protocol (ARIES — Algorithm for Recovery and Isolation Exploiting Semantics):

**During normal operation:**
1. Begin transaction: write `BEGIN` log record.
2. Before modifying a page: write the `UPDATE` log record to WAL, including before-image and after-image. Flush WAL to disk.
3. Apply the modification to the page in the buffer pool (in memory). The page is now dirty.
4. To commit: write `COMMIT` log record to WAL and flush WAL to disk. Only after this is the transaction considered durable.
5. Dirty pages can be flushed to disk lazily (after the WAL record is written).

**During recovery (after a crash):**
1. **Analysis pass:** Scan the WAL from the last checkpoint to identify which transactions were in progress at the time of the crash.
2. **REDO pass:** Replay all log records (including committed and aborted transactions) to bring the data pages to the state they were in at the crash.
3. **UNDO pass:** Roll back all transactions that were not committed at the time of the crash by applying their before-images in reverse order.

### WAL and durability

A WAL record is **durable** when it has been `fsync`-ed to disk (the OS has confirmed the data has reached stable storage, not just the OS buffer). This is the expensive operation — `fsync` can take 1–10 ms on spinning disks and 10–100 µs on SSDs. Group commit (batching multiple transactions' WAL flushes into a single `fsync`) significantly improves throughput.

### WAL in CoW B-tree designs

CoW B-trees (like those used in LMDB and redb) typically **do not use a WAL**. Instead:
- Writes produce new page versions in new locations.
- The **root page pointer** (in the file header) is updated atomically at commit time.
- A crash leaves either the old root pointer (old consistent state) or the new root pointer (new consistent state) — never a partial state.
- Old pages are reclaimed by a garbage collector.

This simplifies crash recovery significantly at the cost of write amplification (each commit rewrites O(log N) pages).

### Relevance to this project

WAL is required if the project uses a standard (in-place update) B-tree. If a CoW B-tree is chosen, WAL may be unnecessary. This is a pivotal decision for tasks 7 and 8.

**Regardless of the B-tree variant chosen:**
- `fsync` discipline must be correct.
- Torn write protection is needed (either via CoW or WAL).
- The checkpoint/recovery mechanism determines startup time after a crash.

---

## 7. Transactions and ACID

### What ACID means

Transactions provide a way to group multiple operations into an all-or-nothing unit with four guarantees:

**Atomicity:** All operations in a transaction succeed, or none of them do. If any step fails (including a crash), the database is rolled back to its state before the transaction began.

**Consistency:** A transaction takes the database from one valid state to another valid state. Consistency is largely enforced by the application and constraints — the database provides the mechanism (atomicity + isolation), the application defines what "valid" means.

**Isolation:** Concurrent transactions appear to execute serially. One transaction cannot see the partial effects of another in-progress transaction (to varying degrees, depending on the isolation level).

**Durability:** Once a transaction is committed, its effects survive crashes. This is what WAL or CoW provides.

### Transaction lifecycle

```
BEGIN
  → operations (read, write, modify)
  → if all succeed: COMMIT (durable, visible to others)
  → if any fail:    ABORT/ROLLBACK (all changes undone)
```

### Read-only vs. read-write transactions

Most databases distinguish:
- **Read-only transactions:** No writes; can be implemented as a snapshot read with no locking (in MVCC systems). Very cheap.
- **Read-write transactions:** Acquire locks (or create new versions in MVCC); must be committed with WAL flush.

### Relevance to this project

Every operation on the database should occur within a transaction — even single-operation reads. The public API (task 10) will need to expose transaction handles. The concurrency model (section 8 and 9) determines how transactions interact with each other.

---

## 8. Concurrency Control: Locking

### The problem of concurrent access

When multiple threads (or processes) access the database simultaneously, their operations can interleave in ways that produce incorrect results. Classic problems:

- **Dirty read:** Thread A reads data written by Thread B before B commits. If B aborts, A has read phantom data.
- **Non-repeatable read:** Thread A reads a value. Thread B modifies and commits it. Thread A reads the same value again and gets a different result.
- **Phantom read:** Thread A queries a range. Thread B inserts a new row in that range and commits. Thread A re-queries and sees a different set of rows.
- **Lost update:** Threads A and B both read a value, compute a new value, and write it back. One thread's update overwrites the other's.
- **Write-write conflict:** Two transactions try to modify the same record — one must wait or be aborted.

### Two-Phase Locking (2PL)

The classical solution: **two-phase locking** (2PL). Every transaction must acquire a lock before accessing data and cannot acquire new locks after releasing any lock.

**Phases:**
1. **Growing phase:** Acquire locks as needed; never release.
2. **Shrinking phase:** Release locks; never acquire new ones.

**Lock types:**
- **Shared lock (S-lock / read lock):** Multiple transactions can hold S-locks on the same resource simultaneously.
- **Exclusive lock (X-lock / write lock):** Only one transaction can hold an X-lock; no other transaction can hold any lock on the same resource.

**Compatibility matrix:**

| Held \ Requested | S-lock | X-lock |
|------------------|--------|--------|
| S-lock           | ✓ OK   | ✗ Wait |
| X-lock           | ✗ Wait | ✗ Wait |

**Strict 2PL (S2PL):** All locks are held until the transaction commits or aborts. This is the most common variant and ensures serializability.

### Lock granularity

Locks can be held at different granularities:
- **Database-level:** One lock for the entire database. Simple but completely serializes all transactions. Acceptable for low-concurrency embedded use cases.
- **Table/relation-level:** One lock per table. Better granularity; allows concurrent access to different tables.
- **Row-level (record-level):** One lock per row. Best concurrency but high overhead (many locks to track).
- **Page-level:** One lock per page. A middle ground.
- **Predicate locks:** Lock a range or predicate (e.g., "all nodes with type = Person"). Prevents phantom reads.

**Intention locks** are a hierarchy mechanism: before acquiring a row-level lock, a transaction acquires an intention lock on the containing table, signaling to others that it holds row-level locks below.

### Deadlock

When Thread A holds lock X and waits for lock Y, while Thread B holds lock Y and waits for lock X, neither can proceed — a **deadlock**. Solutions:

- **Deadlock detection:** Periodically (or on timeout) build a "waits-for" graph; if it has a cycle, abort one of the transactions.
- **Deadlock prevention:** Impose a lock acquisition ordering (e.g., always acquire locks in increasing ID order) so cycles cannot form. Simple but requires knowing all needed locks upfront.
- **Timeout:** If a transaction waits more than N milliseconds, abort it. Simple and practical for embedded databases.

### Relevance to this project

For an embedded database accessed from multiple threads within the same process, a **coarse-grained locking strategy** is often appropriate initially:
- One or a few reader/writer locks (`RwLock`) covering the major data structures.
- Readers can proceed concurrently; a writer gets exclusive access.

This is simpler to implement correctly than fine-grained row-level locking, but limits write concurrency. MVCC (section 9) is a better long-term approach for this project.

---

## 9. MVCC: Multi-Version Concurrency Control

### The core idea

**Multi-Version Concurrency Control (MVCC)** is an alternative to locking that maintains **multiple versions of each data record** so that:
- **Readers never block writers**
- **Writers never block readers**
- Each transaction sees a consistent snapshot of the database as it existed at a specific point in time

MVCC is used by PostgreSQL, MySQL (InnoDB), SQLite (WAL mode), and most modern databases.

### How MVCC works

When a transaction modifies a record, it does not overwrite the old value. Instead, it creates a **new version** with a timestamp (or transaction ID) indicating when this version became visible. The old version is retained.

Each version has:
- **xmin:** The transaction ID that created this version (it becomes visible to transactions that started after xmin committed).
- **xmax:** The transaction ID that deleted/replaced this version (it becomes invisible to transactions that started after xmax committed).

When a transaction reads a record, it applies a **visibility rule**: show me the version where `xmin ≤ my_snapshot_timestamp < xmax`.

```
Record "Alice" versions:
  Version 1: name="Alice", salary=50000  [xmin=10, xmax=25]
  Version 2: name="Alice", salary=60000  [xmin=25, xmax=∞]

Transaction with snapshot timestamp 20:
  → sees Version 1 (10 ≤ 20 < 25)

Transaction with snapshot timestamp 30:
  → sees Version 2 (25 ≤ 30 < ∞)
```

### Snapshot Isolation

MVCC naturally provides **Snapshot Isolation (SI)**: each transaction reads from a consistent snapshot taken at its start time. No dirty reads. No non-repeatable reads. No phantoms (in most implementations).

Snapshot isolation is not full serializability: the "write skew anomaly" can still occur. Serializable Snapshot Isolation (SSI) adds conflict detection to catch write skew.

### MVCC and garbage collection

Old versions accumulate and must eventually be reclaimed. The process of identifying and removing versions that no long transaction can see is called **VACUUM** (PostgreSQL) or **version cleanup**. This is a background process and represents ongoing maintenance work.

### MVCC in CoW B-trees

CoW B-trees implement MVCC implicitly: each committed write produces a new tree root, and old tree roots represent snapshots. A read-only transaction holds a reference to a tree root (a snapshot) and sees a consistent state. Once no transaction holds a reference to an old root, the pages it points to can be reclaimed.

This is the MVCC model used by LMDB and redb — very clean, no separate version chains, garbage collection is straightforward (reclaim pages not reachable from any live root).

### Relevance to this project

MVCC is strongly recommended for this project because:
1. Graph traversal is inherently read-heavy — readers should not be blocked by writers.
2. A CoW B-tree (which the project may choose) provides MVCC essentially for free.
3. MVCC simplifies the API: read-only transactions are cheap snapshots.

**Key design question for task 7:** If using a CoW B-tree, the MVCC story is automatic. If using an in-place B-tree, MVCC requires explicit version tracking per record.

---

## 10. Isolation Levels

Isolation levels are standardized (SQL standard) as a spectrum between performance and correctness. Each level prevents certain anomalies at the cost of increased blocking or overhead.

### The four standard levels

| Level                | Dirty Read | Non-Repeatable Read | Phantom Read | Write Skew |
|----------------------|------------|---------------------|--------------|------------|
| Read Uncommitted     | Possible   | Possible            | Possible     | Possible   |
| Read Committed       | Prevented  | Possible            | Possible     | Possible   |
| Repeatable Read      | Prevented  | Prevented           | Possible*    | Possible   |
| Serializable         | Prevented  | Prevented           | Prevented    | Prevented  |

*PostgreSQL's Repeatable Read also prevents phantoms due to MVCC.

### Practical relevance

For an embedded single-process database, the choice simplifies significantly:
- **Read Uncommitted** is almost never acceptable — partial writes are visible.
- **Read Committed** is the minimum useful level for most applications.
- **Snapshot Isolation** (between Repeatable Read and Serializable) is what MVCC naturally provides and is what most embedded databases offer.
- **Serializable** requires SSI (Serializable Snapshot Isolation) which adds conflict detection overhead.

### Relevance to this project

The project should target at minimum **Snapshot Isolation** with a goal of **Serializable** for write-write conflicts. For an embedded use case with a single writer (common in practice), snapshot isolation for readers + exclusive write access is a natural and safe starting point.

---

## 11. Crash Recovery

### What crash recovery must do

After a crash (power failure, OS kill, process panic), the database file may be in an intermediate state. Recovery must:

1. **Identify what was fully committed** (WAL shows this: a `COMMIT` record with a durable flush).
2. **Redo completed operations** that were not yet reflected in the data pages (pages were dirty in the buffer pool but not yet flushed).
3. **Undo incomplete operations** (transactions that were in progress when the crash occurred).

After recovery, the database is in the state it was in immediately after the last committed transaction.

### Recovery with WAL (ARIES)

Three passes (described in section 6):
1. **Analysis:** Find the last checkpoint, determine active transactions.
2. **REDO:** Replay log from the checkpoint forward, re-applying all operations.
3. **UNDO:** Reverse all operations from transactions that did not commit.

Recovery time is proportional to the size of the WAL between the last checkpoint and the crash. Frequent checkpointing reduces recovery time.

### Recovery with CoW B-trees

No UNDO pass is needed. Because writes always go to new pages and the root pointer is only updated atomically at commit time:
- If the crash occurred before the root pointer was updated: the old root is still valid; the new pages are garbage (collected on next startup).
- If the crash occurred after the root pointer was updated: the new root is valid and consistent.

**Startup recovery for CoW:** Scan the header for the most recent valid root pointer. Reclaim any pages allocated during an incomplete transaction (tracked via a free-space list or transaction log). Typically takes O(1) time, not O(log N) time.

### Torn writes and checksums

A **torn write** occurs when the OS crashes partway through writing a page (e.g., only the first 2 KB of a 4 KB page is written). The solution:
- **Page checksums:** Each page has a checksum in its header. On read, recalculate and compare. A mismatch means the page is corrupted. The database then uses WAL or CoW recovery to reconstruct the correct page.
- **Double-write buffer (InnoDB):** Write the page to a "double-write buffer" area first, then to its final location. On recovery, if the final location is corrupted, restore from the double-write buffer. Adds an extra write per page update.
- **CoW B-tree:** Never overwrites a valid page, so torn writes affect only new pages — which are reclaimed on recovery without being referenced.

### fsync and durability

A database is only as durable as its `fsync` discipline. Key rules:
- `fsync` the WAL (or data file for CoW) before reporting a commit to the caller.
- `fsync` the directory entry when creating a new file (so the directory entry survives a crash).
- On Linux, `O_DSYNC` can be used for data-sync without metadata sync (slightly faster).

**The fsync bug:** Several high-profile incidents (including Postgres and ext4 on Linux) involved databases that assumed `fsync` on a file after a crash would not lose previously fsync-ed data — an assumption that was violated by some OS/filesystem combinations. The lesson: be explicit about fsync ordering and do not assume the OS page cache is a durable medium.

### Relevance to this project

Crash recovery correctness is non-negotiable for a publish-quality database. The file format spec (task 8) must define:
- How the database header stores the current root/state pointer
- How an incomplete transaction is detected on startup
- The recovery procedure (and its time complexity)
- The checksum strategy for detecting torn writes

A **CoW B-tree approach significantly simplifies recovery** and is the recommended direction for this project — but the decision is formally made in task 7 after reading all research documents.

---

## 12. Putting It Together: Request Lifecycle

To cement these concepts, here is the complete lifecycle of a write transaction in a standard WAL-based B+ tree database:

```
1. Client calls db.begin_transaction() → returns Txn handle
   - Buffer pool: no action yet
   - WAL: write BEGIN record (may be lazy)
   - Concurrency: assign transaction ID; record as "active"

2. Client calls txn.insert_node(id, type, properties)
   - Storage engine: look up B+ tree leaf page for the given ID range
   - Buffer pool: load the leaf page (disk read if not cached), pin it
   - WAL: write UPDATE record with before-image + after-image; fsync WAL
   - Buffer pool: modify the in-memory page (now dirty), unpin
   - B+ tree: if leaf is full, split (may cascade, more WAL records + page loads)

3. Client calls txn.commit()
   - WAL: write COMMIT record; fsync WAL (this is the durability moment)
   - Concurrency: mark transaction as committed
   - Buffer pool: dirty pages remain in pool; flushed to disk later (lazy)

4. [Background] Buffer pool eviction
   - When a page needs eviction and it's dirty:
     - Verify its WAL records are already flushed (they should be if committed)
     - Write page to disk; mark clean

5. [Recovery, if crash occurred after step 2 but before step 3]
   - Analysis: find Txn is active, not committed
   - REDO: re-apply the UPDATE record (ensures page is up to date)
   - UNDO: apply the before-image to reverse the change
   - Result: as if the transaction never happened
```

In a **CoW B-tree** database, steps 2 and 3 differ:
```
2. Insert: allocate new pages for the modified path; write new pages to disk
   (no WAL needed — new pages don't affect existing consistent state)
3. Commit: atomically write new root pointer to file header; fsync
   (one fsync at commit; old pages become garbage)
```

---

## 13. Summary: Relevance Matrix for This Project

| Concept            | Relevance | Applies To Tasks |
|--------------------|-----------|------------------|
| Pages              | **Critical** — all data is organized in pages | 7, 8, 16 |
| B-trees (CoW)      | **Critical** — primary index structure; CoW variant eliminates WAL complexity | 7, 8, 16 |
| B-trees (in-place) | **Important to understand** — may use for secondary indexes | 7, 8 |
| LSM-trees          | **Background** — not recommended; explains write-optimized alternatives | 7 |
| Buffer pool        | **Critical** — performance and durability bridge | 7, 16 |
| WAL                | **Critical if not using CoW** — may be unnecessary with CoW B-tree | 7, 8, 16 |
| Transactions/ACID  | **Critical** — all operations must be transactional | 10, 16 |
| Locking (2PL)      | **Important** — fallback concurrency strategy; may use for write serialization | 7, 16 |
| MVCC               | **Critical** — readers must not block writers; CoW B-tree provides this | 7, 9, 16 |
| Isolation levels   | **Important** — must choose target level (Snapshot Isolation minimum) | 7, 10 |
| Crash recovery     | **Critical** — non-negotiable for publish quality | 8, 16 |
| fsync discipline   | **Critical** — correctness depends on it | 8, 9, 15 |

### Recommended starting point for this project

Based on this research, the following architectural direction is **tentatively recommended** (to be formally decided in task 7 with input from tasks 2 and 4):

- **Storage structure:** Copy-on-write B+ tree (like redb or LMDB)
  - Rationale: Eliminates WAL complexity; provides MVCC naturally; crash safety by design; well-proven in Rust ecosystem (redb)
- **Concurrency model:** MVCC via CoW snapshots; single writer at a time; unlimited concurrent readers
  - Rationale: Matches read-heavy graph traversal workload; simple to reason about; no deadlock possible
- **Buffer pool:** Fixed-size LRU or Clock-based pool
  - Rationale: Standard approach; manageable complexity; configurable size
- **Isolation level:** Snapshot Isolation for reads; serializable writes (one writer at a time)
  - Rationale: Strong safety guarantees without SSI complexity overhead

---

## Completion Report: Task 1 — Database Internals Fundamentals

### Status: COMPLETE

### Done Criterion:
The criterion requires a document covering each concept with a plain-language explanation, diagrams where helpful, and a "relevance to this project" assessment for each. ✓

Concepts covered:
- Pages (section 2): explanation, anatomy diagram, free-space management, relevance ✓
- B-trees (section 3): plain-language explanation, tree diagram, CoW variant, relevance ✓
- LSM-trees (section 4): explanation, component breakdown, comparison table, relevance (and why not recommended) ✓
- Buffer pool (section 5): explanation, eviction policies, dirty tracking, fsync interaction, relevance ✓
- WAL (section 6): explanation, structure, ARIES protocol, CoW alternative, relevance ✓
- Transactions/ACID (section 7): definitions, lifecycle, relevance ✓
- Locking/2PL (section 8): explanation, lock types, deadlock, relevance ✓
- MVCC (section 9): explanation, visibility rules, CoW connection, relevance ✓
- Isolation levels (section 10): full table, practical guidance, relevance ✓
- Crash recovery (section 11): WAL recovery, CoW recovery, torn writes, fsync, relevance ✓

### Deliverables:
- `001-db-internals-fundamentals.md` — this document

### Summary:
Produced a comprehensive research document covering all required database internals concepts. Each section is written for a reader with programming experience but no prior database internals knowledge, and each section includes explicit relevance assessment for this project. The document ends with a summary relevance matrix and a tentative architectural recommendation (CoW B-tree) that should be validated against the research from tasks 2 and 4 before being adopted in task 7.

### Context for Next Task:
This document is a dependency for tasks 7 (Graph Storage Model) and 8 (Single-File Format). Task 7 will also depend on tasks 2 and 6. Task 8 will also depend on task 4.

The most important takeaways from this document for downstream tasks are:
1. **Copy-on-write B-trees** are the strongly preferred direction — they eliminate WAL complexity, provide MVCC naturally, and have excellent Rust precedents (redb).
2. **fsync discipline** is a correctness concern, not just a performance concern. The HAL trait layer (task 9) must expose fsync explicitly.
3. **Snapshot Isolation** is the minimum viable isolation level; the concurrency model must guarantee it.

### Residual Concerns:
- The tentative recommendation of a CoW B-tree has not been validated against tasks 2 (Graph Storage Strategies) and 4 (Embedded DB Architectures). Task 7 must make the final decision with full information.
- This document does not cover graph-specific storage concerns (how nodes and edges map to B-tree records). That is the domain of task 2.
- ARIES recovery is described at a high level. If the final design uses a WAL, a more detailed ARIES study (including CLR records for undo logging) may be needed before task 16.

### Upstream Flags:
None.
