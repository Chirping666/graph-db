//! Fuzz target for interior page parsing.
//!
//! Feeds arbitrary byte sequences into `InteriorPage::parse` to surface panics,
//! crashes, or memory-safety issues. `Err` returns for invalid data are
//! expected and correct — only panics are bugs.
//!
//! Run with: `cargo +nightly fuzz run interior_page_parse -- -max_total_time=60`

#![no_main]
use libfuzzer_sys::fuzz_target;

use phonograph_db::storage::page::interior::InteriorPage;
use phonograph_db::storage::page::DEFAULT_PAGE_SIZE;

fuzz_target!(|data: &[u8]| {
    let _ = InteriorPage::parse(data, DEFAULT_PAGE_SIZE);

    // Also try with the data length as page size to exercise boundary checks.
    let _ = InteriorPage::parse(data, data.len());
});
