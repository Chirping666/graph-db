//! Storage backend trait definitions for I/O operations.
//!
//! This module defines the trait hierarchy that all storage backends
//! implement. The traits are `no_std + alloc` compatible and object-safe,
//! enabling both static dispatch (generics) and dynamic dispatch
//! (`dyn StorageBackend`).
//!
//! # Trait hierarchy
//!
//! ```text
//! BackendErrorType (associated Error type)
//!     ├── ReadAt      (&self — concurrent reads)
//!     ├── WriteAt     (&mut self — exclusive writes)
//!     └── Durability  (&mut self — durability control)
//!           └── StorageBackend = ReadAt + WriteAt + Durability (blanket impl)
//! ```
//!
//! [`LockableBackend`] is unconditional (`no_std + alloc` compatible) and
//! lives in [`traits`]. [`OpenableBackend`] is `std`-only and lives in the
//! [`lifecycle`] submodule.

pub mod error;
pub mod traits;
#[cfg(feature = "std")]
pub mod lifecycle;

pub use error::{BackendError, BackendErrorType, StorageErrorKind};
pub use traits::{Durability, LockableBackend, ReadAt, StorageBackend, WriteAt};

#[cfg(feature = "std")]
pub use lifecycle::OpenableBackend;
