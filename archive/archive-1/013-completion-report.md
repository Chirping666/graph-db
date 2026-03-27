# Completion Report: Task 13 — Generate: Top-Level CLAUDE.md

## Status: COMPLETE

## Done Criterion:

The criterion requires CLAUDE.md to define:

1. **Session workflow (8 steps)** — ✓ Steps 1–8 are defined in the "Session Workflow" section: (1) read design document, (2) read scoped CLAUDE.md, (3) read checklist.md, (4) review existing code, (5) create session plan and confirm, (6) implement one checklist item at a time, (7) run tests after each item, (8) produce completion report.

2. **No external DB crate dependencies rule** — ✓ Rule 1 specifies the prohibition with an explicit allowed-dependencies list.

3. **`no_std + alloc` for core code rule** — ✓ Rule 2 specifies which modules must be `no_std`, the feature flag structure, and the verification command.

4. **No baked-in ontology model rule** — ✓ Rule 3 prohibits hardcoded ontology vocabularies and clarifies the mechanism-not-policy boundary.

5. **Documentation on every public item rule** — ✓ Rule 4 specifies doc comment requirements for methods (errors, panics, performance), modules, and the crate root, with a verification command.

6. **Test coverage expectations** — ✓ Rule 5 specifies unit test, integration test, and test organization requirements.

7. **Commit message conventions** — ✓ Rule 6 defines the format, types, scopes, and examples.

All criteria met.

## Deliverables:

- `CLAUDE.md` — project-root file governing Claude Code behavior for all implementation sessions

## Summary:

Produced the top-level CLAUDE.md that governs Claude Code's behavior for all implementation sessions. The document codifies the 8-step session workflow, seven project-wide rules (dependencies, `no_std` boundary, ontology neutrality, documentation, testing, commit messages, code style), the complete module layout, a quick-reference table into the design document, and the five residual concerns carried forward from the design phase.

Key decisions made during this task:
- Included `cargo clippy` and `cargo doc` checks as mandatory per-item verification (not just `cargo test`), because the design document's documentation requirements (§16.7) and code quality expectations demand it.
- Added `cargo check --no-default-features --features alloc` as a mandatory `no_std` verification step, because the `no_std` boundary is a project constraint that cannot be verified by `cargo test` alone (which runs under `std` by default).
- Included the module layout directly in CLAUDE.md (mirroring §3.1 of the design document) so that Claude Code has the project structure immediately visible without needing to navigate to `012-design-document.md` for basic orientation.

## Context for Next Task:

**Task 14 (Generate: Core Data Model & Types — Scoped CLAUDE.md & Checklist)** should read `CLAUDE.md` (this task's deliverable) and `012-design-document.md`. Key items for Task 14:

- The core types module (`types/`) is specified in `012` §4 (IDs, Value, Node, Edge, TypeDefinition, PropertyDeclaration) and §5 (TypeKind, ValueTypeDescriptor).
- The schema traits module (`schema/`) is specified in `012` §5.3–5.4 (TypeRegistryView, PropertyKeyRegistryView).
- The constraint types module (`constraint/`) is specified in `012` §13 (ConstraintValidator, ChangeSet, ConstraintViolation).
- The inference types module (`inference/`) is specified in `012` §14 (InferenceRule, InferenceMode, InferenceResult, ProvenanceRecord, MaterializedMapping).
- The error types module (`error/`) is specified in `012` §15.6 and §16.1.
- All of these modules must compile under `no_std + alloc` (Rule 2 in CLAUDE.md).
- The checklist should include a final item verifying `cargo check --no-default-features --features alloc`.

## Residual Concerns:

None beyond the five design-phase residuals already documented in the "Known Residual Concerns" section of CLAUDE.md.

## Upstream Flags:

None.
