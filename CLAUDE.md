# CLAUDE.md — Project Root

**Project:** Phonograph — Embedded Graph Database with Extensible Schema & Pluggable Inference

---

## Overview

Phonograph is a three-crate Rust workspace implementing an embedded typed property graph database:

| Crate | Purpose | `no_std`? |
|-------|---------|-----------|
| `phonograph` | Graph vocabulary: core types, traits, errors | yes (`no_std + alloc`) |
| `phonograph_db` | Database engine: storage, B+ trees, transactions, buffer pool | yes (`no_std + alloc`) |
| `phonograph_std` | OS/platform layer: `FileBackend`, file locking, convenience API | no (always `std`) |

The design document `030-workspace-redesign.md` is the architectural source of truth for the
workspace structure. Previous task directories and completion reports are in `archive/`.

Always use the latest crate version for dependencies.

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

---

## Project-Wide Rules

### Rule 1: No external database crate dependencies

Do not depend on SQLite, sled, redb, RocksDB, or any other database crate.
Allowed external dependencies are listed in `030-workspace-redesign.md` §13.

### Rule 2: `no_std + alloc` boundary

- `phonograph` must compile with zero non-dev dependencies under `no_std + alloc`.
  Both crates use `#![cfg_attr(not(feature = "std"), no_std)]` and unconditional `extern crate alloc`.
- `phonograph_db` must compile under `no_std + alloc` (using `spin` for sync, `hashbrown` for maps).
- `phonograph_std` is always `std`.

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

---

## Verification Checklist

| # | Check | Command |
|---|-------|---------|
| 1 | `phonograph` compiles `no_std + alloc` | `cargo check -p phonograph --no-default-features` |
| 2 | `phonograph` has zero non-dev dependencies | Inspect `Cargo.toml` |
| 3 | `phonograph` contains NO storage/backend/transaction concepts | `grep -r "ReadAt\|WriteAt\|StorageBackend\|StorageError\|TransactionError" crates/phonograph/src/` → empty |
| 4 | `phonograph_db` compiles `no_std + alloc` | `cargo check -p phonograph_db --no-default-features` |
| 5 | `phonograph_db` has no ungated `use std::` | `grep -r "use std::" crates/phonograph_db/src/` → empty or `#[cfg]`-gated |
| 6 | `phonograph_std` compiles | `cargo check -p phonograph_std` |
| 7 | Full workspace builds | `cargo build --workspace` |
| 8 | All tests pass | `cargo test --workspace` |
| 9 | No clippy warnings | `cargo clippy --workspace --all-targets -- -D warnings` |
| 10 | Docs build | `cargo doc --workspace --no-deps` |

---

## Key Design Decisions

These decisions are **settled**. Do not re-litigate them.

- **R1:** Core crate name is `phonograph`.
- **R4:** Backend traits (`ReadAt`, `WriteAt`, `StorageBackend`) live in `phonograph_db`, not `phonograph`.
- **R5:** `MemoryBackend` lives in `phonograph_db`.
- **R6:** The unified `Error` enum lives in `phonograph_db`.
- **R8:** `phonograph` exports only individual error types (`SchemaError`, `NotFoundError`, `InferenceError`).
- **R9:** Sync primitives use `spin` unconditionally in `phonograph_db`.
- **R10:** `HashMap` replacement is `hashbrown`.
- **R11:** `Database` is generic: `Database<B: StorageBackend>`.
- **R13:** `AnyBackend` lives in `phonograph_std`.
- **R14:** Each crate re-exports the one below it.
- **R17:** The `StorageError` backend trait is renamed to `BackendError` to avoid collision with the `error::StorageError` struct.

---

## Residual Concerns

1. `spin` priority inversion on preemptive OS — acceptable for v1, opt-in `std-sync` feature later.
2. `crc32fast` loses hardware acceleration on `no_std` — acceptable.
3. `hashbrown` `ahash` uses fixed seed on `no_std` — fine for page table keys.
4. Engine requires `alloc` — correct trade-off.
5. Test infrastructure split — ensure no coverage gaps between `phonograph_db` and `phonograph_std`.
