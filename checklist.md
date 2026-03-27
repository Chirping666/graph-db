# checklist.md — Cleanup, Hardening & Edition Upgrade

**Task:** 31  
**CLAUDE.md:** `031-cleanup-hardening/CLAUDE.md`  
**Status:** Pending

Execute items in order. Each step has a verification command — do not proceed until
it passes. After completing each step and passing its verification, mark it done by
changing `- [ ]` to `- [x]` in this file.

---

## Phase 0: Baseline Snapshot

- [x] **0.1 — Record the current test baseline.**
  ```bash
  cargo test --workspace 2>&1 | tail -5
  cargo clippy --workspace --all-targets -- -D warnings
  ```
  Record the exact test count. This is the regression baseline — every test that
  passes here must still pass at the end (with allowed adjustments for changed
  feature gates and reverted let-chains).
  **Verify:** Zero failures, zero clippy warnings.

---

## Phase 1: Edition Upgrade

> **Reference:** CLAUDE.md D1, D3

This phase upgrades the edition and removes MSRV pinning. Do this first because
edition 2024 enables let-chain syntax, which Phase 1 also restores.

- [ ] **1.1 — Update workspace `Cargo.toml`.**
  - Change `edition = "2021"` → `edition = "2024"` in `[workspace.package]`.
  - Remove `rust-version = "1.82"` from `[workspace.package]`.
  - **Verify:** `cargo check --workspace` passes. (If your local toolchain is older
    than 1.85, update it first — edition 2024 requires Rust 1.85+.)

- [ ] **1.2 — Update per-crate `Cargo.toml` files.**
  For each of the three crates (`crates/phonograph/Cargo.toml`,
  `crates/phonograph_db/Cargo.toml`, `crates/phonograph_std/Cargo.toml`):
  - If the crate has its own `edition` key, change it to `"2024"` or remove it
    (it inherits from `workspace.package`).
  - Remove any `rust-version` field.
  - **Verify:** `cargo check --workspace` passes.

- [ ] **1.3 — Revert let-chain refactors.**
  Locate the 5 sites where let-chains were refactored to nested `if` blocks for
  edition 2021 compatibility. Restore the original `if let ... && ...` syntax.

  Known sites (approximate line numbers — search for the nested `if let` patterns):
  - `crates/phonograph_db/src/storage/btree/cursor.rs` (~line 160)
  - `crates/phonograph_db/src/db/database.rs` (~line 405)
  - `crates/phonograph_db/src/db/write_txn.rs` (~line 938)
  - `crates/phonograph_std/tests/inference_tests.rs` (~line 811)
  - `crates/phonograph_std/tests/e2e_integration.rs` (~line 563)

  For each site: read the surrounding code to understand the condition, then
  collapse the nested `if`/`if let` back into a single `if let ... && ...` chain.

  **Verify:**
  ```bash
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  ```

### ▸ Phase 1 Gate

- [ ] **Phase 1 gate:**
  ```bash
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  cargo doc --workspace --no-deps
  ```
  All pass. The workspace is now on edition 2024 with no MSRV constraint.

---

## Phase 2: Feature Flag Simplification

> **Reference:** CLAUDE.md D2

This phase removes the unnecessary `alloc` feature from `phonograph` and
`phonograph_db`, making `extern crate alloc` unconditional.

**⚠ Order matters.** Start with `phonograph` (no dependents in the vocabulary
direction), then `phonograph_db` (depends on `phonograph`), then update
`phonograph_std` and any workspace-level references.

- [ ] **2.1 — Simplify `phonograph` feature flags.**
  In `crates/phonograph/Cargo.toml`:
  - Change the `[features]` section to:
    ```toml
    [features]
    default = ["std"]
    std = []
    ```
  - Remove the `alloc = []` line.
  - Remove `tempfile = "3"` from `[dev-dependencies]` (a vocabulary crate has no
    use for temp files).
  - **Verify:** `cargo check -p phonograph` passes.

- [ ] **2.2 — Remove `#[cfg(feature = "alloc")]` gates from `phonograph` source.**
  In `crates/phonograph/src/lib.rs`:
  - Remove all `#[cfg(feature = "alloc")]` attributes from `pub mod` declarations
    and `pub use` re-exports. These modules are now unconditional.
  - Ensure `extern crate alloc;` is present unconditionally (remove any
    `#[cfg(feature = "alloc")]` guard on it).
  - Keep `#![cfg_attr(not(feature = "std"), no_std)]` — this is still needed.
  - Remove any `#[cfg(feature = "alloc")]` from compile-test assertions
    (e.g., `Send + Sync` static assertions).

  Scan all files under `crates/phonograph/src/` for remaining `cfg(feature = "alloc")`
  and remove them:
  ```bash
  grep -rn 'cfg.*feature.*alloc' crates/phonograph/src/
  ```
  Must return empty.

  **Verify:**
  ```bash
  cargo check -p phonograph
  cargo check -p phonograph --no-default-features
  ```
  Both pass. The second command now compiles the full crate (minus `std::error::Error`
  impls), not an empty shell.

- [ ] **2.3 — Simplify `phonograph_db` feature flags.**
  In `crates/phonograph_db/Cargo.toml`:
  - Change the `[features]` section to:
    ```toml
    [features]
    default = ["std"]
    std = ["phonograph/std"]
    ```
  - Remove the `alloc = [...]` line.
  - Update the `phonograph` dependency: remove `features = ["alloc"]` from the
    dependency specification (alloc is no longer a feature). Keep
    `default-features = false`.
    ```toml
    phonograph = { version = "0.1", path = "../phonograph", default-features = false }
    ```
  - **Verify:** `cargo check -p phonograph_db` passes.

- [ ] **2.4 — Remove `#[cfg(feature = "alloc")]` gates from `phonograph_db` source.**
  In `crates/phonograph_db/src/lib.rs`:
  - Remove all `#[cfg(feature = "alloc")]` attributes from `pub mod` declarations,
    `pub use` re-exports, and the `extern crate alloc;` statement.
  - Keep `#![cfg_attr(not(feature = "std"), no_std)]`.

  Scan all files under `crates/phonograph_db/src/` for remaining
  `cfg(feature = "alloc")` and remove them:
  ```bash
  grep -rn 'cfg.*feature.*alloc' crates/phonograph_db/src/
  ```
  Must return empty.

  **Verify:**
  ```bash
  cargo check -p phonograph_db
  cargo check -p phonograph_db --no-default-features
  ```
  Both pass.

- [ ] **2.5 — Update `phonograph_std` dependency specification.**
  In `crates/phonograph_std/Cargo.toml`:
  - If the `phonograph` or `phonograph_db` dependencies reference `features = ["alloc"]`,
    remove that. The `alloc` feature no longer exists.
  - **Verify:** `cargo check -p phonograph_std` passes.

- [ ] **2.6 — Update workspace-level references.**
  In the workspace root `Cargo.toml`, if `[workspace.dependencies]` specifies
  `features = ["alloc"]` for `phonograph` or `phonograph_db`, remove it.

  Scan the entire workspace for any remaining references to the `alloc` feature:
  ```bash
  grep -rn 'features.*alloc' crates/ Cargo.toml
  ```
  Must return empty (except comments or documentation).

  **Verify:** `cargo check --workspace` passes.

### ▸ Phase 2 Gate

- [ ] **Phase 2 gate:**
  ```bash
  cargo check --workspace
  cargo check -p phonograph --no-default-features
  cargo check -p phonograph_db --no-default-features
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  ```
  All pass. The `--no-default-features` commands now compile fully functional
  `no_std` crates, not empty shells.

---

## Phase 3: Fix Large Property Value Panic

> **Reference:** CLAUDE.md D4

This phase fixes the crash bug where inserting a property value >~1 KB causes
a subtraction overflow panic in the leaf page handling code.

**Background:** The overflow page infrastructure exists and works (`OverflowPage::build`,
`build_chain`, `read_chain` are implemented and tested). The bug is in the B-tree insert
path — when a record's serialized value exceeds the inline threshold, the code attempts
to store it inline anyway, causing an arithmetic underflow.

- [ ] **3.1 — Locate and diagnose the exact panic site.**
  The panic was reported at `leaf.rs:215` (approximate — line numbers may have
  shifted during migration). Reproduce it:
  ```rust
  // In a test:
  let db = /* open in-memory database */;
  let mut wtx = db.write_txn().unwrap();
  let nt = wtx.register_type(TypeDefinitionBuilder::node_type("Big").build()).unwrap();
  let key = wtx.get_or_create_property_key("data").unwrap();
  let big_value = Value::Bytes(vec![0xABu8; 10_000]);
  // This should NOT panic:
  wtx.insert_node(
      NodeBuilder::new().type_label(nt).property(key, big_value).build()
  ).unwrap();
  wtx.commit().unwrap();
  ```
  Run the test, confirm the panic, and identify the exact code path. Document the
  call chain from `insert_node` → serialization → B-tree insert → leaf page handling.

  **Verify:** You can reproduce the panic and understand the root cause.

- [ ] **3.2 — Implement overflow dispatch in the B-tree insert path.**
  The fix requires the B-tree insert code to detect when a serialized record value
  exceeds the inline leaf cell threshold and dispatch to overflow page storage
  instead. This involves:

  1. **Determine the overflow threshold.** Per `008-file-format-spec.md` §8.2, a
     leaf cell triggers overflow when `total_cell_size > (page_size - 44) / 4`.
     For 4 KB pages: `(4096 - 44) / 4 = 1013` bytes.

  2. **At the insert site**, check the serialized value length against the threshold.
     If it exceeds the threshold:
     - Allocate overflow pages via the page allocator.
     - Write the value to overflow pages using `OverflowPage::build_chain`.
     - Create a `LeafCellValue::Overflow { overflow_page_id, total_overflow_len }`
       cell instead of `LeafCellValue::Inline(value_bytes)`.

  3. **At the read site**, the overflow read path should already work (leaf page
     parsing detects `OVERFLOW_SENTINEL` and returns `LeafCellValue::Overflow`).
     Verify that the record deserialization code handles reading from overflow
     pages when it encounters an overflow cell.

  The exact location of the fix depends on the codebase structure. The likely
  insertion points are in `phonograph_db/src/storage/` — either in the B-tree
  insert module or in the storage engine's record-writing logic.

  **⚠ Pitfall — CoW semantics.** Overflow pages allocated during a write transaction
  must follow the same Copy-on-Write discipline as B-tree pages. They are allocated
  fresh (never reuse existing overflow pages) and become garbage when the record is
  updated or deleted.

  **⚠ Pitfall — record updates.** When updating a node/edge whose old value was
  inline but the new value requires overflow (or vice versa), the transition must
  be handled correctly. Also handle the case where both old and new values overflow.

  **Verify:** The panic from step 3.1 no longer occurs.

- [ ] **3.3 — Add tests for large property values.**
  Add integration tests (in `phonograph_std/tests/` or as a new test file):

  1. **10 KB value round-trip:** Insert a node with `Value::Bytes(vec![0xAB; 10_000])`,
     commit, read back, verify exact match.
  2. **100 KB value round-trip:** Same with 100,000 bytes. Exercises multi-page
     overflow chains.
  3. **Update inline → overflow:** Insert a node with a small property (50 bytes),
     update it to a large property (10 KB), read back and verify.
  4. **Update overflow → inline:** Insert with 10 KB, update to 50 bytes, verify.
  5. **Update overflow → overflow:** Insert with 10 KB, update to 20 KB, verify.
  6. **Delete node with overflow value:** Insert with 10 KB, delete the node, commit.
     Verify no crash (overflow pages are freed or become garbage).
  7. **Multiple overflow nodes:** Insert 10 nodes each with 5 KB values. Read all
     back. Verify all values match.

  **Verify:**
  ```bash
  cargo test --workspace -- large_property
  ```
  All tests pass.

- [ ] **3.4 — Update the existing "moderately large" test.**
  The test `e2e_moderately_large_property_value` in
  `crates/phonograph_std/tests/e2e_integration.rs` currently uses 500 bytes with a
  comment noting that 10 KB+ values panic. Update the test:
  - Increase the test value to 10,000 bytes (or add a companion test at that size).
  - Remove the comment about the known bug.

  **Verify:** `cargo test --workspace -- moderately_large` passes.

### ▸ Phase 3 Gate

- [ ] **Phase 3 gate:**
  ```bash
  cargo test --workspace
  ```
  All tests pass, including the new large-value tests. Zero panics.

---

## Phase 4: Panic-to-Result Conversions

> **Reference:** CLAUDE.md D5, audit `archive/audits/2026-03-26-codebase-audit.md` §3

This phase converts remaining `assert!`/`panic!` in public library methods to proper
`Result` error returns. Test-only panics and `debug_assert!` are left as-is.

- [ ] **4.1 — Audit remaining panics in `phonograph_db` public API.**
  Search for panic-inducing patterns in library code:
  ```bash
  grep -rn 'assert!\|panic!\|\.unwrap()\|\.expect(' crates/phonograph_db/src/ | grep -v '#\[cfg(test)\]' | grep -v 'mod tests'
  grep -rn 'assert!\|panic!\|\.unwrap()\|\.expect(' crates/phonograph/src/ | grep -v '#\[cfg(test)\]' | grep -v 'mod tests'
  grep -rn 'assert!\|panic!\|\.unwrap()\|\.expect(' crates/phonograph_std/src/ | grep -v '#\[cfg(test)\]' | grep -v 'mod tests'
  ```
  Categorize each hit:
  - **User-triggerable** → must convert to `Result`. Examples: `OverflowPage::build`
    assert on data length, `LeafPage::build` assert on page capacity.
  - **Internal invariant** (only reachable if the library itself has a bug) → acceptable
    as `debug_assert!` or a comment explaining why it's unreachable.
  - **Spin mutex `.lock()`** → verify that `spin::Mutex::lock()` returns the guard
    directly (not `Result`). If so, no `.unwrap()` is needed and any existing
    `.unwrap()` calls are incorrect (they should just be `.lock()`). If `spin::Mutex`
    does return `Result`, the `.unwrap()` is acceptable (poisoned mutex = prior panic =
    unrecoverable).

  Produce a list of all sites that need conversion. Document it.

  **Verify:** List is complete and categorized.

- [ ] **4.2 — Convert `OverflowPage::build` and `build_chain` panics.**
  `OverflowPage::build` currently panics via `assert!` if `data.len() > max_payload`.
  `OverflowPage::build_chain` panics if `page_ids` is empty or insufficient.

  Both are public methods. Convert to `Result<Vec<u8>, StorageError>` and
  `Result<Vec<Vec<u8>>, StorageError>` respectively.

  Update all call sites.

  **Verify:** `cargo test --workspace` passes.

- [ ] **4.3 — Convert `LeafPage::build` panic.**
  `LeafPage::build` currently panics via `assert!` if cells don't fit in the page.
  Convert to `Result<Vec<u8>, StorageError>`.

  Update all call sites.

  **Verify:** `cargo test --workspace` passes.

- [ ] **4.4 — Convert any other user-triggerable panics found in 4.1.**
  Work through the remaining items from the 4.1 audit. For each:
  - Change the method signature to return `Result` (if not already).
  - Replace the `assert!`/`panic!`/`unwrap()` with a `map_err` or early `return Err(...)`.
  - Update call sites.

  **Verify:** `cargo test --workspace` passes after each conversion.

- [ ] **4.5 — Verify no remaining unwraps in `leaf.rs` parse paths.**
  The `parse` method in `leaf.rs` still contains two `try_into().unwrap()` calls
  inside the overflow cell parsing branch (parsing `overflow_page_id` and
  `total_overflow_len`). These are on slices that are bounds-checked above, but
  they should use `map_err` for consistency and defense-in-depth.

  ```bash
  grep -n 'unwrap()' crates/phonograph_db/src/storage/page/leaf.rs
  ```

  Convert any remaining production-code `.unwrap()` to `.map_err(...)`.

  **Verify:**
  ```bash
  grep -n 'unwrap()' crates/phonograph_db/src/storage/page/leaf.rs
  ```
  Returns only lines inside `#[cfg(test)]` blocks.

### ▸ Phase 4 Gate

- [ ] **Phase 4 gate:**
  ```bash
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  ```
  All pass. No panics in public library code on user-controllable input.

---

## Phase 5: Documentation & Metadata Cleanup

- [ ] **5.1 — Update CHANGELOG.md.**
  - Remove "Large property values (>~1 KB) may cause panics" from Known Limitations.
  - Add a `### Fixed` section under `## [0.1.0]`:
    ```
    ### Fixed
    - Large property values (>1 KB) now correctly use overflow pages instead of panicking
    - Public API methods no longer panic on user-controllable input (return Result instead)
    ```
  - Update "Known Limitations" to reflect only the genuine remaining limitations:
    - `nodes_by_property()` performs a full scan (no property value index)
    - Query methods return owned `Vec`s (no streaming iterator API)
    - No batch insert API
    - `write_txn()` blocks indefinitely (no configurable timeout)
    - Provenance registry loaded entirely in memory

  **Verify:** CHANGELOG.md is well-formatted.

- [ ] **5.2 — Update README.md.**
  - Remove "Large property values (>~1 KB) are not supported in v0.1" from Known
    Limitations.
  - Verify the feature flag documentation reflects the simplified structure (no `alloc`
    feature).

  **Verify:** README.md is accurate.

- [ ] **5.3 — Update project root `CLAUDE.md`.**
  - Update Rule 2 (`no_std + alloc` boundary): remove references to the `alloc` feature
    flag. The boundary is now just `#![cfg_attr(not(feature = "std"), no_std)]` +
    unconditional `extern crate alloc`.
  - Remove any `rust-version` or MSRV references.
  - Update the verification command: `cargo check -p phonograph --no-default-features`
    (no `--features alloc`).
  - Update feature flag structure documentation to match the new simplified flags.

  **Verify:** CLAUDE.md is accurate and internally consistent.

### ▸ Phase 5 Gate

- [ ] **Phase 5 gate:**
  ```bash
  cargo doc --workspace --no-deps
  ```
  Zero warnings.

---

## Phase 6: Final Verification

- [ ] **6.1 — Full workspace build, test, lint, docs.**
  ```bash
  cargo build --workspace
  cargo test --workspace
  cargo clippy --workspace --all-targets -- -D warnings
  cargo doc --workspace --no-deps
  ```
  All pass with zero warnings.

- [ ] **6.2 — `no_std` verification.**
  ```bash
  cargo check -p phonograph --no-default-features
  cargo check -p phonograph_db --no-default-features
  ```
  Both pass. These now compile the full crates under `no_std + alloc` without
  needing `--features alloc`.

- [ ] **6.3 — Regression test count.**
  Compare `cargo test --workspace` output against the Phase 0 baseline. All
  previously passing tests must still pass. New tests from Phase 3 should appear
  in the count.

- [ ] **6.4 — Confirm no remaining audit artifacts.**
  ```bash
  grep -rn 'cfg.*feature.*alloc' crates/phonograph/src/ crates/phonograph_db/src/
  grep -rn 'rust-version' crates/*/Cargo.toml Cargo.toml
  grep -rn 'edition = "2021"' crates/*/Cargo.toml Cargo.toml
  ```
  All three commands return empty.

- [ ] **6.5 — Examples still run.**
  ```bash
  cargo run -p phonograph_std --example basic_usage
  cargo run -p phonograph_std --example owl_lite_ontology
  ```
  Both succeed without panics.

### ▸ Phase 6 Gate — TASK COMPLETE

- [ ] **All verification checks from Phase 6 pass.**
  Write a completion report to `archive/completion-reports/31-cleanup-hardening.md`
  documenting:
  - Status
  - What was changed (summary)
  - Test count before and after
  - Files modified
  - Residual concerns (if any)
