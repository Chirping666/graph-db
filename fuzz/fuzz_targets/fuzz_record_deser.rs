//! Fuzz target for record deserialization.
//!
//! Feeds arbitrary byte sequences into deserialization functions to surface
//! panics, crashes, or memory-safety issues. `Err` returns for invalid data
//! are expected and correct — only panics are bugs.
//!
//! Run with: `cargo +nightly fuzz run fuzz_record_deser -- -max_total_time=60`

#![no_main]
use libfuzzer_sys::fuzz_target;

use phonograph::types::TypeId;
use phonograph_db::storage::serialization;

fuzz_target!(|data: &[u8]| {
    // Deserialize a Value from arbitrary bytes.
    let _ = serialization::deserialize_value(data);

    // Deserialize a PropertyMap from arbitrary bytes.
    let _ = serialization::deserialize_properties(data);

    // Deserialize a TypeDefinition from arbitrary bytes.
    let _ = serialization::deserialize_type_definition(TypeId(1), data);

    // Deserialize a property key name from arbitrary bytes.
    let _ = serialization::deserialize_property_key_name(data);

    // Deserialize a counter value from arbitrary bytes.
    let _ = serialization::deserialize_counter(data);

    // Deserialize a provenance record from arbitrary bytes.
    let _ = serialization::deserialize_provenance(data);

    // Deserialize node and edge records from arbitrary bytes.
    let _ = serialization::NodeRecord::deserialize(data);
    let _ = serialization::EdgeRecord::deserialize(data);
});
