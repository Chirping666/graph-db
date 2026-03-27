//! An embedded graph database with extensible schema and pluggable inference.
//!
//! `phonograph_std` is the batteries-included entry point for the Phonograph
//! workspace. It re-exports everything from [`phonograph`] (core types) and
//! [`phonograph_db`] (database engine), and adds the file-backed storage
//! backend ([`FileBackend`](backend_std::FileBackend)), OS-level file
//! locking, and convenience constructors.
//!
//! For `no_std` usage, depend on [`phonograph_db`] directly.
//!
//! # Quick Start
//!
//! ```no_run
//! let db = phonograph_std::open("/tmp/my.db").unwrap();
//! ```
//!
//! ```
//! let db = phonograph_std::open_in_memory().unwrap();
//! let rtx = db.read_txn().unwrap();
//! assert_eq!(rtx.node_count().unwrap(), 0);
//! ```

// Re-export the full vocabulary and database engine (decision R14).
// phonograph_db already re-exports phonograph::*, so a single glob
// brings in the entire public surface of both inner crates.
pub use phonograph_db::*;

pub mod backend_std;
mod any_backend;

pub use any_backend::{AnyBackend, AnyBackendError};

/// A database backed by either a file or in-memory storage.
///
/// This is a convenience alias for [`phonograph_db::db::Database<AnyBackend>`].
/// For `no_std` usage with a specific backend, use
/// [`phonograph_db::db::Database<B>`](phonograph_db::db::Database) directly.
pub type Database = phonograph_db::db::Database<AnyBackend>;

/// A read transaction on a [`Database`] backed by [`AnyBackend`].
pub type ReadTransaction<'db> = phonograph_db::db::ReadTransaction<'db, AnyBackend>;

/// A write transaction on a [`Database`] backed by [`AnyBackend`].
pub type WriteTransaction<'db> = phonograph_db::db::WriteTransaction<'db, AnyBackend>;

/// Extension methods for [`Database`] backed by [`AnyBackend`].
pub trait DatabaseExt {
    /// Saves the in-memory database contents to a file.
    ///
    /// The resulting file is a valid database file that can be reopened with
    /// [`open`]. Only available when the database is using in-memory storage.
    ///
    /// # Errors
    ///
    /// Returns an error if the database is not using in-memory storage
    /// or if the file write fails.
    fn save_to_file(&self, path: &std::path::Path) -> Result<(), phonograph_db::error::Error>;
}

impl DatabaseExt for Database {
    fn save_to_file(&self, path: &std::path::Path) -> Result<(), phonograph_db::error::Error> {
        self.with_backend(|backend| match backend {
            AnyBackend::Memory(mem) => {
                mem.save_to_file(path).map_err(|e| {
                    phonograph_db::error::Error::Storage(phonograph_db::error::StorageError {
                        message: format!("snapshot save failed: {e}"),
                        source: None,
                    })
                })
            }
            AnyBackend::File(_) => Err(phonograph_db::error::Error::Storage(
                phonograph_db::error::StorageError {
                    message: "save_to_file is only available for in-memory databases".into(),
                    source: None,
                },
            )),
        })
    }
}

/// Configuration for opening a file-backed database.
///
/// Combines a file path and read-only flag with the engine-level
/// [`DatabaseConfig`](phonograph_db::db::DatabaseConfig).
///
/// # Examples
///
/// ```no_run
/// use phonograph_std::FileConfig;
///
/// let config = FileConfig::new("/tmp/my.db");
/// ```
#[derive(Clone, Debug)]
pub struct FileConfig {
    /// Path to the database file.
    pub path: std::path::PathBuf,
    /// If `true`, open the database in read-only mode.
    pub read_only: bool,
    /// Engine-level configuration (page size, buffer pool, etc.).
    pub engine: phonograph_db::db::DatabaseConfig,
}

impl FileConfig {
    /// Creates a new `FileConfig` with the given path and default engine settings.
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self {
            path: path.into(),
            read_only: false,
            engine: phonograph_db::db::DatabaseConfig::default(),
        }
    }

    /// Sets the read-only flag.
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Sets the engine configuration.
    pub fn engine(mut self, engine: phonograph_db::db::DatabaseConfig) -> Self {
        self.engine = engine;
        self
    }
}

/// Converts a backend error into a crate-level [`Error`](phonograph_db::error::Error).
fn map_backend_err<E: phonograph_db::backend::BackendError>(e: E) -> phonograph_db::error::Error {
    phonograph_db::error::Error::Storage(phonograph_db::error::StorageError {
        message: format!("{e}"),
        source: None,
    })
}

/// Opens or creates a persistent database at the given path.
///
/// If the file does not exist, it is created. If it exists, it is opened.
/// Uses default engine configuration.
///
/// For more control, use [`FileConfig`] with [`open_with_config`].
///
/// # Errors
///
/// Returns an error if the file cannot be opened or created, or if the
/// database format is invalid.
///
/// # Examples
///
/// ```no_run
/// let db = phonograph_std::open("/tmp/my.db").unwrap();
/// ```
pub fn open(path: impl AsRef<std::path::Path>) -> Result<Database, phonograph_db::error::Error> {
    open_with_config(FileConfig::new(path.as_ref()))
}

/// Opens or creates a database with the given file configuration.
///
/// # Errors
///
/// Returns an error if the file cannot be opened or created, or if the
/// database format is invalid.
pub fn open_with_config(config: FileConfig) -> Result<Database, phonograph_db::error::Error> {
    use phonograph_db::backend::OpenableBackend;
    use phonograph_db::backend::ReadAt;

    let backend_config = backend_std::FileBackendConfig {
        path: config.path,
        read_only: config.read_only,
    };

    let file_backend =
        backend_std::FileBackend::open_or_create(backend_config).map_err(map_backend_err)?;
    let file_len = file_backend.len().map_err(map_backend_err)?;
    let backend = AnyBackend::File(file_backend);

    if file_len == 0 {
        phonograph_db::db::Database::create(backend, config.engine)
    } else {
        phonograph_db::db::Database::open(backend, config.engine)
    }
}

/// Creates a fresh in-memory database.
///
/// Data is lost when the database is dropped. Useful for testing or
/// ephemeral workloads.
///
/// # Errors
///
/// Returns an error if initialization fails.
///
/// # Examples
///
/// ```
/// let db = phonograph_std::open_in_memory().unwrap();
/// let rtx = db.read_txn().unwrap();
/// assert_eq!(rtx.node_count().unwrap(), 0);
/// ```
pub fn open_in_memory() -> Result<Database, phonograph_db::error::Error> {
    let backend = AnyBackend::Memory(phonograph_db::backend_mem::MemoryBackend::new());
    let config = phonograph_db::db::DatabaseConfig::default().extension_startup_check(false);
    phonograph_db::db::Database::create(backend, config)
}
