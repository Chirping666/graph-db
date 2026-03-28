//! Synchronization primitives — uses OS-native primitives under `std`,
//! falls back to `spin` on `no_std`.

pub(crate) use alloc::sync::Arc;

#[cfg(feature = "std")]
mod impl_std {
    pub(crate) use std::sync::MutexGuard;
    pub(crate) use std::sync::RwLockReadGuard;
    pub(crate) use std::sync::RwLockWriteGuard;

    /// Wrapper around [`std::sync::Mutex`] that panics on poisoning.
    pub(crate) struct Mutex<T>(std::sync::Mutex<T>);

    impl<T> Mutex<T> {
        pub const fn new(value: T) -> Self {
            Self(std::sync::Mutex::new(value))
        }

        pub fn lock(&self) -> MutexGuard<'_, T> {
            self.0.lock().expect("mutex poisoned")
        }

        #[allow(dead_code)]
        pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
            self.0.try_lock().ok()
        }
    }

    /// Wrapper around [`std::sync::RwLock`] that panics on poisoning.
    pub(crate) struct RwLock<T>(std::sync::RwLock<T>);

    impl<T> RwLock<T> {
        pub const fn new(value: T) -> Self {
            Self(std::sync::RwLock::new(value))
        }

        pub fn read(&self) -> RwLockReadGuard<'_, T> {
            self.0.read().expect("rwlock poisoned")
        }

        pub fn write(&self) -> RwLockWriteGuard<'_, T> {
            self.0.write().expect("rwlock poisoned")
        }
    }
}

#[cfg(not(feature = "std"))]
mod impl_spin {
    pub(crate) use spin::Mutex;
    #[allow(unused_imports)]
    pub(crate) use spin::MutexGuard;
    pub(crate) use spin::RwLock;
    #[allow(unused_imports)]
    pub(crate) use spin::RwLockReadGuard;
    #[allow(unused_imports)]
    pub(crate) use spin::RwLockWriteGuard;
}

#[cfg(feature = "std")]
pub(crate) use impl_std::*;

#[cfg(not(feature = "std"))]
pub(crate) use impl_spin::*;
