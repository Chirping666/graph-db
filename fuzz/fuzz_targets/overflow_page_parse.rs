//! Fuzz target for overflow page parsing.
//!
//! Feeds arbitrary byte sequences into `OverflowPage::parse` to surface panics,
//! crashes, or memory-safety issues. `Err` returns for invalid data are
//! expected and correct — only panics are bugs.
//!
//! Run with: `cargo +nightly fuzz run overflow_page_parse -- -max_total_time=60`

#![no_main]
use libfuzzer_sys::fuzz_target;

use phonograph_db::storage::page::overflow::OverflowPage;
use phonograph_db::storage::page::DEFAULT_PAGE_SIZE;

fuzz_target!(|data: &[u8]| {
    let _ = OverflowPage::parse(data, DEFAULT_PAGE_SIZE);

    // Also try with the data length as page size to exercise boundary checks.
    let _ = OverflowPage::parse(data, data.len());
});
