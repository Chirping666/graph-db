//! CoW B+ tree operations.
//!
//! Provides search, insert, delete, and range scan over the eight
//! logical B-trees stored in the database file. All mutations use
//! copy-on-write (CoW) to preserve snapshot isolation.

pub mod search;
pub mod insert;
pub mod delete;
pub mod cursor;
pub mod cow;
