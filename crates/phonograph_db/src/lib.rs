//! Embeddable graph database with B+ tree storage and MVCC transactions.
//!
//! `phonograph_db` is the database engine crate for the Phonograph graph
//! database. It provides storage backends, B+ tree indexing, buffer pool
//! management, and transactional access to a typed property graph.
//!
//! This crate compiles under `no_std + alloc`.

#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

pub mod backend;
#[cfg(feature = "alloc")]
pub mod backend_mem;
#[cfg(feature = "alloc")]
pub mod error;
#[cfg(feature = "alloc")]
pub(crate) mod sync;
#[cfg(feature = "alloc")]
pub mod storage;
#[cfg(feature = "alloc")]
pub mod db;

// Re-export the vocabulary crate (decision R14).
#[cfg(feature = "alloc")]
pub use phonograph::*;
