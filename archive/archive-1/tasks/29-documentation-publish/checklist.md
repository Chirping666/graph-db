# Checklist: Task 29 — Documentation & Publish Preparation

**Governs:** Implementation Task 29  
**Read first:** `tasks/29-documentation-publish/CLAUDE.md`

---

## Phase 1: Audit Existing State

### 1.1 — Verify crate compiles and tests pass

Before making any changes, confirm the baseline:

```bash
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo doc --no-deps
```

All three must pass cleanly. If `cargo doc` produces warnings, record them — fixing those warnings is part of this task.

**Verify:** All three commands succeed. Note any `cargo doc` warnings for Phase 3.

### 1.2 — Inventory public API surface

List every `pub` item in the crate. Run:

```bash
cargo doc --no-deps 2>&1
```

Open the generated documentation (in `target/doc/`) and walk through every module, struct, enum, trait, method, and function. Note any items missing documentation or missing `# Examples` sections.

**⚠ Pitfall — doc-tests from Task 28.** Task 28 should have added doc-tests to all public API items. If any are missing, add them in Phase 3. Do not duplicate doc-tests that already exist.

**Verify:** A written inventory of documentation gaps exists (can be a scratch list — it does not need to be a deliverable).

### 1.3 — Review existing `lib.rs` crate-level docs

Check whether `src/lib.rs` already has:
- `//!` crate-level doc comment
- Quick-start example
- Architecture overview
- Feature flag documentation

Note what exists and what needs to be added or enhanced.

**Verify:** Gaps identified and recorded.

---

## Phase 2: Cargo.toml Metadata

### 2.1 — Add crates.io required metadata

Ensure `Cargo.toml` contains all fields required for `cargo publish`. Add or update:

```toml
[package]
name = "graph_db"  # or the actual crate name
version = "0.1.0"
edition = "2021"
rust-version = "1.75"  # adjust to actual MSRV; see pitfall below
description = "An embedded graph database with extensible schema, pluggable constraint validation, and pluggable inference hooks — a typed property graph engine designed as a foundation for ontology systems and knowledge graphs."
license = "MIT OR Apache-2.0"
repository = "https://github.com/user/graph-db"  # placeholder — user fills in
documentation = "https://docs.rs/graph_db"
readme = "README.md"
keywords = ["graph-database", "embedded-database", "property-graph", "ontology", "knowledge-graph"]
categories = ["database-implementations", "data-structures"]
```

**⚠ Pitfall — MSRV.** If the actual minimum Rust version is unknown, set `rust-version` to the version used during development and add a note to the completion report. The `keywords` field is limited to 5 entries and each must be ≤ 20 characters. The `categories` field must use values from crates.io's [valid categories list](https://crates.io/category_slugs).

**⚠ Pitfall — crate name.** The design documents use `graph_db` as a placeholder. The user may want to change this before publishing. All documentation and examples should use the crate name as it appears in `Cargo.toml`'s `[package] name` field.

**Verify:** `cargo package --list` runs without errors and includes the expected files.

### 2.2 — Verify or create license file

Check whether a `LICENSE` or `LICENSE-MIT` and `LICENSE-APACHE` file exists at the project root.

- If `license = "MIT OR Apache-2.0"`: both `LICENSE-MIT` and `LICENSE-APACHE` should exist.
- If `license = "MIT"`: a `LICENSE-MIT` (or `LICENSE`) file should exist.

If missing, create the appropriate license file(s) with standard text and the correct copyright holder (use the year the project was created and the repository owner's name or organization).

**Verify:** License file(s) exist and match the `license` field in `Cargo.toml`.

### 2.3 — Verify `[package] exclude` or `include`

Ensure the published crate does not include unnecessary files (e.g., `tasks/`, design documents, `.github/`, fuzz corpora). Add an `exclude` field if needed:

```toml
[package]
exclude = [
    "tasks/",
    "fuzz/",
    ".github/",
]
```

Alternatively, use `include` to whitelist only the necessary files:

```toml
[package]
include = [
    "src/**/*",
    "examples/**/*",
    "tests/**/*",
    "benches/**/*",
    "README.md",
    "CHANGELOG.md",
    "LICENSE-MIT",
    "LICENSE-APACHE",
    "Cargo.toml",
]
```

**Verify:** `cargo package --list` shows only the intended files. No design documents, task files, or fuzz corpora are included.

---

## Phase 3: Crate-Level Documentation (`src/lib.rs`)

### 3.1 — Write or enhance the crate-level doc comment

At the top of `src/lib.rs`, ensure there is a comprehensive `//!` doc comment block containing:

1. **One-line summary:** What the crate is (an embedded graph database with extensible schema and pluggable inference).

2. **Overview paragraph:** Describe the crate's purpose — a typed property graph engine designed as a foundation for ontology systems, knowledge graphs, and typed graph applications. Emphasize that it provides mechanism (types, constraints, inference hooks) but not policy (no built-in OWL, RDF, SKOS types).

3. **Quick-start example** as a `//! ```rust` doc-test that:
   - Opens an in-memory database (`DatabaseConfig::in_memory()`)
   - Registers a node type and a property key
   - Opens a write transaction
   - Inserts a node with a property
   - Commits
   - Opens a read transaction
   - Queries the node back
   - Asserts on the result

4. **Architecture section** with a text-art layer diagram:
   ```
   //! ## Architecture
   //!
   //! ```text
   //! ┌─────────────────────────────────────┐
   //! │  Application / Downstream Crate     │
   //! ├─────────────────────────────────────┤
   //! │  Public API (Database, Transactions)│
   //! ├─────────────────────────────────────┤
   //! │  Query & Traversal Engine           │
   //! ├─────────────────────────────────────┤
   //! │  Storage Engine (B+ trees, pages)   │
   //! ├─────────────────────────────────────┤
   //! │  HAL (Hardware Abstraction Layer)   │
   //! ├──────────────┬──────────────────────┤
   //! │  std backend  │  In-memory backend  │
   //! └──────────────┴──────────────────────┘
   //! ```
   ```

5. **Feature flags section:**
   - `std` (default) — enables the persistent file-backed storage backend, `std::io::Error` integration, and the full database engine
   - `alloc` — implied by `std`; enables the `no_std + alloc` core (all types, traits, and the in-memory backend)

6. **Thread safety summary:** `Database` is `Send + Sync`; transactions are `!Send` and `!Sync` (extract owned data to share across threads).

**⚠ Pitfall — doc-test compilation.** The quick-start example in `//!` is compiled and run by `cargo test --doc`. It must use only public API types and match the actual implementation. Test it immediately after writing.

**Verify:**
- `cargo test --doc -- lib` passes (the crate-level doc-test compiles and runs).
- `cargo doc --no-deps` produces zero warnings.

### 3.2 — Fix any remaining `cargo doc` warnings

Address every warning from `cargo doc --no-deps`:
- Missing documentation on `pub` items → add `///` doc comments
- Broken intra-doc links → fix the link target
- Missing `# Examples` sections → add doc-tests (only if Task 28 did not already cover them)

**⚠ Pitfall — do not duplicate doc-tests.** If Task 28 already added a doc-test for a method, do not add a second one. Only add doc-tests for items that are still missing them.

**Verify:** `cargo doc --no-deps` produces zero warnings. `cargo test --doc` passes.

---

## Phase 4: README.md

### 4.1 — Create README.md

Create `README.md` at the project root with the following sections. The tone should be clear, professional, and welcoming to new users.

**Section 1: Title and badges**

```markdown
# graph_db

An embedded graph database with extensible schema, pluggable constraint validation, and pluggable inference hooks.
```

Optionally include badges for: crates.io version, docs.rs link, license, CI status (if a CI pipeline exists).

**Section 2: Overview**

A 2–3 paragraph description covering:
- What it is: an embedded typed property graph database in Rust
- What it is not: not an ontology engine — it is the layer *beneath* one
- Design philosophy: provides mechanism for types, constraints, and inference; does not prescribe which types, constraints, or inference rules exist
- Target audience: developers building knowledge graphs, ontology systems (OWL, SKOS, custom models), or typed graph applications

**Section 3: Features**

A concise feature list:
- Typed property graph (typed nodes with properties, typed directed edges with properties)
- Full persistence with crash safety (single-file format, dual-superblock commit)
- MVCC concurrency (single-writer, multiple-reader, snapshot isolation)
- Extensible type/schema system (user-defined node types, edge types, type hierarchies, property declarations)
- Pluggable constraint validation (trait-based — implement `ConstraintValidator`)
- Pluggable inference hooks (trait-based — implement `InferenceRule`, with materialized and ephemeral modes)
- `no_std + alloc` core (HAL trait system for custom storage backends)
- In-memory backend (for testing or non-persistent use cases, with optional snapshot-to-disk)
- Pure Rust — no external database dependencies
- Explicit transaction model (`read_txn()` / `write_txn()` / `commit()`)

**Section 4: Quick Start**

A complete, minimal code example that:
- Adds `graph_db` to `Cargo.toml`
- Opens an in-memory database
- Registers a type and property key
- Inserts a node
- Queries it back
- Asserts correctness

This should be the same example as in the `lib.rs` crate-level docs (or a close variant). Note: README code blocks are not compiled by `cargo test`, so keep the code simple and manually verify it matches the compiled doc-test.

**Section 5: Architecture**

A brief paragraph and the same text-art layer diagram from `lib.rs`. Describe each layer in 1–2 sentences.

**Section 6: Extension System**

A paragraph explaining how to build ontology layers on top of the crate:
- Register custom types to model your domain
- Implement `ConstraintValidator` to enforce domain rules
- Implement `InferenceRule` to derive new facts
- Reference the `examples/owl_lite_ontology.rs` example

**Section 7: Feature Flags**

Document the `std` and `alloc` feature flags and their effects.

**Section 8: Minimum Supported Rust Version (MSRV)**

State the MSRV (matching `rust-version` in Cargo.toml).

**Section 9: Known Limitations**

A brief, honest list of v0.1.0 limitations (derived from `012-design-document.md` §18):
- `nodes_by_property()` performs a full scan (no property value index in v1)
- Query methods return owned `Vec`s (no streaming iterator API yet)
- No batch insert API
- `write_txn()` blocks indefinitely (no timeout)

**Section 10: License**

State the license (MIT OR Apache-2.0, or as declared in Cargo.toml).

**⚠ Pitfall — README length.** Keep it focused. The README is a landing page, not a manual. Link to docs.rs for full API documentation and to the `examples/` directory for extended usage.

**Verify:** README.md exists, is well-formatted, and all code examples are syntactically valid Rust.

---

## Phase 5: Standalone Examples

### 5.1 — Create `examples/basic_usage.rs`

Create a standalone example that demonstrates the core workflow. The example must:

1. Open an in-memory database (or persistent to a temporary directory)
2. Register at least two node types and one edge type with a type hierarchy (e.g., `Person` extends `Entity`, plus a `KNOWS` edge type)
3. Register at least two property keys (e.g., `"name"`, `"age"`)
4. Insert several nodes with properties
5. Insert edges between nodes
6. Query nodes by type (including with `include_subtypes`)
7. Traverse edges (outgoing, incoming)
8. Print results to stdout

The example should have explanatory comments and be readable as a tutorial. Use `fn main() -> Result<(), graph_db::Error>` as the entry point.

**⚠ Pitfall — example must compile against the actual crate.** Read the existing code in `src/` to confirm method names, argument types, and return types. Do not rely solely on the design documents — the implementation may have minor differences.

**Verify:** `cargo run --example basic_usage` compiles and runs successfully, producing readable output.

### 5.2 — Create `examples/owl_lite_ontology.rs`

Create a standalone example that demonstrates building a minimal OWL Lite ontology layer on the crate's extension system. This is the key differentiating example required by the task specification. The example must:

1. **Define OWL-inspired types:**
   - Register node types: `"owl:Class"`, `"owl:Individual"` (or simplified names like `"Class"`, `"Individual"`)
   - Register edge types: `"rdf:type"` (membership), `"rdfs:subClassOf"` (subsumption)
   - Register property keys: `"rdfs:label"`, `"owl:maxCardinality"` (or similar)

2. **Implement a custom constraint validator** (e.g., `MaxCardinalityValidator`):
   - Enforces that nodes of a given class have at most N outgoing edges of a specific type
   - Implements the `ConstraintValidator` trait
   - Returns `ConstraintViolation` when the cardinality is exceeded

3. **Implement a custom inference rule** (e.g., `SubclassPropagationRule`):
   - For every node with `rdf:type` edges to class A, if A is a subclass of B (via `rdfs:subClassOf`), infers that the node also has type B
   - Implements the `InferenceRule` trait
   - Returns `InferredFact::NodeTypeAssignment` (or `NewEdge` of type `rdf:type`)

4. **Build a small ontology:**
   - Create a class hierarchy: `Animal` → `Mammal` → `Dog`
   - Create individuals of type `Dog`
   - Register the constraint validator and inference rule with the database

5. **Run inference and demonstrate results:**
   - Call `run_inference()` (materialized mode)
   - Query to show that subclass propagation produced type assignments (Dog individuals also have Mammal and Animal types)

6. **Demonstrate constraint validation:**
   - Attempt an operation that violates the cardinality constraint
   - Show the validation error

The example should have extensive comments explaining the OWL Lite concepts being modeled and how they map to the crate's primitives. It should be readable by someone unfamiliar with OWL.

**⚠ Pitfall — InferenceRule trait signature.** The `infer()` method receives `&dyn GraphView`, `&dyn TypeRegistryView`, and `&dyn PropertyKeyRegistryView`. Confirm the exact trait signature from the implemented code before writing the example.

**⚠ Pitfall — ConstraintValidator trait signature.** The `validate()` method receives a `&ChangeSet` and `&dyn GraphView`. Confirm the exact signature.

**⚠ Pitfall — InferredFact variants.** Confirm which variants exist in the implementation (`NodeTypeAssignment`, `NewNode`, `NewEdge`, `NewProperty`, etc.) and use only implemented variants.

**⚠ Pitfall — self-contained example.** All OWL-specific types (`SubclassPropagationRule`, `MaxCardinalityValidator`, etc.) must be defined within the example file. Do not import from a hypothetical downstream crate.

**Verify:** `cargo run --example owl_lite_ontology` compiles and runs successfully, producing output that shows the inference results and constraint validation behavior.

### 5.3 — Verify all examples

Run every example:

```bash
cargo run --example basic_usage
cargo run --example owl_lite_ontology
```

Both must compile without warnings and run to completion without panicking.

**Verify:** Both commands succeed.

---

## Phase 6: CHANGELOG.md

### 6.1 — Create CHANGELOG.md

Create `CHANGELOG.md` at the project root following the [Keep a Changelog](https://keepachangelog.com/) format:

```markdown
# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - YYYY-MM-DD

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
- Provenance registry loaded entirely in memory
```

Replace `YYYY-MM-DD` with the actual date when the task is completed.

**Verify:** CHANGELOG.md exists, is well-formatted, and the feature list is accurate (cross-check against what is actually implemented).

---

## Phase 7: Final Verification

### 7.1 — Full `cargo doc` audit

```bash
cargo doc --no-deps 2>&1
```

**Zero warnings.** If any warnings remain, fix them before proceeding.

### 7.2 — Full test suite (including doc-tests)

```bash
cargo test
```

All tests pass, including any new doc-tests added in Phase 3. Zero failures.

### 7.3 — Examples compile and run

```bash
cargo run --example basic_usage
cargo run --example owl_lite_ontology
```

Both succeed without panics.

### 7.4 — Clippy

```bash
cargo clippy --all-targets --all-features -- -D warnings
```

Zero warnings.

### 7.5 — Package audit

```bash
cargo package --list
```

Verify the output includes:
- `src/` (all source files)
- `examples/basic_usage.rs`
- `examples/owl_lite_ontology.rs`
- `tests/` (integration tests)
- `README.md`
- `CHANGELOG.md`
- `LICENSE-MIT` and/or `LICENSE-APACHE` (matching the declared license)
- `Cargo.toml`

Verify the output does NOT include:
- `tasks/` directory
- `fuzz/` directory (if excluded)
- Design documents (`.md` files from the design phase)
- `.git/` or `.github/`

### 7.6 — Dry-run publish (if available)

```bash
cargo publish --dry-run
```

If this command is available and the network allows it, run it to catch any metadata errors. If it's not available (e.g., no network access), skip this step and note it in the completion report.

**Verify:** Dry-run succeeds, or is skipped with a note.

### 7.7 — README rendering check

Open `README.md` in a Markdown previewer (or run `cargo doc` and check the crate's docs.rs-style landing page). Verify:
- Headers render correctly
- Code blocks have syntax highlighting
- The layer diagram renders as intended (monospace)
- No broken links

**Verify:** README renders correctly.

---

## Post-Completion

Produce a completion report following the format in the master project prompt's Instance Rules section. Include:

- Status (COMPLETE / PARTIAL / BLOCKED).
- Verification evidence from Phase 7 (cargo doc output, test output, example output, package list).
- List of any documentation gaps that could not be resolved (e.g., a method that should exist but doesn't).
- List of any minimal code changes made to support documentation compilation (e.g., a missing `pub use`), with justification.
- MSRV determination: how it was established (tested on specific Rust version, or set to development version as a default).
- Any bugs discovered while writing examples (with reproduction steps).
- Residual concerns (e.g., crate name is a placeholder, repository URL is a placeholder).
- Note that this is the final task — no downstream task depends on this output.
