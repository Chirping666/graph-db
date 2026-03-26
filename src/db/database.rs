//! The [`Database`] struct — lifecycle, extension registration, transaction creation.
//!
//! `Database` is the primary entry point for the graph database. It manages
//! the storage engine, schema cache, extension registries, and transaction
//! creation. It is `Send + Sync` and can be shared across threads via `Arc`.

use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};

use crate::constraint::ConstraintValidator;
use crate::error::{Error, StorageError};
use crate::backend::{self, OpenableBackend, ReadAt, StorageErrorKind, StorageErrorType, WriteAt};
use crate::backend_mem::{MemoryBackend, MemoryError};
use crate::backend_std::file_backend::{FileBackend, FileBackendConfig, FileError};
use crate::inference::InferenceRule;
use crate::storage::page::PageId;
use crate::storage::serialization;
use crate::storage::snapshot::Snapshot;
use crate::storage::{StorageEngine, StorageEngineConfig};
use crate::types::{PropertyKeyId, TypeId};

use super::config::{DatabaseConfig, StorageMode};
use super::inference_engine::InferenceEngine;
use super::read_txn::ReadTransaction;
use super::schema_cache::SchemaCache;
use super::write_txn::WriteTransaction;

// ---------------------------------------------------------------------------
// AnyBackend — internal enum dispatch for multiple storage backends
// ---------------------------------------------------------------------------

/// Unified error type for [`AnyBackend`].
#[derive(Debug)]
pub(crate) enum AnyBackendError {
    /// Error from the file-backed storage backend.
    File(FileError),
    /// Error from the in-memory storage backend.
    Memory(MemoryError),
}

impl core::fmt::Display for AnyBackendError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            AnyBackendError::File(e) => e.fmt(f),
            AnyBackendError::Memory(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for AnyBackendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            AnyBackendError::File(e) => e.source(),
            AnyBackendError::Memory(_) => None,
        }
    }
}

impl backend::StorageError for AnyBackendError {
    fn kind(&self) -> StorageErrorKind {
        match self {
            AnyBackendError::File(e) => e.kind(),
            AnyBackendError::Memory(e) => e.kind(),
        }
    }
}

/// Internal backend enum supporting both file and in-memory storage.
///
/// Not exposed in the public API. Implements all backend traits via
/// match-and-delegate so that `StorageEngine<AnyBackend>` works
/// identically regardless of the underlying backend.
pub(crate) enum AnyBackend {
    /// File-backed persistent storage.
    File(FileBackend),
    /// In-memory storage backed by `Vec<u8>`.
    Memory(MemoryBackend),
}

impl StorageErrorType for AnyBackend {
    type Error = AnyBackendError;
}

impl ReadAt for AnyBackend {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), AnyBackendError> {
        match self {
            AnyBackend::File(f) => f.read_at(offset, buf).map_err(AnyBackendError::File),
            AnyBackend::Memory(m) => m.read_at(offset, buf).map_err(AnyBackendError::Memory),
        }
    }

    fn len(&self) -> Result<u64, AnyBackendError> {
        match self {
            AnyBackend::File(f) => f.len().map_err(AnyBackendError::File),
            AnyBackend::Memory(m) => m.len().map_err(AnyBackendError::Memory),
        }
    }
}

impl WriteAt for AnyBackend {
    fn write_at(&mut self, offset: u64, buf: &[u8]) -> Result<(), AnyBackendError> {
        match self {
            AnyBackend::File(f) => f.write_at(offset, buf).map_err(AnyBackendError::File),
            AnyBackend::Memory(m) => m.write_at(offset, buf).map_err(AnyBackendError::Memory),
        }
    }

    fn set_len(&mut self, new_size: u64) -> Result<(), AnyBackendError> {
        match self {
            AnyBackend::File(f) => f.set_len(new_size).map_err(AnyBackendError::File),
            AnyBackend::Memory(m) => m.set_len(new_size).map_err(AnyBackendError::Memory),
        }
    }
}

impl backend::Durability for AnyBackend {
    fn sync_data(&mut self) -> Result<(), AnyBackendError> {
        match self {
            AnyBackend::File(f) => backend::Durability::sync_data(f).map_err(AnyBackendError::File),
            AnyBackend::Memory(m) => {
                backend::Durability::sync_data(m).map_err(AnyBackendError::Memory)
            }
        }
    }

    fn sync_all(&mut self) -> Result<(), AnyBackendError> {
        match self {
            AnyBackend::File(f) => backend::Durability::sync_all(f).map_err(AnyBackendError::File),
            AnyBackend::Memory(m) => {
                backend::Durability::sync_all(m).map_err(AnyBackendError::Memory)
            }
        }
    }
}

/// Names of extensions that were persisted in the database file.
#[derive(Clone, Debug, Default)]
pub(crate) struct PersistedExtensionNames {
    /// Constraint validator names persisted in the Schema Store.
    pub constraints: Vec<String>,
    /// Inference rule names persisted in the Schema Store.
    pub inference_rules: Vec<String>,
}

/// Lists extensions that are persisted in the database but not currently
/// registered in memory.
///
/// # Examples
///
/// ```
/// # use graph_db::db::{database::Database, config::DatabaseConfig};
/// let db = Database::open(DatabaseConfig::in_memory()).unwrap();
/// let missing = db.missing_extensions();
/// assert!(missing.is_empty());
/// ```
#[derive(Clone, Debug, Default)]
pub struct MissingExtensions {
    /// Constraint validators persisted but not registered.
    pub constraint_validators: Vec<String>,
    /// Inference rules persisted but not registered.
    pub inference_rules: Vec<String>,
}

impl MissingExtensions {
    /// Returns `true` if no extensions are missing.
    pub fn is_empty(&self) -> bool {
        self.constraint_validators.is_empty() && self.inference_rules.is_empty()
    }
}

/// Shared state for the database, protected by `Mutex`/`RwLock`.
pub(crate) struct DatabaseInner {
    /// The storage engine providing B-tree operations.
    pub storage: Mutex<StorageEngine<AnyBackend>>,
    /// Write lock: only one write transaction at a time.
    pub write_mutex: Mutex<()>,
    /// Current snapshot (latest committed root pointers).
    pub current_snapshot: RwLock<Arc<Snapshot>>,
    /// In-memory schema cache.
    pub schema_cache: RwLock<SchemaCache>,
    /// Registered constraint validators.
    pub constraint_registry: RwLock<Vec<Box<dyn ConstraintValidator>>>,
    /// Inference engine: rule registry, result cache, and provenance tracking.
    pub inference_engine: Mutex<InferenceEngine>,
    /// Extension names persisted in the database.
    pub persisted_extension_names: RwLock<PersistedExtensionNames>,
    /// Configuration.
    #[allow(dead_code)]
    pub config: DatabaseConfig,
}

// SAFETY: All shared state in DatabaseInner is protected by Mutex/RwLock.
// ConstraintValidator and InferenceRule are Send + Sync by trait bound.
unsafe impl Send for DatabaseInner {}
unsafe impl Sync for DatabaseInner {}

/// The primary entry point for the graph database.
///
/// `Database` manages the storage engine, schema cache, extension registries,
/// and creates transactions. It is `Send + Sync` and can be safely shared
/// across threads.
///
/// # Examples
///
/// ```
/// use graph_db::db::database::Database;
/// use graph_db::db::config::DatabaseConfig;
///
/// // Open an in-memory database
/// let db = Database::open(DatabaseConfig::in_memory()).unwrap();
/// let rtx = db.read_txn().unwrap();
/// assert_eq!(rtx.node_count().unwrap(), 0);
/// ```
pub struct Database {
    pub(crate) inner: Arc<DatabaseInner>,
}

/// Helper to convert a backend error to a crate StorageError.
fn map_backend_err<E: crate::backend::StorageError>(e: E) -> StorageError {
    StorageError {
        message: format!("{e}"),
        source: None,
    }
}

impl Database {
    /// Opens or creates a database at the configured location.
    ///
    /// For persistent mode, creates the file if it doesn't exist, or opens
    /// an existing database file. For in-memory mode, creates a fresh
    /// database in RAM. Loads the schema cache from the Schema Store.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be opened or the format is invalid.
    ///
    /// # Examples
    ///
    /// ```
    /// use graph_db::db::database::Database;
    /// use graph_db::db::config::DatabaseConfig;
    ///
    /// let db = Database::open(DatabaseConfig::in_memory()).unwrap();
    /// ```
    pub fn open(config: DatabaseConfig) -> Result<Self, Error> {
        config.validate()?;
        match &config.mode {
            StorageMode::Persistent { path } => {
                let path = path.clone();
                Self::open_persistent(&path, config)
            }
            StorageMode::InMemory => Self::open_in_memory(config),
        }
    }

    fn open_persistent(path: &Path, config: DatabaseConfig) -> Result<Self, Error> {
        let engine_config = StorageEngineConfig {
            page_size: config.page_size,
            buffer_pool_frames: config.buffer_pool_frames,
            application_id: 0,
        };

        let backend_config = FileBackendConfig {
            path: path.to_path_buf(),
            read_only: false,
        };

        // Try to open; if it fails, create.
        let file_backend = FileBackend::open_or_create(backend_config).map_err(map_backend_err)?;
        let file_len = file_backend.len().map_err(map_backend_err)?;
        let backend = AnyBackend::File(file_backend);

        let engine = if file_len == 0 {
            StorageEngine::create(backend, engine_config)?
        } else {
            StorageEngine::open(backend, engine_config)?
        };

        Self::finish_open(engine, config)
    }

    fn open_in_memory(config: DatabaseConfig) -> Result<Self, Error> {
        let engine_config = StorageEngineConfig {
            page_size: config.page_size,
            buffer_pool_frames: config.buffer_pool_frames,
            application_id: 0,
        };

        let backend = AnyBackend::Memory(MemoryBackend::new());
        let engine = StorageEngine::create(backend, engine_config)?;

        Self::finish_open(engine, config)
    }

    /// Shared initialization after the storage engine is created or opened.
    fn finish_open(
        engine: StorageEngine<AnyBackend>,
        config: DatabaseConfig,
    ) -> Result<Self, Error> {
        let snapshot = engine.current_snapshot();

        let mut engine = engine;
        let mut schema_cache = SchemaCache::new();
        let mut persisted_names = PersistedExtensionNames::default();

        Self::load_schema(
            &mut engine,
            &snapshot,
            &mut schema_cache,
            &mut persisted_names,
        )?;

        // Create inference engine and load persisted provenance records.
        let mut inference_engine = InferenceEngine::new(config.inference_cache_size);
        if !snapshot.roots.schema_store.is_null() {
            let provenance_entries =
                Self::collect_range(&mut engine, snapshot.roots.schema_store, &[0x06], &[0x07])?;
            inference_engine
                .provenance_mut()
                .load_from_entries(provenance_entries.into_iter());
        }

        let inner = DatabaseInner {
            storage: Mutex::new(engine),
            write_mutex: Mutex::new(()),
            current_snapshot: RwLock::new(Arc::new(snapshot)),
            schema_cache: RwLock::new(schema_cache),
            constraint_registry: RwLock::new(Vec::new()),
            inference_engine: Mutex::new(inference_engine),
            persisted_extension_names: RwLock::new(persisted_names),
            config,
        };

        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    /// Collects all key-value pairs from a B-tree range scan.
    #[allow(clippy::type_complexity)]
    fn collect_range(
        engine: &mut StorageEngine<AnyBackend>,
        root: PageId,
        start: &[u8],
        end: &[u8],
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>, StorageError> {
        engine.range_scan(root, start, Some(end))
    }

    /// Loads types, property keys, counters, hierarchy edges, and extension
    /// names from the Schema Store B-tree into the schema cache.
    fn load_schema(
        engine: &mut StorageEngine<AnyBackend>,
        snapshot: &Snapshot,
        cache: &mut SchemaCache,
        persisted_names: &mut PersistedExtensionNames,
    ) -> Result<(), Error> {
        let schema_root = snapshot.roots.schema_store;
        if schema_root.is_null() {
            return Ok(());
        }

        // --- Load type definitions (prefix 0x01) ---
        let type_entries = Self::collect_range(engine, schema_root, &[0x01], &[0x02])?;
        for (key, value) in &type_entries {
            if key.len() >= 5 && key[0] == 0x01 {
                let id_bytes: [u8; 4] = key[1..5].try_into().map_err(|_| {
                    StorageError {
                        message: "schema: invalid type_id key length".into(),
                        source: None,
                    }
                })?;
                let type_id = TypeId(u32::from_be_bytes(id_bytes));
                let td = serialization::deserialize_type_definition(type_id, value)?;
                cache.load_type(td)?;
            }
        }

        // --- Load property keys (prefix 0x02) ---
        let pk_entries = Self::collect_range(engine, schema_root, &[0x02], &[0x03])?;
        for (key, value) in &pk_entries {
            if key.len() >= 5 && key[0] == 0x02 {
                let id_bytes: [u8; 4] = key[1..5].try_into().map_err(|_| {
                    StorageError {
                        message: "schema: invalid property_key_id key length".into(),
                        source: None,
                    }
                })?;
                let key_id = PropertyKeyId(u32::from_be_bytes(id_bytes));
                let name = serialization::deserialize_property_key_name(value)?;
                cache.load_property_key(key_id, name);
            }
        }

        // --- Load counters (prefix 0x03) ---
        let counter_entries = Self::collect_range(engine, schema_root, &[0x03], &[0x04])?;
        for (key, value) in &counter_entries {
            if key.len() >= 2 && key[0] == 0x03 && value.len() >= 8 {
                let counter_bytes: [u8; 8] = value[..8].try_into().map_err(|_| {
                    StorageError {
                        message: "schema: invalid counter value length".into(),
                        source: None,
                    }
                })?;
                let counter_val = u64::from_le_bytes(counter_bytes);
                match key[1] {
                    0x01 => cache.next_node_id = counter_val,
                    0x02 => cache.next_edge_id = counter_val,
                    0x03 => cache.next_type_id = counter_val as u32,
                    0x04 => cache.next_property_key_id = counter_val as u32,
                    _ => {}
                }
            }
        }

        // Rebuild subtypes cache after all types are loaded.
        cache.rebuild_subtypes_cache();

        // --- Load extension names (prefix 0x05) ---
        let ext_entries = Self::collect_range(engine, schema_root, &[0x05], &[0x06])?;
        for (key, _value) in &ext_entries {
            if key.len() >= 4 && key[0] == 0x05 {
                let kind = key[1];
                let name_len_bytes: [u8; 2] = key[2..4].try_into().map_err(|_| {
                    StorageError {
                        message: "schema: invalid extension name_len field length".into(),
                        source: None,
                    }
                })?;
                let name_len = u16::from_le_bytes(name_len_bytes) as usize;
                if key.len() >= 4 + name_len {
                    if let Ok(name) = std::str::from_utf8(&key[4..4 + name_len]) {
                        match kind {
                            0x01 => persisted_names.constraints.push(name.to_string()),
                            0x02 => {
                                persisted_names.inference_rules.push(name.to_string())
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Begins a read-only transaction with snapshot isolation.
    ///
    /// The transaction sees a consistent snapshot of the database as of
    /// the most recent commit. Multiple read transactions can be active
    /// concurrently.
    ///
    /// # Errors
    ///
    /// Returns an error if the internal state is poisoned (panic in another thread).
    ///
    /// # Examples
    ///
    /// ```
    /// # use graph_db::db::{database::Database, config::DatabaseConfig};
    /// let db = Database::open(DatabaseConfig::in_memory()).unwrap();
    /// let rtx = db.read_txn().unwrap();
    /// assert_eq!(rtx.node_count().unwrap(), 0);
    /// ```
    pub fn read_txn(&self) -> Result<ReadTransaction<'_>, Error> {
        let snapshot = {
            let current = self.inner.current_snapshot.read().unwrap();
            Arc::clone(&current)
        };
        let schema_cache = {
            let cache = self.inner.schema_cache.read().unwrap();
            cache.clone()
        };
        Ok(ReadTransaction::new(&self.inner, snapshot, schema_cache))
    }

    /// Begins a read-write transaction.
    ///
    /// Only one write transaction can be active at a time. This method blocks
    /// until the write lock is available.
    ///
    /// The transaction sees a consistent snapshot plus its own pending changes
    /// (read-your-own-writes).
    ///
    /// # Errors
    ///
    /// Returns an error if the write lock is poisoned.
    ///
    /// # Examples
    ///
    /// ```
    /// # use graph_db::db::{database::Database, config::DatabaseConfig, builders::*};
    /// let db = Database::open(DatabaseConfig::in_memory()).unwrap();
    /// let mut wtx = db.write_txn().unwrap();
    /// let t = wtx.register_type(TypeDefinitionBuilder::node_type("N").build()).unwrap();
    /// wtx.insert_node(NodeBuilder::new().type_label(t).build()).unwrap();
    /// wtx.commit().unwrap();
    /// ```
    pub fn write_txn(&self) -> Result<WriteTransaction<'_>, Error> {
        let guard = self.inner.write_mutex.lock().unwrap();
        let snapshot = {
            let current = self.inner.current_snapshot.read().unwrap();
            Arc::clone(&current)
        };
        let schema_cache = {
            let cache = self.inner.schema_cache.read().unwrap();
            cache.clone()
        };
        Ok(WriteTransaction::new(
            &self.inner,
            snapshot,
            schema_cache,
            guard,
        ))
    }

    /// Registers a constraint validator.
    ///
    /// If a validator with the same name is already registered, it is replaced.
    ///
    /// # Errors
    ///
    /// Returns an error if the internal state is poisoned.
    pub fn register_constraint(
        &self,
        validator: Box<dyn ConstraintValidator>,
    ) -> Result<(), Error> {
        let name = validator.name().to_string();
        let mut registry = self.inner.constraint_registry.write().unwrap();
        registry.retain(|v| v.name() != name);
        registry.push(validator);
        Ok(())
    }

    /// Unregisters a constraint validator by name.
    ///
    /// Returns `true` if a validator was found and removed.
    ///
    /// # Errors
    ///
    /// Returns an error if the internal state is poisoned.
    pub fn unregister_constraint(&self, name: &str) -> Result<bool, Error> {
        let mut registry = self.inner.constraint_registry.write().unwrap();
        let before = registry.len();
        registry.retain(|v| v.name() != name);
        Ok(registry.len() < before)
    }

    /// Registers an inference rule.
    ///
    /// If a rule with the same name is already registered, it is replaced
    /// (preserving its position in the registration order).
    ///
    /// # Errors
    ///
    /// Returns an error if the internal state is poisoned.
    pub fn register_inference_rule(
        &self,
        rule: Box<dyn InferenceRule>,
    ) -> Result<(), Error> {
        let mut engine = self.inner.inference_engine.lock().unwrap();
        engine.register_rule(rule);
        Ok(())
    }

    /// Unregisters an inference rule by name.
    ///
    /// Returns `true` if a rule was found and removed.
    ///
    /// # Errors
    ///
    /// Returns an error if the internal state is poisoned.
    pub fn unregister_inference_rule(&self, name: &str) -> Result<bool, Error> {
        let mut engine = self.inner.inference_engine.lock().unwrap();
        Ok(engine.unregister_rule(name))
    }

    /// Returns the names of all registered constraint validators.
    ///
    /// # Examples
    ///
    /// ```
    /// # use graph_db::db::{database::Database, config::DatabaseConfig};
    /// let db = Database::open(DatabaseConfig::in_memory()).unwrap();
    /// assert!(db.constraint_names().is_empty());
    /// ```
    pub fn constraint_names(&self) -> Vec<String> {
        let registry = self.inner.constraint_registry.read().unwrap();
        registry.iter().map(|v| v.name().to_string()).collect()
    }

    /// Returns the names of all registered inference rules in registration order.
    ///
    /// # Examples
    ///
    /// ```
    /// # use graph_db::db::{database::Database, config::DatabaseConfig};
    /// let db = Database::open(DatabaseConfig::in_memory()).unwrap();
    /// assert!(db.inference_rule_names().is_empty());
    /// ```
    pub fn inference_rule_names(&self) -> Vec<String> {
        let engine = self.inner.inference_engine.lock().unwrap();
        engine.rule_names()
    }

    /// Saves the in-memory database contents to a file.
    ///
    /// Only available when the database is using in-memory storage.
    /// The resulting file is a valid database file that can be opened
    /// with [`DatabaseConfig::persistent`](super::config::DatabaseConfig::persistent).
    ///
    /// # Errors
    ///
    /// Returns an error if the database is not using in-memory storage
    /// or if the file write fails.
    pub fn save_to_file(&self, path: &Path) -> Result<(), Error> {
        let engine = self.inner.storage.lock().unwrap();
        match engine.backend() {
            AnyBackend::Memory(mem) => mem.save_to_file(path).map_err(|e| {
                Error::Storage(StorageError {
                    message: format!("snapshot save failed: {e}"),
                    source: None,
                })
            }),
            AnyBackend::File(_) => Err(Error::Storage(StorageError {
                message: "save_to_file is only available for in-memory databases".to_string(),
                source: None,
            })),
        }
    }

    /// Returns extensions that are persisted in the database but not
    /// currently registered in memory.
    pub fn missing_extensions(&self) -> MissingExtensions {
        let persisted = self.inner.persisted_extension_names.read().unwrap();
        let constraints = self.inner.constraint_registry.read().unwrap();
        let engine = self.inner.inference_engine.lock().unwrap();

        let registered_constraints: std::collections::HashSet<String> =
            constraints.iter().map(|v| v.name().to_string()).collect();
        let registered_rules: std::collections::HashSet<String> =
            engine.rule_names().into_iter().collect();

        MissingExtensions {
            constraint_validators: persisted
                .constraints
                .iter()
                .filter(|n| !registered_constraints.contains(n.as_str()))
                .cloned()
                .collect(),
            inference_rules: persisted
                .inference_rules
                .iter()
                .filter(|n| !registered_rules.contains(n.as_str()))
                .cloned()
                .collect(),
        }
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        // Best-effort flush on drop. We need split borrows from the engine,
        // which StorageEngine doesn't directly support. The commit() method
        // already flushes, so this is only needed if the user drops without
        // committing — which means we want to discard changes anyway.
        // No-op for now; committed data is already durable.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constraint::{ChangeSet, ConstraintViolation};
    use crate::inference::InferenceResult;
    use crate::schema::{GraphView, PropertyKeyRegistryView, TypeRegistryView};
    use crate::types::TypeId;

    struct TestValidator;
    impl ConstraintValidator for TestValidator {
        fn name(&self) -> &str {
            "TestValidator"
        }
        fn applies_to_types(&self) -> Option<Vec<TypeId>> {
            None
        }
        fn validate(
            &self,
            _changes: &ChangeSet<'_>,
            _graph: &dyn GraphView,
            _types: &dyn TypeRegistryView,
            _keys: &dyn PropertyKeyRegistryView,
        ) -> Vec<ConstraintViolation> {
            Vec::new()
        }
    }

    struct TestRule;
    impl InferenceRule for TestRule {
        fn name(&self) -> &str {
            "TestRule"
        }
        fn applies_to_types(&self) -> Option<Vec<TypeId>> {
            None
        }
        fn infer(
            &self,
            _graph: &dyn GraphView,
            _types: &dyn TypeRegistryView,
            _keys: &dyn PropertyKeyRegistryView,
        ) -> InferenceResult {
            InferenceResult {
                facts: Vec::new(),
                rule_name: "TestRule".to_string(),
            }
        }
    }

    #[test]
    fn open_persistent_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let config = DatabaseConfig::persistent(&path);
        let db = Database::open(config).unwrap();
        drop(db);

        // Reopen
        let config2 = DatabaseConfig::persistent(&path);
        let _db2 = Database::open(config2).unwrap();
    }

    #[test]
    fn open_in_memory_database() {
        let db = Database::open(DatabaseConfig::in_memory()).unwrap();
        let _rtx = db.read_txn().unwrap();
    }

    #[test]
    fn register_and_list_constraints() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let db = Database::open(DatabaseConfig::persistent(&path)).unwrap();

        db.register_constraint(Box::new(TestValidator)).unwrap();
        let names = db.constraint_names();
        assert_eq!(names, vec!["TestValidator"]);
    }

    #[test]
    fn register_and_list_inference_rules() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let db = Database::open(DatabaseConfig::persistent(&path)).unwrap();

        db.register_inference_rule(Box::new(TestRule)).unwrap();
        let names = db.inference_rule_names();
        assert_eq!(names, vec!["TestRule"]);
    }

    #[test]
    fn unregister_constraint() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let db = Database::open(DatabaseConfig::persistent(&path)).unwrap();

        db.register_constraint(Box::new(TestValidator)).unwrap();
        assert!(db.unregister_constraint("TestValidator").unwrap());
        assert!(!db.unregister_constraint("NonExistent").unwrap());
        assert!(db.constraint_names().is_empty());
    }

    #[test]
    fn missing_extensions_empty_when_none_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.db");
        let db = Database::open(DatabaseConfig::persistent(&path)).unwrap();
        assert!(db.missing_extensions().is_empty());
    }
}
