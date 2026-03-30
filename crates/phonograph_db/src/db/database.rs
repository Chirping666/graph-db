//! The [`Database`] struct — lifecycle, extension registration, transaction creation.
//!
//! `Database` is the primary entry point for the graph database. It manages
//! the storage engine, schema cache, extension registries, and transaction
//! creation. It is `Send + Sync` and can be shared across threads via `Arc`.

extern crate alloc;

use alloc::{boxed::Box, format, string::{String, ToString}, vec::Vec};

use crate::backend::StorageBackend;
use crate::sync::{Arc, Mutex, RwLock};

use phonograph::constraint::ConstraintValidator;
use crate::error::{Error, StorageError};
use crate::backend::BackendError;
use phonograph::inference::InferenceRule;
use crate::storage::page::PageId;
use crate::storage::serialization;
use crate::storage::snapshot::Snapshot;
use crate::storage::{StorageEngine, StorageEngineConfig};
use phonograph::types::{PropertyKeyId, TypeId};

use super::config::DatabaseConfig;
use super::inference_engine::InferenceEngine;
use super::read_txn::ReadTransaction;
use super::schema_cache::SchemaCache;
use super::write_txn::WriteTransaction;

// ---------------------------------------------------------------------------
// Extension name tracking
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// DatabaseInner
// ---------------------------------------------------------------------------

/// Shared state for the database, protected by `Mutex`/`RwLock`.
pub(crate) struct DatabaseInner<B: StorageBackend> {
    /// The storage engine providing B-tree operations.
    pub storage: Mutex<StorageEngine<B>>,
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
// The backend B must be Send for the whole struct to be Send.
unsafe impl<B: StorageBackend + Send> Send for DatabaseInner<B> {}
unsafe impl<B: StorageBackend + Send> Sync for DatabaseInner<B> {}

// ---------------------------------------------------------------------------
// Database
// ---------------------------------------------------------------------------

/// The primary entry point for the graph database.
///
/// `Database` manages the storage engine, schema cache, extension registries,
/// and creates transactions. It is `Send + Sync` and can be safely shared
/// across threads.
///
/// `Database` is generic over the storage backend `B`. Use
/// [`Database::create`] to initialize a fresh database on a backend, or
/// [`Database::open`] to open an existing database from a backend.
pub struct Database<B: StorageBackend> {
    pub(crate) inner: Arc<DatabaseInner<B>>,
}

/// Helper to convert a backend error to a crate `StorageError`.
#[allow(dead_code)]
fn map_backend_err<E: BackendError>(e: E) -> StorageError {
    StorageError {
        message: format!("{e}"),
        #[cfg(feature = "std")]
        source: None,
    }
}

impl<B: StorageBackend> Database<B> {
    /// Creates a fresh database on the given backend.
    ///
    /// Initializes the storage engine, writes the file identity header and
    /// dual superblocks, and loads the (empty) schema cache. The backend
    /// should be empty or its contents will be overwritten.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend I/O fails or the configuration is invalid.
    pub fn create(backend: B, config: DatabaseConfig) -> Result<Self, Error> {
        config.validate()?;
        let engine_config = StorageEngineConfig {
            page_size: config.page_size,
            buffer_pool_frames: config.buffer_pool_frames,
            application_id: config.application_id,
        };
        let engine = StorageEngine::create(backend, engine_config)?;
        Self::finish_open(engine, config)
    }

    /// Opens an existing database from the given backend.
    ///
    /// Validates the file identity header, selects the active superblock,
    /// and loads the schema cache from the Schema Store.
    ///
    /// # Errors
    ///
    /// Returns an error if the backend is empty, the format is invalid,
    /// or I/O fails.
    pub fn open(backend: B, config: DatabaseConfig) -> Result<Self, Error> {
        config.validate()?;
        let engine_config = StorageEngineConfig {
            page_size: config.page_size,
            buffer_pool_frames: config.buffer_pool_frames,
            application_id: config.application_id,
        };
        let engine = StorageEngine::open(backend, engine_config)?;
        Self::finish_open(engine, config)
    }

    /// Shared initialization after the storage engine is created or opened.
    fn finish_open(
        engine: StorageEngine<B>,
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
        engine: &mut StorageEngine<B>,
        root: PageId,
        start: &[u8],
        end: &[u8],
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>, StorageError> {
        engine.range_scan(root, start, Some(end))
    }

    /// Loads types, property keys, counters, hierarchy edges, and extension
    /// names from the Schema Store B-tree into the schema cache.
    fn load_schema(
        engine: &mut StorageEngine<B>,
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
                        #[cfg(feature = "std")]
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
                        #[cfg(feature = "std")]
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
                        #[cfg(feature = "std")]
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
                        #[cfg(feature = "std")]
                        source: None,
                    }
                })?;
                let name_len = u16::from_le_bytes(name_len_bytes) as usize;
                if key.len() >= 4 + name_len && let Ok(name) = core::str::from_utf8(&key[4..4 + name_len]) {
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
    /// Returns an error if the internal state cannot be accessed.
    pub fn read_txn(&self) -> Result<ReadTransaction<'_, B>, Error> {
        let snapshot = {
            let current = self.inner.current_snapshot.read();
            Arc::clone(&current)
        };
        let schema_cache = {
            let cache = self.inner.schema_cache.read();
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
    /// Returns an error if the write lock cannot be acquired.
    pub fn write_txn(&self) -> Result<WriteTransaction<'_, B>, Error> {
        let guard = self.inner.write_mutex.lock();
        let snapshot = {
            let current = self.inner.current_snapshot.read();
            Arc::clone(&current)
        };
        let schema_cache = {
            let cache = self.inner.schema_cache.read();
            cache.clone()
        };
        Ok(WriteTransaction::new(
            &self.inner,
            snapshot,
            schema_cache,
            guard,
        ))
    }

    /// Begins a read-write transaction with a timeout.
    ///
    /// Attempts to acquire the write lock within `timeout`. If the lock
    /// cannot be acquired before the deadline, returns
    /// [`TransactionError::WriteLockTimeout`](crate::error::TransactionError::WriteLockTimeout).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Transaction`] with [`TransactionError::WriteLockTimeout`](crate::error::TransactionError::WriteLockTimeout)
    /// if the timeout elapses before the write lock is acquired.
    #[cfg(feature = "std")]
    pub fn try_write_txn(
        &self,
        timeout: std::time::Duration,
    ) -> Result<WriteTransaction<'_, B>, Error> {
        use crate::error::TransactionError;

        let deadline = std::time::Instant::now() + timeout;
        let guard = loop {
            if let Some(g) = self.inner.write_mutex.try_lock() {
                break g;
            }
            if std::time::Instant::now() >= deadline {
                return Err(TransactionError::WriteLockTimeout.into());
            }
            std::thread::sleep(std::time::Duration::from_micros(100));
        };
        let snapshot = {
            let current = self.inner.current_snapshot.read();
            Arc::clone(&current)
        };
        let schema_cache = {
            let cache = self.inner.schema_cache.read();
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
    /// Returns an error if the internal state cannot be accessed.
    pub fn register_constraint(
        &self,
        validator: Box<dyn ConstraintValidator>,
    ) -> Result<(), Error> {
        let name = validator.name().to_string();
        let mut registry = self.inner.constraint_registry.write();
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
    /// Returns an error if the internal state cannot be accessed.
    pub fn unregister_constraint(&self, name: &str) -> Result<bool, Error> {
        let mut registry = self.inner.constraint_registry.write();
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
    /// Returns an error if the internal state cannot be accessed.
    pub fn register_inference_rule(
        &self,
        rule: Box<dyn InferenceRule>,
    ) -> Result<(), Error> {
        let mut engine = self.inner.inference_engine.lock();
        engine.register_rule(rule);
        Ok(())
    }

    /// Unregisters an inference rule by name.
    ///
    /// Returns `true` if a rule was found and removed.
    ///
    /// # Errors
    ///
    /// Returns an error if the internal state cannot be accessed.
    pub fn unregister_inference_rule(&self, name: &str) -> Result<bool, Error> {
        let mut engine = self.inner.inference_engine.lock();
        Ok(engine.unregister_rule(name))
    }

    /// Returns the names of all registered constraint validators.
    pub fn constraint_names(&self) -> Vec<String> {
        let registry = self.inner.constraint_registry.read();
        registry.iter().map(|v| v.name().to_string()).collect()
    }

    /// Returns the names of all registered inference rules in registration order.
    pub fn inference_rule_names(&self) -> Vec<String> {
        let engine = self.inner.inference_engine.lock();
        engine.rule_names()
    }

    /// Returns extensions that are persisted in the database but not
    /// currently registered in memory.
    pub fn missing_extensions(&self) -> MissingExtensions {
        let persisted = self.inner.persisted_extension_names.read();
        let constraints = self.inner.constraint_registry.read();
        let engine = self.inner.inference_engine.lock();

        let registered_constraints: hashbrown::HashSet<String> =
            constraints.iter().map(|v| v.name().to_string()).collect();
        let registered_rules: hashbrown::HashSet<String> =
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

    /// Invokes a closure with a shared reference to the underlying storage backend.
    ///
    /// The storage lock is held for the duration of the closure. Avoid
    /// long-running or blocking operations inside `f`.
    pub fn with_backend<R>(&self, f: impl FnOnce(&B) -> R) -> R {
        let engine = self.inner.storage.lock();
        f(engine.backend())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend_mem::MemoryBackend;
    use phonograph::constraint::{ChangeSet, ConstraintViolation};
    use phonograph::inference::InferenceResult;
    use phonograph::schema::{GraphView, PropertyKeyRegistryView, TypeRegistryView};
    use phonograph::types::TypeId;

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

    /// Helper: create a fresh in-memory database with default config.
    fn create_test_db() -> Database<MemoryBackend> {
        Database::create(MemoryBackend::new(), DatabaseConfig::default()).unwrap()
    }

    #[test]
    fn create_in_memory_database() {
        let db = create_test_db();
        let _rtx = db.read_txn().unwrap();
    }

    #[test]
    fn register_and_list_constraints() {
        let db = create_test_db();
        db.register_constraint(Box::new(TestValidator)).unwrap();
        let names = db.constraint_names();
        assert_eq!(names, vec!["TestValidator"]);
    }

    #[test]
    fn register_and_list_inference_rules() {
        let db = create_test_db();
        db.register_inference_rule(Box::new(TestRule)).unwrap();
        let names = db.inference_rule_names();
        assert_eq!(names, vec!["TestRule"]);
    }

    #[test]
    fn unregister_constraint() {
        let db = create_test_db();
        db.register_constraint(Box::new(TestValidator)).unwrap();
        assert!(db.unregister_constraint("TestValidator").unwrap());
        assert!(!db.unregister_constraint("NonExistent").unwrap());
        assert!(db.constraint_names().is_empty());
    }

    #[test]
    fn missing_extensions_empty_when_none_persisted() {
        let db = create_test_db();
        assert!(db.missing_extensions().is_empty());
    }
}
