//! `std` persistent file backend.
//!
//! This module provides [`FileBackend`], the primary durable storage
//! backend for the database. It uses `pread()`/`pwrite()` on Unix
//! and `ReadFile()`/`WriteFile()` with explicit offsets on Windows.

pub mod file_backend;

pub use file_backend::{FileBackend, FileBackendConfig, FileError, FileLockGuard};
