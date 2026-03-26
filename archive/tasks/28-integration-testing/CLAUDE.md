# CLAUDE.md — Task 28: Implement Integration Testing & Hardening

**Project:** Embedded Graph Database with Extensible Schema & Pluggable Inference  
**Implementation Task:** 28 (preparation task: 20)  
**Scope:** Integration tests (`tests/`), doc-tests on all public API items, fuzz harness (`fuzz/`)  
**Status:** Pending  
**Depends on:** Task 27 (in-memory HAL backend)  
**Preparation depends on:** Task 12 (`012-design-document.md`), Task 19 (`tasks/27-in-memory-backend/`)

---

## Orientation

This is Task 28, the integration testing and hardening phase. It sits at the end of the implementation sequence (after all functional modules are complete) and validates the entire system end-to-end. Within the project's hierarchy, this is one task in a 4-phase, 29-task project. Sibling implementation tasks are 22 (core types), 23 (HAL + std backend), 24 (storage engine), 25 (query engine), 26 (inference hooks), 27 (in-memory backend), and 29 (docs/publish). Task 29 depends on this task's output.

**What this task does:**

1. **End-to-end integration tests** — 7 scenarios that exercise the full stack from `Database::open()` through transactions, schema, CRUD, queries, traversals, constraints, inference, persistence, and the in-memory backend. These tests live in `tests/` (not inside `src/`).
2. **Fuzz testing** — A `cargo-fuzz` harness that feeds random byte sequences into key deserialization and database operation paths to surface panics, crashes, or memory-safety issues.
3. **Concurrency stress tests** — Multi-threaded scenarios that push the single-writer MVCC model with high reader/writer contention, rapid transaction turnover, and cross-thread data consistency assertions.
4. **Doc-test coverage** — Every public API item (`pub fn`, `pub struct`, `pub enum`, `pub trait`, `pub type`) receives a runnable `/// # Examples` doc-test. This ensures the API documentation is both accurate and useful.

**What this task does NOT do:**

- Fix bugs found during testing. If a test reveals a bug in a module implemented by Tasks 22–27, the bug must be reported in the completion report with a clear reproduction, but the fix belongs to the relevant module owner — not this task.
- Benchmark performance. Performance characteristics are informational, not acceptance criteria.
- Add new public API surface. This task is read-only with respect to the API; it only tests and documents what exists.

**Relationship to earlier tests:** Tasks 22–27 each include unit tests and some integration tests within their own modules. This task adds *cross-module* integration tests that no single module could own — scenarios that exercise the full stack, including persistence round-trips (close + reopen), cross-backend equivalence (persistent vs. in-memory), and the extension system lifecycle (register → use → persist → reload → re-register → verify).

---

## Required Reading

Before writing any tests, read these documents in order:

1. **`012-design-document.md`** — The single source of truth. Key sections:
   - §2 — Architecture overview (understand the full layer stack you are testing)
   - §3 — Crate structure, feature flags (`std`, `alloc`)
   - §5 — Type system and schema (for test data setup)
   - §6 — Graph storage model (B-tree catalog — understand what persists where)
   - §10 — Concurrency model (single-writer MVCC: the invariants your stress tests must verify)
   - §11 — Transaction lifecycle (commit protocol, snapshot isolation)
   - §12 — Crash safety (the guarantees your persistence tests validate)
   - §13 — Constraint validation (ChangeSet, validator dispatch, validation modes)
   - §14 — Inference hook architecture (rule registration, triggering, materialization, provenance)
   - §15 — Public API surface (every method you are writing doc-tests for)
   - §16 — Cross-cutting concerns (error handling, naming conventions)
   - §18 — Known limitations and deferred work (so you don't test for unsupported behavior)

2. **`010-api-surface-spec.md`** — The authoritative public API reference. Every public method in this document needs a doc-test. Key sections:
   - §5 — Database lifecycle, `DatabaseConfig`, `Database`
   - §6 — Transaction API (`ReadTransaction`, `WriteTransaction`)
   - §7 — Schema operations
   - §8 — Node operations
   - §9 — Edge operations
   - §10 — Graph traversal and query (`GraphReader` trait, counting methods)
   - §11 — Constraint validation API (`validate_all()`)
   - §12 — Inference API (`run_inference`, `run_all_inference`, `InferenceMode`)
   - §13 — Extension registration API
   - §14 — Builder helpers
   - §§15–18 — Full usage examples (model your doc-tests after these)

3. **`006-schema-extension-spec.md`** — For understanding the extension traits:
   - §10 — `ConstraintValidator` trait and `ChangeSet`
   - §11 — `InferenceRule` trait
   - §12 — Extension registration and lifecycle

4. **`011-inference-hook-design.md`** — For inference-specific test scenarios:
   - §3 — InferenceEngine architecture
   - §5 — Rule registry
   - §6 — Triggering and dispatch
   - §8 — Provenance system

5. **`007-graph-storage-model.md`** — For understanding data persistence:
   - §4 — B-tree catalog (what to verify survives a close/reopen cycle)
   - §11 — Concurrency control strategy (invariants for stress tests)
   - §14 — ID allocation and recycling (edge cases for reuse after delete)

6. **`CLAUDE.md` (project root)** — Project-wide rules, especially:
   - Rule 3 (no baked-in ontology model — test validators/rules are test-only, not shipped)
   - Rule 4 (documentation on every public item — doc-tests are part of this)
   - Rule 5 (test coverage expectations — this task fulfills integration test requirements)

7. **Existing test code in `src/` and `tests/`** — Review what unit and integration tests already exist from Tasks 22–27 so you do not duplicate effort and so you build on established test helpers.

---

## Definition of Done

All of the following must be true for this task to be considered complete:

1. **5+ end-to-end integration tests pass** — at least the 7 scenarios defined in the checklist, all in `tests/`.
2. **Full extension system round-trip test passes** — one scenario registers custom types, a custom constraint validator, and a custom inference rule, inserts data, validates, infers, persists (close + reopen), re-registers extensions, and verifies all data (including materialized inferred facts and provenance).
3. **Concurrent access scenarios pass** — at least 3 concurrency stress tests demonstrating snapshot isolation, write serialization, and high-contention reader/writer scenarios with no data races, panics, or corruption.
4. **Fuzz testing runs without crashes** — `cargo fuzz` harness exists and runs for the specified duration without panics or memory-safety issues. A minimum 60-second run with no findings.
5. **All public API functions have doc-tests** — every `pub fn`, `pub struct` (with usage), `pub enum` (with construction), `pub trait` (with implementation sketch), and `pub type` in the crate has at least one runnable `/// # Examples` block. `cargo test --doc` passes with zero failures.
6. **`cargo test` passes** — all existing tests still pass, plus all new tests.
7. **`cargo clippy --all-targets --all-features -- -D warnings`** — zero warnings.
8. **`cargo doc --no-deps`** — zero warnings.

---

## Key Pitfalls and Edge Cases

1. **Test isolation.** Each integration test must create its own database (temp file or in-memory). Tests must not share state. Use `tempfile::TempDir` for persistent tests to ensure cleanup.

2. **Extension re-registration after reopen.** Extension *trait objects* are not serialized. After closing and reopening a database, the application must re-register extensions. The test must verify: (a) `missing_extensions()` reports the expected names before re-registration, (b) after re-registration, operations work correctly, and (c) previously materialized inferred facts are still present.

3. **Doc-test `no_std` types.** Doc-tests for types in `no_std` modules (`types/`, `schema/`, `constraint/`, `inference/`, `error/`) run under `std` (Rust's doc-test harness requires `std`). This is fine — the types are usable under both. Just don't use `std`-only features in the doc-test code for these modules.

4. **Fuzz target scope.** Focus fuzz targets on deserialization paths (parsing page bytes, record bytes, key bytes) and public API entry points (creating a database from arbitrary config, inserting nodes with arbitrary data). Do not fuzz internal `unsafe` code directly — fuzz the safe wrappers that call it.

5. **Concurrency test determinism.** Multi-threaded tests are inherently non-deterministic. Use barriers (`std::sync::Barrier`) and channels to synchronize threads to specific orderings where needed. For stress tests, assert invariants (e.g., "all reads return consistent snapshots") rather than specific orderings.

6. **Test runtime.** Fuzz tests and stress tests can be slow. Mark long-running tests with `#[ignore]` and a comment explaining the expected duration. CI can run them with `cargo test -- --ignored`.

7. **`Database` sharing across threads.** Wrap in `Arc<Database>`. Transactions are `!Send` — each thread must create its own transaction.

---

## Test Organization

```
tests/
├── integration/
│   ├── mod.rs                    // shared test helpers (graph builders, assertion utilities)
│   ├── e2e_basic_crud.rs         // Scenario 1: basic CRUD round-trip
│   ├── e2e_schema_hierarchy.rs   // Scenario 2: type hierarchy and subtype queries
│   ├── e2e_persistence.rs        // Scenario 3: persistence close/reopen round-trip
│   ├── e2e_extension_roundtrip.rs// Scenario 4: full extension system round-trip
│   ├── e2e_cross_backend.rs      // Scenario 5: persistent vs in-memory equivalence
│   ├── e2e_complex_traversal.rs  // Scenario 6: complex multi-hop traversal
│   └── e2e_edge_cases.rs         // Scenario 7: edge cases and error paths
├── concurrency/
│   ├── mod.rs                    // shared concurrency test helpers
│   ├── snapshot_isolation.rs     // reader/writer isolation stress
│   ├── write_contention.rs       // write serialization under contention
│   └── high_throughput.rs        // high reader/writer throughput stress
└── helpers/
    └── mod.rs                    // test-only ConstraintValidator and InferenceRule impls

fuzz/
├── Cargo.toml
└── fuzz_targets/
    ├── fuzz_record_deser.rs      // fuzz record deserialization
    └── fuzz_api_operations.rs    // fuzz API operation sequences
```

**Note:** The exact file layout depends on what already exists from Tasks 22–27. If `tests/` already has integration tests, merge into the existing structure rather than creating a parallel structure. The checklist specifies this more concretely.

---

## Error Handling in Tests

Tests should verify error conditions explicitly, not just success paths:

| Situation | Expected Error |
|-----------|---------------|
| Get nonexistent node | `Error::NotFound(NotFoundError::Node(id))` |
| Delete nonexistent edge | `Error::NotFound(NotFoundError::Edge(id))` |
| Duplicate type name | `Error::Schema(SchemaError::DuplicateTypeName { .. })` |
| Constraint violation at commit | `Error::ConstraintViolation(violations)` |
| Invoke unregistered inference rule | `Error::Inference(InferenceError::RuleNotFound(name))` |
| Write operations on read transaction | `Error::Transaction(TransactionError::ReadOnly)` |
