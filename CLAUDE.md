# CLAUDE.md — Project Root

**Project:** Phonograph — Embedded Graph Database with Extensible Schema & Pluggable Inference  
**Phase:** Workspace Redesign (three-crate `no_std` architecture)  
**Design document:** `030-workspace-redesign.md`  
**This file governs Claude Code's behavior for the workspace migration.**

---

## Overview

This project is being restructured from a two-crate workspace (`graph_db_core` + `graph_db`)
into a three-crate workspace:

| Crate | Purpose | `no_std`? |
|-------|---------|-----------|
| `phonograph` | Graph vocabulary: core types, traits, errors | yes (`no_std + alloc`) |
| `phonograph_db` | Database engine: storage, B+ trees, transactions, buffer pool | yes (`no_std + alloc`) |
| `phonograph_std` | OS/platform layer: `FileBackend`, file locking, convenience API | no (always `std`) |

The design document `030-workspace-redesign.md` is the **single source of truth** for all
architectural decisions in this phase. When in doubt, that document takes precedence.

### Archive

All design documents, task directories, completion reports, and the previous `CLAUDE.md`
from the Tasks 1–29 implementation phase are preserved in `archive/`. They are reference
material only — the current phase is governed entirely by this file and `030-workspace-redesign.md`.

Three documents govern this phase: this file (CLAUDE.md) defines session behavior and rules, 030-workspace-redesign.md is the architectural source of truth, and checklist.md is the ordered migration plan with verification gates.

---

## Session Workflow

Every Claude Code session follows these steps in order. Do not skip steps.

### 1. Read the design document

Read `030-workspace-redesign.md` — specifically the sections relevant to the current
migration phase (see §14 Migration Plan for the five-phase breakdown). If the session
targets a specific crate, focus on that crate's section (§5, §6, or §7).

Then read checklist.md — the ordered migration steps with verification gates. Identify which step you are currently on before planning the session.

### 2. Review existing code

Before writing any code, examine the current state:
- `crates/` — the workspace crate layout (evolving during migration)
- `Cargo.toml` (workspace root) — workspace members and shared dependencies
- Each crate's `Cargo.toml` — dependencies and feature flags
- `src/` — any code not yet migrated

Understand what has already been moved and what remains.

### 3. Create a session plan and confirm with the user

Before implementing, produce a brief plan:
- Which checklist items you will tackle in this session (reference by number, e.g., "2.3 through 2.6")
- Any ambiguities or questions
- Any deviations from the design document you anticipate (with justification)

Wait for the user to confirm before proceeding.

### 4. Implement incrementally

Work through the migration steps sequentially. For each change:
1. Make the change
2. Ensure it compiles (`cargo check --workspace`)
3. Run the relevant tests
4. Only move to the next step after the current one passes

Keep the workspace compiling at every intermediate step. Do not batch large
refactors into a single uncommitted change.

### 5. Run verification after each significant milestone

After completing a migration phase or significant sub-step:
- `cargo check --workspace` — everything compiles
- `cargo test --workspace` — all tests pass
- `cargo clippy --workspace --all-targets -- -D warnings` — no warnings
- `cargo doc --workspace --no-deps` — no documentation warnings

For `no_std` verification on `phonograph` and `phonograph_db`:
- `cargo check -p phonograph --no-default-features --features alloc`
- `cargo check -p phonograph_db --no-default-features --features alloc`

### 6. Produce a summary when done

At the end of each session, summarize:
- What was completed
- What remains
- Any issues encountered or deviations from the design
- Verification evidence (test counts, clippy/doc output)

---

## Project-Wide Rules

These rules carry forward from the previous phase and apply to all code in the workspace.

### Rule 1: No external database crate dependencies

Do not depend on SQLite, sled, redb, RocksDB, or any other database crate.
Allowed external dependencies are listed in `030-workspace-redesign.md` §13.

### Rule 2: `no_std + alloc` boundary

- `phonograph` must compile with zero non-dev dependencies and `no_std + alloc`.
- `phonograph_db` must compile under `no_std + alloc` (using `spin` for sync, `hashbrown` for maps).
- `phonograph_std` is always `std`.

Verification commands are in the session workflow (step 5).

### Rule 3: No baked-in ontology model

The crate provides **mechanism**, not **policy**. No OWL, RDF, SKOS, or other ontology
vocabulary is built in. The type system, constraint validators, and inference rules are
user-extensible extension points.

### Rule 4: Documentation on every public item

Every `pub` item must have a doc comment. Methods should document errors, panics, and
performance characteristics where relevant. Verify with `cargo doc --workspace --no-deps`.

### Rule 5: Test coverage

- Unit tests live in `#[cfg(test)]` modules alongside the code they test.
- Integration tests live in `tests/` at the workspace root or per-crate.
- All 311+ existing tests must continue to pass after migration.

### Rule 6: Commit message conventions

Format: `type(scope): description`

Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`, `ci`  
Scopes: `phonograph`, `phonograph_db`, `phonograph_std`, `workspace`, or module names.

Examples:
- `refactor(workspace): rename graph_db_core to phonograph`
- `refactor(phonograph_db): generify Database<B: StorageBackend>`
- `feat(phonograph_std): add open() and open_in_memory() convenience constructors`

### Rule 7: Code style

- `rustfmt` defaults (no custom `.rustfmt.toml` unless necessary).
- No `unwrap()` in library code (tests are fine).
- Minimize `unsafe`. Each `unsafe` block must have a `// SAFETY:` comment.
- Byte layout: little-endian integers, 2-byte or 4-byte length prefixes for variable fields.

---

## Migration Phases (from `030-workspace-redesign.md` §14)

| Phase | Summary |
|-------|---------|
| 1 | Rename `graph_db_core` → `phonograph`, strip non-vocabulary modules |
| 2 | Create `phonograph_db`, move storage/db code, generify `Database<B>`, swap sync/map |
| 3 | Create `phonograph_std`, move `FileBackend`, add convenience API |
| 4 | Retire or facade the old `graph_db` crate |
| 5 | Full workspace verification (§17 checklist) |

---

## Verification Checklist (from `030-workspace-redesign.md` §17)

| # | Check | Command |
|---|-------|---------|
| 1 | `phonograph` compiles `no_std + alloc` | `cargo check -p phonograph --no-default-features --features alloc` |
| 2 | `phonograph` has zero non-dev dependencies | Inspect `Cargo.toml` |
| 3 | `phonograph` contains NO storage/backend/transaction concepts | `grep -r "ReadAt\|WriteAt\|StorageBackend\|StorageError\|TransactionError" crates/phonograph/src/` → empty |
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

## Key Design Decisions (from `030-workspace-redesign.md` §18)

These decisions are **settled**. Do not re-litigate them during implementation.

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

## Residual Concerns (from `030-workspace-redesign.md` §20)

1. `spin` priority inversion on preemptive OS — acceptable for v1, opt-in `std-sync` feature later.
2. `crc32fast` loses hardware acceleration on `no_std` — acceptable.
3. `hashbrown` `ahash` uses fixed seed on `no_std` — fine for page table keys.
4. Engine requires `alloc` — correct trade-off.
5. Test infrastructure split — ensure no coverage gaps between `phonograph_db` and `phonograph_std`.
6. Name registration — reserve `phonograph`, `phonograph_db`, `phonograph_std` on crates.io.
7. `StorageError` name collision — resolved via `BackendError` rename (R17).
