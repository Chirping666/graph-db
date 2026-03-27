# 010 — Rust API Surface Design Specification

**Project:** Embedded Graph Database with Extensible Schema & Pluggable Inference  
**Task:** 10 — Design: Rust API Surface  
**Status:** Complete  
**Depends on:** Task 6 (`006-schema-extension-spec.md`), Task 7 (`007-graph-storage-model.md`)  
**Intended audience:** All downstream design and implementation tasks. A reader familiar with Rust should be able to understand every public type, method, and trait in this document and write application code against it.

---

## Table of Contents

1. [Purpose and Scope](#1-purpose-and-scope)
2. [Design Principles](#2-design-principles)
3. [Crate Structure and Feature Flags](#3-crate-structure-and-feature-flags)
4. [Error Handling](#4-error-handling)
5. [Database Lifecycle: Opening and Configuration](#5-database-lifecycle-opening-and-configuration)
6. [Transaction API](#6-transaction-api)
7. [Schema Operations: Types and Property Keys](#7-schema-operations-types-and-property-keys)
8. [Node Operations](#8-node-operations)
9. [Edge Operations](#9-edge-operations)
10. [Graph Traversal and Query](#10-graph-traversal-and-query)
11. [Constraint Validation API](#11-constraint-validation-api)
12. [Inference API](#12-inference-api)
13. [Extension Registration API](#13-extension-registration-api)
14. [Builder Helpers](#14-builder-helpers)
15. [Full Usage Example: Custom Type Hierarchy](#15-full-usage-example-custom-type-hierarchy)
16. [Full Usage Example: Custom Constraint Validator](#16-full-usage-example-custom-constraint-validator)
17. [Full Usage Example: Custom Inference Rule](#17-full-usage-example-custom-inference-rule)
18. [Full Usage Example: Transactional Workflow](#18-full-usage-example-transactional-workflow)
19. [Ergonomics Review](#19-ergonomics-review)
20. [Out of Scope](#20-out-of-scope)
21. [Design Decision Log](#21-design-decision-log)

---

## 1. Purpose and Scope

This document is the authoritative specification for the **public Rust API** of the embedded graph database crate. It defines how application code interacts with the database: creating and opening databases, managing transactions, manipulating the schema, performing CRUD operations on nodes and edges, traversing the graph, registering and invoking extensions (constraint validators and inference rules), and configuring persistence.

### What this document defines

- Every public type, method, and trait that application code uses
- The transaction model as exposed to callers
- Builder patterns for ergonomic construction
- Error types and their semantics
- Usage examples for every major operation

### What this document does NOT define

- Internal implementation details (buffer pool internals, B-tree operations, page management)
- The on-disk format (Task 8)
- The HAL trait layer (Task 9)
- Inference caching, invalidation, and dependency tracking (Task 11)

### Relationship to upstream documents

- **Task 6** defined the core types (`Node`, `Edge`, `Value`, `TypeDefinition`, etc.) and the extension traits (`ConstraintValidator`, `InferenceRule`). This document wraps those in ergonomic transaction-scoped APIs.
- **Task 7** defined the concurrency model (single-writer MVCC), transaction lifecycle, and buffer pool configuration. This document exposes those decisions as API semantics.

---

## 2. Design Principles

These principles guide every API design decision. When two concerns conflict, lower-numbered principles take precedence.

1. **Transactions are the unit of work.** All reads and writes happen within a transaction. There is no "auto-commit" mode. This makes the concurrency model explicit and prevents accidental data races.

2. **Borrowing over cloning.** Transaction handles borrow from the `Database`, preventing the database from being dropped while transactions are active. This is enforced by Rust lifetimes, not runtime checks.

3. **Fail early with clear errors.** Operations that can fail return `Result`. Error types carry enough context for the caller to understand what went wrong and where. Panics are reserved for programmer errors (e.g., using a transaction after it's been committed).

4. **Minimal surface, maximum composability.** The API provides primitive operations that compose well. Complex workflows (e.g., "insert a node with properties and connect it to another node") are composed from primitive calls, not special-cased methods.

5. **`no_std + alloc` compatible for types, `std` for the database engine.** All types in the public API (`Node`, `Edge`, `Value`, `TypeDefinition`, etc.) are `no_std + alloc` compatible. The `Database` struct and transaction types require `std` (they use `Mutex`, `RwLock`, file I/O). This split is expressed via feature flags.

6. **No method takes `&mut self` on the `Database` itself.** The `Database` is shared across threads via `Arc`. All mutation happens through write transactions, which hold the internal write lock. This makes the `Database` `Send + Sync` and shareable without external synchronization.

---

## 3. Crate Structure and Feature Flags

### 3.1 Module layout

```
graph_db/
├── lib.rs              // Re-exports; feature-gated top-level items
├── types/              // Core types: Node, Edge, Value, TypeDefinition, etc.
│   └── mod.rs          // All types from 006-schema-extension-spec.md
├── schema/             // TypeRegistryView, PropertyKeyRegistryView traits
├── constraint/         // ConstraintValidator trait, ChangeSet, ConstraintViolation
├── inference/          // InferenceRule trait, InferredFact, InferenceResult
├── error/              // Error types
├── db/                 // Database struct, transactions (std-only)
│   ├── config.rs       // DatabaseConfig, builder
│   ├── database.rs     // Database struct
│   ├── read_txn.rs     // ReadTransaction
│   └── write_txn.rs    // WriteTransaction
└── hal/                // HAL traits (Task 9)
```

### 3.2 Feature flags

```toml
[features]
default = ["std"]
std = []       # Enables Database, transactions, file I/O, std HAL backend
```

When `std` is disabled, only the `types`, `schema`, `constraint`, and `inference` modules are available. This allows downstream `no_std` crates to depend on the type definitions and trait interfaces without pulling in the database engine.

### 3.3 Re-exports at crate root

```rust
// Always available (no_std + alloc)
pub use types::{
    NodeId, EdgeId, TypeId, PropertyKeyId,
    Value, ValueTypeDescriptor,
    PropertyMap,
    Node, Edge,
    TypeKind, TypeDefinition, PropertyDeclaration,
};
pub use schema::{TypeRegistryView, PropertyKeyRegistryView};
pub use constraint::{
    ConstraintValidator, ConstraintViolation, ViolationSubject,
    ChangeSet, NodeChange, EdgeChange,
};
pub use inference::{
    InferenceRule, InferredFact, InferenceResult, InferenceMode,
};
pub use error::Error;

// std-only
#[cfg(feature = "std")]
pub use db::{
    Database, DatabaseConfig,
    ReadTransaction, WriteTransaction,
};
```

---

## 4. Error Handling

### 4.1 Top-level error type

```rust
use alloc::string::String;

/// The top-level error type for all database operations.
#[derive(Debug)]
pub enum Error {
    /// A schema/type registry operation failed.
    Schema(SchemaError),

    /// A constraint violation was detected at commit time.
    ConstraintViolation(Vec<ConstraintViolation>),

    /// A storage-layer error occurred (I/O failure, corruption, etc.).
    Storage(StorageError),

    /// The requested entity was not found.
    NotFound(NotFoundError),

    /// A transaction-level error (e.g., attempting to write in a read-only txn).
    Transaction(TransactionError),

    /// An inference rule produced an error.
    Inference(InferenceError),
}
```

### 4.2 Specific error types

```rust
/// Schema-related errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SchemaError {
    /// A type with this name already exists in the same TypeKind namespace.
    DuplicateTypeName { name: String, kind: TypeKind },

    /// The specified type ID does not exist.
    TypeNotFound(TypeId),

    /// Adding this supertype relationship would create a cycle.
    CycleDetected { child: TypeId, would_be_parent: TypeId },

    /// A referenced supertype does not exist.
    SupertypeNotFound(TypeId),

    /// Type kind mismatch (e.g., a node type listing an edge type as supertype).
    KindMismatch { expected: TypeKind, found: TypeKind },

    /// A property key with this name already exists with a different ID.
    DuplicatePropertyKey { name: String },

    /// The specified property key ID does not exist.
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

/// Storage-layer errors.
#[derive(Debug)]
pub struct StorageError {
    /// A human-readable description of the error.
    pub message: String,
    /// The underlying I/O error, if any.
    #[cfg(feature = "std")]
    pub source: Option<std::io::Error>,
}

/// Transaction-level errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransactionError {
    /// Attempted a write operation on a read-only transaction.
    ReadOnly,

    /// The transaction has already been committed or aborted.
    AlreadyFinished,

    /// Timed out waiting for the write lock.
    WriteLockTimeout,
}

/// Inference errors.
#[derive(Clone, Debug)]
pub enum InferenceError {
    /// The named inference rule is not registered.
    RuleNotFound(String),

    /// An inference rule produced an invalid fact (e.g., referencing
    /// a non-existent node).
    InvalidFact { rule_name: String, message: String },
}
```

### 4.3 Rationale

**Single top-level `Error` enum:** All public methods return `Result<T, Error>`. This lets callers use `?` uniformly. The variants are specific enough for pattern matching when callers need to distinguish error kinds.

**`ConstraintViolation` as an error variant:** When `commit()` fails because validators rejected the transaction, the caller receives all violations in one error. This lets the caller report all problems to the user, not just the first one.

**`StorageError` wraps `std::io::Error` conditionally:** The `source` field is only present with the `std` feature, keeping the error type usable in `no_std` contexts (where `String`-based messages are sufficient).

---

## 5. Database Lifecycle: Opening and Configuration

### 5.1 DatabaseConfig

```rust
use std::path::Path;

/// Configuration for opening or creating a database.
///
/// Use the builder pattern via `DatabaseConfig::persistent()` or
/// `DatabaseConfig::in_memory()`.
pub struct DatabaseConfig {
    /// The storage mode.
    mode: StorageMode,

    /// Buffer pool size in number of page frames.
    /// Default: 1024 (4 MB with 4 KB pages).
    /// Minimum: 64.
    buffer_pool_frames: usize,

    /// Page size in bytes. Default: 4096.
    /// Must be a power of two, minimum 4096.
    page_size: usize,

    /// Whether to run a startup check that compares registered
    /// extension names against the names persisted in the database.
    /// Default: true (warns on mismatch; does not fail).
    extension_startup_check: bool,
}

/// How the database stores data.
pub enum StorageMode {
    /// Persistent storage to a single file on disk.
    Persistent {
        /// Path to the database file. Created if it does not exist.
        path: std::path::PathBuf,
    },
    /// In-memory storage. Data is lost when the database is dropped
    /// unless explicitly snapshotted.
    InMemory,
}

impl DatabaseConfig {
    /// Create a configuration for a persistent database at the given path.
    ///
    /// # Example
    /// ```
    /// let config = DatabaseConfig::persistent("my_graph.db");
    /// ```
    pub fn persistent(path: impl AsRef<Path>) -> Self {
        Self {
            mode: StorageMode::Persistent {
                path: path.as_ref().to_path_buf(),
            },
            buffer_pool_frames: 1024,
            page_size: 4096,
            extension_startup_check: true,
        }
    }

    /// Create a configuration for an in-memory database.
    ///
    /// # Example
    /// ```
    /// let config = DatabaseConfig::in_memory();
    /// ```
    pub fn in_memory() -> Self {
        Self {
            mode: StorageMode::InMemory,
            buffer_pool_frames: 1024,
            page_size: 4096,
            extension_startup_check: false,
        }
    }

    /// Set the buffer pool size in page frames.
    /// Minimum: 64. Default: 1024.
    pub fn buffer_pool_frames(mut self, frames: usize) -> Self {
        self.buffer_pool_frames = frames.max(64);
        self
    }

    /// Set the page size in bytes. Must be a power of two, minimum 4096.
    /// Default: 4096.
    pub fn page_size(mut self, size: usize) -> Self {
        assert!(size >= 4096 && size.is_power_of_two(),
            "page_size must be a power of two and at least 4096");
        self.page_size = size;
        self
    }

    /// Enable or disable the extension startup check.
    pub fn extension_startup_check(mut self, enabled: bool) -> Self {
        self.extension_startup_check = enabled;
        self
    }
}
```

### 5.2 Database

```rust
use std::sync::Arc;

/// A handle to the graph database.
///
/// `Database` is `Send + Sync` and can be shared across threads
/// via `Arc<Database>`. All reads and writes happen through transactions
/// obtained from the database.
///
/// # Concurrency Model
///
/// - Unlimited concurrent read-only transactions.
/// - At most one write transaction at a time (others block until it completes).
/// - Read transactions see a consistent snapshot as of when they began.
/// - Write transactions operate on the latest committed state.
///
/// See `007-graph-storage-model.md` Sections 11–12 for the full
/// concurrency design.
pub struct Database { /* internal state */ }

// Database is Send + Sync — internal state is protected by Mutex/RwLock.
unsafe impl Send for Database {}
unsafe impl Sync for Database {}

impl Database {
    /// Open or create a database with the given configuration.
    ///
    /// If the database file exists, it is opened and validated.
    /// If it does not exist (persistent mode), it is created.
    ///
    /// # Errors
    /// - `Error::Storage` if the file cannot be opened/created or is corrupt.
    pub fn open(config: DatabaseConfig) -> Result<Self, Error> { ... }

    /// Begin a read-only transaction.
    ///
    /// The returned transaction sees a consistent snapshot of the database
    /// as it exists at this moment. Concurrent writes do not affect it.
    ///
    /// Read transactions are lightweight (no locks held beyond initial
    /// snapshot acquisition) and can be held for extended periods, though
    /// long-lived read transactions prevent garbage collection of old
    /// snapshots.
    pub fn read_txn(&self) -> Result<ReadTransaction<'_>, Error> { ... }

    /// Begin a read-write transaction.
    ///
    /// Blocks until the write lock is available (only one write
    /// transaction can be active at a time). The transaction operates
    /// on the latest committed state and sees its own writes
    /// (read-your-own-writes semantics).
    ///
    /// # Errors
    /// - `Error::Transaction(WriteLockTimeout)` if a timeout is configured
    ///   and the lock is not acquired in time.
    pub fn write_txn(&self) -> Result<WriteTransaction<'_>, Error> { ... }

    /// Register a constraint validator.
    ///
    /// The validator is immediately active for all future transaction
    /// commits. If a validator with the same name is already registered,
    /// it is replaced.
    ///
    /// Extension names are persisted in the database metadata so that
    /// the startup check can detect missing extensions.
    pub fn register_constraint(
        &self,
        validator: Box<dyn ConstraintValidator>,
    ) -> Result<(), Error> { ... }

    /// Unregister a constraint validator by name.
    /// Returns `true` if a validator with that name was found and removed.
    pub fn unregister_constraint(&self, name: &str) -> Result<bool, Error> { ... }

    /// Register an inference rule.
    ///
    /// The rule is immediately available for invocation. If a rule with
    /// the same name is already registered, it is replaced.
    pub fn register_inference_rule(
        &self,
        rule: Box<dyn InferenceRule>,
    ) -> Result<(), Error> { ... }

    /// Unregister an inference rule by name.
    /// Returns `true` if a rule with that name was found and removed.
    pub fn unregister_inference_rule(&self, name: &str) -> Result<bool, Error> { ... }

    /// Return the names of all registered constraint validators.
    pub fn constraint_names(&self) -> Vec<String> { ... }

    /// Return the names of all registered inference rules.
    pub fn inference_rule_names(&self) -> Vec<String> { ... }

    /// Return the names of extensions that were persisted in the database
    /// but are not currently registered. Empty if all are registered or
    /// if the database is new.
    pub fn missing_extensions(&self) -> MissingExtensions { ... }

    /// Snapshot the current state to a file (in-memory mode only).
    ///
    /// # Errors
    /// - `Error::Transaction(ReadOnly)` if the database is in persistent mode
    ///   (snapshotting is only for in-memory databases).
    /// - `Error::Storage` on I/O failure.
    #[cfg(feature = "std")]
    pub fn snapshot_to_file(&self, path: impl AsRef<Path>) -> Result<(), Error> { ... }

    /// Load state from a file (in-memory mode only).
    /// Replaces the current in-memory state.
    ///
    /// # Errors
    /// - Same as `snapshot_to_file`.
    #[cfg(feature = "std")]
    pub fn load_from_file(&self, path: impl AsRef<Path>) -> Result<(), Error> { ... }
}

/// Information about extensions that are persisted in the database
/// but not currently registered.
pub struct MissingExtensions {
    pub constraint_validators: Vec<String>,
    pub inference_rules: Vec<String>,
}

impl MissingExtensions {
    /// Returns true if there are no missing extensions.
    pub fn is_empty(&self) -> bool {
        self.constraint_validators.is_empty()
            && self.inference_rules.is_empty()
    }
}
```

### 5.3 Database shutdown

The `Database` implements `Drop`. On drop, it cleanly shuts down: releases all resources, flushes dirty pages (in persistent mode), and closes the file handle. If there are active transactions at drop time, the drop blocks until they complete (or, in a future version, aborts them — this is an implementation decision, not an API decision).

### 5.4 Rationale

**Extension registration on `Database`, not in transactions:** Constraint validators and inference rules are long-lived objects that span many transactions. Registering them inside a write transaction would be awkward — the user would need a write transaction just to set up their extensions, and the registration would need to be part of a commit. Instead, registration is a direct method on `Database`. Internally, the database takes a brief internal lock to update the extension registry. Extension names are persisted to the Schema Store B-tree as part of the next write transaction's commit.

**`read_txn` and `write_txn` return borrowed types:** The `'_` lifetime ties transactions to the `Database`, preventing the database from being dropped while transactions are live. This is a zero-cost safety guarantee enforced by the borrow checker.

**No `Arc<Database>` in the API signature:** The user is free to wrap `Database` in `Arc` for cross-thread sharing. The API does not force this — single-threaded users can use `&Database` directly. This keeps the API minimal.

---

## 6. Transaction API

### 6.1 ReadTransaction

```rust
/// A read-only transaction.
///
/// Provides a consistent snapshot of the database as of the moment
/// the transaction was created. All reads within this transaction
/// see the same state, regardless of concurrent writes.
///
/// Dropped automatically when it goes out of scope. Can also be
/// explicitly finished by calling `finish()`.
pub struct ReadTransaction<'db> { /* borrows Database */ }

impl<'db> ReadTransaction<'db> {
    // --- Node reads ---

    /// Look up a node by its ID.
    pub fn get_node(&self, id: NodeId) -> Result<Option<Node>, Error> { ... }

    /// Return all nodes in the database.
    /// Caution: may be expensive for large graphs.
    pub fn all_nodes(&self) -> Result<Vec<Node>, Error> { ... }

    // --- Edge reads ---

    /// Look up an edge by its ID.
    pub fn get_edge(&self, id: EdgeId) -> Result<Option<Edge>, Error> { ... }

    // --- Traversal ---

    /// Return all outgoing edges from a node.
    /// If `edge_type` is Some, only edges of that type are returned.
    pub fn outgoing_edges(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<Vec<Edge>, Error> { ... }

    /// Return all incoming edges to a node.
    /// If `edge_type` is Some, only edges of that type are returned.
    pub fn incoming_edges(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<Vec<Edge>, Error> { ... }

    /// Return the target nodes of all outgoing edges from a node.
    /// Convenience method equivalent to:
    ///   outgoing_edges(node, edge_type)
    ///     .map(|e| get_node(e.target))
    pub fn neighbors(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<Vec<Node>, Error> { ... }

    // --- Type-based queries ---

    /// Return all nodes with the given type label.
    /// If `include_subtypes` is true, also returns nodes whose type
    /// is a subtype of the given type.
    pub fn nodes_by_type(
        &self,
        type_id: TypeId,
        include_subtypes: bool,
    ) -> Result<Vec<Node>, Error> { ... }

    /// Return all edges with the given type label.
    pub fn edges_by_type(
        &self,
        type_id: TypeId,
        include_subtypes: bool,
    ) -> Result<Vec<Edge>, Error> { ... }

    // --- Property-based queries ---

    /// Find all nodes that have a specific property key-value pair.
    ///
    /// **Performance note:** In v1, this performs a full scan of the
    /// node store. It is adequate for constraint validation but may be
    /// slow for large graphs in hot-path queries.
    pub fn nodes_by_property(
        &self,
        key: PropertyKeyId,
        value: &Value,
    ) -> Result<Vec<Node>, Error> { ... }

    // --- Schema reads ---

    /// Access the type registry (read-only).
    pub fn type_registry(&self) -> &dyn TypeRegistryView { ... }

    /// Access the property key registry (read-only).
    pub fn property_key_registry(&self) -> &dyn PropertyKeyRegistryView { ... }

    // --- Inference ---

    /// Run a specific inference rule by name and return the results
    /// without materializing them.
    ///
    /// The rule sees the transaction's snapshot as its graph view.
    pub fn run_inference(
        &self,
        rule_name: &str,
    ) -> Result<InferenceResult, Error> { ... }

    /// Run all registered inference rules and return the combined results
    /// without materializing them.
    pub fn run_all_inference(&self) -> Result<Vec<InferenceResult>, Error> { ... }

    // --- Lifecycle ---

    /// Explicitly finish this transaction, releasing the snapshot.
    ///
    /// This is optional — the transaction is also finished on drop.
    /// Explicitly finishing is useful when the caller wants to release
    /// the snapshot earlier (to allow garbage collection of old pages).
    pub fn finish(self) { ... }
}
```

### 6.2 WriteTransaction

```rust
/// A read-write transaction.
///
/// Holds the exclusive write lock for the duration of its lifetime.
/// Provides read-your-own-writes semantics: reads within this
/// transaction see pending (uncommitted) changes.
///
/// Must be explicitly committed via `commit()`. If dropped without
/// committing, the transaction is automatically aborted (all pending
/// changes are discarded).
///
/// # Constraint Validation
///
/// On `commit()`, all registered constraint validators are executed.
/// If any validator returns violations, the commit is rejected and
/// the violations are returned as `Error::ConstraintViolation`.
/// The transaction remains open — the caller can fix the issues and
/// try committing again, or abort.
pub struct WriteTransaction<'db> { /* borrows Database */ }

impl<'db> WriteTransaction<'db> {
    // =========================================================
    // All read methods from ReadTransaction are also available.
    // They see the current snapshot overlaid with pending writes.
    // =========================================================

    /// Look up a node by its ID (sees pending changes).
    pub fn get_node(&self, id: NodeId) -> Result<Option<Node>, Error> { ... }

    pub fn get_edge(&self, id: EdgeId) -> Result<Option<Edge>, Error> { ... }

    pub fn outgoing_edges(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<Vec<Edge>, Error> { ... }

    pub fn incoming_edges(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<Vec<Edge>, Error> { ... }

    pub fn neighbors(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<Vec<Node>, Error> { ... }

    pub fn nodes_by_type(
        &self,
        type_id: TypeId,
        include_subtypes: bool,
    ) -> Result<Vec<Node>, Error> { ... }

    pub fn edges_by_type(
        &self,
        type_id: TypeId,
        include_subtypes: bool,
    ) -> Result<Vec<Edge>, Error> { ... }

    pub fn nodes_by_property(
        &self,
        key: PropertyKeyId,
        value: &Value,
    ) -> Result<Vec<Node>, Error> { ... }

    pub fn type_registry(&self) -> &dyn TypeRegistryView { ... }

    pub fn property_key_registry(&self) -> &dyn PropertyKeyRegistryView { ... }

    // =========================================================
    // Schema mutation operations
    // =========================================================

    /// Register a new type definition.
    ///
    /// The type is assigned a `TypeId` by the database. The `id` field
    /// on the input `TypeDefinition` is ignored — use `TypeId(0)` as
    /// a placeholder.
    ///
    /// # Errors
    /// - `Error::Schema(DuplicateTypeName)` if a type with the same
    ///   name and kind already exists.
    /// - `Error::Schema(SupertypeNotFound)` if any listed supertype
    ///   does not exist.
    /// - `Error::Schema(KindMismatch)` if a supertype is of a different kind.
    /// - `Error::Schema(CycleDetected)` if adding the supertypes would
    ///   create a cycle.
    pub fn register_type(
        &mut self,
        definition: TypeDefinition,
    ) -> Result<TypeId, Error> { ... }

    /// Update an existing type definition.
    ///
    /// Replaces the type definition for the given `TypeId`. The `id`
    /// field in the input must match the target `TypeId`.
    ///
    /// **Note:** Modifying a type does not automatically revalidate
    /// existing data. Callers should run `validate_all()` after schema
    /// changes to detect newly-introduced violations.
    ///
    /// # Errors
    /// - Same schema errors as `register_type`.
    /// - `Error::NotFound(Type)` if the type does not exist.
    pub fn update_type(
        &mut self,
        definition: TypeDefinition,
    ) -> Result<(), Error> { ... }

    /// Add a supertype relationship: `child` becomes a subtype of `parent`.
    ///
    /// Convenience method equivalent to modifying the child's `supertypes`
    /// list via `update_type`.
    ///
    /// # Errors
    /// - `Error::Schema(CycleDetected)` if this would create a cycle.
    /// - `Error::NotFound(Type)` if either type does not exist.
    /// - `Error::Schema(KindMismatch)` if the types are of different kinds.
    pub fn add_supertype(
        &mut self,
        child: TypeId,
        parent: TypeId,
    ) -> Result<(), Error> { ... }

    /// Remove a supertype relationship.
    /// Returns `true` if the relationship existed and was removed.
    pub fn remove_supertype(
        &mut self,
        child: TypeId,
        parent: TypeId,
    ) -> Result<bool, Error> { ... }

    /// Register or get a property key by name.
    ///
    /// If a property key with this name already exists, returns its ID.
    /// Otherwise, registers a new key and returns the new ID.
    /// This is idempotent — calling with the same name always returns
    /// the same ID.
    pub fn get_or_create_property_key(
        &mut self,
        name: &str,
    ) -> Result<PropertyKeyId, Error> { ... }

    /// Look up a property key ID by name. Returns `None` if unregistered.
    /// Does not create the key.
    pub fn get_property_key(&self, name: &str) -> Option<PropertyKeyId> { ... }

    /// Look up a property key name by ID. Returns `None` if unregistered.
    pub fn get_property_key_name(&self, id: PropertyKeyId) -> Option<&str> { ... }

    // =========================================================
    // Node mutation operations
    // =========================================================

    /// Insert a new node into the graph.
    ///
    /// The node is assigned a new `NodeId` by the database. The `id`
    /// field on the input `Node` is ignored — use `NodeId(0)` as a
    /// placeholder, or use `NodeBuilder` (Section 14) for convenience.
    ///
    /// Returns the assigned `NodeId`.
    pub fn insert_node(&mut self, node: Node) -> Result<NodeId, Error> { ... }

    /// Update an existing node's properties and/or type labels.
    ///
    /// The `id` field identifies which node to update. All other fields
    /// are replaced wholesale.
    ///
    /// # Errors
    /// - `Error::NotFound(Node)` if the node does not exist.
    pub fn update_node(&mut self, node: Node) -> Result<(), Error> { ... }

    /// Delete a node by its ID.
    ///
    /// All edges incident to this node (incoming and outgoing) are
    /// also deleted. This is a cascading delete — the caller does not
    /// need to remove edges manually.
    ///
    /// # Errors
    /// - `Error::NotFound(Node)` if the node does not exist.
    pub fn delete_node(&mut self, id: NodeId) -> Result<(), Error> { ... }

    /// Set a single property on an existing node.
    /// If the property already exists, its value is replaced.
    /// If `value` is `Value::Null`, the property is removed.
    ///
    /// Convenience method that avoids a full `update_node` round-trip.
    pub fn set_node_property(
        &mut self,
        node_id: NodeId,
        key: PropertyKeyId,
        value: Value,
    ) -> Result<(), Error> { ... }

    /// Remove a single property from an existing node.
    /// Returns the old value, or `None` if the property was not present.
    pub fn remove_node_property(
        &mut self,
        node_id: NodeId,
        key: PropertyKeyId,
    ) -> Result<Option<Value>, Error> { ... }

    /// Add a type label to an existing node.
    /// No-op if the node already has this type.
    pub fn add_node_type(
        &mut self,
        node_id: NodeId,
        type_id: TypeId,
    ) -> Result<(), Error> { ... }

    /// Remove a type label from an existing node.
    /// Returns `true` if the type was present and removed.
    pub fn remove_node_type(
        &mut self,
        node_id: NodeId,
        type_id: TypeId,
    ) -> Result<bool, Error> { ... }

    // =========================================================
    // Edge mutation operations
    // =========================================================

    /// Insert a new edge into the graph.
    ///
    /// The edge is assigned a new `EdgeId` by the database. The `id`
    /// field on the input `Edge` is ignored.
    ///
    /// # Errors
    /// - `Error::NotFound(Node)` if the source or target node does not exist.
    pub fn insert_edge(&mut self, edge: Edge) -> Result<EdgeId, Error> { ... }

    /// Update an existing edge's properties and/or type labels.
    ///
    /// The `id` field identifies which edge to update. The `source` and
    /// `target` fields cannot be changed (an edge's endpoints are immutable
    /// once created; delete and re-create to change endpoints).
    ///
    /// # Errors
    /// - `Error::NotFound(Edge)` if the edge does not exist.
    pub fn update_edge(&mut self, edge: Edge) -> Result<(), Error> { ... }

    /// Delete an edge by its ID.
    ///
    /// # Errors
    /// - `Error::NotFound(Edge)` if the edge does not exist.
    pub fn delete_edge(&mut self, id: EdgeId) -> Result<(), Error> { ... }

    /// Set a single property on an existing edge.
    pub fn set_edge_property(
        &mut self,
        edge_id: EdgeId,
        key: PropertyKeyId,
        value: Value,
    ) -> Result<(), Error> { ... }

    /// Remove a single property from an existing edge.
    pub fn remove_edge_property(
        &mut self,
        edge_id: EdgeId,
        key: PropertyKeyId,
    ) -> Result<Option<Value>, Error> { ... }

    /// Add a type label to an existing edge.
    pub fn add_edge_type(
        &mut self,
        edge_id: EdgeId,
        type_id: TypeId,
    ) -> Result<(), Error> { ... }

    /// Remove a type label from an existing edge.
    pub fn remove_edge_type(
        &mut self,
        edge_id: EdgeId,
        type_id: TypeId,
    ) -> Result<bool, Error> { ... }

    // =========================================================
    // Inference operations (within a write transaction)
    // =========================================================

    /// Run a specific inference rule and optionally materialize results.
    ///
    /// - `InferenceMode::Ephemeral`: returns results without writing them.
    /// - `InferenceMode::Materialized`: writes inferred facts to the graph
    ///   as part of this transaction. New nodes/edges receive new IDs.
    ///
    /// Materialized facts are included in the ChangeSet and are subject
    /// to constraint validation at commit time.
    pub fn run_inference(
        &mut self,
        rule_name: &str,
        mode: InferenceMode,
    ) -> Result<InferenceResult, Error> { ... }

    /// Run all registered inference rules.
    pub fn run_all_inference(
        &mut self,
        mode: InferenceMode,
    ) -> Result<Vec<InferenceResult>, Error> { ... }

    // =========================================================
    // Validation
    // =========================================================

    /// Run all registered constraint validators against the current
    /// pending changes without committing.
    ///
    /// Returns an empty Vec if all constraints pass.
    ///
    /// This is a dry-run validation — useful for checking constraints
    /// before committing, or for validating the entire database after
    /// a schema change.
    pub fn validate(&self) -> Result<Vec<ConstraintViolation>, Error> { ... }

    /// Run all registered constraint validators against the entire
    /// database (not just pending changes).
    ///
    /// Produces a synthetic ChangeSet treating every node and edge
    /// as newly inserted, then runs all validators. Useful for
    /// revalidation after schema changes.
    pub fn validate_all(&self) -> Result<Vec<ConstraintViolation>, Error> { ... }

    // =========================================================
    // Transaction lifecycle
    // =========================================================

    /// Commit this transaction.
    ///
    /// 1. Builds a `ChangeSet` from all pending mutations.
    /// 2. Runs all registered constraint validators.
    /// 3. If any validator returns violations, the commit is rejected:
    ///    the pending changes remain in the transaction's buffer and
    ///    `Error::ConstraintViolation` is returned. The transaction
    ///    remains open — the caller can fix issues and retry.
    /// 4. If all validators pass, the changes are persisted atomically.
    ///
    /// # Errors
    /// - `Error::ConstraintViolation` if validation fails.
    /// - `Error::Storage` on I/O failure.
    pub fn commit(self) -> Result<(), Error> { ... }

    /// Abort this transaction, discarding all pending changes.
    ///
    /// This is also called automatically if the transaction is dropped
    /// without being committed.
    pub fn abort(self) { ... }
}
```

### 6.3 InferenceMode

```rust
/// How inferred facts should be handled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InferenceMode {
    /// Inferred facts are returned but not written to the graph.
    Ephemeral,

    /// Inferred facts are written to the graph as part of the
    /// current transaction.
    Materialized,
}
```

### 6.4 Design rationale

**`commit(self)` consumes the transaction:** After a successful commit, the write lock is released and the transaction is finished. Consuming `self` prevents accidentally using a committed transaction. The caller cannot call `commit()` twice.

**Failed commit does not consume the transaction:** The original design had `commit(self)` always consuming, but this prevents retry after a constraint violation. Instead, we use `commit(self)` that consumes on success. On `Error::ConstraintViolation`, the caller holds a reference to the error and the transaction has been consumed — they must open a new transaction. **Revised decision:** The transaction **is** consumed on commit failure. The rationale is simplicity: after a failed commit, the pending state is ambiguous (should the caller be expected to "fix" individual mutations?). Starting a fresh transaction is clearer. The constraint violations in the error give the caller enough information to build the corrected transaction.

**`WriteTransaction` takes `&mut self` for mutations:** This prevents aliasing the transaction across threads (Rust's borrow rules enforce exclusive access for `&mut`). Since there's only one write transaction at a time, `&mut` is always available to the holder.

**Read methods duplicated on `WriteTransaction`:** Rust does not support trait inheritance in a way that lets `WriteTransaction` "extend" `ReadTransaction` while having different internal state. The read methods on `WriteTransaction` use the overlay (base snapshot + pending changes), while `ReadTransaction`'s read methods use only the snapshot. Duplication is the most straightforward approach. A shared `GraphReader` trait (Section 10) mitigates the ergonomic cost.

---

## 7. Schema Operations: Types and Property Keys

Schema operations are methods on `WriteTransaction` (for mutations) and both transaction types (for reads). The full signatures are in Section 6. This section provides additional context.

### 7.1 Type registration workflow

A typical type registration workflow:

```rust
let db = Database::open(DatabaseConfig::in_memory())?;
let mut txn = db.write_txn()?;

// Register property keys first
let name_key = txn.get_or_create_property_key("name")?;
let age_key = txn.get_or_create_property_key("age")?;

// Register a node type
let person_type_id = txn.register_type(TypeDefinition {
    id: TypeId(0), // placeholder; assigned by database
    name: "Person".into(),
    kind: TypeKind::Node,
    supertypes: vec![],
    property_declarations: vec![
        PropertyDeclaration {
            key: name_key,
            value_type: ValueTypeDescriptor::String,
            required: true,
            multi_valued: false,
            metadata: PropertyMap::new(),
        },
        PropertyDeclaration {
            key: age_key,
            value_type: ValueTypeDescriptor::U64,
            required: false,
            multi_valued: false,
            metadata: PropertyMap::new(),
        },
    ],
    open: true,
    metadata: PropertyMap::new(),
})?;

txn.commit()?;
```

### 7.2 Property key lifecycle

Property keys are interned strings. Once created, a key ID never changes or is recycled. The `get_or_create_property_key` method is the primary way to obtain a key ID — it creates on first use, returns the existing ID on subsequent calls. This idempotent behavior simplifies setup code: callers don't need to check whether a key already exists.

The read-only `get_property_key` method (no creation) is available on both transaction types for cases where the caller only wants to query without side effects.

### 7.3 Schema reads via TypeRegistryView

Both `ReadTransaction` and `WriteTransaction` expose a `type_registry()` method returning `&dyn TypeRegistryView`. This provides all the type hierarchy query methods defined in `006-schema-extension-spec.md` Section 7.6: `get_type`, `get_type_by_name`, `all_types`, `types_by_kind`, `direct_supertypes`, `all_supertypes`, `direct_subtypes`, `all_subtypes`, `is_subtype_of`, and `effective_property_declarations`.

---

## 8. Node Operations

Node operations are methods on `WriteTransaction` (for mutations) and both transaction types (for reads). The full signatures are in Section 6.

### 8.1 Node creation pattern

```rust
let mut txn = db.write_txn()?;

// Method 1: Direct struct construction
let node_id = txn.insert_node(Node {
    id: NodeId(0), // placeholder
    type_labels: vec![person_type_id],
    properties: {
        let mut props = PropertyMap::new();
        props.insert(name_key, Value::String("Alice".into()));
        props.insert(age_key, Value::U64(30));
        props
    },
    is_anonymous: false,
})?;

// Method 2: Using the builder (Section 14)
let node_id = txn.insert_node(
    NodeBuilder::new()
        .type_label(person_type_id)
        .property(name_key, Value::String("Bob".into()))
        .property(age_key, Value::U64(25))
        .build()
)?;
```

### 8.2 Cascading delete semantics

When a node is deleted, all incident edges (both incoming and outgoing) are automatically deleted. This is because edges reference nodes by `NodeId`, and a dangling edge reference would be a data integrity violation. The cascading deletes are part of the same transaction and appear in the `ChangeSet` as individual edge deletions.

### 8.3 Partial updates

The `set_node_property`, `remove_node_property`, `add_node_type`, and `remove_node_type` methods provide fine-grained updates without requiring the caller to read the entire node, modify it, and write it back. This is both an ergonomic and performance improvement — the implementation can produce a more targeted `ChangeSet` entry.

---

## 9. Edge Operations

### 9.1 Edge creation pattern

```rust
let mut txn = db.write_txn()?;

// Register an edge type
let knows_type = txn.register_type(TypeDefinition {
    id: TypeId(0),
    name: "knows".into(),
    kind: TypeKind::Edge,
    supertypes: vec![],
    property_declarations: vec![],
    open: true,
    metadata: PropertyMap::new(),
})?;

// Create nodes
let alice = txn.insert_node(
    NodeBuilder::new().type_label(person_type_id)
        .property(name_key, Value::String("Alice".into()))
        .build()
)?;
let bob = txn.insert_node(
    NodeBuilder::new().type_label(person_type_id)
        .property(name_key, Value::String("Bob".into()))
        .build()
)?;

// Create an edge
let edge_id = txn.insert_edge(Edge {
    id: EdgeId(0), // placeholder
    type_labels: vec![knows_type],
    source: alice,
    target: bob,
    properties: PropertyMap::new(),
})?;

txn.commit()?;
```

### 9.2 Immutable endpoints

An edge's `source` and `target` are fixed at creation time and cannot be changed via `update_edge`. If the caller passes different source/target values in `update_edge`, they are ignored (only properties and type labels are updated). This is a deliberate design choice: changing an edge's endpoints is semantically equivalent to deleting the old edge and creating a new one, and the API should make that explicit rather than hiding it behind an "update."

### 9.3 Parallel edges

Multiple edges between the same source and target (with the same or different types) are permitted. This is required for multi-graph semantics (e.g., RDF allows multiple triples between the same subject and object with different predicates).

---

## 10. Graph Traversal and Query

### 10.1 GraphReader trait

To avoid duplicating all read method signatures between `ReadTransaction` and `WriteTransaction`, the API provides a trait that both implement:

```rust
/// A trait for read-only access to the graph.
///
/// Both `ReadTransaction` and `WriteTransaction` implement this trait.
/// Generic code that only needs read access can accept `&dyn GraphReader`.
pub trait GraphReader {
    /// Look up a node by its ID.
    fn get_node(&self, id: NodeId) -> Result<Option<Node>, Error>;

    /// Look up an edge by its ID.
    fn get_edge(&self, id: EdgeId) -> Result<Option<Edge>, Error>;

    /// Return all outgoing edges from a node.
    fn outgoing_edges(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<Vec<Edge>, Error>;

    /// Return all incoming edges to a node.
    fn incoming_edges(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<Vec<Edge>, Error>;

    /// Return the target nodes of outgoing edges.
    fn neighbors(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<Vec<Node>, Error>;

    /// Return all nodes with the given type label.
    fn nodes_by_type(
        &self,
        type_id: TypeId,
        include_subtypes: bool,
    ) -> Result<Vec<Node>, Error>;

    /// Return all edges with the given type label.
    fn edges_by_type(
        &self,
        type_id: TypeId,
        include_subtypes: bool,
    ) -> Result<Vec<Edge>, Error>;

    /// Find nodes by a property key-value pair.
    fn nodes_by_property(
        &self,
        key: PropertyKeyId,
        value: &Value,
    ) -> Result<Vec<Node>, Error>;

    /// Access the type registry.
    fn type_registry(&self) -> &dyn TypeRegistryView;

    /// Access the property key registry.
    fn property_key_registry(&self) -> &dyn PropertyKeyRegistryView;
}
```

### 10.2 Relationship to GraphView

The `GraphView` trait from `006-schema-extension-spec.md` (Section 10.3) is the internal interface provided to constraint validators and inference rules. `GraphReader` is the public API equivalent. The differences:

| Aspect | `GraphView` (internal) | `GraphReader` (public) |
|--------|----------------------|----------------------|
| Returns | `&Node`, `&Edge` (borrowed) | `Node`, `Edge` (owned) |
| Error handling | Infallible (panics on storage errors) | Returns `Result<_, Error>` |
| Thread safety | Not required (`&dyn GraphView` is single-thread) | Not required on the trait; `ReadTransaction` and `WriteTransaction` are `!Send` |
| Users | Constraint validators, inference rules | Application code |

**Rationale for owned returns:** The public API returns owned `Node` and `Edge` values because the transaction's internal buffer pool may evict pages at any time. Returning borrowed references would require the caller to hold pins into the buffer pool, which leaks internal implementation concerns. Owned values are simpler and safer, at the cost of cloning. For most use cases (small property bags), this cost is negligible.

### 10.3 Multi-hop traversal patterns

The API provides primitive operations (one-hop neighbors) that compose into multi-hop traversals:

```rust
// Find all 2-hop neighbors: friends of friends
let txn = db.read_txn()?;

let alice_friends = txn.neighbors(alice_id, Some(knows_type))?;
let mut friends_of_friends = Vec::new();
for friend in &alice_friends {
    let fof = txn.neighbors(friend.id, Some(knows_type))?;
    friends_of_friends.extend(fof);
}
```

The API deliberately does not provide a built-in multi-hop traversal method. Multi-hop traversals have too many variations (depth limits, filtering predicates, cycle detection, aggregation) to capture in a single method signature. Application code composes single-hop operations, which is both more flexible and more transparent about performance characteristics.

### 10.4 Counting

For cases where the caller only needs counts, not full records:

```rust
impl<'db> ReadTransaction<'db> {
    /// Count nodes in the database.
    pub fn node_count(&self) -> Result<u64, Error> { ... }

    /// Count edges in the database.
    pub fn edge_count(&self) -> Result<u64, Error> { ... }

    /// Count outgoing edges from a node (optionally filtered by type).
    pub fn outgoing_edge_count(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<u64, Error> { ... }

    /// Count incoming edges to a node (optionally filtered by type).
    pub fn incoming_edge_count(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<u64, Error> { ... }
}
```

These counting methods avoid materializing full records and can be implemented as B-tree key-only scans, which are faster than fetching and deserializing full records.

---

## 11. Constraint Validation API

### 11.1 Registration

Constraint validators are registered on the `Database` (Section 5.2). This section covers the constraint-related types that application code interacts with.

### 11.2 Implementing a constraint validator

The `ConstraintValidator` trait is defined in `006-schema-extension-spec.md` Section 10.5 and re-exported from the crate root. Application code implements this trait:

```rust
use graph_db::{
    ConstraintValidator, ConstraintViolation, ViolationSubject,
    ChangeSet, GraphView, TypeRegistryView, PropertyKeyRegistryView,
    TypeId, NodeChange,
};

struct RequiredPropertyValidator {
    target_type: TypeId,
    required_key: PropertyKeyId,
    property_name: String,
}

impl ConstraintValidator for RequiredPropertyValidator {
    fn name(&self) -> &str {
        "RequiredPropertyValidator"
    }

    fn applies_to_types(&self) -> Option<Vec<TypeId>> {
        Some(vec![self.target_type])
    }

    fn validate(
        &self,
        changes: &ChangeSet<'_>,
        graph: &dyn GraphView,
        types: &dyn TypeRegistryView,
        keys: &dyn PropertyKeyRegistryView,
    ) -> Vec<ConstraintViolation> {
        let mut violations = Vec::new();

        for node in changes.inserted_nodes() {
            if node.type_labels.contains(&self.target_type)
                && !node.properties.contains_key(&self.required_key)
            {
                violations.push(ConstraintViolation {
                    violation_kind: "REQUIRED_PROPERTY_MISSING".into(),
                    message: format!(
                        "Node {:?} of type {:?} is missing required property '{}'",
                        node.id, self.target_type, self.property_name
                    ),
                    subject: Some(ViolationSubject::Node(node.id)),
                });
            }
        }

        for (before, after) in changes.modified_nodes() {
            if after.type_labels.contains(&self.target_type)
                && !after.properties.contains_key(&self.required_key)
            {
                violations.push(ConstraintViolation {
                    violation_kind: "REQUIRED_PROPERTY_MISSING".into(),
                    message: format!(
                        "Node {:?} of type {:?} lost required property '{}'",
                        after.id, self.target_type, self.property_name
                    ),
                    subject: Some(ViolationSubject::Node(after.id)),
                });
            }
        }

        violations
    }
}
```

### 11.3 Commit-time validation flow

The validation flow is transparent to the caller:

```rust
let mut txn = db.write_txn()?;
txn.insert_node(/* ... missing required property ... */)?;

match txn.commit() {
    Ok(()) => println!("Committed successfully"),
    Err(Error::ConstraintViolation(violations)) => {
        for v in &violations {
            eprintln!("Violation: {} — {}", v.violation_kind, v.message);
        }
    }
    Err(e) => return Err(e),
}
```

### 11.4 Dry-run validation

```rust
let mut txn = db.write_txn()?;
txn.insert_node(/* ... */)?;

// Check constraints without committing
let violations = txn.validate()?;
if !violations.is_empty() {
    // Fix issues or abort
    txn.abort();
} else {
    txn.commit()?;
}
```

### 11.5 Full-database revalidation

```rust
let txn = db.write_txn()?;
// After a schema change, revalidate all existing data
let violations = txn.validate_all()?;
if !violations.is_empty() {
    eprintln!("Existing data has {} violations after schema change",
              violations.len());
}
txn.abort(); // or commit, depending on workflow
```

---

## 12. Inference API

### 12.1 Registration

Inference rules are registered on the `Database` (Section 5.2), following the same pattern as constraint validators.

### 12.2 Running inference in a read transaction

Inference in a read transaction is always ephemeral (no writes possible):

```rust
let txn = db.read_txn()?;
let result = txn.run_inference("TransitiveClosureRule")?;
for fact in &result.facts {
    match fact {
        InferredFact::NewEdge { source, target, .. } => {
            println!("Inferred edge: {:?} -> {:?}", source, target);
        }
        _ => {}
    }
}
```

### 12.3 Running inference in a write transaction

In a write transaction, the caller chooses whether to materialize:

```rust
let mut txn = db.write_txn()?;

// Ephemeral: inspect results without writing
let result = txn.run_inference("SubclassPropagation", InferenceMode::Ephemeral)?;
println!("Would produce {} facts", result.facts.len());

// Materialized: write inferred facts into the graph
let result = txn.run_inference("SubclassPropagation", InferenceMode::Materialized)?;
println!("Materialized {} facts", result.facts.len());

// Materialized facts are subject to constraint validation at commit
txn.commit()?;
```

### 12.4 Implementing an inference rule

```rust
use graph_db::{
    InferenceRule, InferenceResult, InferredFact,
    GraphView, TypeRegistryView, PropertyKeyRegistryView,
    TypeId, EdgeId,
};

/// Infers inverse edges: for every `knows` edge A→B,
/// infers a `known_by` edge B→A.
struct InverseEdgeRule {
    source_edge_type: TypeId,
    inverse_edge_type: TypeId,
}

impl InferenceRule for InverseEdgeRule {
    fn name(&self) -> &str {
        "InverseEdgeRule"
    }

    fn applies_to_types(&self) -> Option<Vec<TypeId>> {
        Some(vec![self.source_edge_type])
    }

    fn infer(
        &self,
        graph: &dyn GraphView,
        _types: &dyn TypeRegistryView,
        _keys: &dyn PropertyKeyRegistryView,
    ) -> InferenceResult {
        let mut facts = Vec::new();

        for edge in graph.edges_by_type(self.source_edge_type, false) {
            // Check if the inverse already exists
            let existing = graph.outgoing_edges(edge.target, Some(self.inverse_edge_type));
            let already_exists = existing.iter().any(|e| e.target == edge.source);

            if !already_exists {
                facts.push(InferredFact::NewEdge {
                    type_labels: vec![self.inverse_edge_type],
                    source: edge.target,
                    target: edge.source,
                    properties: PropertyMap::new(),
                });
            }
        }

        InferenceResult {
            facts,
            rule_name: self.name().into(),
        }
    }
}
```

---

## 13. Extension Registration API

### 13.1 Registration pattern

The typical pattern for setting up extensions at application startup:

```rust
let db = Database::open(DatabaseConfig::persistent("my.db"))?;

// Register constraint validators
db.register_constraint(Box::new(RequiredPropertyValidator {
    target_type: person_type_id,
    required_key: name_key,
    property_name: "name".into(),
}))?;

// Register inference rules
db.register_inference_rule(Box::new(InverseEdgeRule {
    source_edge_type: knows_type_id,
    inverse_edge_type: known_by_type_id,
}))?;

// Check for missing extensions (from a previous session)
let missing = db.missing_extensions();
if !missing.is_empty() {
    eprintln!("Warning: previously registered extensions are missing:");
    for name in &missing.constraint_validators {
        eprintln!("  Constraint: {}", name);
    }
    for name in &missing.inference_rules {
        eprintln!("  Inference rule: {}", name);
    }
}
```

### 13.2 Extension lifecycle across sessions

1. Application opens database and registers extensions.
2. Extension names are recorded in the Schema Store B-tree (persisted on next commit).
3. Application closes database.
4. Application reopens database and re-registers extensions.
5. If any previously-persisted names are not re-registered, `missing_extensions()` reports them.

The database does not fail on missing extensions — it warns. This allows graceful handling of extension version changes (e.g., an extension was renamed or removed).

### 13.3 Unregistration

```rust
// Unregister a constraint validator
let was_removed = db.unregister_constraint("RequiredPropertyValidator")?;
assert!(was_removed);

// Unregister an inference rule
let was_removed = db.unregister_inference_rule("InverseEdgeRule")?;
assert!(was_removed);
```

---

## 14. Builder Helpers

### 14.1 NodeBuilder

```rust
/// A convenience builder for constructing `Node` values.
///
/// Avoids the need to manually construct `PropertyMap`s and set
/// the placeholder ID.
pub struct NodeBuilder {
    type_labels: Vec<TypeId>,
    properties: PropertyMap,
    is_anonymous: bool,
}

impl NodeBuilder {
    /// Create a new empty node builder.
    pub fn new() -> Self {
        Self {
            type_labels: Vec::new(),
            properties: PropertyMap::new(),
            is_anonymous: false,
        }
    }

    /// Add a type label.
    pub fn type_label(mut self, type_id: TypeId) -> Self {
        if !self.type_labels.contains(&type_id) {
            self.type_labels.push(type_id);
            self.type_labels.sort();
        }
        self
    }

    /// Add multiple type labels.
    pub fn type_labels(mut self, ids: impl IntoIterator<Item = TypeId>) -> Self {
        for id in ids {
            if !self.type_labels.contains(&id) {
                self.type_labels.push(id);
            }
        }
        self.type_labels.sort();
        self
    }

    /// Set a property.
    pub fn property(mut self, key: PropertyKeyId, value: Value) -> Self {
        self.properties.insert(key, value);
        self
    }

    /// Mark this node as anonymous.
    pub fn anonymous(mut self, is_anonymous: bool) -> Self {
        self.is_anonymous = is_anonymous;
        self
    }

    /// Build the `Node`. The `id` is set to `NodeId(0)` — the
    /// database will assign a real ID on insert.
    pub fn build(self) -> Node {
        Node {
            id: NodeId(0),
            type_labels: self.type_labels,
            properties: self.properties,
            is_anonymous: self.is_anonymous,
        }
    }
}

impl Default for NodeBuilder {
    fn default() -> Self { Self::new() }
}
```

### 14.2 EdgeBuilder

```rust
/// A convenience builder for constructing `Edge` values.
pub struct EdgeBuilder {
    type_labels: Vec<TypeId>,
    source: NodeId,
    target: NodeId,
    properties: PropertyMap,
}

impl EdgeBuilder {
    /// Create a new edge builder with the given source and target.
    pub fn new(source: NodeId, target: NodeId) -> Self {
        Self {
            type_labels: Vec::new(),
            source,
            target,
            properties: PropertyMap::new(),
        }
    }

    /// Add a type label.
    pub fn type_label(mut self, type_id: TypeId) -> Self {
        if !self.type_labels.contains(&type_id) {
            self.type_labels.push(type_id);
            self.type_labels.sort();
        }
        self
    }

    /// Set a property.
    pub fn property(mut self, key: PropertyKeyId, value: Value) -> Self {
        self.properties.insert(key, value);
        self
    }

    /// Build the `Edge`. The `id` is set to `EdgeId(0)`.
    pub fn build(self) -> Edge {
        Edge {
            id: EdgeId(0),
            type_labels: self.type_labels,
            source: self.source,
            target: self.target,
            properties: self.properties,
        }
    }
}
```

### 14.3 TypeDefinitionBuilder

```rust
/// A convenience builder for constructing `TypeDefinition` values.
pub struct TypeDefinitionBuilder {
    name: String,
    kind: TypeKind,
    supertypes: Vec<TypeId>,
    property_declarations: Vec<PropertyDeclaration>,
    open: bool,
    metadata: PropertyMap,
}

impl TypeDefinitionBuilder {
    /// Create a new type definition builder.
    pub fn new(name: impl Into<String>, kind: TypeKind) -> Self {
        Self {
            name: name.into(),
            kind,
            supertypes: Vec::new(),
            property_declarations: Vec::new(),
            open: true,
            metadata: PropertyMap::new(),
        }
    }

    /// Shorthand for a node type builder.
    pub fn node_type(name: impl Into<String>) -> Self {
        Self::new(name, TypeKind::Node)
    }

    /// Shorthand for an edge type builder.
    pub fn edge_type(name: impl Into<String>) -> Self {
        Self::new(name, TypeKind::Edge)
    }

    /// Add a supertype.
    pub fn supertype(mut self, parent: TypeId) -> Self {
        self.supertypes.push(parent);
        self
    }

    /// Add a property declaration.
    pub fn property(
        mut self,
        key: PropertyKeyId,
        value_type: ValueTypeDescriptor,
        required: bool,
    ) -> Self {
        self.property_declarations.push(PropertyDeclaration {
            key,
            value_type,
            required,
            multi_valued: false,
            metadata: PropertyMap::new(),
        });
        self
    }

    /// Add a property declaration with full control.
    pub fn property_declaration(mut self, decl: PropertyDeclaration) -> Self {
        self.property_declarations.push(decl);
        self
    }

    /// Mark this type as closed (instances must conform exactly to
    /// the declared schema).
    pub fn closed(mut self) -> Self {
        self.open = false;
        self
    }

    /// Add metadata to the type definition.
    pub fn metadata_entry(mut self, key: PropertyKeyId, value: Value) -> Self {
        self.metadata.insert(key, value);
        self
    }

    /// Build the `TypeDefinition`. The `id` is set to `TypeId(0)`.
    pub fn build(self) -> TypeDefinition {
        TypeDefinition {
            id: TypeId(0),
            name: self.name,
            kind: self.kind,
            supertypes: self.supertypes,
            property_declarations: self.property_declarations,
            open: self.open,
            metadata: self.metadata,
        }
    }
}
```

---

## 15. Full Usage Example: Custom Type Hierarchy

This example demonstrates registering a type hierarchy with inheritance and property declarations.

```rust
use graph_db::*;

fn setup_type_hierarchy() -> Result<(), Error> {
    let db = Database::open(DatabaseConfig::in_memory())?;
    let mut txn = db.write_txn()?;

    // Register property keys
    let name_key = txn.get_or_create_property_key("name")?;
    let age_key = txn.get_or_create_property_key("age")?;
    let email_key = txn.get_or_create_property_key("email")?;
    let student_id_key = txn.get_or_create_property_key("student_id")?;
    let department_key = txn.get_or_create_property_key("department")?;

    // Register root type: Entity
    let entity_type = txn.register_type(
        TypeDefinitionBuilder::node_type("Entity")
            .property(name_key, ValueTypeDescriptor::String, true)
            .build()
    )?;

    // Register Person as a subtype of Entity
    let person_type = txn.register_type(
        TypeDefinitionBuilder::node_type("Person")
            .supertype(entity_type)
            .property(age_key, ValueTypeDescriptor::U64, false)
            .property(email_key, ValueTypeDescriptor::String, false)
            .build()
    )?;

    // Register Student as a subtype of Person
    let student_type = txn.register_type(
        TypeDefinitionBuilder::node_type("Student")
            .supertype(person_type)
            .property(student_id_key, ValueTypeDescriptor::String, true)
            .build()
    )?;

    // Register Professor as a subtype of Person
    let professor_type = txn.register_type(
        TypeDefinitionBuilder::node_type("Professor")
            .supertype(person_type)
            .property(department_key, ValueTypeDescriptor::String, true)
            .build()
    )?;

    txn.commit()?;

    // Verify the hierarchy
    let txn = db.read_txn()?;
    let registry = txn.type_registry();

    // Student's effective declarations include inherited ones
    let student_decls = registry.effective_property_declarations(student_type);
    // Should include: name (from Entity), age, email (from Person),
    //                 student_id (from Student)
    assert_eq!(student_decls.len(), 4);

    // Hierarchy queries
    assert!(registry.is_subtype_of(student_type, entity_type));
    assert!(registry.is_subtype_of(professor_type, person_type));
    assert!(!registry.is_subtype_of(student_type, professor_type));

    let person_subtypes = registry.all_subtypes(person_type);
    assert!(person_subtypes.contains(&student_type));
    assert!(person_subtypes.contains(&professor_type));

    txn.finish();
    Ok(())
}
```

---

## 16. Full Usage Example: Custom Constraint Validator

This example demonstrates implementing and registering a cardinality constraint.

```rust
use graph_db::*;

/// Enforces a maximum cardinality on outgoing edges of a specific type
/// from nodes of a specific type.
struct MaxOutgoingEdgesValidator {
    node_type: TypeId,
    edge_type: TypeId,
    max_count: usize,
}

impl ConstraintValidator for MaxOutgoingEdgesValidator {
    fn name(&self) -> &str {
        "MaxOutgoingEdgesValidator"
    }

    fn applies_to_types(&self) -> Option<Vec<TypeId>> {
        // Interested in both the node type and the edge type
        Some(vec![self.node_type, self.edge_type])
    }

    fn validate(
        &self,
        changes: &ChangeSet<'_>,
        graph: &dyn GraphView,
        types: &dyn TypeRegistryView,
        _keys: &dyn PropertyKeyRegistryView,
    ) -> Vec<ConstraintViolation> {
        let mut violations = Vec::new();

        // Check every node of the target type that was inserted or modified,
        // plus every node that had edges inserted.
        let mut nodes_to_check: Vec<NodeId> = Vec::new();

        for node in changes.inserted_nodes() {
            if node.type_labels.contains(&self.node_type) {
                nodes_to_check.push(node.id);
            }
        }

        for edge in changes.inserted_edges() {
            if edge.type_labels.contains(&self.edge_type) {
                nodes_to_check.push(edge.source);
            }
        }

        nodes_to_check.sort();
        nodes_to_check.dedup();

        for node_id in nodes_to_check {
            let outgoing = graph.outgoing_edges(node_id, Some(self.edge_type));
            if outgoing.len() > self.max_count {
                violations.push(ConstraintViolation {
                    violation_kind: "MAX_CARDINALITY_EXCEEDED".into(),
                    message: format!(
                        "Node {:?} has {} outgoing edges of type {:?}, max is {}",
                        node_id, outgoing.len(), self.edge_type, self.max_count
                    ),
                    subject: Some(ViolationSubject::Node(node_id)),
                });
            }
        }

        violations
    }
}

fn demonstrate_constraint() -> Result<(), Error> {
    let db = Database::open(DatabaseConfig::in_memory())?;

    // Set up types
    let mut txn = db.write_txn()?;
    let person = txn.register_type(
        TypeDefinitionBuilder::node_type("Person").build()
    )?;
    let spouse_of = txn.register_type(
        TypeDefinitionBuilder::edge_type("spouse_of").build()
    )?;
    txn.commit()?;

    // Register constraint: a person can have at most 1 spouse_of edge
    db.register_constraint(Box::new(MaxOutgoingEdgesValidator {
        node_type: person,
        edge_type: spouse_of,
        max_count: 1,
    }))?;

    // This should succeed
    let mut txn = db.write_txn()?;
    let alice = txn.insert_node(NodeBuilder::new().type_label(person).build())?;
    let bob = txn.insert_node(NodeBuilder::new().type_label(person).build())?;
    txn.insert_edge(
        EdgeBuilder::new(alice, bob).type_label(spouse_of).build()
    )?;
    txn.commit()?; // OK

    // This should fail
    let mut txn = db.write_txn()?;
    let charlie = txn.insert_node(NodeBuilder::new().type_label(person).build())?;
    txn.insert_edge(
        EdgeBuilder::new(alice, charlie).type_label(spouse_of).build()
    )?;
    match txn.commit() {
        Err(Error::ConstraintViolation(violations)) => {
            assert_eq!(violations.len(), 1);
            assert_eq!(violations[0].violation_kind, "MAX_CARDINALITY_EXCEEDED");
            println!("Constraint correctly rejected bigamy!");
        }
        other => panic!("Expected constraint violation, got: {:?}", other),
    }

    Ok(())
}
```

---

## 17. Full Usage Example: Custom Inference Rule

This example demonstrates implementing a transitive closure inference rule.

```rust
use graph_db::*;

/// Infers transitive edges: if A→B and B→C via edges of `transitive_type`,
/// infers A→C.
struct TransitiveClosureRule {
    transitive_type: TypeId,
}

impl InferenceRule for TransitiveClosureRule {
    fn name(&self) -> &str {
        "TransitiveClosureRule"
    }

    fn applies_to_types(&self) -> Option<Vec<TypeId>> {
        Some(vec![self.transitive_type])
    }

    fn infer(
        &self,
        graph: &dyn GraphView,
        _types: &dyn TypeRegistryView,
        _keys: &dyn PropertyKeyRegistryView,
    ) -> InferenceResult {
        let mut facts = Vec::new();

        // Collect all edges of the transitive type
        let edges = graph.edges_by_type(self.transitive_type, false);

        // Build adjacency map: source -> set of targets
        let mut adjacency: BTreeMap<NodeId, Vec<NodeId>> = BTreeMap::new();
        for edge in &edges {
            adjacency.entry(edge.source).or_default().push(edge.target);
        }

        // For each A→B, check all B→C and infer A→C if not present
        for edge in &edges {
            let a = edge.source;
            let b = edge.target;
            if let Some(c_nodes) = adjacency.get(&b) {
                for &c in c_nodes {
                    if c == a { continue; } // skip self-loops
                    // Check if A→C already exists
                    let existing = graph.outgoing_edges(a, Some(self.transitive_type));
                    let already_exists = existing.iter().any(|e| e.target == c);
                    if !already_exists {
                        facts.push(InferredFact::NewEdge {
                            type_labels: vec![self.transitive_type],
                            source: a,
                            target: c,
                            properties: PropertyMap::new(),
                        });
                    }
                }
            }
        }

        InferenceResult {
            facts,
            rule_name: self.name().into(),
        }
    }
}

fn demonstrate_inference() -> Result<(), Error> {
    let db = Database::open(DatabaseConfig::in_memory())?;

    let mut txn = db.write_txn()?;
    let concept = txn.register_type(
        TypeDefinitionBuilder::node_type("Concept").build()
    )?;
    let broader = txn.register_type(
        TypeDefinitionBuilder::edge_type("broader").build()
    )?;
    let name_key = txn.get_or_create_property_key("name")?;

    let a = txn.insert_node(
        NodeBuilder::new().type_label(concept)
            .property(name_key, Value::String("Fruit".into())).build()
    )?;
    let b = txn.insert_node(
        NodeBuilder::new().type_label(concept)
            .property(name_key, Value::String("Food".into())).build()
    )?;
    let c = txn.insert_node(
        NodeBuilder::new().type_label(concept)
            .property(name_key, Value::String("Nourishment".into())).build()
    )?;

    // Fruit → broader → Food → broader → Nourishment
    txn.insert_edge(EdgeBuilder::new(a, b).type_label(broader).build())?;
    txn.insert_edge(EdgeBuilder::new(b, c).type_label(broader).build())?;
    txn.commit()?;

    // Register the inference rule
    db.register_inference_rule(Box::new(TransitiveClosureRule {
        transitive_type: broader,
    }))?;

    // Run inference ephemerally to preview
    let txn = db.read_txn()?;
    let result = txn.run_inference("TransitiveClosureRule")?;
    assert_eq!(result.facts.len(), 1); // Fruit → broader → Nourishment
    txn.finish();

    // Materialize the inference
    let mut txn = db.write_txn()?;
    let result = txn.run_inference("TransitiveClosureRule", InferenceMode::Materialized)?;
    assert_eq!(result.facts.len(), 1);
    txn.commit()?;

    // Verify the materialized edge exists
    let txn = db.read_txn()?;
    let fruit_broader = txn.outgoing_edges(a, Some(broader))?;
    // Should now have 2 edges: Fruit→Food and Fruit→Nourishment
    assert_eq!(fruit_broader.len(), 2);
    txn.finish();

    Ok(())
}
```

---

## 18. Full Usage Example: Transactional Workflow

This example demonstrates the concurrency model, snapshot isolation, and error handling.

```rust
use graph_db::*;
use std::sync::Arc;
use std::thread;

fn demonstrate_transactions() -> Result<(), Error> {
    let db = Arc::new(Database::open(DatabaseConfig::persistent("social.db"))?);

    // Set up schema
    {
        let mut txn = db.write_txn()?;
        let _person = txn.register_type(
            TypeDefinitionBuilder::node_type("Person").build()
        )?;
        let _knows = txn.register_type(
            TypeDefinitionBuilder::edge_type("knows").build()
        )?;
        txn.commit()?;
    }

    // Resolve type IDs for use across threads
    let person;
    let knows;
    let name_key;
    {
        let txn = db.read_txn()?;
        let reg = txn.type_registry();
        person = reg.get_type_by_name("Person", TypeKind::Node).unwrap().id;
        knows = reg.get_type_by_name("knows", TypeKind::Edge).unwrap().id;
        name_key = txn.property_key_registry().get_key_id("name").unwrap();
        txn.finish();
    }

    // Concurrent reads: many threads can read simultaneously
    let db_clone = Arc::clone(&db);
    let reader_handle = thread::spawn(move || -> Result<(), Error> {
        let txn = db_clone.read_txn()?;
        let all_people = txn.nodes_by_type(person, false)?;
        println!("Reader sees {} people", all_people.len());
        // This snapshot is frozen — even if a writer adds more
        // people concurrently, this reader still sees the same count.
        thread::sleep(std::time::Duration::from_millis(100));
        let still_same = txn.nodes_by_type(person, false)?;
        assert_eq!(all_people.len(), still_same.len());
        txn.finish();
        Ok(())
    });

    // Sequential writes: only one writer at a time
    {
        let mut txn = db.write_txn()?;
        let alice = txn.insert_node(
            NodeBuilder::new().type_label(person)
                .property(name_key, Value::String("Alice".into())).build()
        )?;
        let bob = txn.insert_node(
            NodeBuilder::new().type_label(person)
                .property(name_key, Value::String("Bob".into())).build()
        )?;
        txn.insert_edge(EdgeBuilder::new(alice, bob).type_label(knows).build())?;

        // Read-your-own-writes: this sees alice and bob
        let people = txn.nodes_by_type(person, false)?;
        assert_eq!(people.len(), 2);

        txn.commit()?;
    }

    reader_handle.join().unwrap()?;

    // Transaction abort: changes are discarded
    {
        let mut txn = db.write_txn()?;
        txn.insert_node(
            NodeBuilder::new().type_label(person)
                .property(name_key, Value::String("Charlie".into())).build()
        )?;
        txn.abort(); // Charlie is discarded
    }

    // Verify Charlie was not persisted
    {
        let txn = db.read_txn()?;
        let people = txn.nodes_by_type(person, false)?;
        assert_eq!(people.len(), 2); // Only Alice and Bob
        txn.finish();
    }

    Ok(())
}
```

---

## 19. Ergonomics Review

This section evaluates the API against common usage patterns and identifies potential pain points.

### 19.1 Strengths

**Clear ownership model.** Transactions borrow the database, preventing use-after-close. Write transactions take `&mut self` for mutations, preventing aliased writes. Commit consumes the transaction, preventing double-commit.

**Minimal boilerplate for common operations.** The builder helpers (`NodeBuilder`, `EdgeBuilder`, `TypeDefinitionBuilder`) eliminate the most verbose patterns. Property key interning via `get_or_create_property_key` is idempotent and safe to call repeatedly.

**Consistent error handling.** All operations return `Result<T, Error>`. The error enum is exhaustive and pattern-matchable. Constraint violations include enough context for meaningful user-facing error messages.

**Transaction-scoped reads are zero-cost.** A read transaction is a snapshot pointer copy — no locks held, no coordination with writers.

### 19.2 Known friction points and mitigations

| Friction | Mitigation |
|----------|-----------|
| Property keys require `PropertyKeyId`, not strings | `get_or_create_property_key` is available on write transactions. Read transactions use `get_property_key`. Application code typically resolves keys once at startup and reuses the IDs. |
| `NodeId(0)` placeholder for new nodes feels unnatural | `NodeBuilder` hides the placeholder. The `insert_node` documentation makes the contract clear. |
| No multi-hop traversal primitive | Composing `neighbors()` calls is straightforward (Section 10.3). A future version could add a `traverse()` method with configurable depth limits, but this is deferred to avoid API surface bloat. |
| Read methods duplicated across transaction types | `GraphReader` trait (Section 10.1) allows generic code to work with either transaction type. |
| `Value::F64` prevents `Eq`/`Hash` on `Value` | This is a fundamental constraint from IEEE 754 (NaN ≠ NaN). Callers who need `Value` in sets should wrap it in a newtype with a total-ordering convention. Documented in `006-schema-extension-spec.md` residual concern #1. |
| Extension names are global strings, not typed | Simple and predictable. If namespace collisions become a problem, the convention `"crate_name::ValidatorName"` is documented. |

### 19.3 Thread safety summary

| Type | Send | Sync | Notes |
|------|------|------|-------|
| `Database` | ✓ | ✓ | All internal state is `Mutex`/`RwLock` protected |
| `ReadTransaction` | ✗ | ✗ | Tied to a specific thread; holds a snapshot reference |
| `WriteTransaction` | ✗ | ✗ | Holds the write lock; `&mut self` prevents sharing |
| `Node`, `Edge`, `Value`, etc. | ✓ | ✓ | Pure data types; `Clone + Debug` |
| `ConstraintValidator` | ✓ | ✓ | Required by trait bound |
| `InferenceRule` | ✓ | ✓ | Required by trait bound |

**Transactions are `!Send` and `!Sync`.** This is intentional: a transaction is a cursor into the buffer pool and cannot be safely moved across threads. If a user needs to share query results across threads, they extract the data (owned `Node`/`Edge` values) from the transaction and send those.

---

## 20. Out of Scope

### 20.1 Items explicitly deferred

| Item | Reason | Where it may live |
|------|--------|-------------------|
| Iterator-based query API (lazy traversal) | Requires careful lifetime management with the buffer pool. Simpler to start with `Vec` returns and add iterators in v2. | Future version |
| Batch insert API | Optimization for bulk loading. The current per-item API works correctly; batch insert is a performance optimization. | Future version |
| Write lock timeout configuration | The current API blocks indefinitely on `write_txn()`. Timeout support is straightforward to add. | `DatabaseConfig` in future version |
| `Cursor` / `RangeQuery` API | For large result sets, streaming results would avoid materializing entire `Vec`s. | Future version |
| Property value index management API | Property value indexes are deferred in v1 (007 Section 7.5). | Future version |
| Database compaction / vacuum API | Reclaiming space from old snapshots. | Future version |
| Database statistics (page count, tree depth, etc.) | Useful for diagnostics but not core functionality. | Future version |
| Query language (SPARQL, Cypher, etc.) | Explicitly out of scope per project constraints. | Downstream crate |

### 20.2 Items that belong to other tasks

| Item | Task |
|------|------|
| HAL trait definitions | Task 9 |
| Inference caching, invalidation, dependency tracking | Task 11 |
| On-disk format details | Task 8 |

---

## 21. Design Decision Log

| # | Decision | Alternatives Considered | Rationale |
|---|----------|------------------------|-----------|
| A1 | Transactions as the unit of work (no auto-commit) | Auto-commit mode; implicit transactions | Explicit transactions make the concurrency model clear. Auto-commit hides performance characteristics (each auto-committed operation is a separate fsync). |
| A2 | `commit(self)` consumes the transaction | `commit(&mut self)` allows retry | Simplicity: after commit (success or failure), the transaction's lifecycle is over. For constraint violations, the caller builds a new corrected transaction rather than patching the old one. |
| A3 | Extension registration on `Database`, not in transactions | Register extensions inside write transactions | Extensions are long-lived; requiring a transaction for registration is unnecessarily cumbersome. Internal locking is sufficient. |
| A4 | `GraphReader` trait for shared read interface | Duplicate methods without a trait; use `Deref` coercion | Trait allows generic functions that accept either transaction type. `Deref` would couple `WriteTransaction` to `ReadTransaction` and confuse the borrow checker. |
| A5 | Owned returns (`Vec<Node>`) rather than borrowed | `&Node` via buffer pool pinning; iterator-based | Owned returns are simpler, don't leak buffer pool internals, and work across thread boundaries. Performance cost is acceptable for v1. |
| A6 | Builders for Node, Edge, TypeDefinition | No builders (direct struct construction); macros | Builders eliminate placeholder IDs and PropertyMap ceremony. Macros are harder to document and debug. Direct struct construction is still available for users who prefer it. |
| A7 | `set_node_property` / `remove_node_property` partial updates | Only full-node `update_node` | Partial updates are the most common mutation pattern. Full update requires read-modify-write, which is both verbose and potentially racy (though single-writer makes it safe). |
| A8 | `validate_all()` for full-database revalidation | Only change-based validation | Schema changes can make existing data invalid. `validate_all()` synthesizes a full-insert ChangeSet to catch this. |
| A9 | Cascading delete on node deletion | Require caller to delete edges first; leave dangling edges | Dangling edges violate referential integrity. Requiring manual edge deletion is error-prone. Cascade is the least surprising behavior. |
| A10 | Immutable edge endpoints | Allow endpoint modification via `update_edge` | Changing endpoints is semantically a different relationship. Delete-and-recreate is more explicit. Allowing endpoint changes would complicate adjacency index maintenance in the WriteBuffer. |
| A11 | `read_txn` / `write_txn` naming | `begin_read` / `begin_write`; `transaction(ReadOnly)` / `transaction(ReadWrite)` | Short, idiomatic Rust naming. `read_txn` / `write_txn` is consistent with redb and other Rust database crates. |
| A12 | Transactions are `!Send` and `!Sync` | Make transactions `Send` | Transactions hold references into the buffer pool and snapshot state. Making them `Send` would require pinning and atomic reference counting on every page access — significant overhead for a capability most users don't need. |
| A13 | `run_inference` in `ReadTransaction` is always ephemeral | Disallow inference in read transactions | Read transactions are a natural context for "what-if" queries. Ephemeral inference provides this without requiring a write lock. |
| A14 | `missing_extensions()` is advisory, not an error | Fail on missing extensions; ignore silently | Failing prevents opening databases after extension refactoring. Ignoring silently hides potentially important information. Advisory warning is the middle ground. |
| A15 | `snapshot_to_file` / `load_from_file` only for in-memory mode | Allow snapshot of persistent databases | Persistent databases already have their file on disk. Snapshotting is only meaningful for in-memory databases that want optional durability. |
| A16 | Counting methods (`node_count`, `outgoing_edge_count`, etc.) | Only full-fetch methods; count as a `GraphReader` method | Counting avoids materializing and deserializing full records. Implemented as key-only B-tree scans. Common enough pattern to warrant dedicated methods. |

---

## Completion Report: Task 10 — Rust API Surface

### Status: COMPLETE

### Done Criterion:

The criterion requires:

1. Full public API as Rust type signatures and trait definitions — ✓ Sections 3–14 define every public type, method, and trait
2. Usage examples for every major operation — ✓ Examples throughout Sections 7–13, plus full examples in Sections 15–18
3. Examples showing custom type hierarchy registration — ✓ Section 15
4. Examples showing custom constraint registration — ✓ Sections 11.2, 16
5. Examples showing custom inference rule registration — ✓ Sections 12.4, 17
6. Examples showing transactional usage — ✓ Section 18
7. Reviewed for ergonomics and consistency — ✓ Section 19

All criteria met.

### Deliverables:
- `010-api-surface-spec.md` — this document

### Summary:

Designed the complete public Rust API for the embedded graph database crate. The API is organized around a `Database` handle with transaction-based access: `ReadTransaction` for consistent-snapshot reads and `WriteTransaction` for mutations. Both transaction types expose graph traversal, type hierarchy queries, and inference invocation. Write transactions additionally expose schema mutation, node/edge CRUD, constraint validation (dry-run and at-commit), and materialized inference.

Key design decisions include: transactions as the sole unit of work (no auto-commit), owned return values (for simplicity and thread safety), immutable edge endpoints, cascading node deletion, extension registration on the Database rather than within transactions, and advisory (not failing) missing-extension warnings.

Builder helpers for Node, Edge, and TypeDefinition reduce boilerplate for common patterns. A `GraphReader` trait provides a shared read interface for generic code. The `InferenceMode` enum lets callers choose between ephemeral and materialized inference per invocation.

### Context for Next Task:

**Task 11 (Inference Hook Architecture)** should read `010-api-surface-spec.md` (this deliverable) and `006-schema-extension-spec.md`. Key items for Task 11:

- The inference API (Sections 6.1, 6.2, 12) defines how callers trigger inference and choose ephemeral vs. materialized mode. Task 11 builds the caching, invalidation, and dependency tracking infrastructure that sits behind this API.
- `run_inference` in a `ReadTransaction` is always ephemeral. Task 11 should ensure the inference engine can operate on a read-only snapshot.
- `run_inference` in a `WriteTransaction` with `InferenceMode::Materialized` writes inferred facts into the WriteBuffer. These facts are part of the transaction's ChangeSet and are subject to constraint validation at commit time. Task 11 must design how materialized facts are tracked (so they can be identified as inferred vs. asserted).
- The `validate_all()` method (Section 6.2) produces a synthetic full-insert ChangeSet. If materialized inferred facts exist, Task 11 should decide whether they are included in the synthetic ChangeSet or handled separately.

**Task 12 (Design Synthesis)** should read this deliverable alongside all other design documents (006–011). The API surface is the user-facing integration point that connects all the design components.

### Residual Concerns:

1. **Iterator-based query API deferred.** Returning `Vec<Node>` from query methods materializes entire result sets. For large graphs, this is wasteful. A future version should add a `Cursor`-like iterator that streams results from the B-tree. This requires careful lifetime management and is deferred to avoid blocking v1 with API complexity.

2. **Batch insert API deferred.** Importing a large graph one node at a time is functional but slow (each insert modifies the WriteBuffer individually). A `batch_insert_nodes(Vec<Node>)` method could provide significant performance improvements. Deferred because it's an optimization, not a correctness concern.

3. **Write lock timeout.** The current `write_txn()` blocks indefinitely. For production use, a configurable timeout (returning `Error::Transaction(WriteLockTimeout)`) is important. This is easy to add to `DatabaseConfig` without breaking API changes.

4. **`Value` equality for `nodes_by_property`.** The `nodes_by_property` method compares `Value` instances for equality, but `Value` only implements `PartialEq` (not `Eq`) due to `f64`. For `F64` values, `PartialEq` works correctly in most cases but returns `false` for NaN comparisons. This is documented behavior, not a bug, but callers should be aware.

5. **Extension name collision across kinds.** A constraint validator and an inference rule can have the same name (they are in separate registries). This was flagged in `006-schema-extension-spec.md` residual concern #3. The convention `"kind::name"` (e.g., `"constraint::RequiredProperty"`) is recommended in documentation but not enforced.

### Upstream Flags:

1. **`ReadTransaction::run_inference` signature differs from `WriteTransaction::run_inference` — ADVISORY.**
   - What was discovered: `ReadTransaction::run_inference` takes only `rule_name: &str` (always ephemeral), while `WriteTransaction::run_inference` takes `rule_name: &str, mode: InferenceMode`. This asymmetry is intentional but Task 11 should be aware of it when designing the inference engine's internal interface.
   - Which task(s) it affects: Task 11
   - Severity: ADVISORY
   - Suggested action: Task 11's inference engine should accept an optional `WriteBuffer` reference. When `None` (read transaction), results are returned without materialization. When `Some` (write transaction with `Materialized` mode), results are written to the buffer.

2. **`validate_all()` performance concern — ADVISORY.**
   - What was discovered: `validate_all()` synthesizes a ChangeSet treating every node and edge as an insertion. For large databases, this produces a very large ChangeSet. Validators that are O(n) in ChangeSet size will be O(N) in database size.
   - Which task(s) it affects: Task 12 (documentation of performance characteristics), Task 11 (if inference interacts with validation)
   - Severity: ADVISORY
   - Suggested action: Document in the API that `validate_all()` is intended for schema-change scenarios and is O(N) in database size. For routine operations, the incremental `validate()` at commit time is sufficient.
