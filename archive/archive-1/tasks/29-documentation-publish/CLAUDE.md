# CLAUDE.md — Task 29: Documentation & Publish Preparation

**Project:** Embedded Graph Database with Extensible Schema & Pluggable Inference  
**Implementation Task:** 29 (preparation task: 21)  
**Scope:** README, crate-level docs, standalone examples, CHANGELOG, Cargo.toml metadata  
**Status:** Pending  
**Depends on:** Task 28 (integration testing & hardening)  
**Preparation depends on:** Task 12 (`012-design-document.md`), Task 20 (`tasks/28-integration-testing/`)

---

## Orientation

This is Task 29, the final implementation task in the project. It produces all user-facing documentation and metadata required to publish the crate to crates.io. Within the project's hierarchy, this is one task in a 4-phase, 29-task project. Sibling implementation tasks are 22 (core types), 23 (HAL + std backend), 24 (storage engine), 25 (query engine), 26 (inference hooks), 27 (in-memory backend), 28 (integration testing). No task depends on this task's output — it is the terminal node in the dependency graph.

By the time this task begins, the entire crate should be functionally complete: all modules implemented, all unit tests passing, all integration tests passing, all doc-tests on public API items passing, fuzz testing completed without crashes, and `cargo clippy` clean. This task adds the final layer of polish: user-facing documentation, standalone runnable examples, publication metadata, and a CHANGELOG.

**What this task does:**

1. **README.md** — project overview, quick-start example, feature list, architecture summary, feature flags, MSRV policy, license, contribution guide
2. **Crate-level documentation** — audit and enhance `//!` docs in `src/lib.rs` (quick-start, architecture, feature flags)
3. **Standalone examples** — 2+ files in `examples/` that compile and run, including one demonstrating a simple ontology layer (minimal OWL Lite subset) on the extension system
4. **CHANGELOG.md** — initial release entry documenting the v0.1.0 feature set
5. **Cargo.toml metadata** — all fields required for crates.io publication (description, license, repository, keywords, categories, documentation, edition, rust-version)
6. **Final `cargo doc` audit** — ensure zero warnings and complete coverage

**What this task does NOT do:**

- Add or modify any functional code (types, storage engine, query engine, etc.)
- Add new tests beyond those embedded in examples and doc-tests
- Fix bugs. If documentation reveals a bug, report it in the completion report with reproduction steps, but do not fix it here
- Actually publish to crates.io. This task prepares everything; the user publishes manually

---

## Required Reading

Before writing any documentation, read these documents in order:

1. **`012-design-document.md`** — The single source of truth. Key sections for this task:
   - §1 — Purpose and reading guide (high-level project description)
   - §2 — Architecture overview and layer diagram (for the architecture summary in README)
   - §3 — Crate structure, feature flags, and dependencies (for feature flag documentation)
   - §4 — Core data model overview (for the feature list)
   - §5 — Type system and schema overview (for the extensibility narrative)
   - §13 — Constraint validation system overview (for the extension system explanation)
   - §14 — Inference hook architecture overview (for the extension system explanation)
   - §15 — Public API surface (for the quick-start example)
   - §16 — Cross-cutting concerns (naming, error handling — for usage guidance)
   - §18 — Known limitations and deferred work (for the CHANGELOG and README "limitations" section)

2. **`010-api-surface-spec.md`** — Authoritative reference for the public API. Key sections:
   - §5 — Database lifecycle and configuration (for the quick-start example)
   - §15 — Full usage example: custom type hierarchy (adapt for README quick-start)
   - §16 — Full usage example: custom constraint validator (basis for the ontology example)
   - §17 — Full usage example: custom inference rule (basis for the ontology example)
   - §19 — Ergonomics review (thread safety summary for the README)

3. **`006-schema-extension-spec.md`** — Reference for the ontology layer example:
   - §15 — OWL Lite walkthrough (type registration, constraints, inference rules)
   - §16 — SKOS walkthrough (alternative example pattern)

4. **`011-inference-hook-design.md`** — Reference for the inference example:
   - §16 — Inverse edge rule walkthrough
   - §17 — OWL subclass propagation walkthrough

5. **`009-hal-trait-design.md`** — Reference for feature flag documentation:
   - §3 — Crate structure and feature flags

6. **`CLAUDE.md` (project root)** — Project-wide rules: documentation standards (Rule 4), code style (Rule 7), commit conventions (Rule 6). The crate root documentation requirements in Rule 4 apply directly to this task.

---

## Definition of Done

All of the following must be true:

1. **`cargo doc --no-deps` produces zero warnings.** Every `pub` item has documentation. The crate root (`src/lib.rs`) contains a quick-start example, architecture overview, and feature flag documentation.

2. **README.md exists at the project root** and contains:
   - Project overview (what it is, what it is not)
   - Quick-start code example (open database, create types, insert nodes/edges, query, close)
   - Feature list (typed property graph, persistence, MVCC, extensible schema, constraint validation, inference hooks, `no_std + alloc` core, in-memory backend)
   - Architecture summary with layer diagram
   - Feature flags (`std`, `alloc`)
   - MSRV (minimum supported Rust version)
   - License

3. **2+ standalone examples exist in `examples/`**, each compiles and runs:
   - `examples/basic_usage.rs` — demonstrates opening a database, registering types, inserting nodes and edges, querying, and closing
   - `examples/owl_lite_ontology.rs` — demonstrates building a minimal OWL Lite subset on the extension system: registering OWL-inspired types, implementing a custom constraint validator (e.g., max-cardinality), implementing a custom inference rule (e.g., subclass propagation), running inference, and querying results

4. **CHANGELOG.md exists at the project root** with an initial `## 0.1.0` entry listing the feature set.

5. **Cargo.toml contains all crates.io metadata:** `description`, `license`, `repository`, `keywords`, `categories`, `documentation`, `edition`, `rust-version`, `readme`.

6. **All examples compile and run:** `cargo run --example basic_usage` and `cargo run --example owl_lite_ontology` both succeed.

7. **No functional code changes.** This task is documentation-only. If a code fix is needed (e.g., a doc-test reveals a missing re-export), it must be minimal, documented in the completion report, and limited to making documentation compile.

---

## Key Decisions Pre-Made

These decisions are inherited from the design documents and must be followed, not re-decided:

| Decision | Source | Implication for this task |
|----------|--------|--------------------------|
| Crate name: `graph_db` (placeholder — user may rename before publish) | `010-api-surface-spec.md` §15 | Use `graph_db` in all examples and documentation |
| Feature flags: `std` (default), `alloc` | `012-design-document.md` §3 | Document both flags in README and `lib.rs` |
| No baked-in ontology model | Master prompt, constraints | The OWL Lite example is *example code*, not part of the crate |
| Transactions as the unit of work | `010-api-surface-spec.md` §6 | Quick-start must show explicit `write_txn()` / `read_txn()` / `commit()` |
| `!Send`, `!Sync` transactions | `010-api-surface-spec.md` §19.3 | Mention in the README thread safety section |
| In-memory mode via `DatabaseConfig::in_memory()` | `010-api-surface-spec.md` §5 | Quick-start can use in-memory for simplicity |

---

## Scope Boundaries

- **Do not modify `src/` code** beyond minimal adjustments needed for documentation to compile (e.g., adding a missing `pub use` that was overlooked). Any such adjustment must be documented in the completion report.
- **Do not add new public API surface.** No new structs, traits, methods, or functions.
- **Do not add new tests in `tests/`.** This task adds only the examples in `examples/` and any documentation improvements.
- **The ontology example is demonstration code, not a library.** It lives in `examples/`, not in `src/`. It may define structs that implement `ConstraintValidator` and `InferenceRule`, but these are local to the example file.

---

## Pitfalls and Edge Cases

1. **Example compilation against the actual crate.** The examples in `examples/` depend on the crate's public API exactly as implemented. If any API method signature differs from what the design documents specify, the example must match the *actual implementation*, not the spec. Read the existing code before writing examples.

2. **`cargo doc` link resolution.** Intra-doc links (`[`Database`]`) must resolve. Run `cargo doc --no-deps` after every documentation change and fix broken links immediately.

3. **README code examples are not compiled by `cargo test`.** Unlike doc-tests in `src/`, code blocks in README.md are not automatically tested. Keep the README quick-start minimal and ensure it matches the actual doc-test in `lib.rs` (which *is* compiled).

4. **crates.io metadata validation.** Before calling the task complete, run `cargo package --list` to verify the package includes all expected files (README, LICENSE, CHANGELOG, examples). Run `cargo publish --dry-run` if available to catch metadata errors.

5. **License file.** crates.io requires a LICENSE or LICENSE-MIT file at the project root. Verify it exists. If not, create it (the project uses MIT or Apache-2.0 dual license — check Cargo.toml for the declared license and ensure the file matches).

6. **MSRV (Minimum Supported Rust Version).** Set `rust-version` in Cargo.toml to the oldest Rust version the crate compiles on. If unknown, set it to the current stable version and note this in the completion report as something to verify later.

7. **The OWL Lite example must be self-contained.** It should not require any external crates beyond `graph_db` itself. All OWL-specific types, validators, and inference rules are defined locally within the example file.
