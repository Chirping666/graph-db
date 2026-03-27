//! Database engine configuration.
//!
//! Provides [`DatabaseConfig`] for specifying engine-level parameters.
//! Path/file configuration lives in `phonograph_std`.

use alloc::format;

/// Configuration for the database engine.
///
/// Contains only engine-level parameters (page size, buffer pool, etc.).
/// File path and storage mode configuration is handled by `phonograph_std`.
///
/// # Examples
///
/// ```
/// use phonograph_db::db::DatabaseConfig;
///
/// let config = DatabaseConfig::default()
///     .buffer_pool_frames(256)
///     .page_size(8192);
/// assert_eq!(config.buffer_pool_frames, 256);
/// assert_eq!(config.page_size, 8192);
/// ```
#[derive(Clone, Debug)]
pub struct DatabaseConfig {
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
    /// Application ID stored in the database header.
    ///
    /// Default: 0.
    pub application_id: u32,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            buffer_pool_frames: 1024,
            page_size: 4096,
            extension_startup_check: true,
            inference_cache_size: 64,
            application_id: 0,
        }
    }
}

impl DatabaseConfig {
    /// Sets the number of page frames in the buffer pool.
    ///
    /// Values below 64 are clamped to 64.
    pub fn buffer_pool_frames(mut self, frames: usize) -> Self {
        self.buffer_pool_frames = frames.max(64);
        self
    }

    /// Sets the page size in bytes.
    ///
    /// The value must be a power of two and at least 512. Invalid values
    /// are accepted here but will be rejected by [`validate()`](Self::validate),
    /// which is called automatically by `Database::create` and `Database::open`.
    pub fn page_size(mut self, size: usize) -> Self {
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

    /// Sets the application ID stored in the database header.
    pub fn application_id(mut self, id: u32) -> Self {
        self.application_id = id;
        self
    }

    /// Validates the configuration, returning an error if any values are invalid.
    ///
    /// Called automatically by [`Database::create`](super::Database::create) and
    /// [`Database::open`](super::Database::open).
    ///
    /// # Errors
    ///
    /// Returns an error if `page_size` is not a power of two or is less than 512.
    pub fn validate(&self) -> Result<(), crate::error::Error> {
        if !self.page_size.is_power_of_two() {
            return Err(crate::error::Error::Storage(crate::error::StorageError {
                message: format!(
                    "page_size {} is not a power of two",
                    self.page_size
                ),
                #[cfg(feature = "std")]
                source: None,
            }));
        }
        if self.page_size < 512 {
            return Err(crate::error::Error::Storage(crate::error::StorageError {
                message: format!(
                    "page_size {} is less than minimum 512",
                    self.page_size
                ),
                #[cfg(feature = "std")]
                source: None,
            }));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config() {
        let config = DatabaseConfig::default();
        assert_eq!(config.buffer_pool_frames, 1024);
        assert_eq!(config.page_size, 4096);
        assert!(config.extension_startup_check);
        assert_eq!(config.inference_cache_size, 64);
        assert_eq!(config.application_id, 0);
    }

    #[test]
    fn builder_chaining() {
        let config = DatabaseConfig::default()
            .buffer_pool_frames(256)
            .page_size(8192)
            .extension_startup_check(false)
            .inference_cache_size(128)
            .application_id(42);
        assert_eq!(config.buffer_pool_frames, 256);
        assert_eq!(config.page_size, 8192);
        assert!(!config.extension_startup_check);
        assert_eq!(config.inference_cache_size, 128);
        assert_eq!(config.application_id, 42);
    }

    #[test]
    fn buffer_pool_frames_clamps_minimum() {
        let config = DatabaseConfig::default().buffer_pool_frames(10);
        assert_eq!(config.buffer_pool_frames, 64);
    }

    #[test]
    fn page_size_accepts_invalid_defers_to_validate() {
        let config = DatabaseConfig::default().page_size(1000);
        assert_eq!(config.page_size, 1000);
        assert!(config.validate().is_err());

        let config = DatabaseConfig::default().page_size(256);
        assert_eq!(config.page_size, 256);
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_catches_invalid_page_size() {
        let config = DatabaseConfig {
            page_size: 1000,
            ..DatabaseConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_catches_small_page_size() {
        let config = DatabaseConfig {
            page_size: 256,
            ..DatabaseConfig::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn validate_accepts_valid_config() {
        let config = DatabaseConfig::default();
        assert!(config.validate().is_ok());
    }
}
