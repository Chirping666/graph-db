# CLAUDE.md — Project Root

**Project:** Phonograph — Embedded Graph Database with Extensible Schema & Pluggable Inference
**Repository:** `https://github.com/Chirping666/graph-db`

---

## Overview

Phonograph is a three-crate Rust workspace implementing an embedded typed property graph
database. The design document `030-workspace-redesign.md` is the architectural source of truth
for the workspace structure. Previous task directories and completion reports are in `archive/`.

Always use the latest crate version for dependencies.

| Crate | Path | Purpose | `no_std`? |
|-------|------|---------|-----------|
| `phonograph` | `crates/phonograph/` | Graph vocabulary: core types, traits, errors | yes (`no_std + alloc`) |
| `phonograph_db` | `crates/phonograph_db/` | Database engine: storage, B+ trees, MVCC, buffer pool | yes (`no_std + alloc`) |
| `phonograph_std` | `crates/phonograph_std/` | OS/platform layer: `FileBackend`, file locking, convenience API | no (always `std`) |

### Feature Flags

Both `phonograph` and `phonograph_db` are `no_std + alloc` crates. The `alloc` dependency is
unconditional — there is no `alloc` feature flag. The only feature flag is `std` (default on),
which enables `std::error::Error` impls and similar std-only functionality.

**`phonograph`:**
```toml
[features]
default = ["std"]
std = []
```

**`phonograph_db`:**
```toml
[features]
default = ["std"]
std = ["phonograph/std"]
```

**`phonograph_std`:** No feature flags (always `std`).

### Directory Structure

```
phonograph/                         # workspace root
├── CLAUDE.md                       # this file
├── checklist.md                    # current work checklist (if active)
├── Cargo.toml                      # workspace manifest
├── README.md
├── CHANGELOG.md
├── LICENSE-MIT / LICENSE-APACHE
├── crates/
│   ├── phonograph/                 # Crate 1: graph vocabulary
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── types/
│   │       ├── schema/
│   │       ├── constraint/
│   │       ├── inference/
│   │       └── error/
│   ├── phonograph_db/              # Crate 2: database engine
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── sync.rs
│   │       ├── error/
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
│           └── backend_std/
│               └── file_backend.rs
├── fuzz/                           # cargo-fuzz targets (not a workspace member)
├── archive/                        # completed checklists and historical reports
└── tests/                          # workspace-level tests (if any)
```

---

## Session Workflow

When the user asks you to execute a checklist step:

1. **Read this file** if you haven't already in this session.
2. **Read `checklist.md`** at the project root to understand the full plan and where
   the requested step fits.
3. **Review the relevant code** before making changes. Understand what exists so you
   build on it correctly.
4. **Execute the step** the user asked for. Implement, compile, verify.
5. **Run the verification command** specified in the checklist step. Do not consider
   the step done until verification passes.
6. **Mark the step done** by changing `- [ ]` to `- [x]` in `checklist.md`.

After each step, also run:
```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
Fix failures before reporting the step as complete.

If a `no_std` verification is relevant:
```bash
cargo check -p phonograph --no-default-features
cargo check -p phonograph_db --no-default-features
```

---

## Project-Wide Rules

These rules apply to every session, every module, every line of code.

### Rule 1: No external database crate dependencies

Do not depend on SQLite, sled, redb, RocksDB, or any other database crate.
Allowed external dependencies are listed in `030-workspace-redesign.md` §13.

Allowed dependencies:
- **`phonograph`:** Zero non-dev dependencies.
- **`phonograph_db`:** `spin` (no_std sync), `hashbrown` (no_std HashMap), `crc32fast`,
  `xxhash-rust`.
- **`phonograph_std`:** `libc` (Unix), `phonograph`, `phonograph_db`.
- **Dev dependencies:** `tempfile`, `libfuzzer-sys` (fuzz crate only).

### Rule 2: `no_std + alloc` boundary

- `phonograph` must compile with zero non-dev dependencies under `no_std + alloc`.
- `phonograph_db` must compile under `no_std + alloc` (using `spin` for sync when `std`
  is off, `std::sync` when `std` is on; `hashbrown` for maps).
- `phonograph_std` is always `std`.
- Both `no_std` crates use `#![cfg_attr(not(feature = "std"), no_std)]` and unconditional
  `extern crate alloc`.

Verification:
```bash
cargo check -p phonograph --no-default-features
cargo check -p phonograph_db --no-default-features
```

### Rule 3: No baked-in ontology model

The crate provides **mechanism**, not **policy**. No OWL, RDF, SKOS, or other ontology
vocabulary is built in. The type system, constraint validators, and inference rules are
user-extensible extension points.

### Rule 4: Documentation on every public item

Every `pub` item must have a doc comment. Methods should document errors, panics, and
performance characteristics where relevant. Verify with `cargo doc --workspace --no-deps`.

### Rule 5: Test coverage

- Unit tests live in `#[cfg(test)]` modules alongside the code they test.
- Integration tests live in `tests/` per-crate.

### Rule 6: Code style

- Edition 2024. No MSRV policy — use the latest stable Rust toolchain.
- `rustfmt` defaults (no custom `.rustfmt.toml` unless necessary).
- No `unwrap()` in library code (tests are fine). Public API methods return `Result` on
  user-controllable input — no `assert!`/`panic!` for user-triggerable conditions.
- Minimize `unsafe`. Each `unsafe` block must have a `// SAFETY:` comment.
- Byte layout: little-endian integers, 2-byte or 4-byte length prefixes for variable fields.
- Commit messages: `type(scope): description`. Types: `feat`, `fix`, `refactor`, `test`,
  `docs`, `chore`, `ci`. Scopes: `phonograph`, `phonograph_db`, `phonograph_std`,
  `workspace`, or module names.

---

## Architectural Principles

These principles govern all code in the workspace. They apply to every current and future
change.

### A1: `phonograph_db` is fully platform-agnostic

No OS references, no `flock`, no `LockFileEx`, no `F_FULLFSYNC`, no `std::path`, no `libc`.
It defines abstract traits only. If it needs locking semantics, it defines a `LockableBackend`
trait — it never references how locking is implemented. All platform-specific code lives in
`phonograph_std` (or future platform-specific crates like `phonograph_unix`,
`phonograph_windows`).

### A2: No dynamic dispatch for backends

`Database<B>` is generic over `B: StorageBackend`. Users monomorphize to
`Database<FileBackend>` or `Database<MemoryBackend>` directly. No `AnyBackend` enum, no
`dyn StorageBackend`. Convenience functions in `phonograph_std` return concrete types:
`FileDatabase` and `MemoryDatabase`.

### A3: No re-exports between crates

`phonograph_db` does NOT `pub use phonograph::*`. `phonograph_std` does NOT
`pub use phonograph_db::*`. Users import from the crate that defines each type:
- Core vocabulary (`Value`, `NodeId`, `TypeId`, `ConstraintValidator`, etc.) → `phonograph::`
- Database engine (`Database`, `DatabaseConfig`, builders, errors) → `phonograph_db::`
- Platform types (`FileBackend`, `FileConfig`, convenience functions) → `phonograph_std::`

### A4: Sync primitives are platform-aware

`phonograph_db/src/sync.rs` uses `std::sync` when the `std` feature is active and `spin`
on `no_std`. This eliminates priority inversion for std users while maintaining no_std
compatibility. The wrapper provides a uniform API across both paths.

### A5: Defense in depth against corrupt data

Corrupt or malicious data must never cause infinite loops or panics in library code. All
parse paths have bounded iteration and return `Result` on malformed input. Overflow chains
are length-capped via `MAX_OVERFLOW_CHAIN_LENGTH`. Page-parsing functions are fuzz-tested.

### A6: Async stays out of the engine

The engine is synchronous. No async traits, no executor dependencies. Async wrappers are
the responsibility of downstream crates (e.g., a hypothetical `phonograph_tokio` using
`spawn_blocking`). This is a deliberate design choice for an embedded database where
buffer pool cache hits complete in nanoseconds.

### A7: Prefer static dispatch over dynamic dispatch

Use generics (`impl Trait` / `<T: Trait>`) rather than trait objects (`&dyn Trait`,
`Box<dyn Trait>`) wherever the concrete type is known at the call site. Dynamic dispatch
is acceptable **only** when heterogeneous runtime collections are required — specifically:

- `Vec<Box<dyn ConstraintValidator>>` — user-registered validators with different concrete types.
- `Vec<Box<dyn InferenceRule>>` — user-registered inference rules with different concrete types.

All other trait usage should be monomorphized:
- Internal infrastructure traits (`SnapshotReader`) must use `impl Trait` or generics
  at call sites, not `&dyn`.
- The `ConstraintValidator::validate()` and `InferenceRule::infer()` method signatures
  must remain object-safe (they are called through `Box<dyn ...>`). Their view parameters
  (`&dyn GraphView`, `&dyn TypeRegistryView`, `&dyn PropertyKeyRegistryView`) are
  acceptable dynamic dispatch because the traits must be object-safe for this reason and
  the cost per commit is negligible compared to B-tree I/O.

### A8: Semantic correctness for float comparisons

`Value` does not implement `Eq` because it contains `f64`. All property-value comparisons
in engine code (overlay views, `nodes_by_property`, change tracking) must use
`Value::total_eq()` instead of `PartialEq`. This ensures NaN-valued properties are
matchable and the engine behaves deterministically. `PartialEq` is still derived for
convenience in user-land comparisons and tests, but the engine's internal query paths
must always use `total_eq`.

### A9: `OverlayGraphView` respects the full `GraphView` contract

`OverlayGraphView` is the `GraphView` implementation given to constraint validators and
inference rules. It must fully implement every aspect of the `GraphView` trait contract,
including `include_subtypes` resolution in `nodes_by_type` and `edges_by_type`. The
`OverlayGraphView` holds a reference to the `SchemaCache` and delegates subtype
resolution to it.

---

## Verification Checklist

| # | Check | Command |
|---|-------|---------|
| 1 | `phonograph` compiles `no_std + alloc` | `cargo check -p phonograph --no-default-features` |
| 2 | `phonograph` has zero non-dev dependencies | Inspect `Cargo.toml` |
| 3 | `phonograph` contains NO storage/backend/transaction concepts | `grep -r "ReadAt\|WriteAt\|StorageBackend\|StorageError\|TransactionError" crates/phonograph/src/` → empty |
| 4 | `phonograph_db` compiles `no_std + alloc` | `cargo check -p phonograph_db --no-default-features` |
| 5 | `phonograph_db` has no ungated `use std::` | `grep -r "use std::" crates/phonograph_db/src/` → empty or `#[cfg]`-gated |
| 6 | `phonograph_db` has no OS-specific references | `grep -r "flock\|LockFileEx\|F_FULLFSYNC\|libc::" crates/phonograph_db/src/` → empty |
| 7 | `phonograph_std` compiles | `cargo check -p phonograph_std` |
| 8 | No re-exports between crates | `grep -rn 'pub use phonograph::' crates/phonograph_db/src/lib.rs` and `grep -rn 'pub use phonograph_db::' crates/phonograph_std/src/lib.rs` → both empty |
| 9 | No `AnyBackend` | `grep -rn 'AnyBackend' crates/` → empty |
| 10 | Full workspace builds | `cargo build --workspace` |
| 11 | All tests pass | `cargo test --workspace` |
| 12 | No clippy warnings | `cargo clippy --workspace --all-targets -- -D warnings` |
| 13 | Docs build | `cargo doc --workspace --no-deps` |
| 14 | No `&dyn SnapshotReader` in non-test code | `grep -rn 'dyn SnapshotReader' crates/phonograph_db/src/` → empty or test-only |
| 15 | `OverlayGraphView` holds schema reference | `grep 'schema:' crates/phonograph_db/src/db/graph_view.rs` → contains `&SchemaCache` field |
| 16 | No `PartialEq` property lookups in engine code | `grep -n 'properties.get.*== Some' crates/phonograph_db/src/db/` → empty |
| 17 | Counter deserialization is bounds-checked | `grep -A2 'next_type_id\|next_property_key_id' crates/phonograph_db/src/db/database.rs` → shows `u32::try_from` or range check |

---

## Key Design Decisions

These decisions are **settled**. Do not re-litigate them.

- **R1:** Core crate name is `phonograph`.
- **R4:** Backend traits (`ReadAt`, `WriteAt`, `StorageBackend`) live in `phonograph_db`, not `phonograph`.
- **R5:** `MemoryBackend` lives in `phonograph_db`.
- **R6:** The unified `Error` enum lives in `phonograph_db`.
- **R8:** `phonograph` exports only individual error types (`SchemaError`, `NotFoundError`, `InferenceError`).
- **R10:** `HashMap` replacement is `hashbrown`.
- **R11:** `Database` is generic: `Database<B: StorageBackend>`.
- **R17:** The `StorageError` backend trait is renamed to `BackendError` to avoid collision with the `error::StorageError` struct.

### Superseded Decisions

| Old Decision | Superseded By | Rationale |
|-------------|---------------|-----------|
| R9: `spin` unconditionally in `phonograph_db` | A4: `spin` on `no_std`, `std::sync` on `std` | Fixes priority inversion for std users |
| R13: `AnyBackend` lives in `phonograph_std` | A2: `AnyBackend` removed entirely | No dynamic dispatch; users monomorphize directly |
| R14: Each crate re-exports the one below it | A3: No re-exports between crates | Explicit imports; no name collisions |

---

## Residual Concerns

1. `crc32fast` loses hardware acceleration on `no_std` — acceptable for v1.
2. `hashbrown` `ahash` uses fixed seed on `no_std` — fine for page table keys (not
   security-sensitive).
3. Engine requires `alloc` — correct trade-off for an embedded DB with dynamic data.
4. `write_txn()` blocks indefinitely on `no_std` (no `try_write_txn` without `std::time`).
   On `std`, `try_write_txn(timeout)` is available.
5. Provenance registry loaded entirely in memory — could consume ~50 MB for databases
   with millions of inferred entities. Lazy loading is a future optimization.
6. `Value` does not implement `Eq` due to `f64`. Use `Value::total_eq()` for deterministic
   comparison where needed. Engine query paths use `total_eq` exclusively (A8).
7. `OverlayGraphView::build` supports changeset-scoped preloading via an optional
   `affected_types` hint. When the hint is provided, only base entities with matching
   types and adjacency neighbors of changed nodes are loaded. When no hint is provided
   (e.g. `validate_all()`), all entities are loaded. This is a trade-off: if a
   validator queries for a type NOT in the affected set, the result may be incomplete.
   A fully lazy/incremental overlay (loading on demand during `&self` calls) would
   remove this limitation but requires interior mutability or trait signature changes.
8. `InferenceCache` key lookup allocates a `String` on every `get()` call. A zero-alloc
   lookup via `BTreeMap` with a borrowed key wrapper is a future optimization.
