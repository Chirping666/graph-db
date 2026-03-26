//! Database configuration and storage mode selection.
//!
//! Provides [`DatabaseConfig`] for specifying how a database is opened and
//! [`StorageMode`] for choosing between persistent file storage and in-memory
//! storage.

use std::path::PathBuf;

/// Determines whether the database uses persistent file storage or in-memory
/// storage.
///
/// # Examples
///
/// ```
/// use graph_db::db::config::StorageMode;
///
/// let mode = StorageMode::InMemory;
/// assert!(matches!(mode, StorageMode::InMemory));
/// ```
#[derive(Clone, Debug)]
pub enum StorageMode {
    /// Persistent storage backed by a file at the given path.
    Persistent {
        /// Path to the database file.
        path: PathBuf,
    },
    /// In-memory storage. Data is lost when the database is dropped.
    InMemory,
}

/// Configuration for opening a database.
///
/// Use the builder methods [`DatabaseConfig::persistent`] or
/// [`DatabaseConfig::in_memory`] to create a configuration with sensible
/// defaults, then chain additional setters as needed.
///
/// # Examples
///
/// ```no_run
/// use graph_db::db::config::DatabaseConfig;
///
/// let config = DatabaseConfig::persistent("/tmp/my.db")
///     .buffer_pool_frames(256)
///     .page_size(8192);
/// ```
#[derive(Clone, Debug)]
pub struct DatabaseConfig {
    /// Storage mode (persistent or in-memory).
    pub mode: StorageMode,
    /// Number of page frames in the buffer pool.
    ///
    /// Minimum: 64. Default: 1024.
    pub buffer_pool_frames: usize,
    /// Page size in bytes. Must be a power of two.
    ///
    /// Default: 4096.
    pub page_size: usize,
    /// Whether to check for missing extensions at startup.
    ///
    /// Default: `true`.
    pub extension_startup_check: bool,
    /// Maximum number of cached inference results.
    ///
    /// Default: 64.
    pub inference_cache_size: usize,
}

impl DatabaseConfig {
    /// Creates a configuration for persistent storage at the given path
    /// with sensible defaults.
    pub fn persistent(path: impl Into<PathBuf>) -> Self {
        Self {
            mode: StorageMode::Persistent { path: path.into() },
            buffer_pool_frames: 1024,
            page_size: 4096,
            extension_startup_check: true,
            inference_cache_size: 64,
        }
    }

    /// Creates a configuration for in-memory storage with sensible defaults.
    ///
    /// The `extension_startup_check` defaults to `false` because a fresh
    /// in-memory database has no previously persisted extension list to
    /// check against.
    pub fn in_memory() -> Self {
        Self {
            mode: StorageMode::InMemory,
            buffer_pool_frames: 1024,
            page_size: 4096,
            extension_startup_check: false,
            inference_cache_size: 64,
        }
    }

    /// Sets the number of page frames in the buffer pool.
    ///
    /// Values below 64 are clamped to 64.
    pub fn buffer_pool_frames(mut self, frames: usize) -> Self {
        self.buffer_pool_frames = frames.max(64);
        self
    }

    /// Sets the page size in bytes.
    ///
    /// # Panics
    ///
    /// Panics if `size` is not a power of two or is less than 512.
    pub fn page_size(mut self, size: usize) -> Self {
        assert!(size.is_power_of_two(), "page_size must be a power of two");
        assert!(size >= 512, "page_size must be at least 512");
        self.page_size = size;
        self
    }

    /// Sets whether to check for missing extensions at startup.
    pub fn extension_startup_check(mut self, check: bool) -> Self {
        self.extension_startup_check = check;
        self
    }

    /// Sets the maximum number of cached inference results.
    pub fn inference_cache_size(mut self, size: usize) -> Self {
        self.inference_cache_size = size;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persistent_config_defaults() {
        let config = DatabaseConfig::persistent("/tmp/test.db");
        assert!(matches!(config.mode, StorageMode::Persistent { .. }));
        assert_eq!(config.buffer_pool_frames, 1024);
        assert_eq!(config.page_size, 4096);
        assert!(config.extension_startup_check);
        assert_eq!(config.inference_cache_size, 64);
    }

    #[test]
    fn in_memory_config_defaults() {
        let config = DatabaseConfig::in_memory();
        assert!(matches!(config.mode, StorageMode::InMemory));
        assert_eq!(config.buffer_pool_frames, 1024);
        assert_eq!(config.page_size, 4096);
    }

    #[test]
    fn builder_chaining() {
        let config = DatabaseConfig::in_memory()
            .buffer_pool_frames(256)
            .page_size(8192)
            .extension_startup_check(false)
            .inference_cache_size(128);
        assert_eq!(config.buffer_pool_frames, 256);
        assert_eq!(config.page_size, 8192);
        assert!(!config.extension_startup_check);
        assert_eq!(config.inference_cache_size, 128);
    }

    #[test]
    fn buffer_pool_frames_clamps_minimum() {
        let config = DatabaseConfig::in_memory().buffer_pool_frames(10);
        assert_eq!(config.buffer_pool_frames, 64);
    }

    #[test]
    #[should_panic(expected = "power of two")]
    fn page_size_must_be_power_of_two() {
        DatabaseConfig::in_memory().page_size(1000);
    }

    #[test]
    #[should_panic(expected = "at least 512")]
    fn page_size_minimum() {
        DatabaseConfig::in_memory().page_size(256);
    }
}
