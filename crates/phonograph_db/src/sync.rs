//! Re-exports of synchronization primitives for `no_std` compatibility.
//!
//! Uses `spin` crate unconditionally for mutex and rwlock, and
//! `alloc::sync::Arc` for reference counting.

pub(crate) use alloc::sync::Arc;
pub(crate) use spin::Mutex;
pub(crate) use spin::MutexGuard;
pub(crate) use spin::RwLock;
