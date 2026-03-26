# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

### Known Limitations

- `nodes_by_property()` performs a full scan (no property value index)
- Query methods return owned `Vec`s (no streaming iterator API)
- No batch insert API
- `write_txn()` blocks indefinitely (no configurable timeout)
- Large property values (>~1 KB) may cause panics
- Provenance registry loaded entirely in memory
