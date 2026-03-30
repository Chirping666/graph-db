//! Fuzz target for superblock validation and deserialization.
//!
//! Feeds arbitrary byte sequences into `Superblock::validate` and
//! `Superblock::deserialize` to surface panics, crashes, or memory-safety
//! issues. `Err` returns for invalid data are expected and correct — only
//! panics are bugs.
//!
//! Run with: `cargo +nightly fuzz run superblock_validation -- -max_total_time=60`

#![no_main]
use libfuzzer_sys::fuzz_target;

use phonograph_db::storage::format::{FileIdentityHeader, Superblock};

fuzz_target!(|data: &[u8]| {
    let _ = Superblock::validate(data);
    let _ = Superblock::deserialize(data);
    let _ = FileIdentityHeader::deserialize(data);
});
