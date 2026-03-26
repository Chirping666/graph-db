//! Lifecycle and locking traits for storage backends.
//!
//! [`OpenableBackend`] provides open/create semantics and
//! [`LockableBackend`] provides advisory file locking.
//! Both traits are `std`-only.

#[cfg(feature = "std")]
use super::error::StorageErrorType;
#[cfg(feature = "std")]
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
#[cfg(feature = "std")]
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

/// Advisory file locking for single-process exclusivity.
///
/// The database requires exclusive access to prevent corruption from
/// concurrent processes. This trait provides the locking mechanism; the
/// database engine calls it at open time and releases the lock at close.
///
/// This trait is `std`-only. In-memory backends do not need file locking.
///
/// # Advisory vs. mandatory locking
///
/// - **Unix:** `flock()` is advisory — it prevents cooperative processes
///   from accessing the file but cannot stop non-cooperating ones.
/// - **Windows:** `LockFile()` is mandatory.
///
/// This trait is object-safe.
#[cfg(feature = "std")]
pub trait LockableBackend: StorageErrorType {
    /// The guard value representing a held lock. Dropping it releases
    /// the lock (RAII pattern).
    ///
    /// Must be [`Send`] so the guard can be held across thread boundaries
    /// (the database's engine may be `Send + Sync`).
    type LockGuard: Send;

    /// Attempts to acquire an exclusive lock on the storage medium.
    ///
    /// This is **non-blocking**: it returns immediately with a lock guard
    /// or an error. The database should fail immediately on contention
    /// rather than blocking indefinitely.
    ///
    /// # Errors
    ///
    /// - [`LockContention`](super::StorageErrorKind::LockContention) if
    ///   another process holds the lock.
    /// - [`Io`](super::StorageErrorKind::Io) on other locking failures.
    fn try_lock_exclusive(&self) -> Result<Self::LockGuard, Self::Error>;
}
