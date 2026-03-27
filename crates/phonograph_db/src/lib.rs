//! Embeddable graph database with B+ tree storage and MVCC transactions.
//!
//! `phonograph_db` is the database engine crate for the Phonograph graph
//! database. It provides storage backends, B+ tree indexing, buffer pool
//! management, and transactional access to a typed property graph.
//!
//! This crate compiles under `no_std + alloc`.

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod backend;
pub mod backend_mem;
pub mod error;
pub(crate) mod sync;
pub mod storage;
pub mod db;

// Re-export the vocabulary crate (decision R14).
pub use phonograph::*;
