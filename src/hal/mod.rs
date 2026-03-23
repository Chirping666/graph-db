//! Hardware Abstraction Layer (HAL) — trait definitions for storage I/O.
//!
//! This module defines the trait hierarchy that all storage backends
//! implement. The traits are `no_std + alloc` compatible and object-safe,
//! enabling both static dispatch (generics) and dynamic dispatch
//! (`dyn StorageBackend`).
//!
//! # Trait hierarchy
//!
//! ```text
//! StorageErrorType (associated Error type)
//!     ├── ReadAt   (&self — concurrent reads)
//!     ├── WriteAt  (&mut self — exclusive writes)
//!     └── Sync     (&mut self — durability control)
//!           └── StorageBackend = ReadAt + WriteAt + Sync (blanket impl)
//! ```
//!
//! Lifecycle traits ([`OpenableBackend`], [`LockableBackend`]) are `std`-only
//! and live in the [`lifecycle`] submodule.

pub mod error;
pub mod traits;
pub mod lifecycle;

pub use error::{StorageError, StorageErrorKind, StorageErrorType};
pub use traits::{ReadAt, StorageBackend, Sync, WriteAt};

#[cfg(feature = "std")]
pub use lifecycle::{LockableBackend, OpenableBackend};
