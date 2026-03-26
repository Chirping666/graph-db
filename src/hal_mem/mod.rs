//! In-memory storage backend.
//!
//! Provides [`MemoryBackend`], an in-memory storage backend backed by a
//! `Vec<u8>`. Useful for testing, ephemeral databases, and `no_std + alloc`
//! environments without a filesystem.
//!
//! `MemoryBackend` implements the same HAL traits ([`ReadAt`](crate::hal::ReadAt),
//! [`WriteAt`](crate::hal::WriteAt), [`hal::Sync`](crate::hal::Sync),
//! [`StorageBackend`](crate::hal::StorageBackend)) as
//! [`FileBackend`](crate::hal_std::FileBackend), so the entire storage engine
//! operates identically regardless of which backend is in use.
//!
//! # Optional snapshot support
//!
//! With the `std` feature enabled, [`MemoryBackend`] can save its contents to
//! a file ([`save_to_file`](MemoryBackend::save_to_file)) and load from one
//! ([`load_from_file`](MemoryBackend::load_from_file)). The resulting file is
//! byte-for-byte identical to one created by `FileBackend`.

pub mod memory_backend;

pub use memory_backend::{MemoryBackend, MemoryError};
