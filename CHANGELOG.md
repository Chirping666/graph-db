# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Sync primitives use `std::sync` when `std` feature is active (fixes priority
  inversion). `spin` is used only on `no_std`.
- `AnyBackend` removed — convenience functions return concrete `FileDatabase` and
  `MemoryDatabase` types.
- Re-exports removed — import types from the crate that defines them.
- `LockableBackend` trait is now unconditional (not `std`-gated).

### Added

- `Value::total_eq()` for deterministic float comparison.
- `property_map_total_eq()` helper for comparing property maps.
- `Database::try_write_txn(timeout)` for non-blocking write lock acquisition.
- `MAX_OVERFLOW_CHAIN_LENGTH` and enforcement in overflow page reading.
- Fuzz targets for page parsing and superblock validation.
- `compile_error!` on unsupported platforms in `phonograph_std`.

### Fixed

- Priority inversion under `std` due to unconditional `spin` mutex usage.
- Potential infinite loop on corrupt overflow page chains.

## [0.1.0] - 2026-03-26

### Added

- Typed property graph data model: typed nodes with properties, typed directed edges with properties
- Persistent single-file storage with crash safety (dual-superblock atomic commit)
- MVCC concurrency: single-writer, multiple-reader, snapshot isolation
- Extensible type/schema system: user-defined node types, edge types, type hierarchies, property declarations
- Pluggable constraint validation via `ConstraintValidator` trait
- Pluggable inference hooks via `InferenceRule` trait (materialized and ephemeral modes)
- Inference result caching with generation-based invalidation
- Provenance tracking for inferred entities
- `no_std + alloc` core with HAL (Hardware Abstraction Layer) trait system
- `std` persistent backend (`FileBackend`) with file locking and fsync discipline
- In-memory backend (`MemoryBackend`) with optional snapshot-to-disk / load-from-disk
- Buffer pool with clock eviction
- Copy-on-Write B+ tree storage engine
- Builder patterns for nodes, edges, and type definitions
- Graph traversal: edges by source/target, nodes by type, multi-hop traversal
- `Database`, `ReadTransaction`, `WriteTransaction` public API
- Comprehensive error hierarchy (`Error`, `SchemaError`, `StorageError`, `TransactionError`, `InferenceError`, `NotFoundError`)
- Examples: basic usage, OWL Lite ontology layer demonstration

### Fixed

- Large property values (>1 KB) now correctly use overflow pages instead of panicking
- Public API methods no longer panic on user-controllable input (return Result instead)

### Known Limitations

- `nodes_by_property()` performs a full scan (no property value index)
- Query methods return owned `Vec`s (no streaming iterator API)
- No batch insert API
- `write_txn()` blocks indefinitely (no configurable timeout)
- Provenance registry loaded entirely in memory
