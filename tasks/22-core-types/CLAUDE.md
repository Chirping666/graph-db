# CLAUDE.md — Task 22: Implement Core Data Model & Types

**Project:** Embedded Graph Database with Extensible Schema & Pluggable Inference
**Implementation Task:** 22 (preparation task: 14)
**Module:** `src/types/`, `src/schema/`, `src/constraint/`, `src/inference/`, `src/error/`
**Status:** Pending
**Depends on:** None (foundational task)

---

## Objective

Implement all foundational Rust types for the embedded graph database crate: newtype identifiers, the dynamically-typed value system, property bags, node and edge structs, the type/schema system (type definitions, type hierarchy, property declarations), read-only schema view traits, the constraint validation trait and its supporting types, the inference rule trait and its supporting types, and the unified error type hierarchy.

These types form the `no_std + alloc` core that every other module in the crate depends on. No type defined in this task may require `std`. All types must be fully documented and unit-tested.

---

## Required Reading

Before writing any code, read these documents from the project knowledge. Read them in the order listed — later documents build on earlier ones.

1. **`012-design-document.md`** — The single authoritative design reference. Read at minimum:
   - §2 (Architecture overview & layer diagram)
   - §3 (Crate structure & feature flags)
   - §4 (Core data model — IDs, Value, Node, Edge)
   - §5 (Type system & schema)
   - §13 (Constraint validation)
   - §14 (Inference hook architecture)
   - §15 (Public API surface — especially §15.6 Error types)
   - §16 (Cross-cutting concerns — error handling, naming, `no_std` boundary)

2. **`006-schema-extension-spec.md`** — The upstream specification for all types and traits in this task. This document contains the full Rust type signatures with doc comments. When in doubt about a type's fields, derives, or doc comments, this document is authoritative (refined by `012-design-document.md` where the two disagree).

3. **`010-api-surface-spec.md`** — Defines the `Error` hierarchy (§4), `InferenceMode` (§6.3), and the `GraphReader` trait (§10.1) which is *not* part of this task but helps understand the boundary. Read §4 (Error Handling) and §6.3 (InferenceMode) specifically.

4. **`011-inference-hook-design.md`** — Defines `ProvenanceRecord`, `InferredEntity`, `MaterializedMapping`, and the `ProvenanceRegistry` (internal). Read §7 (Result Representation), §8 (Provenance Tracking). Only the data types go in this task — the engine logic is Task 18.

5. **`CLAUDE.md` (project root)** — Project-wide rules: `no_std + alloc` requirements, documentation standards, test expectations, code style, module layout, feature flags.

---

## Modules Produced by This Task

| Module | Path | Contents |
|--------|------|----------|
| **types** | `src/types/mod.rs` | `NodeId`, `EdgeId`, `TypeId`, `PropertyKeyId`, `Value`, `ValueTypeDescriptor`, `PropertyMap`, `Node`, `Edge`, `TypeKind`, `TypeDefinition`, `PropertyDeclaration` |
| **schema** | `src/schema/mod.rs` | `GraphView` trait, `TypeRegistryView` trait, `PropertyKeyRegistryView` trait |
| **constraint** | `src/constraint/mod.rs` | `NodeChange`, `EdgeChange`, `ChangeSet`, `ConstraintViolation`, `ViolationSubject`, `ConstraintValidator` trait |
| **inference** | `src/inference/mod.rs` | `InferredFact`, `InferenceResult`, `InferenceMode`, `ProvenanceRecord`, `InferredEntity`, `MaterializedMapping`, `InferenceRule` trait |
| **error** | `src/error/mod.rs` | `Error`, `SchemaError`, `StorageError`, `NotFoundError`, `TransactionError`, `InferenceError` |
| **lib.rs** | `src/lib.rs` | Crate root with `no_std` attribute, feature flag configuration, module declarations, re-exports |
| **Cargo.toml** | `Cargo.toml` | Crate metadata, feature flags (`std`, `alloc`), no external DB dependencies |

---

## Definition of Done

All of the following must be true before this task is COMPLETE:

1. **All types compile under `no_std + alloc`.** Verified by `cargo check --no-default-features --features alloc`.

2. **All types also compile under `std` (default features).** Verified by `cargo check`.

3. **Every `pub` item has a doc comment** (`///`) following the standards in the project-root `CLAUDE.md` Rule 4. Verified by `cargo doc --no-deps` producing zero warnings.

4. **Every newtype ID has**: `Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug` derives, a `pub` inner field, and tests for construction, comparison, and sentinel value behavior.

5. **`Value` has**: `Clone, Debug, PartialEq` derives (NOT `Eq` — `f64` prevents it). A `matches_descriptor(&self, &ValueTypeDescriptor) -> bool` helper method is tested for every variant combination.

6. **`Node` and `Edge` have**: `Clone, Debug, PartialEq` derives. Constructor functions and tests for basic construction.

7. **`TypeDefinition` has**: `Clone, Debug, PartialEq` derives. Tests for construction with supertypes and property declarations.

8. **All trait definitions compile** and are object-safe (can be used as `dyn Trait`). `ConstraintValidator` and `InferenceRule` require `Send + Sync`. Verified by a compile-time test.

9. **All error types** implement `Debug` and `Display` (via `core::fmt`). The top-level `Error` has `From` impls for all inner error types. `StorageError` conditionally includes `source: Option<std::io::Error>` behind `#[cfg(feature = "std")]`.

10. **Unit tests pass**: `cargo test` with zero failures.

11. **Clippy clean**: `cargo clippy --all-targets --all-features -- -D warnings` with zero warnings.

12. **Types match the design documents.** Field names, types, derives, and doc comments match `006-schema-extension-spec.md` and `012-design-document.md`. Any intentional deviations from the spec are documented in the completion report with rationale.

---

## Key Pitfalls and Edge Cases

These are the known hazards for this task. The checklist calls them out at the relevant steps.

1. **`Value` does not implement `Eq`** due to `f64`. Do not derive `Eq` on `Value`, `Node`, `Edge`, `PropertyDeclaration`, `TypeDefinition`, `InferredFact`, `InferenceResult`, or any type that transitively contains `Value`. Types that do not contain `Value` (IDs, `TypeKind`, `ValueTypeDescriptor`, `ViolationSubject`, error types, `InferenceMode`, `InferredEntity`) should derive `Eq`.

2. **`alloc` imports, not `std`.** Use `alloc::string::String`, `alloc::vec::Vec`, `alloc::collections::BTreeMap`, `alloc::boxed::Box` — never `std::` equivalents. Add `extern crate alloc;` in lib.rs, gated on `#[cfg(feature = "alloc")]`.

3. **`PropertyMap` is a type alias**, not a newtype. It must be `pub type PropertyMap = BTreeMap<PropertyKeyId, Value>;`.

4. **Reserved ID values.** `NodeId(0)`, `EdgeId(0)`, `TypeId(0)`, `PropertyKeyId(0)` are null sentinels. Provide `is_null()` and `NULL` constant on each ID type. Test these.

5. **`GraphView` trait must be object-safe.** No generic methods, no `Self: Sized` bounds on methods. It returns `Vec<&Node>` / `Vec<&Edge>` for collection methods (borrowed from the database's storage, not owned). Verify with a compile-time assertion (`fn _assert_object_safe(_: &dyn GraphView) {}`).

6. **`ChangeSet` has private fields with a public constructor.** The `node_changes` and `edge_changes` fields are `&'a [NodeChange]` and `&'a [EdgeChange]` — private, with accessor methods. Provide `pub fn new(...)` for construction by the database engine.

7. **`ConstraintValidator` and `InferenceRule` traits must require `Send + Sync`** as supertraits, since they are stored as `Box<dyn Trait>` in a multi-threaded `Database`.

8. **`StorageError` has conditional compilation.** The `source` field exists only with `#[cfg(feature = "std")]`. Implement `Display` for both configurations. Under `std`, implement `std::error::Error` for all error types.

9. **`InferredEntity` needs `Eq, Ord, Hash`** (it does not contain `Value`). This is required because it is used as a `BTreeMap` key in the `ProvenanceRegistry`.

10. **`ChangeSet` iterator methods return `impl Iterator`.** These use closures, which is fine for concrete types but prevents `ChangeSet` from being object-safe. This is acceptable — `ChangeSet` is a concrete struct, not a trait.

---

## Work Plan

Execute `checklist.md` sequentially. Each checklist item is one logical step. Run the verification checks after each step before proceeding.
