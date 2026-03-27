# 030 — Workspace Redesign: Three-Crate `no_std` Architecture

**Task:** 30 — Workspace Redesign  
**Status:** DESIGN (pending implementation)  
**Depends on:** All prior implementation tasks (22–29)  
**Goal:** Split the workspace into three crates so the database engine runs in `no_std + alloc` environments, and the core types crate is independently publishable under a new name.

---

## Table of Contents

1. [Motivation](#1-motivation)
2. [Current Architecture](#2-current-architecture)
3. [Naming](#3-naming)
4. [Proposed Three-Crate Architecture](#4-proposed-three-crate-architecture)
5. [Crate 1: `phonograph` — Graph Vocabulary](#5-crate-1-phonograph--graph-vocabulary)
6. [Crate 2: `phonograph_db` — Database](#6-crate-2-phonograph_db--database)
7. [Crate 3: `phonograph_std` — OS/Platform Layer](#7-crate-3-phonograph_std--osplatform-layer)
8. [Error Hierarchy Redesign](#8-error-hierarchy-redesign)
9. [The `Database<B>` Generification](#9-the-databaseb-generification)
10. [Sync Primitives Strategy](#10-sync-primitives-strategy)
11. [HashMap Replacement](#11-hashmap-replacement)
12. [Feature Flag Design](#12-feature-flag-design)
13. [Dependency Graph](#13-dependency-graph)
14. [Migration Plan](#14-migration-plan)
15. [Public API Changes & Migration Guide](#15-public-api-changes--migration-guide)
16. [Workspace Cargo.toml Layout](#16-workspace-cargotoml-layout)
17. [Verification Checklist](#17-verification-checklist)
18. [Design Decision Log](#18-design-decision-log)
19. [Naming Alternatives](#19-naming-alternatives)
20. [Residual Concerns](#20-residual-concerns)

---

## 1. Motivation

The current workspace has two crates: `graph_db_core` (a `no_std + alloc` foundation) and `graph_db` (the full engine, `std`-only). This design has three limitations:

1. **The database engine is `std`-only.** The `Database` struct, transactions, storage engine, buffer pool, and B+ tree operations all live in `graph_db` and require `std`. But their actual `std` dependencies are narrow: `Mutex`/`RwLock` from `std::sync`, `HashMap` from `std::collections`, and the concrete `FileBackend`. The algorithmic core — page management, B+ tree traversal, CoW path copying, MVCC snapshot isolation, serialization — is pure computation on byte buffers and could run on bare metal with a heap allocator.

2. **`graph_db_core` mixes concerns.** It bundles graph vocabulary (types, traits, errors) with storage backend traits (`ReadAt`, `WriteAt`, `StorageBackend`) and a unified `Error` enum that includes `StorageError` and `TransactionError` — concepts that belong in the database, not in a graph vocabulary library. Separating these makes the core a clean, focused crate.

3. **OS-specific code is tangled into the engine.** The `FileBackend` (with `libc::fcntl`, `libc::flock`, OS file locking, `std::fs`) and the `AnyBackend` enum dispatch (which hard-codes `FileBackend`) force the entire `db/` module into `std`. Isolating them into a thin OS-integration crate untangles this.

---

## 2. Current Architecture

```
┌──────────────────────────────────────────────────┐
│ graph_db (std-only)                              │
│                                                  │
│  backend_std/    FileBackend, FileLockGuard       │
│  storage/        StorageEngine<B>, BufferPool,    │
│                  BTree, PageAllocator, serializ.  │
│  db/             Database, ReadTransaction,       │
│                  WriteTransaction, SchemaCache,    │
│                  WriteBuffer, InferenceEngine      │
├──────────────────────────────────────────────────┤
│ graph_db_core (no_std + alloc)                   │
│                                                  │
│  types/          Node, Edge, Value, IDs           │
│  schema/         GraphView, TypeRegistryView      │
│  constraint/     ConstraintValidator, ChangeSet   │
│  inference/      InferenceRule, InferredFact      │
│  error/          Error (unified), SchemaError,    │
│                  StorageError, TransactionError    │
│  backend/        ReadAt, WriteAt, Durability      │
│  backend_mem/    MemoryBackend                    │
└──────────────────────────────────────────────────┘
```

### Why the Current Split Is Wrong

The unified `Error` enum in `graph_db_core` contains:

```rust
pub enum Error {
    Schema(SchemaError),
    ConstraintViolation(Vec<ConstraintViolation>),
    Storage(StorageError),         // database concern — doesn't belong in core
    NotFound(NotFoundError),
    Transaction(TransactionError), // database concern — doesn't belong in core
    Inference(InferenceError),
}
```

But no core trait actually uses this enum:

- `ConstraintValidator::validate` → `Vec<ConstraintViolation>`
- `InferenceRule::infer` → `Result<InferenceResult, InferenceError>`

The unified `Error` is only consumed by `Database`, `ReadTransaction`, and `WriteTransaction` — all database types. It should live with them.

---

## 3. Naming

### Primary Recommendation: **`phonograph`**

A phonograph reads and writes grooves on a spinning disk. This database reads and writes records to storage. The metaphor is almost perfect:

- **"phono" + "graph"** — literally "sound writer," but universally recognized as the iconic record player
- Has **"graph"** baked right into the name
- A real English word — instantly memorable, Googleable, zero ambiguity in pronunciation
- Not found on crates.io (verified 2026-03-26)

### The Three Crate Names

| Crate | Name | What it is |
|-------|------|------------|
| Graph vocabulary | **`phonograph`** | Standalone graph types, traits, and errors — pure graph vocabulary |
| Database | **`phonograph_db`** | `no_std + alloc` graph database (B+ tree, transactions, MVCC, backends) |
| OS/platform layer | **`phonograph_std`** | `std`-only: FileBackend, AnyBackend, convenience Database alias |

See [§19 Naming Alternatives](#19-naming-alternatives) for other options.

---

## 4. Proposed Three-Crate Architecture

```
┌────────────────────────────────────────────────────────────────┐
│                    Application Layer                            │
├────────────────┬────────────────────────────┬──────────────────┤
│                │                            │                  │
│ phonograph_std │  phonograph_db             │  phonograph      │
│ (std + libc)   │  (no_std + alloc)          │  (no_std + alloc)│
│                │                            │                  │
│ FileBackend    │  Unified Error enum        │  Node, Edge      │
│ FileBackendCfg │  StorageError, Txn Error   │  Value, IDs      │
│ FileLockGuard  │  ReadAt, WriteAt           │  TypeDefinition  │
│ AnyBackend     │  Durability, Storage-      │  GraphView       │
│ OpenableBackend│    Backend traits          │  SchemaError     │
│ LockableBackend│  MemoryBackend             │  ConstraintVal*  │
│ type Database  │  Database<B>               │  InferenceRule   │
│  = Db<Any>     │  StorageEngine<B>          │  InferenceError  │
│ convenience    │  BufferPool                │  NotFoundError   │
│ open() etc.    │  BTree, PageAllocator      │  Constraint-     │
│                │  ReadTransaction<B>        │    Violation     │
│                │  WriteTransaction<B>       │                  │
│                │  WriteBuffer, SchemaCache  │                  │
│                │  InferenceEngine           │                  │
│                │  Serialization             │                  │
│                │  Snapshot, Page types       │                  │
│                │  DatabaseConfig (no paths)  │                  │
└────────────────┴────────────────────────────┴──────────────────┘
       ▲                     ▲                        ▲
       │                     │                        │
  requires std          no_std + alloc           no_std + alloc
  + libc (unix)         spin, hashbrown          ZERO dependencies
                        crc32fast, xxhash        pure graph vocab
```

### Dependency Flow

```
phonograph_std ──depends-on──▶ phonograph_db ──depends-on──▶ phonograph
```

Each crate only depends on the one to its right. No cycles. Clean layering.

---

## 5. Crate 1: `phonograph` — Graph Vocabulary

### Identity

```toml
[package]
name = "phonograph"
version = "0.1.0"
edition = "2021"
rust-version = "1.82"
description = "Core types, traits, and error hierarchy for typed property graphs (no_std + alloc)"
keywords = ["graph", "property-graph", "no-std", "types", "schema"]
categories = ["data-structures", "no-std"]
```

### What It Contains

Pure graph vocabulary. No storage, no transactions, no databases.

| Module | Contents |
|--------|----------|
| `types/` | `NodeId`, `EdgeId`, `TypeId`, `PropertyKeyId`, `Value`, `ValueTypeDescriptor`, `PropertyMap`, `Node`, `Edge`, `TypeDefinition`, `PropertyDeclaration`, `TypeKind` |
| `schema/` | `GraphView`, `TypeRegistryView`, `PropertyKeyRegistryView` |
| `constraint/` | `ConstraintValidator`, `ConstraintViolation`, `ViolationSubject`, `ChangeSet`, `NodeChange`, `EdgeChange` |
| `inference/` | `InferenceRule`, `InferredFact`, `InferenceResult`, `InferenceMode`, `InferredEntity`, `MaterializedMapping`, `ProvenanceRecord` |
| `error/` | `SchemaError`, `NotFoundError`, `InferenceError` (individual error types only — no unified `Error` enum) |

### What Does NOT Live Here

| Item | Why not | New home |
|------|---------|----------|
| `Error` (unified enum) | Only used by Database/Transaction types | `phonograph_db` |
| `StorageError` (struct) | Storage is a database concern | `phonograph_db` |
| `TransactionError` | Transactions are a database concern | `phonograph_db` |
| `backend/` traits | Byte I/O is a storage concern | `phonograph_db` |
| `backend_mem/` | Implements backend traits | `phonograph_db` |

### Feature Flags

```toml
[features]
default = ["std"]
std = ["alloc"]    # Enables std::error::Error impls
alloc = []         # Enables types that require heap allocation

[dependencies]
# NONE — zero external dependencies, pure Rust
```

### Standalone Use Cases

- Building custom graph data structures without the database
- Defining graph schemas and type hierarchies in `no_std` environments
- Sharing graph types between services (e.g., over gRPC/protobuf)
- Implementing `ConstraintValidator` or `InferenceRule` in a downstream crate
- Educational use — the types are self-documenting and standalone

---

## 6. Crate 2: `phonograph_db` — Database

### Identity

```toml
[package]
name = "phonograph_db"
version = "0.1.0"
edition = "2021"
rust-version = "1.82"
description = "Embeddable graph database with B+ tree storage and MVCC transactions (no_std + alloc)"
keywords = ["graph-database", "embedded-database", "no-std", "btree", "mvcc"]
categories = ["database-implementations", "no-std"]
```

### What It Contains

Everything related to *storing, retrieving, and transacting on* a graph.

| Module | Contents | Origin |
|--------|----------|--------|
| `error/` | Unified `Error` enum, `StorageError`, `TransactionError` | Moved from `graph_db_core::error` (partially) |
| `backend/` | `ReadAt`, `WriteAt`, `Durability`, `StorageBackend`, `StorageErrorKind`, `StorageErrorType`, `BackendError` trait | Moved from `graph_db_core::backend` |
| `backend_mem/` | `MemoryBackend`, `MemoryError` | Moved from `graph_db_core::backend_mem` |
| `storage/` | `StorageEngine<B>`, `StorageEngineConfig` | From `graph_db::storage` |
| `storage::page` | Page types, headers, serialization | From `graph_db::storage::page` |
| `storage::btree` | `BTree`, `BTreeConfig`, `BTreeCursor`, CoW operations | From `graph_db::storage::btree` |
| `storage::buffer_pool` | `BufferPool`, clock eviction | From `graph_db::storage::buffer_pool` |
| `storage::allocator` | `PageAllocator`, free-space management | From `graph_db::storage::allocator` |
| `storage::format` | File identity header, superblock, dual-superblock commit | From `graph_db::storage::format` |
| `storage::serialization` | Record serialization, key encoding | From `graph_db::storage::serialization` |
| `storage::snapshot` | `Snapshot`, `SnapshotRoots` | From `graph_db::storage::snapshot` |
| `db/` | `Database<B>`, `DatabaseConfig` (no paths) | From `graph_db::db` |
| `db::read_txn` | `ReadTransaction<B>` | From `graph_db::db::read_txn` |
| `db::write_txn` | `WriteTransaction<B>` | From `graph_db::db::write_txn` |
| `db::write_buffer` | `WriteBuffer`, change tracking | From `graph_db::db::write_buffer` |
| `db::schema_cache` | `SchemaCache` | From `graph_db::db::schema_cache` |
| `db::inference_engine` | `InferenceEngine`, provenance | From `graph_db::db::inference_engine` |
| `db::builders` | `NodeBuilder`, `EdgeBuilder`, `TypeDefinitionBuilder` | From `graph_db::db::builders` |
| `db::graph_view` | `OverlayGraphView`, `SnapshotReader` | From `graph_db::db::graph_view` |
| `db::graph_reader` | `GraphReader` trait | From `graph_db::db::graph_reader` |

### Feature Flags

```toml
[features]
default = ["std"]
std = ["alloc", "phonograph/std"]
alloc = ["phonograph/alloc"]

[dependencies]
phonograph = { version = "0.1", path = "../phonograph", default-features = false, features = ["alloc"] }
spin = { version = "0.9", default-features = false, features = ["mutex", "rwlock"] }
hashbrown = { version = "0.15", default-features = false, features = ["ahash"] }
crc32fast = "1"
xxhash-rust = { version = "0.8", default-features = false, features = ["xxh3"] }
```

### Re-exports

The database re-exports everything from `phonograph` so users get the full vocabulary without a second dependency:

```rust
// phonograph_db/src/lib.rs
pub use phonograph::*;  // All core types, traits, individual errors
```

### The `no_std` Shim

```rust
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub(crate) mod sync {
    pub use spin::Mutex;
    pub use spin::MutexGuard;
    pub use spin::RwLock;
    pub use spin::RwLockReadGuard;
    pub use spin::RwLockWriteGuard;
    pub use alloc::sync::Arc;
}
```

---

## 7. Crate 3: `phonograph_std` — OS/Platform Layer

### Identity

```toml
[package]
name = "phonograph_std"
version = "0.1.0"
edition = "2021"
rust-version = "1.82"
description = "std-only extensions for phonograph: file backend, OS locking, and convenience Database type"
keywords = ["graph-database", "file-backend", "embedded-database"]
categories = ["database-implementations"]
```

### What It Contains

| Module | Contents | Why `std`? |
|--------|----------|------------|
| `backend_std/` | `FileBackend`, `FileBackendConfig`, `FileError` | `std::fs::File`, `pread`/`pwrite` |
| `backend_std/` | `FileLockGuard` | `libc::flock` (Unix), `LockFileEx` (Windows) |
| `backend/lifecycle` | `OpenableBackend`, `LockableBackend` trait impls | `std::path::Path` in implementations |
| `any_backend` | `AnyBackend` enum (File + Memory) | Bundles `FileBackend` |
| `lib.rs` | `type Database = phonograph_db::Database<AnyBackend>` | Convenience alias |
| `lib.rs` | `fn open(path)`, `fn open_in_memory()` | Convenience constructors |

### Cargo.toml

```toml
[dependencies]
phonograph = { version = "0.1", path = "../phonograph" }
phonograph_db = { version = "0.1", path = "../phonograph_db" }

[target.'cfg(unix)'.dependencies]
libc = "0.2"

[dev-dependencies]
tempfile = "3"
```

This crate is always `std`. No feature flags.

### Re-exports

```rust
pub use phonograph::*;
pub use phonograph_db::*;

pub mod backend_std;
mod any_backend;

pub type Database = phonograph_db::Database<any_backend::AnyBackend>;

pub fn open(path: impl AsRef<std::path::Path>) -> Result<Database, phonograph_db::Error> { ... }
pub fn open_in_memory() -> Result<Database, phonograph_db::Error> { ... }
```

---

## 8. Error Hierarchy Redesign

This is the key structural change. The unified `Error` enum moves from the core crate to the database crate, and the core keeps only the individual error types that its traits actually use.

### `phonograph` (core) — Individual Error Types Only

```rust
// phonograph::error

/// Schema/type-system errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaError {
    DuplicateTypeName { name: String, kind: TypeKind },
    TypeNotFound(TypeId),
    CycleDetected { child: TypeId, would_be_parent: TypeId },
    SupertypeNotFound(TypeId),
    KindMismatch { expected: TypeKind, found: TypeKind },
    DuplicatePropertyKey { name: String },
    PropertyKeyNotFound(PropertyKeyId),
}

/// Entity-not-found errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotFoundError {
    Node(NodeId),
    Edge(EdgeId),
    Type(TypeId),
    PropertyKey(PropertyKeyId),
}

/// Inference rule errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InferenceError {
    RuleNotFound { name: String },
    RuleError { name: String, message: String },
    CycleDetected { rule_names: Vec<String> },
}
```

These are self-contained. They reference only graph types (`TypeId`, `NodeId`, etc.) which live in the same crate.

### `phonograph_db` — Unified Error Enum + Database-Specific Errors

```rust
// phonograph_db::error

/// Storage-layer error.
#[derive(Debug)]
pub struct StorageError {
    pub message: String,
    pub source: Option<Box<dyn core::fmt::Display>>,
}

/// Transaction-level error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransactionError {
    ReadOnly,
    AlreadyCommitted,
    AlreadyAborted,
}

/// The unified error type for all database operations.
#[derive(Debug)]
pub enum Error {
    /// A schema/type registry operation failed.
    Schema(phonograph::SchemaError),
    /// Constraint violations detected at commit time.
    ConstraintViolation(Vec<phonograph::ConstraintViolation>),
    /// A storage-layer error occurred.
    Storage(StorageError),
    /// The requested entity was not found.
    NotFound(phonograph::NotFoundError),
    /// A transaction-level error.
    Transaction(TransactionError),
    /// An inference rule produced an error.
    Inference(phonograph::InferenceError),
}
```

### Why This Works

The dependency only flows downward:

- `phonograph_db::Error` references `phonograph::SchemaError`, `phonograph::NotFoundError`, etc. ✓ (db depends on core)
- `phonograph::ConstraintValidator` returns `Vec<ConstraintViolation>`, not `Error`. ✓ (core doesn't need the unified enum)
- `phonograph::InferenceRule` returns `Result<..., InferenceError>`, not `Error`. ✓ (core doesn't need the unified enum)

No circular dependency. The core crate defines the building-block error types. The database crate composes them into a unified error.

---

## 9. The `Database<B>` Generification

The current `Database` hard-codes `AnyBackend`:

```rust
// CURRENT
pub struct Database {
    pub(crate) inner: Arc<DatabaseInner>,  // StorageEngine<AnyBackend>
}
```

The new design makes it generic:

```rust
// NEW (phonograph_db)
pub struct Database<B: StorageBackend> {
    pub(crate) inner: Arc<DatabaseInner<B>>,
}

pub(crate) struct DatabaseInner<B: StorageBackend> {
    pub storage: Mutex<StorageEngine<B>>,
    pub write_mutex: Mutex<()>,
    pub current_snapshot: RwLock<Arc<Snapshot>>,
    pub schema_cache: RwLock<SchemaCache>,
    pub constraint_registry: RwLock<Vec<Box<dyn ConstraintValidator>>>,
    pub inference_engine: Mutex<InferenceEngine>,
    pub persisted_extension_names: RwLock<PersistedExtensionNames>,
    pub config: DatabaseConfig,
}
```

### Cascading Generics

```rust
pub struct ReadTransaction<'db, B: StorageBackend> { ... }
pub struct WriteTransaction<'db, B: StorageBackend> { ... }
```

### `DatabaseConfig` Split

```rust
// phonograph_db — no_std safe
pub struct DatabaseConfig {
    pub page_size: usize,
    pub buffer_pool_frames: usize,
    pub inference_cache_size: usize,
    pub application_id: u32,
}

// phonograph_std — std only
pub struct FileConfig {
    pub path: PathBuf,
    pub read_only: bool,
    pub engine: phonograph_db::DatabaseConfig,
}
```

### How Users Create a Database

**`no_std`:**
```rust
use phonograph_db::{Database, DatabaseConfig};
use phonograph_db::backend_mem::MemoryBackend;

let db = Database::create(MemoryBackend::new(), DatabaseConfig::default())?;
```

**`std`:**
```rust
let db = phonograph_std::open("my_database.db")?;
let db = phonograph_std::open_in_memory()?;
```

---

## 10. Sync Primitives Strategy

**Decision:** `spin` unconditionally in `phonograph_db`.

```rust
// phonograph_db/src/sync.rs
pub(crate) use spin::Mutex;
pub(crate) use spin::MutexGuard;
pub(crate) use spin::RwLock;
pub(crate) use spin::RwLockReadGuard;
pub(crate) use spin::RwLockWriteGuard;
pub(crate) use alloc::sync::Arc;
```

**Rationale:** One code path, no conditional compilation. Performance is adequate for single-writer MVCC with short critical sections. `spin` is zero transitive deps, ~200 LOC, battle-tested in embedded Rust. If profiling reveals contention, an opt-in `std-sync` feature can be added later.

---

## 11. HashMap Replacement

Replace `std::collections::HashMap` → `hashbrown::HashMap` in the buffer pool page table. `hashbrown` *is* `std::HashMap` (since Rust 1.36) without the `std` wrapper. One-line import change, identical API.

---

## 12. Feature Flag Design

### `phonograph`
```toml
[features]
default = ["std"]
std = ["alloc"]
alloc = []
```

### `phonograph_db`
```toml
[features]
default = ["std"]
std = ["alloc", "phonograph/std"]
alloc = ["phonograph/alloc"]
```

### `phonograph_std`
No feature flags — always `std`.

### User-Facing Matrix

| Environment | Depend on | Features |
|-------------|-----------|----------|
| Bare metal / WASM | `phonograph_db` | `default-features = false, features = ["alloc"]` |
| `no_std` types only | `phonograph` | `default-features = false, features = ["alloc"]` |
| Standard Rust app | `phonograph_std` | (default) |
| Lib needing graph types | `phonograph` | (default) |

---

## 13. Dependency Graph

| Crate | Dependencies | `no_std`? |
|-------|-------------|-----------|
| `phonograph` | **none** | yes |
| `phonograph_db` | `phonograph`, `spin`, `hashbrown`, `crc32fast`, `xxhash-rust` | yes |
| `phonograph_std` | `phonograph`, `phonograph_db`, `libc` (unix) | no |

### Transitive deps for `phonograph_db`: ~6 small `no_std`-compatible crates.
### `phonograph` has zero dependencies.

---

## 14. Migration Plan

### Phase 1: Rename `graph_db_core` → `phonograph` + Strip Non-Vocabulary Modules

1. Rename `crates/graph_db_core/` → `crates/phonograph/`
2. Update `Cargo.toml` name to `phonograph`
3. **Remove** `backend/` module
4. **Remove** `backend_mem/` module
5. **Remove** `StorageError` struct and `TransactionError` from `error/`
6. **Remove** the unified `Error` enum from `error/`
7. Keep `SchemaError`, `NotFoundError`, `InferenceError` as standalone exports
8. Remove backend re-exports from `lib.rs`
9. Verify `constraint/`, `inference/`, `schema/` have zero backend references
10. `cargo check --no-default-features --features alloc`

### Phase 2: Create `phonograph_db` Crate

1. Create `crates/phonograph_db/`
2. Move `backend/` and `backend_mem/` from old `graph_db_core`
3. Move `storage/` and `db/` from `graph_db`
4. **Create `error/` module** with `StorageError`, `TransactionError`, and the unified `Error` enum (referencing `phonograph::SchemaError` etc.)
5. Add `spin`, `hashbrown` deps
6. Replace `std::sync::*` → `spin::*`
7. Replace `std::collections::HashMap` → `hashbrown::HashMap`
8. Generify `Database<B>`, `ReadTransaction<B>`, `WriteTransaction<B>`
9. Remove `AnyBackend` (moves to `phonograph_std`)
10. Strip `PathBuf`/`StorageMode` from `DatabaseConfig`
11. Rewrite `Database::open`/`Database::create` to take a backend `B`
12. Add `#![cfg_attr(not(feature = "std"), no_std)]`, `extern crate alloc`
13. Add `pub use phonograph::*;` re-export
14. `cargo check --no-default-features --features alloc` — must pass

### Phase 3: Create `phonograph_std` Crate

1. Create `crates/phonograph_std/`
2. Move `backend_std/` from `graph_db`
3. Create `AnyBackend` enum (moved from `db/database.rs`)
4. Create `type Database = phonograph_db::Database<AnyBackend>`
5. Create `open(path)`, `open_in_memory()` convenience functions
6. Re-export both inner crates

### Phase 4: Retire or Facade `graph_db`

Either delete `graph_db` or make it a one-line `pub use phonograph_std::*;` for backwards compat.

### Phase 5: Verification

Full workspace: `cargo check`, `cargo test`, `cargo clippy`, `cargo doc`.

---

## 15. Public API Changes & Migration Guide

### `std` Users

**Before:**
```rust
use graph_db::{Database, DatabaseConfig, Value, Error};
let db = Database::open(DatabaseConfig::in_memory())?;
```

**After:**
```rust
use phonograph_std::{Database, Value, Error};
let db = phonograph_std::open_in_memory()?;
```

### `no_std` Users (New)

```rust
#![no_std]
extern crate alloc;
use phonograph_db::{Database, DatabaseConfig, Value, NodeBuilder, TypeDefinitionBuilder};
use phonograph_db::backend_mem::MemoryBackend;

let db = Database::create(MemoryBackend::new(), DatabaseConfig::default())?;
```

### Custom Backend Implementors

```rust
use phonograph_db::backend::{ReadAt, WriteAt, Durability, StorageBackend, StorageErrorType, BackendError};
use phonograph_db::{Database, DatabaseConfig};

struct FlashBackend { /* ... */ }
// impl StorageErrorType, BackendError, ReadAt, WriteAt, Durability for FlashBackend ...
let db = Database::create(FlashBackend::new(), DatabaseConfig::default())?;
```

---

## 16. Workspace Cargo.toml Layout

```toml
[workspace]
members = [
    "crates/phonograph",
    "crates/phonograph_db",
    "crates/phonograph_std",
]
resolver = "2"

[workspace.package]
edition = "2021"
rust-version = "1.82"
license = "MIT OR Apache-2.0"
repository = "https://github.com/user/phonograph"

[workspace.dependencies]
phonograph = { version = "0.1", path = "crates/phonograph" }
phonograph_db = { version = "0.1", path = "crates/phonograph_db" }
phonograph_std = { version = "0.1", path = "crates/phonograph_std" }
spin = { version = "0.9", default-features = false, features = ["mutex", "rwlock"] }
hashbrown = { version = "0.15", default-features = false, features = ["ahash"] }
crc32fast = "1"
xxhash-rust = { version = "0.8", default-features = false, features = ["xxh3"] }
libc = "0.2"
tempfile = "3"
```

### Directory Structure

```
phonograph/                         # workspace root
├── Cargo.toml                      # workspace manifest
├── crates/
│   ├── phonograph/                 # Crate 1: graph vocabulary
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── types/
│   │       ├── schema/
│   │       ├── constraint/
│   │       ├── inference/
│   │       └── error/              # SchemaError, NotFoundError, InferenceError only
│   ├── phonograph_db/              # Crate 2: database
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── sync.rs
│   │       ├── error/              # Unified Error, StorageError, TransactionError
│   │       ├── backend/
│   │       ├── backend_mem/
│   │       ├── storage/
│   │       │   ├── page/
│   │       │   ├── btree/
│   │       │   ├── buffer_pool.rs
│   │       │   ├── allocator.rs
│   │       │   ├── format.rs
│   │       │   ├── serialization.rs
│   │       │   └── snapshot.rs
│   │       └── db/
│   │           ├── database.rs
│   │           ├── config.rs
│   │           ├── read_txn.rs
│   │           ├── write_txn.rs
│   │           ├── write_buffer.rs
│   │           ├── schema_cache.rs
│   │           ├── inference_engine.rs
│   │           ├── builders.rs
│   │           ├── graph_view.rs
│   │           └── graph_reader.rs
│   └── phonograph_std/             # Crate 3: OS/platform layer
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs
│           ├── backend_std/
│           │   └── file_backend.rs
│           └── any_backend.rs
├── tests/
└── README.md
```

---

## 17. Verification Checklist

| # | Check | Command |
|---|-------|---------|
| 1 | `phonograph` compiles `no_std + alloc` | `cargo check -p phonograph --no-default-features --features alloc` |
| 2 | `phonograph` has zero non-dev dependencies | Inspect `Cargo.toml` |
| 3 | `phonograph` contains NO storage/backend/transaction concepts | `grep -r "ReadAt\|WriteAt\|StorageBackend\|MemoryBackend\|StorageError\|TransactionError" crates/phonograph/src/` → empty |
| 4 | `phonograph_db` compiles `no_std + alloc` | `cargo check -p phonograph_db --no-default-features --features alloc` |
| 5 | `phonograph_db` has no ungated `use std::` | `grep -r "use std::" crates/phonograph_db/src/` → empty or `#[cfg]`-gated |
| 6 | `phonograph_std` compiles | `cargo check -p phonograph_std` |
| 7 | Full workspace builds | `cargo build --workspace` |
| 8 | All tests pass | `cargo test --workspace` |
| 9 | No clippy warnings | `cargo clippy --workspace --all-targets -- -D warnings` |
| 10 | Docs build | `cargo doc --workspace --no-deps` |
| 11 | `Database<MemoryBackend>` works on `no_std` | Doc-test or integration test |
| 12 | `Database<AnyBackend>` works on `std` | Integration test |
| 13 | All 311+ existing tests pass | `cargo test --workspace` |

---

## 18. Design Decision Log

| # | Decision | Choice | Rationale |
|---|----------|--------|-----------|
| R1 | Core crate name | `phonograph` | Phonograph reads/writes grooves on disk ≈ DB reads/writes records. Real word, "graph" in it, memorable. |
| R2 | Database crate name | `phonograph_db` | Short, clear about what it is (a database), pairs naturally with `phonograph`. |
| R3 | Number of crates | 3 | Minimal split: `no_std` DB + standalone vocabulary + OS isolation |
| R4 | Backend traits location | `phonograph_db` | Byte I/O is a database concern, not graph vocabulary |
| R5 | `MemoryBackend` location | `phonograph_db` | Implements backend traits which live in `phonograph_db` |
| R6 | Unified `Error` enum location | `phonograph_db` | Only used by `Database`/transactions. Core traits use individual error types. |
| R7 | `StorageError`/`TransactionError` | `phonograph_db` | Storage and transactions are database concerns |
| R8 | Core error exports | Individual types only (`SchemaError`, `NotFoundError`, `InferenceError`) | These are what core traits actually use. No unified enum needed. |
| R9 | Sync primitives | `spin` unconditionally | Simplicity. Adequate for single-writer MVCC. |
| R10 | HashMap replacement | `hashbrown` | *Is* std::HashMap without std wrapper |
| R11 | `Database` generification | `Database<B: StorageBackend>` | Removes `AnyBackend` coupling. Enables custom backends. |
| R12 | `DatabaseConfig` split | Engine config (no paths) + file config (std) | Paths need `std::path` |
| R13 | `AnyBackend` location | `phonograph_std` | Bundles `FileBackend` which needs `std` |
| R14 | Re-export strategy | Each crate re-exports the one below it | Single import = full access |
| R15 | Core dependency count | Zero | Maximally reusable, trustworthy |
| R16 | Backwards compat | Optional `graph_db` facade | One-line `pub use phonograph_std::*` |
| R17 | Backend error trait rename | `StorageError` trait → `BackendError` trait | Avoids name collision with `error::StorageError` struct now that both live in `phonograph_db` |

---

## 19. Naming Alternatives

| Name | Theme | Derived Names |
|------|-------|---------------|
| **`phonograph`** ★ | record player: reads/writes grooves on disk | `phonograph_db`, `phonograph_std` |
| **`ferograph`** | ferro (iron/Rust) + graph | `ferograph_db`, `ferograph_std` |
| **`nexograph`** | nexus + graph | `nexograph_db`, `nexograph_std` |
| **`forgeraph`** | forge + graph | `forgeraph_db`, `forgeraph_std` |

---

## 20. Residual Concerns

1. **`spin` priority inversion on preemptive OS.** Theoretical for our workload (short critical sections, single-writer). An opt-in `std-sync` feature could address this later.

2. **`crc32fast` `no_std` loses hardware acceleration.** Page checksums may be slower on bare metal. Acceptable for v1.

3. **`hashbrown` `ahash` uses fixed seed on `no_std`.** Fine — page table keys are sequential integers, not security-sensitive.

4. **Engine requires `alloc`.** Targets without a heap allocator cannot use the database. Correct trade-off for a database.

5. **Test infrastructure split.** File-backend tests → `phonograph_std`. Engine tests → `phonograph_db` (MemoryBackend only). Ensure no coverage gaps.

6. **Name registration.** Reserve `phonograph`, `phonograph_db`, `phonograph_std` on crates.io promptly.

7. **`StorageError` name collision resolved.** Previously both a struct (core) and a trait (backend) shared the name "StorageError." With both now in `phonograph_db`, the backend trait is renamed to `BackendError` to eliminate ambiguity. `error::StorageError` is the struct (message + source). `backend::BackendError` is the trait that backend implementations conform to. Clean separation, no confusion.
