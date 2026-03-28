//! Lifecycle traits for storage backends.
//!
//! [`OpenableBackend`] provides open/create semantics for backends
//! managing external resources. This module is `std`-only.
//!
//! For advisory file locking, see [`LockableBackend`](super::traits::LockableBackend)
//! which is unconditional (`no_std + alloc` compatible).

use super::traits::StorageBackend;

/// Open/create semantics for storage backends managing external resources.
///
/// This trait is `std`-only because filesystem operations have no `no_std`
/// analogue. The core I/O traits ([`ReadAt`](super::traits::ReadAt),
/// [`WriteAt`](super::traits::WriteAt), [`Durability`](super::traits::Durability))
/// do **not** require this — a `no_std` backend can be constructed by
/// other means and then used via [`StorageBackend`].
///
/// This trait is **not** object-safe (has `Sized` bound). After
/// construction, the backend is used through `dyn StorageBackend`.
///
/// The default [`open_or_create`](OpenableBackend::open_or_create)
/// implementation has a TOCTOU race; concrete backends (e.g., `FileBackend`)
/// should override it with an atomic implementation.
pub trait OpenableBackend: StorageBackend + Sized {
    /// Configuration for opening or creating the backend.
    type Config;

    /// Opens an existing storage medium.
    ///
    /// # Errors
    ///
    /// Returns an error if the medium does not exist or cannot be opened.
    fn open(config: Self::Config) -> Result<Self, Self::Error>;

    /// Creates a new storage medium.
    ///
    /// # Errors
    ///
    /// Returns an error if the medium cannot be created (e.g., it already
    /// exists when using `create_new` semantics).
    fn create(config: Self::Config) -> Result<Self, Self::Error>;

    /// Opens an existing storage medium, or creates it if it does not exist.
    ///
    /// The default implementation tries [`open`](OpenableBackend::open)
    /// first, then [`create`](OpenableBackend::create) on failure. This
    /// has a TOCTOU race — concrete backends should override with an
    /// atomic implementation where possible.
    ///
    /// # Errors
    ///
    /// Returns an error if neither opening nor creating succeeds.
    fn open_or_create(config: Self::Config) -> Result<Self, Self::Error>
    where
        Self::Config: Clone,
    {
        match Self::open(config.clone()) {
            Ok(backend) => Ok(backend),
            Err(_) => Self::create(config),
        }
    }
}
