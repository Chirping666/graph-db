//! Read-write transactions with read-your-own-writes semantics.
//!
//! A [`WriteTransaction`] holds an exclusive write lock, a base snapshot,
//! and a `WriteBuffer` for pending changes. Reads overlay the buffer on
//! the snapshot. On `commit(self)`, changes are validated, materialized
//! into B-trees, and atomically committed.

use std::marker::PhantomData;
use std::sync::{Arc, MutexGuard};

use crate::constraint::{ChangeSet, ConstraintViolation};
use crate::error::{Error, InferenceError, NotFoundError};
use crate::inference::{
    InferenceMode, InferenceResult, InferredEntity, InferredFact, MaterializedMapping,
    ProvenanceRecord,
};
use crate::schema::{PropertyKeyRegistryView, TypeRegistryView};
use crate::storage::btree::cow::CowResult;
use crate::storage::page::PageId;
use crate::storage::serialization;
use crate::storage::snapshot::{Snapshot, SnapshotRoots};
use crate::storage::StorageEngine;
use crate::types::{
    Edge, EdgeId, Node, NodeId, PropertyKeyId, TypeDefinition, TypeId, Value,
};

use super::database::DatabaseInner;
use super::graph_view::{OverlayGraphView, SnapshotReader};
use super::inference_engine::ProvenanceRegistry;
use super::read_txn::ReadTransaction;
use super::schema_cache::SchemaCache;
use super::write_buffer::{SchemaChange, WriteBuffer};

/// A read-write transaction providing read-your-own-writes and atomic commit.
///
/// Created via [`Database::write_txn`](super::Database::write_txn). Only one write transaction can be
/// active at a time (enforced by the write mutex).
///
/// `commit(self)` consumes the transaction on both success and failure.
/// `abort(self)` explicitly discards changes. If dropped without calling
/// either, changes are discarded automatically.
///
/// `WriteTransaction` is `!Send` and `!Sync` per design decision A12.
///
/// # Examples
///
/// ```
/// use graph_db::db::database::Database;
/// use graph_db::db::config::DatabaseConfig;
/// use graph_db::db::builders::{NodeBuilder, EdgeBuilder, TypeDefinitionBuilder};
/// use graph_db::types::Value;
///
/// let db = Database::open(DatabaseConfig::in_memory()).unwrap();
/// let mut wtx = db.write_txn().unwrap();
///
/// // Register types and property keys
/// let person = wtx.register_type(TypeDefinitionBuilder::node_type("Person").build()).unwrap();
/// let knows = wtx.register_type(TypeDefinitionBuilder::edge_type("knows").build()).unwrap();
/// let name = wtx.get_or_create_property_key("name").unwrap();
///
/// // Insert data
/// let alice = wtx.insert_node(
///     NodeBuilder::new().type_label(person).property(name, Value::String("Alice".into())).build()
/// ).unwrap();
/// let bob = wtx.insert_node(
///     NodeBuilder::new().type_label(person).property(name, Value::String("Bob".into())).build()
/// ).unwrap();
/// wtx.insert_edge(EdgeBuilder::new(alice, bob).type_label(knows).build()).unwrap();
///
/// // Read-your-own-writes
/// assert_eq!(wtx.node_count().unwrap(), 2);
///
/// // Commit
/// wtx.commit().unwrap();
/// ```
pub struct WriteTransaction<'db> {
    pub(crate) inner: &'db DatabaseInner,
    pub(crate) snapshot: Arc<Snapshot>,
    pub(crate) buffer: WriteBuffer,
    pub(crate) schema_cache: SchemaCache,
    _write_guard: MutexGuard<'db, ()>,
    finished: bool,
    /// Whether the write buffer has been mutated (used for cache bypass).
    dirty: bool,
    /// Materialized mapping from the most recent `run_inference` call.
    last_materialization: Option<MaterializedMapping>,
    /// Transaction-local provenance records from materialization.
    pending_provenance: Vec<(InferredEntity, ProvenanceRecord)>,
    /// Entities whose provenance was removed during cleanup (for commit).
    provenance_removals: Vec<InferredEntity>,
    _not_send: PhantomData<*const ()>,
}

impl<'db> WriteTransaction<'db> {
    /// Creates a new write transaction (called by `Database::write_txn`).
    pub(crate) fn new(
        inner: &'db DatabaseInner,
        snapshot: Arc<Snapshot>,
        schema_cache: SchemaCache,
        write_guard: MutexGuard<'db, ()>,
    ) -> Self {
        Self {
            inner,
            snapshot,
            buffer: WriteBuffer::new(),
            schema_cache,
            _write_guard: write_guard,
            finished: false,
            dirty: false,
            last_materialization: None,
            pending_provenance: Vec::new(),
            provenance_removals: Vec::new(),
            _not_send: PhantomData,
        }
    }

    /// Helper: create a temporary ReadTransaction-like reader for base snapshot.
    fn base_reader(&self) -> BaseSnapshotReader<'_, 'db> {
        BaseSnapshotReader { txn: self }
    }

    // ------------------------------------------------------------------
    // Read methods (overlay on base snapshot)
    // ------------------------------------------------------------------

    /// Returns the node with the given id, checking the write buffer first.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn get_node(&self, id: NodeId) -> Result<Option<Node>, Error> {
        if self.buffer.is_node_deleted(id) {
            return Ok(None);
        }
        if let Some(node) = self.buffer.get_pending_node(id) {
            return Ok(Some(node.clone()));
        }
        self.read_base_node(id)
    }

    /// Returns the edge with the given id, checking the write buffer first.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn get_edge(&self, id: EdgeId) -> Result<Option<Edge>, Error> {
        if self.buffer.is_edge_deleted(id) {
            return Ok(None);
        }
        if let Some(edge) = self.buffer.get_pending_edge(id) {
            return Ok(Some(edge.clone()));
        }
        self.read_base_edge(id)
    }

    /// Returns all nodes, overlaying buffer changes on the base snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn all_nodes(&self) -> Result<Vec<Node>, Error> {
        // Reuse ReadTransaction's infrastructure for base reads
        let rtx = self.as_base_read_txn();
        let mut base_nodes = rtx.all_nodes()?;

        // Apply overlay
        base_nodes.retain(|n| !self.buffer.is_node_deleted(n.id));
        for node in base_nodes.iter_mut() {
            if let Some(pending) = self.buffer.get_pending_node(node.id) {
                *node = pending.clone();
            }
        }
        // Add inserted nodes
        for node in self.buffer.inserted_nodes().values() {
            base_nodes.push(node.clone());
        }
        Ok(base_nodes)
    }

    /// Returns all edges, overlaying buffer changes on the base snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn all_edges(&self) -> Result<Vec<Edge>, Error> {
        let rtx = self.as_base_read_txn();
        let start = [0u8; 8];
        let entries =
            rtx.storage_range_scan(self.snapshot.roots.edge_store, &start, None)?;
        let mut edges = Vec::with_capacity(entries.len());
        for (key, value) in &entries {
            let edge_id = serialization::decode_edge_key(key);
            edges.push(ReadTransaction::deserialize_edge(edge_id, value)?);
        }

        // Apply overlay
        edges.retain(|e| !self.buffer.is_edge_deleted(e.id));
        for edge in edges.iter_mut() {
            if let Some(pending) = self.buffer.get_pending_edge(edge.id) {
                *edge = pending.clone();
            }
        }
        for edge in self.buffer.inserted_edges().values() {
            edges.push(edge.clone());
        }
        Ok(edges)
    }

    /// Returns outgoing edges, overlaying buffer changes.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn outgoing_edges(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<Vec<Edge>, Error> {
        let rtx = self.as_base_read_txn();
        let mut edges = rtx.outgoing_edges(node, edge_type)?;

        // Remove deleted, apply updates
        edges.retain(|e| !self.buffer.is_edge_deleted(e.id));
        for edge in edges.iter_mut() {
            if let Some(pending) = self.buffer.get_pending_edge(edge.id) {
                *edge = pending.clone();
            }
        }
        // Add inserted edges matching source (and type filter)
        for edge in self.buffer.inserted_edges().values() {
            if edge.source == node
                && edge_type.is_none_or(|t| edge.type_labels.contains(&t))
            {
                edges.push(edge.clone());
            }
        }
        Ok(edges)
    }

    /// Returns incoming edges, overlaying buffer changes.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn incoming_edges(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<Vec<Edge>, Error> {
        let rtx = self.as_base_read_txn();
        let mut edges = rtx.incoming_edges(node, edge_type)?;

        edges.retain(|e| !self.buffer.is_edge_deleted(e.id));
        for edge in edges.iter_mut() {
            if let Some(pending) = self.buffer.get_pending_edge(edge.id) {
                *edge = pending.clone();
            }
        }
        for edge in self.buffer.inserted_edges().values() {
            if edge.target == node
                && edge_type.is_none_or(|t| edge.type_labels.contains(&t))
            {
                edges.push(edge.clone());
            }
        }
        Ok(edges)
    }

    /// Returns neighbors via outgoing edges, with overlay.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn neighbors(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<Vec<Node>, Error> {
        let edges = self.outgoing_edges(node, edge_type)?;
        let mut nodes = Vec::with_capacity(edges.len());
        for edge in edges {
            if let Some(n) = self.get_node(edge.target)? {
                nodes.push(n);
            }
        }
        Ok(nodes)
    }

    /// Returns nodes by type, with overlay.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn nodes_by_type(
        &self,
        type_id: TypeId,
        include_subtypes: bool,
    ) -> Result<Vec<Node>, Error> {
        let rtx = self.as_base_read_txn();
        // Use a temporary SchemaCache for subtype resolution
        let mut type_ids = vec![type_id];
        if include_subtypes {
            type_ids.extend(self.schema_cache.all_subtypes(type_id));
        }

        let mut result = Vec::new();
        for tid in &type_ids {
            let start = serialization::encode_type_index_key(0x00, *tid, 0);
            let end = serialization::encode_type_index_key(0x00, *tid, u64::MAX);
            let entries = rtx.storage_range_scan(
                self.snapshot.roots.type_index,
                &start,
                Some(&end),
            )?;
            for (key, _) in &entries {
                let (_, _, entity_id) = serialization::decode_type_index_key(key);
                let node_id = NodeId(entity_id);
                if self.buffer.is_node_deleted(node_id) {
                    continue;
                }
                if let Some(node) = self.get_node(node_id)? {
                    result.push(node);
                }
            }
        }

        // Add inserted nodes matching type
        for node in self.buffer.inserted_nodes().values() {
            for tid in &type_ids {
                if node.type_labels.contains(tid) {
                    result.push(node.clone());
                    break;
                }
            }
        }
        Ok(result)
    }

    /// Returns edges by type, with overlay.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn edges_by_type(
        &self,
        type_id: TypeId,
        include_subtypes: bool,
    ) -> Result<Vec<Edge>, Error> {
        let rtx = self.as_base_read_txn();
        let mut type_ids = vec![type_id];
        if include_subtypes {
            type_ids.extend(self.schema_cache.all_subtypes(type_id));
        }

        let mut result = Vec::new();
        for tid in &type_ids {
            let start = serialization::encode_type_index_key(0x01, *tid, 0);
            let end = serialization::encode_type_index_key(0x01, *tid, u64::MAX);
            let entries = rtx.storage_range_scan(
                self.snapshot.roots.type_index,
                &start,
                Some(&end),
            )?;
            for (key, _) in &entries {
                let (_, _, entity_id) = serialization::decode_type_index_key(key);
                let edge_id = EdgeId(entity_id);
                if self.buffer.is_edge_deleted(edge_id) {
                    continue;
                }
                if let Some(edge) = self.get_edge(edge_id)? {
                    result.push(edge);
                }
            }
        }

        for edge in self.buffer.inserted_edges().values() {
            for tid in &type_ids {
                if edge.type_labels.contains(tid) {
                    result.push(edge.clone());
                    break;
                }
            }
        }
        Ok(result)
    }

    /// Returns nodes by property, full scan with overlay.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn nodes_by_property(
        &self,
        key: PropertyKeyId,
        value: &Value,
    ) -> Result<Vec<Node>, Error> {
        let all = self.all_nodes()?;
        Ok(all
            .into_iter()
            .filter(|n| n.properties.get(&key) == Some(value))
            .collect())
    }

    /// Returns the total node count with overlay.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn node_count(&self) -> Result<u64, Error> {
        Ok(self.all_nodes()?.len() as u64)
    }

    /// Returns the total edge count with overlay.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn edge_count(&self) -> Result<u64, Error> {
        Ok(self.all_edges()?.len() as u64)
    }

    /// Returns the outgoing edge count with overlay.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn outgoing_edge_count(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<u64, Error> {
        Ok(self.outgoing_edges(node, edge_type)?.len() as u64)
    }

    /// Returns the incoming edge count with overlay.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn incoming_edge_count(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<u64, Error> {
        Ok(self.incoming_edges(node, edge_type)?.len() as u64)
    }

    // ------------------------------------------------------------------
    // Schema access
    // ------------------------------------------------------------------

    /// Returns a reference to the type registry view.
    pub fn type_registry(&self) -> &dyn TypeRegistryView {
        &self.schema_cache
    }

    /// Returns the property key ID for the given name.
    pub fn get_property_key(&self, name: &str) -> Option<PropertyKeyId> {
        self.schema_cache.get_key_id(name)
    }

    /// Returns the property key name for the given ID.
    pub fn get_property_key_name(&self, id: PropertyKeyId) -> Option<&str> {
        self.schema_cache.get_key_name(id)
    }

    /// Returns a reference to the property key registry view.
    pub fn property_key_registry(&self) -> &dyn PropertyKeyRegistryView {
        &self.schema_cache
    }

    // ------------------------------------------------------------------
    // Schema mutations
    // ------------------------------------------------------------------

    /// Registers a new type definition, assigning it a `TypeId`.
    ///
    /// # Errors
    ///
    /// Returns an error if the type name is duplicate or supertypes are invalid.
    ///
    /// # Examples
    ///
    /// ```
    /// # use graph_db::db::{database::Database, config::DatabaseConfig, builders::*};
    /// # let db = Database::open(DatabaseConfig::in_memory()).unwrap();
    /// let mut wtx = db.write_txn().unwrap();
    /// let person = wtx.register_type(
    ///     TypeDefinitionBuilder::node_type("Person").build()
    /// ).unwrap();
    /// assert!(!person.is_null());
    /// wtx.commit().unwrap();
    /// ```
    pub fn register_type(&mut self, def: TypeDefinition) -> Result<TypeId, Error> {
        let type_id = self.schema_cache.register_type(def.clone())?;
        let mut registered = def;
        registered.id = type_id;
        self.buffer
            .record_schema_change(SchemaChange::TypeRegistered(registered));
        Ok(type_id)
    }

    /// Returns the existing property key ID for `name`, or creates a new one.
    ///
    /// # Errors
    ///
    /// Returns an error on internal failure.
    pub fn get_or_create_property_key(&mut self, name: &str) -> Result<PropertyKeyId, Error> {
        if let Some(id) = self.schema_cache.get_key_id(name) {
            return Ok(id);
        }
        let id = self.schema_cache.get_or_create_property_key(name);
        self.buffer.record_schema_change(
            SchemaChange::PropertyKeyRegistered(
                super::schema_cache::PropertyKeyDefinition {
                    id,
                    name: name.to_string(),
                },
            ),
        );
        Ok(id)
    }

    // ------------------------------------------------------------------
    // Node mutations
    // ------------------------------------------------------------------

    /// Inserts a new node, assigning it a `NodeId`.
    ///
    /// # Errors
    ///
    /// Returns an error on internal failure.
    ///
    /// # Examples
    ///
    /// ```
    /// # use graph_db::db::{database::Database, config::DatabaseConfig, builders::*};
    /// # use graph_db::types::Value;
    /// # let db = Database::open(DatabaseConfig::in_memory()).unwrap();
    /// let mut wtx = db.write_txn().unwrap();
    /// let t = wtx.register_type(TypeDefinitionBuilder::node_type("N").build()).unwrap();
    /// let k = wtx.get_or_create_property_key("name").unwrap();
    /// let id = wtx.insert_node(
    ///     NodeBuilder::new().type_label(t).property(k, Value::String("Alice".into())).build()
    /// ).unwrap();
    /// assert!(!id.is_null());
    /// # wtx.commit().unwrap();
    /// ```
    pub fn insert_node(&mut self, node: Node) -> Result<NodeId, Error> {
        let id = self.schema_cache.allocate_node_id();
        let mut node = node;
        node.id = id;
        node.type_labels.sort();
        node.type_labels.dedup();
        self.buffer.insert_node(node);
        self.dirty = true;
        Ok(id)
    }

    /// Updates an existing node.
    ///
    /// # Errors
    ///
    /// Returns `Error::NotFound` if the node does not exist.
    pub fn update_node(&mut self, node: Node) -> Result<(), Error> {
        let current = self
            .get_node(node.id)?
            .ok_or(Error::NotFound(NotFoundError::Node(node.id)))?;
        let mut updated = node;
        updated.type_labels.sort();
        updated.type_labels.dedup();
        self.buffer.update_node(current, updated);
        self.dirty = true;
        Ok(())
    }

    /// Deletes a node and cascades to delete all incident edges.
    ///
    /// # Errors
    ///
    /// Returns `Error::NotFound` if the node does not exist.
    ///
    /// # Examples
    ///
    /// ```
    /// # use graph_db::db::{database::Database, config::DatabaseConfig, builders::*};
    /// # let db = Database::open(DatabaseConfig::in_memory()).unwrap();
    /// # let mut wtx = db.write_txn().unwrap();
    /// # let t = wtx.register_type(TypeDefinitionBuilder::node_type("N").build()).unwrap();
    /// # let id = wtx.insert_node(NodeBuilder::new().type_label(t).build()).unwrap();
    /// wtx.delete_node(id).unwrap();
    /// assert!(wtx.get_node(id).unwrap().is_none());
    /// # wtx.commit().unwrap();
    /// ```
    pub fn delete_node(&mut self, id: NodeId) -> Result<(), Error> {
        let node = self
            .get_node(id)?
            .ok_or(Error::NotFound(NotFoundError::Node(id)))?;

        // Cascade: delete all incident edges
        let outgoing = self.outgoing_edges(id, None)?;
        for edge in outgoing {
            self.buffer.delete_edge(edge);
        }
        let incoming = self.incoming_edges(id, None)?;
        for edge in incoming {
            self.buffer.delete_edge(edge); // duplicate deletes are no-ops
        }

        self.buffer.delete_node(node);
        self.dirty = true;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Edge mutations
    // ------------------------------------------------------------------

    /// Inserts a new edge, assigning it an `EdgeId`.
    ///
    /// Verifies that source and target nodes exist.
    ///
    /// # Errors
    ///
    /// Returns `Error::NotFound` if either endpoint node does not exist.
    ///
    /// # Examples
    ///
    /// ```
    /// # use graph_db::db::{database::Database, config::DatabaseConfig, builders::*};
    /// # let db = Database::open(DatabaseConfig::in_memory()).unwrap();
    /// let mut wtx = db.write_txn().unwrap();
    /// let nt = wtx.register_type(TypeDefinitionBuilder::node_type("N").build()).unwrap();
    /// let et = wtx.register_type(TypeDefinitionBuilder::edge_type("E").build()).unwrap();
    /// let a = wtx.insert_node(NodeBuilder::new().type_label(nt).build()).unwrap();
    /// let b = wtx.insert_node(NodeBuilder::new().type_label(nt).build()).unwrap();
    /// let eid = wtx.insert_edge(EdgeBuilder::new(a, b).type_label(et).build()).unwrap();
    /// assert!(!eid.is_null());
    /// # wtx.commit().unwrap();
    /// ```
    pub fn insert_edge(&mut self, edge: Edge) -> Result<EdgeId, Error> {
        let id = self.schema_cache.allocate_edge_id();
        let mut edge = edge;
        edge.id = id;
        edge.type_labels.sort();
        edge.type_labels.dedup();

        if self.get_node(edge.source)?.is_none() {
            return Err(Error::NotFound(NotFoundError::Node(edge.source)));
        }
        if self.get_node(edge.target)?.is_none() {
            return Err(Error::NotFound(NotFoundError::Node(edge.target)));
        }

        self.buffer.insert_edge(edge);
        self.dirty = true;
        Ok(id)
    }

    /// Updates an existing edge. Source and target are immutable.
    ///
    /// # Errors
    ///
    /// Returns `Error::NotFound` if the edge does not exist.
    pub fn update_edge(&mut self, edge: Edge) -> Result<(), Error> {
        let current = self
            .get_edge(edge.id)?
            .ok_or(Error::NotFound(NotFoundError::Edge(edge.id)))?;
        let mut updated = edge;
        // Immutable endpoints (design decision A10)
        updated.source = current.source;
        updated.target = current.target;
        updated.type_labels.sort();
        updated.type_labels.dedup();
        self.buffer.update_edge(current, updated);
        self.dirty = true;
        Ok(())
    }

    /// Deletes an edge.
    ///
    /// # Errors
    ///
    /// Returns `Error::NotFound` if the edge does not exist.
    pub fn delete_edge(&mut self, id: EdgeId) -> Result<(), Error> {
        let edge = self
            .get_edge(id)?
            .ok_or(Error::NotFound(NotFoundError::Edge(id)))?;
        self.buffer.delete_edge(edge);
        self.dirty = true;
        Ok(())
    }

    // ------------------------------------------------------------------
    // Property helpers
    // ------------------------------------------------------------------

    /// Sets a property on a node.
    ///
    /// # Errors
    ///
    /// Returns `Error::NotFound` if the node does not exist.
    ///
    /// # Examples
    ///
    /// ```
    /// # use graph_db::db::{database::Database, config::DatabaseConfig, builders::*};
    /// # use graph_db::types::Value;
    /// # let db = Database::open(DatabaseConfig::in_memory()).unwrap();
    /// # let mut wtx = db.write_txn().unwrap();
    /// # let t = wtx.register_type(TypeDefinitionBuilder::node_type("N").build()).unwrap();
    /// # let k = wtx.get_or_create_property_key("name").unwrap();
    /// # let id = wtx.insert_node(NodeBuilder::new().type_label(t).build()).unwrap();
    /// wtx.set_node_property(id, k, Value::String("Alice".into())).unwrap();
    /// let node = wtx.get_node(id).unwrap().unwrap();
    /// assert_eq!(node.properties.get(&k), Some(&Value::String("Alice".into())));
    /// # wtx.commit().unwrap();
    /// ```
    pub fn set_node_property(
        &mut self,
        id: NodeId,
        key: PropertyKeyId,
        value: Value,
    ) -> Result<(), Error> {
        let mut node = self
            .get_node(id)?
            .ok_or(Error::NotFound(NotFoundError::Node(id)))?;
        let before = node.clone();
        node.properties.insert(key, value);
        self.buffer.update_node(before, node);
        self.dirty = true;
        Ok(())
    }

    /// Removes a property from a node.
    ///
    /// # Errors
    ///
    /// Returns `Error::NotFound` if the node does not exist.
    pub fn remove_node_property(
        &mut self,
        id: NodeId,
        key: PropertyKeyId,
    ) -> Result<Option<Value>, Error> {
        let mut node = self
            .get_node(id)?
            .ok_or(Error::NotFound(NotFoundError::Node(id)))?;
        let before = node.clone();
        let removed = node.properties.remove(&key);
        if removed.is_some() {
            self.buffer.update_node(before, node);
            self.dirty = true;
        }
        Ok(removed)
    }

    /// Sets a property on an edge.
    ///
    /// # Errors
    ///
    /// Returns `Error::NotFound` if the edge does not exist.
    pub fn set_edge_property(
        &mut self,
        id: EdgeId,
        key: PropertyKeyId,
        value: Value,
    ) -> Result<(), Error> {
        let mut edge = self
            .get_edge(id)?
            .ok_or(Error::NotFound(NotFoundError::Edge(id)))?;
        let before = edge.clone();
        edge.properties.insert(key, value);
        self.buffer.update_edge(before, edge);
        self.dirty = true;
        Ok(())
    }

    /// Removes a property from an edge.
    ///
    /// # Errors
    ///
    /// Returns `Error::NotFound` if the edge does not exist.
    pub fn remove_edge_property(
        &mut self,
        id: EdgeId,
        key: PropertyKeyId,
    ) -> Result<Option<Value>, Error> {
        let mut edge = self
            .get_edge(id)?
            .ok_or(Error::NotFound(NotFoundError::Edge(id)))?;
        let before = edge.clone();
        let removed = edge.properties.remove(&key);
        if removed.is_some() {
            self.buffer.update_edge(before, edge);
            self.dirty = true;
        }
        Ok(removed)
    }

    // ------------------------------------------------------------------
    // Type label helpers
    // ------------------------------------------------------------------

    /// Adds a type label to a node.
    ///
    /// # Errors
    ///
    /// Returns `Error::NotFound` if the node does not exist.
    pub fn add_node_type(&mut self, id: NodeId, type_id: TypeId) -> Result<(), Error> {
        let mut node = self
            .get_node(id)?
            .ok_or(Error::NotFound(NotFoundError::Node(id)))?;
        if !node.type_labels.contains(&type_id) {
            let before = node.clone();
            node.type_labels.push(type_id);
            node.type_labels.sort();
            self.buffer.update_node(before, node);
            self.dirty = true;
        }
        Ok(())
    }

    /// Removes a type label from a node.
    ///
    /// # Errors
    ///
    /// Returns `Error::NotFound` if the node does not exist.
    pub fn remove_node_type(&mut self, id: NodeId, type_id: TypeId) -> Result<bool, Error> {
        let mut node = self
            .get_node(id)?
            .ok_or(Error::NotFound(NotFoundError::Node(id)))?;
        if let Some(pos) = node.type_labels.iter().position(|t| *t == type_id) {
            let before = node.clone();
            node.type_labels.remove(pos);
            self.buffer.update_node(before, node);
            self.dirty = true;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Adds a type label to an edge.
    ///
    /// # Errors
    ///
    /// Returns `Error::NotFound` if the edge does not exist.
    pub fn add_edge_type(&mut self, id: EdgeId, type_id: TypeId) -> Result<(), Error> {
        let mut edge = self
            .get_edge(id)?
            .ok_or(Error::NotFound(NotFoundError::Edge(id)))?;
        if !edge.type_labels.contains(&type_id) {
            let before = edge.clone();
            edge.type_labels.push(type_id);
            edge.type_labels.sort();
            self.buffer.update_edge(before, edge);
            self.dirty = true;
        }
        Ok(())
    }

    /// Removes a type label from an edge.
    ///
    /// # Errors
    ///
    /// Returns `Error::NotFound` if the edge does not exist.
    pub fn remove_edge_type(&mut self, id: EdgeId, type_id: TypeId) -> Result<bool, Error> {
        let mut edge = self
            .get_edge(id)?
            .ok_or(Error::NotFound(NotFoundError::Edge(id)))?;
        if let Some(pos) = edge.type_labels.iter().position(|t| *t == type_id) {
            let before = edge.clone();
            edge.type_labels.remove(pos);
            self.buffer.update_edge(before, edge);
            self.dirty = true;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    // ------------------------------------------------------------------
    // Validation
    // ------------------------------------------------------------------

    /// Dry-run validation against pending changes.
    ///
    /// Builds a `ChangeSet` and dispatches all registered constraint validators.
    /// Does not commit.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn validate(&self) -> Result<Vec<ConstraintViolation>, Error> {
        let (node_changes, edge_changes) = self.buffer.build_changeset();
        if node_changes.is_empty() && edge_changes.is_empty() {
            return Ok(Vec::new());
        }
        self.run_validators(&node_changes, &edge_changes)
    }

    /// Full revalidation: treats all data as newly inserted.
    ///
    /// **Warning:** This is O(N) for the entire database.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn validate_all(&self) -> Result<Vec<ConstraintViolation>, Error> {
        use crate::constraint::{EdgeChange, NodeChange};
        let all_nodes = self.all_nodes()?;
        let all_edges = self.all_edges()?;
        let node_changes: Vec<NodeChange> =
            all_nodes.into_iter().map(NodeChange::Inserted).collect();
        let edge_changes: Vec<EdgeChange> =
            all_edges.into_iter().map(EdgeChange::Inserted).collect();
        self.run_validators(&node_changes, &edge_changes)
    }

    /// Runs validators against the given changes.
    fn run_validators(
        &self,
        node_changes: &[crate::constraint::NodeChange],
        edge_changes: &[crate::constraint::EdgeChange],
    ) -> Result<Vec<ConstraintViolation>, Error> {
        let changeset = ChangeSet::new(node_changes, edge_changes);
        let affected_types = changeset.affected_types();

        // Build the overlay graph view for validators
        let graph_view = OverlayGraphView::build(
            &self.base_reader(),
            &self.buffer,
            &self.schema_cache,
        );

        let validators = self.inner.constraint_registry.read().unwrap();
        let mut all_violations = Vec::new();
        for validator in validators.iter() {
            if let Some(applies_to) = validator.applies_to_types() {
                if !applies_to.iter().any(|t| affected_types.contains(t)) {
                    continue;
                }
            }
            let violations = validator.validate(
                &changeset,
                &graph_view,
                &self.schema_cache,
                &self.schema_cache,
            );
            all_violations.extend(violations);
        }
        Ok(all_violations)
    }

    // ------------------------------------------------------------------
    // Commit
    // ------------------------------------------------------------------

    /// Commits all pending changes to the database.
    ///
    /// Steps:
    /// 1. Build ChangeSet from WriteBuffer
    /// 2. Run constraint validators — if any violations, return error
    /// 3. Materialize B-tree changes via storage engine
    /// 4. Atomic commit (2-fsync protocol)
    /// 5. Update global snapshot and schema cache
    ///
    /// Consumes the transaction on both success and failure.
    ///
    /// # Errors
    ///
    /// Returns `Error::ConstraintViolation` if validators reject changes,
    /// or `Error::Storage` on I/O failure.
    ///
    /// # Examples
    ///
    /// ```
    /// # use graph_db::db::{database::Database, config::DatabaseConfig, builders::*};
    /// # let db = Database::open(DatabaseConfig::in_memory()).unwrap();
    /// let mut wtx = db.write_txn().unwrap();
    /// let t = wtx.register_type(TypeDefinitionBuilder::node_type("N").build()).unwrap();
    /// wtx.insert_node(NodeBuilder::new().type_label(t).build()).unwrap();
    /// wtx.commit().unwrap();
    ///
    /// let rtx = db.read_txn().unwrap();
    /// assert_eq!(rtx.node_count().unwrap(), 1);
    /// ```
    pub fn commit(mut self) -> Result<(), Error> {
        self.finished = true;

        // Step 1-2: Validate
        if !self.buffer.is_empty() {
            let (node_changes, edge_changes) = self.buffer.build_changeset();
            if !node_changes.is_empty() || !edge_changes.is_empty() {
                let violations = self.run_validators(&node_changes, &edge_changes)?;
                if !violations.is_empty() {
                    return Err(Error::ConstraintViolation(violations));
                }
            }
        }

        // Step 3: Materialize B-tree changes
        let mut engine = self.inner.storage.lock().unwrap();
        let txn_id = engine.transaction_id() + 1;
        let mut roots = self.snapshot.roots.clone();
        let mut all_freed: Vec<(u64, PageId)> = Vec::new();

        // Schema changes first
        for change in self.buffer.schema_changes() {
            match change {
                SchemaChange::TypeRegistered(td) => {
                    let key = serialization::encode_schema_type_key(td.id);
                    let value = serialization::serialize_type_definition(td);
                    let cow = engine.insert(roots.schema_store, &key, &value, txn_id)?;
                    apply_cow(&mut roots.schema_store, &cow, txn_id, &mut all_freed);

                    // Hierarchy edges
                    for st in &td.supertypes {
                        let hkey = serialization::encode_schema_hierarchy_key(td.id, *st);
                        let cow = engine.insert(roots.schema_store, &hkey, &[], txn_id)?;
                        apply_cow(
                            &mut roots.schema_store,
                            &cow,
                            txn_id,
                            &mut all_freed,
                        );
                    }
                }
                SchemaChange::PropertyKeyRegistered(pk) => {
                    let key = serialization::encode_schema_property_key(pk.id);
                    let value = serialization::serialize_property_key_name(&pk.name);
                    let cow = engine.insert(roots.schema_store, &key, &value, txn_id)?;
                    apply_cow(&mut roots.schema_store, &cow, txn_id, &mut all_freed);
                }
                SchemaChange::ExtensionNameRegistered { kind, name } => {
                    let kind_byte = match *kind {
                        "constraint" => 0x01,
                        "inference" => 0x02,
                        _ => continue,
                    };
                    let key = serialization::encode_schema_extension_key(kind_byte, name);
                    let cow = engine.insert(roots.schema_store, &key, &[], txn_id)?;
                    apply_cow(&mut roots.schema_store, &cow, txn_id, &mut all_freed);
                }
                SchemaChange::ExtensionNameUnregistered { kind, name } => {
                    let kind_byte = match *kind {
                        "constraint" => 0x01,
                        "inference" => 0x02,
                        _ => continue,
                    };
                    let key = serialization::encode_schema_extension_key(kind_byte, name);
                    if let Some(cow) = engine.delete(roots.schema_store, &key, txn_id)? {
                        apply_cow(
                            &mut roots.schema_store,
                            &cow,
                            txn_id,
                            &mut all_freed,
                        );
                    }
                }
            }
        }

        // Persist provenance changes (removals then inserts, prefix 0x06).
        for entity in &self.provenance_removals {
            let (key, _) = ProvenanceRegistry::encode_entry(
                entity,
                // Dummy record for key encoding only.
                &ProvenanceRecord {
                    rule_name: String::new(),
                    materialized_at: 0,
                },
            );
            if let Some(cow) = engine.delete(roots.schema_store, &key, txn_id)? {
                apply_cow(
                    &mut roots.schema_store,
                    &cow,
                    txn_id,
                    &mut all_freed,
                );
            }
        }
        for (entity, record) in &self.pending_provenance {
            let (key, value) = ProvenanceRegistry::encode_entry(entity, record);
            let cow = engine.insert(roots.schema_store, &key, &value, txn_id)?;
            apply_cow(&mut roots.schema_store, &cow, txn_id, &mut all_freed);
        }

        // Persist ID counters
        for (counter_name, counter_val) in [
            (0x01u8, self.schema_cache.next_node_id),
            (0x02, self.schema_cache.next_edge_id),
            (0x03, self.schema_cache.next_type_id as u64),
            (0x04, self.schema_cache.next_property_key_id as u64),
        ] {
            let key = serialization::encode_schema_counter_key(counter_name);
            let value = counter_val.to_le_bytes().to_vec();
            let cow = engine.insert(roots.schema_store, &key, &value, txn_id)?;
            apply_cow(&mut roots.schema_store, &cow, txn_id, &mut all_freed);
        }

        commit_node_changes(
            &self.buffer, &mut engine, &mut roots, txn_id, &mut all_freed,
        )?;
        commit_edge_changes(
            &self.buffer, &mut engine, &mut roots, txn_id, &mut all_freed,
        )?;

        // Step 4: Atomic commit
        let new_snapshot = engine.commit(roots, all_freed)?;

        // Step 5: Update globals
        drop(engine); // release storage mutex before taking RwLock
        {
            let mut current = self.inner.current_snapshot.write().unwrap();
            *current = Arc::new(new_snapshot);
        }
        {
            let mut global_cache = self.inner.schema_cache.write().unwrap();
            *global_cache = self.schema_cache.clone();
        }

        Ok(())
    }

    /// Explicitly aborts the transaction, discarding all changes.
    ///
    /// # Examples
    ///
    /// ```
    /// # use graph_db::db::{database::Database, config::DatabaseConfig, builders::*};
    /// # let db = Database::open(DatabaseConfig::in_memory()).unwrap();
    /// let mut wtx = db.write_txn().unwrap();
    /// let t = wtx.register_type(TypeDefinitionBuilder::node_type("N").build()).unwrap();
    /// wtx.insert_node(NodeBuilder::new().type_label(t).build()).unwrap();
    /// wtx.abort(); // changes discarded
    ///
    /// let rtx = db.read_txn().unwrap();
    /// assert_eq!(rtx.node_count().unwrap(), 0);
    /// ```
    pub fn abort(mut self) {
        self.finished = true;
    }

    // ------------------------------------------------------------------
    // Inference dispatch
    // ------------------------------------------------------------------

    /// Runs a named inference rule with the given mode.
    ///
    /// In `Ephemeral` mode, returns inferred facts without modifying the graph.
    /// In `Materialized` mode, writes inferred facts to the write buffer,
    /// records provenance, and makes the `MaterializedMapping` available via
    /// [`last_materialization_mapping`](Self::last_materialization_mapping).
    ///
    /// # Errors
    ///
    /// Returns `Error::Inference(RuleNotFound)` if the rule is not registered.
    /// Returns `Error::Inference(InvalidFact)` if a materialized fact is invalid.
    pub fn run_inference(
        &mut self,
        rule_name: &str,
        mode: InferenceMode,
    ) -> Result<InferenceResult, Error> {
        // Reset last materialization mapping.
        self.last_materialization = None;

        let generation = self.snapshot.transaction_id;
        let result = {
            let mut engine = self.inner.inference_engine.lock().unwrap();

            // Check that the rule exists.
            if engine.get_rule(rule_name).is_none() {
                return Err(Error::Inference(InferenceError::RuleNotFound(
                    rule_name.to_string(),
                )));
            }

            // Cache check: skip if dirty.
            if !self.dirty {
                if let Some(cached) = engine.cache_get(rule_name, generation) {
                    if mode == InferenceMode::Ephemeral {
                        return Ok(cached);
                    }
                    // For materialized mode, use cached result but continue to materialization.
                    cached
                } else {
                    // Cache miss — invoke the rule.
                    let reader = BaseSnapshotReader { txn: self };
                    let view =
                        OverlayGraphView::build(&reader, &self.buffer, &self.schema_cache);
                    let rule = engine.get_rule(rule_name).unwrap();
                    let result = rule.infer(&view, &self.schema_cache, &self.schema_cache);
                    engine.cache_insert(rule_name.to_string(), generation, result.clone());
                    result
                }
            } else {
                // Dirty — always invoke the rule, never cache.
                let reader = BaseSnapshotReader { txn: self };
                let view =
                    OverlayGraphView::build(&reader, &self.buffer, &self.schema_cache);
                let rule = engine.get_rule(rule_name).unwrap();
                rule.infer(&view, &self.schema_cache, &self.schema_cache)
            }
        };

        if mode == InferenceMode::Ephemeral {
            return Ok(result);
        }

        // --- Materialized mode ---
        self.materialize_facts(rule_name, &result)?;
        Ok(result)
    }

    /// Runs all registered inference rules sequentially in registration order.
    ///
    /// In `Materialized` mode, each rule's output is written to the write buffer
    /// before the next rule runs, enabling rule chaining.
    ///
    /// # Errors
    ///
    /// Returns an error if any rule fails.
    pub fn run_all_inference(
        &mut self,
        mode: InferenceMode,
    ) -> Result<Vec<InferenceResult>, Error> {
        let rule_names: Vec<String> = {
            let engine = self.inner.inference_engine.lock().unwrap();
            engine.rule_names()
        };
        let mut results = Vec::with_capacity(rule_names.len());
        for name in &rule_names {
            results.push(self.run_inference(name, mode)?);
        }
        Ok(results)
    }

    /// Returns the materialized mapping from the most recent `run_inference`
    /// call, or `None` if no materialized run has occurred.
    pub fn last_materialization_mapping(&self) -> Option<&MaterializedMapping> {
        self.last_materialization.as_ref()
    }

    /// Materializes inferred facts: validates, cleans up old facts, inserts
    /// new facts, records provenance, and builds the `MaterializedMapping`.
    fn materialize_facts(
        &mut self,
        rule_name: &str,
        result: &InferenceResult,
    ) -> Result<(), Error> {
        let txn_id = self.snapshot.transaction_id + 1;

        // Step 1: Validate all facts before making any changes.
        self.validate_inferred_facts(rule_name, &result.facts)?;

        // Step 2: Cleanup old materialized facts from this rule.
        let old_entities = {
            let mut engine = self.inner.inference_engine.lock().unwrap();
            engine.provenance_mut().remove_by_rule(rule_name)
        };
        // Also remove from pending provenance.
        self.pending_provenance
            .retain(|(e, _)| !old_entities.contains(e));
        // Track removals for commit-time persistence.
        self.provenance_removals.extend(old_entities.iter().cloned());

        // Delete old entities from the write buffer (in reverse order to handle edges before nodes).
        for entity in &old_entities {
            match entity {
                InferredEntity::Node(id) => {
                    // Cascade delete (ignore errors if already deleted).
                    let _ = self.delete_node(*id);
                }
                InferredEntity::Edge(id) => {
                    let _ = self.delete_edge(*id);
                }
                InferredEntity::NodeProperty { node, key } => {
                    let _ = self.remove_node_property(*node, *key);
                }
                InferredEntity::EdgeProperty { edge, key } => {
                    let _ = self.remove_edge_property(*edge, *key);
                }
                InferredEntity::NodeType { node, type_id } => {
                    let _ = self.remove_node_type(*node, *type_id);
                }
                InferredEntity::EdgeType { edge, type_id } => {
                    let _ = self.remove_edge_type(*edge, *type_id);
                }
            }
        }

        // Step 3: Insert new facts and build MaterializedMapping.
        let mut mapping = MaterializedMapping {
            new_node_ids: Vec::new(),
            new_edge_ids: Vec::new(),
        };
        let mut new_provenance = Vec::new();

        for (i, fact) in result.facts.iter().enumerate() {
            match fact {
                InferredFact::NewNode {
                    type_labels,
                    properties,
                    is_anonymous,
                } => {
                    let node = Node {
                        id: NodeId(0), // will be assigned
                        type_labels: type_labels.clone(),
                        properties: properties.clone(),
                        is_anonymous: *is_anonymous,
                    };
                    let id = self.insert_node(node)?;
                    mapping.new_node_ids.push((i, id));
                    new_provenance.push((
                        InferredEntity::Node(id),
                        ProvenanceRecord {
                            rule_name: rule_name.to_string(),
                            materialized_at: txn_id,
                        },
                    ));
                }
                InferredFact::NewEdge {
                    type_labels,
                    source,
                    target,
                    properties,
                } => {
                    let edge = Edge {
                        id: EdgeId(0), // will be assigned
                        type_labels: type_labels.clone(),
                        source: *source,
                        target: *target,
                        properties: properties.clone(),
                    };
                    let id = self.insert_edge(edge)?;
                    mapping.new_edge_ids.push((i, id));
                    new_provenance.push((
                        InferredEntity::Edge(id),
                        ProvenanceRecord {
                            rule_name: rule_name.to_string(),
                            materialized_at: txn_id,
                        },
                    ));
                }
                InferredFact::NodePropertyUpdate { node, key, value } => {
                    self.set_node_property(*node, *key, value.clone())?;
                    new_provenance.push((
                        InferredEntity::NodeProperty {
                            node: *node,
                            key: *key,
                        },
                        ProvenanceRecord {
                            rule_name: rule_name.to_string(),
                            materialized_at: txn_id,
                        },
                    ));
                }
                InferredFact::EdgePropertyUpdate { edge, key, value } => {
                    self.set_edge_property(*edge, *key, value.clone())?;
                    new_provenance.push((
                        InferredEntity::EdgeProperty {
                            edge: *edge,
                            key: *key,
                        },
                        ProvenanceRecord {
                            rule_name: rule_name.to_string(),
                            materialized_at: txn_id,
                        },
                    ));
                }
                InferredFact::NodeTypeAssignment { node, type_id } => {
                    self.add_node_type(*node, *type_id)?;
                    new_provenance.push((
                        InferredEntity::NodeType {
                            node: *node,
                            type_id: *type_id,
                        },
                        ProvenanceRecord {
                            rule_name: rule_name.to_string(),
                            materialized_at: txn_id,
                        },
                    ));
                }
                InferredFact::EdgeTypeAssignment { edge, type_id } => {
                    self.add_edge_type(*edge, *type_id)?;
                    new_provenance.push((
                        InferredEntity::EdgeType {
                            edge: *edge,
                            type_id: *type_id,
                        },
                        ProvenanceRecord {
                            rule_name: rule_name.to_string(),
                            materialized_at: txn_id,
                        },
                    ));
                }
            }
        }

        // Step 4: Record provenance.
        {
            let mut engine = self.inner.inference_engine.lock().unwrap();
            for (entity, record) in &new_provenance {
                engine.provenance_mut().record(
                    entity.clone(),
                    &record.rule_name,
                    record.materialized_at,
                );
            }
        }
        self.pending_provenance.extend(new_provenance);

        // Step 5: Store mapping and mark dirty.
        self.last_materialization = Some(mapping);
        self.dirty = true;

        Ok(())
    }

    /// Validates each inferred fact before materialization.
    ///
    /// Checks that referenced types are registered, referenced nodes/edges
    /// exist, and property keys are registered. Returns an error on the first
    /// invalid fact.
    fn validate_inferred_facts(
        &self,
        rule_name: &str,
        facts: &[InferredFact],
    ) -> Result<(), Error> {
        use crate::types::TypeKind;

        for fact in facts {
            match fact {
                InferredFact::NewNode { type_labels, .. } => {
                    for tid in type_labels {
                        if let Some(td) = self.schema_cache.get_type(*tid) {
                            if td.kind != TypeKind::Node {
                                return Err(Error::Inference(InferenceError::InvalidFact {
                                    rule_name: rule_name.to_string(),
                                    message: format!(
                                        "type {} is not a node type",
                                        tid.0
                                    ),
                                }));
                            }
                        } else {
                            return Err(Error::Inference(InferenceError::InvalidFact {
                                rule_name: rule_name.to_string(),
                                message: format!("type {} is not registered", tid.0),
                            }));
                        }
                    }
                }
                InferredFact::NewEdge {
                    type_labels,
                    source,
                    target,
                    ..
                } => {
                    if self.get_node(*source)?.is_none() {
                        return Err(Error::Inference(InferenceError::InvalidFact {
                            rule_name: rule_name.to_string(),
                            message: format!("source node {} does not exist", source.0),
                        }));
                    }
                    if self.get_node(*target)?.is_none() {
                        return Err(Error::Inference(InferenceError::InvalidFact {
                            rule_name: rule_name.to_string(),
                            message: format!("target node {} does not exist", target.0),
                        }));
                    }
                    for tid in type_labels {
                        if let Some(td) = self.schema_cache.get_type(*tid) {
                            if td.kind != TypeKind::Edge {
                                return Err(Error::Inference(InferenceError::InvalidFact {
                                    rule_name: rule_name.to_string(),
                                    message: format!(
                                        "type {} is not an edge type",
                                        tid.0
                                    ),
                                }));
                            }
                        } else {
                            return Err(Error::Inference(InferenceError::InvalidFact {
                                rule_name: rule_name.to_string(),
                                message: format!("type {} is not registered", tid.0),
                            }));
                        }
                    }
                }
                InferredFact::NodePropertyUpdate { node, key, .. } => {
                    if self.get_node(*node)?.is_none() {
                        return Err(Error::Inference(InferenceError::InvalidFact {
                            rule_name: rule_name.to_string(),
                            message: format!("node {} does not exist", node.0),
                        }));
                    }
                    if self.schema_cache.get_key_name(*key).is_none() {
                        return Err(Error::Inference(InferenceError::InvalidFact {
                            rule_name: rule_name.to_string(),
                            message: format!("property key {} is not registered", key.0),
                        }));
                    }
                }
                InferredFact::EdgePropertyUpdate { edge, key, .. } => {
                    if self.get_edge(*edge)?.is_none() {
                        return Err(Error::Inference(InferenceError::InvalidFact {
                            rule_name: rule_name.to_string(),
                            message: format!("edge {} does not exist", edge.0),
                        }));
                    }
                    if self.schema_cache.get_key_name(*key).is_none() {
                        return Err(Error::Inference(InferenceError::InvalidFact {
                            rule_name: rule_name.to_string(),
                            message: format!("property key {} is not registered", key.0),
                        }));
                    }
                }
                InferredFact::NodeTypeAssignment { node, type_id } => {
                    if self.get_node(*node)?.is_none() {
                        return Err(Error::Inference(InferenceError::InvalidFact {
                            rule_name: rule_name.to_string(),
                            message: format!("node {} does not exist", node.0),
                        }));
                    }
                    if self.schema_cache.get_type(*type_id).is_none() {
                        return Err(Error::Inference(InferenceError::InvalidFact {
                            rule_name: rule_name.to_string(),
                            message: format!("type {} is not registered", type_id.0),
                        }));
                    }
                }
                InferredFact::EdgeTypeAssignment { edge, type_id } => {
                    if self.get_edge(*edge)?.is_none() {
                        return Err(Error::Inference(InferenceError::InvalidFact {
                            rule_name: rule_name.to_string(),
                            message: format!("edge {} does not exist", edge.0),
                        }));
                    }
                    if self.schema_cache.get_type(*type_id).is_none() {
                        return Err(Error::Inference(InferenceError::InvalidFact {
                            rule_name: rule_name.to_string(),
                            message: format!("type {} is not registered", type_id.0),
                        }));
                    }
                }
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Provenance queries
    // ------------------------------------------------------------------

    /// Returns whether a node was created by an inference rule.
    ///
    /// Checks both committed provenance and pending (uncommitted) provenance
    /// from materializations in this transaction.
    ///
    /// # Errors
    ///
    /// Returns an error on storage failure.
    pub fn is_inferred_node(&self, id: NodeId) -> Result<bool, Error> {
        let entity = InferredEntity::Node(id);
        // Check pending provenance first.
        if self.pending_provenance.iter().any(|(e, _)| *e == entity) {
            return Ok(true);
        }
        // Check if removed in this transaction.
        if self.provenance_removals.contains(&entity) {
            return Ok(false);
        }
        let engine = self.inner.inference_engine.lock().unwrap();
        Ok(engine.provenance().is_inferred(&entity))
    }

    /// Returns whether an edge was created by an inference rule.
    ///
    /// # Errors
    ///
    /// Returns an error on storage failure.
    pub fn is_inferred_edge(&self, id: EdgeId) -> Result<bool, Error> {
        let entity = InferredEntity::Edge(id);
        if self.pending_provenance.iter().any(|(e, _)| *e == entity) {
            return Ok(true);
        }
        if self.provenance_removals.contains(&entity) {
            return Ok(false);
        }
        let engine = self.inner.inference_engine.lock().unwrap();
        Ok(engine.provenance().is_inferred(&entity))
    }

    /// Returns the provenance record for an inferred node, or `None`
    /// if the node was user-asserted.
    ///
    /// # Errors
    ///
    /// Returns an error on storage failure.
    pub fn node_provenance(
        &self,
        id: NodeId,
    ) -> Result<Option<ProvenanceRecord>, Error> {
        let entity = InferredEntity::Node(id);
        // Check pending provenance first.
        if let Some((_, record)) = self.pending_provenance.iter().find(|(e, _)| *e == entity) {
            return Ok(Some(record.clone()));
        }
        if self.provenance_removals.contains(&entity) {
            return Ok(None);
        }
        let engine = self.inner.inference_engine.lock().unwrap();
        Ok(engine.provenance().get(&entity).cloned())
    }

    /// Returns the provenance record for an inferred edge, or `None`
    /// if the edge was user-asserted.
    ///
    /// # Errors
    ///
    /// Returns an error on storage failure.
    pub fn edge_provenance(
        &self,
        id: EdgeId,
    ) -> Result<Option<ProvenanceRecord>, Error> {
        let entity = InferredEntity::Edge(id);
        if let Some((_, record)) = self.pending_provenance.iter().find(|(e, _)| *e == entity) {
            return Ok(Some(record.clone()));
        }
        if self.provenance_removals.contains(&entity) {
            return Ok(None);
        }
        let engine = self.inner.inference_engine.lock().unwrap();
        Ok(engine.provenance().get(&entity).cloned())
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    /// Creates a temporary ReadTransaction-like view for base snapshot reads.
    fn as_base_read_txn(&self) -> ReadTransaction<'_> {
        ReadTransaction::new(
            self.inner,
            Arc::clone(&self.snapshot),
            self.schema_cache.clone(),
        )
    }

    /// Reads a node directly from the base snapshot (bypassing buffer).
    fn read_base_node(&self, id: NodeId) -> Result<Option<Node>, Error> {
        let key = serialization::encode_node_key(id);
        let rtx = self.as_base_read_txn();
        match rtx.storage_search(self.snapshot.roots.node_store, &key)? {
            Some(value) => Ok(Some(ReadTransaction::deserialize_node(id, &value)?)),
            None => Ok(None),
        }
    }

    /// Reads an edge directly from the base snapshot (bypassing buffer).
    fn read_base_edge(&self, id: EdgeId) -> Result<Option<Edge>, Error> {
        let key = serialization::encode_edge_key(id);
        let rtx = self.as_base_read_txn();
        match rtx.storage_search(self.snapshot.roots.edge_store, &key)? {
            Some(value) => Ok(Some(ReadTransaction::deserialize_edge(id, &value)?)),
            None => Ok(None),
        }
    }
}

impl Drop for WriteTransaction<'_> {
    fn drop(&mut self) {
        if !self.finished {
            // Abort: WriteBuffer is dropped, MutexGuard releases the lock.
            self.finished = true;
        }
    }
}

/// Adapter that implements `SnapshotReader` for the base snapshot
/// (used by `OverlayGraphView::build` during validation).
struct BaseSnapshotReader<'a, 'db> {
    txn: &'a WriteTransaction<'db>,
}

impl<'a, 'db> SnapshotReader for BaseSnapshotReader<'a, 'db> {
    fn get_node(&self, id: NodeId) -> Option<Node> {
        self.txn.read_base_node(id).ok().flatten()
    }

    fn get_edge(&self, id: EdgeId) -> Option<Edge> {
        self.txn.read_base_edge(id).ok().flatten()
    }

    fn outgoing_edges(&self, node: NodeId, edge_type: Option<TypeId>) -> Vec<Edge> {
        let rtx = self.txn.as_base_read_txn();
        rtx.outgoing_edges(node, edge_type).unwrap_or_default()
    }

    fn incoming_edges(&self, node: NodeId, edge_type: Option<TypeId>) -> Vec<Edge> {
        let rtx = self.txn.as_base_read_txn();
        rtx.incoming_edges(node, edge_type).unwrap_or_default()
    }

    fn all_nodes(&self) -> Vec<Node> {
        let rtx = self.txn.as_base_read_txn();
        rtx.all_nodes().unwrap_or_default()
    }

    fn all_edges(&self) -> Vec<Edge> {
        let rtx = self.txn.as_base_read_txn();
        let start = [0u8; 8];
        let entries = rtx
            .storage_range_scan(self.txn.snapshot.roots.edge_store, &start, None)
            .unwrap_or_default();
        let mut edges = Vec::new();
        for (key, value) in &entries {
            let edge_id = serialization::decode_edge_key(key);
            if let Ok(edge) = ReadTransaction::deserialize_edge(edge_id, value) {
                edges.push(edge);
            }
        }
        edges
    }
}

/// Applies a CowResult to a root pointer and tracks freed pages.
fn apply_cow(
    root: &mut PageId,
    cow: &CowResult,
    txn_id: u64,
    freed: &mut Vec<(u64, PageId)>,
) {
    *root = cow.new_root;
    for &page in &cow.freed_pages {
        freed.push((txn_id, page));
    }
}

/// Inserts edge adjacency and type index entries for the given type labels.
#[allow(clippy::too_many_arguments)]
fn insert_edge_indexes<B: crate::backend::StorageBackend>(
    engine: &mut StorageEngine<B>,
    roots: &mut SnapshotRoots,
    txn_id: u64,
    freed: &mut Vec<(u64, PageId)>,
    edge_id: EdgeId,
    source: NodeId,
    target: NodeId,
    type_labels: &[TypeId],
) -> Result<(), Error> {
    for tid in type_labels {
        let okey = serialization::encode_outgoing_adj_key(source, *tid, edge_id);
        let cow = engine.insert(roots.outgoing_adj, &okey, &[], txn_id)?;
        apply_cow(&mut roots.outgoing_adj, &cow, txn_id, freed);

        let ikey = serialization::encode_incoming_adj_key(target, *tid, edge_id);
        let cow = engine.insert(roots.incoming_adj, &ikey, &[], txn_id)?;
        apply_cow(&mut roots.incoming_adj, &cow, txn_id, freed);

        let tkey = serialization::encode_type_index_key(0x01, *tid, edge_id.0);
        let cow = engine.insert(roots.type_index, &tkey, &[], txn_id)?;
        apply_cow(&mut roots.type_index, &cow, txn_id, freed);
    }
    Ok(())
}

/// Materializes node inserts, updates, and deletes into the B-tree.
fn commit_node_changes<B: crate::backend::StorageBackend>(
    buffer: &WriteBuffer,
    engine: &mut StorageEngine<B>,
    roots: &mut SnapshotRoots,
    txn_id: u64,
    freed: &mut Vec<(u64, PageId)>,
) -> Result<(), Error> {
    for node in buffer.inserted_nodes().values() {
        let key = serialization::encode_node_key(node.id);
        let props = serialization::serialize_properties(&node.properties);
        let record = serialization::NodeRecord::from_node(node, &props, None);
        let value = record.serialize();
        let cow = engine.insert(roots.node_store, &key, &value, txn_id)?;
        apply_cow(&mut roots.node_store, &cow, txn_id, freed);

        for tid in &node.type_labels {
            let tkey = serialization::encode_type_index_key(0x00, *tid, node.id.0);
            let cow = engine.insert(roots.type_index, &tkey, &[], txn_id)?;
            apply_cow(&mut roots.type_index, &cow, txn_id, freed);
        }
    }

    for (before, after) in buffer.updated_nodes().values() {
        let key = serialization::encode_node_key(after.id);
        let props = serialization::serialize_properties(&after.properties);
        let record = serialization::NodeRecord::from_node(after, &props, None);
        let value = record.serialize();
        let cow = engine.insert(roots.node_store, &key, &value, txn_id)?;
        apply_cow(&mut roots.node_store, &cow, txn_id, freed);

        for tid in &before.type_labels {
            if !after.type_labels.contains(tid) {
                let tkey = serialization::encode_type_index_key(0x00, *tid, after.id.0);
                if let Some(cow) = engine.delete(roots.type_index, &tkey, txn_id)? {
                    apply_cow(&mut roots.type_index, &cow, txn_id, freed);
                }
            }
        }
        for tid in &after.type_labels {
            if !before.type_labels.contains(tid) {
                let tkey = serialization::encode_type_index_key(0x00, *tid, after.id.0);
                let cow = engine.insert(roots.type_index, &tkey, &[], txn_id)?;
                apply_cow(&mut roots.type_index, &cow, txn_id, freed);
            }
        }
    }

    for node in buffer.deleted_nodes().values() {
        let key = serialization::encode_node_key(node.id);
        if let Some(cow) = engine.delete(roots.node_store, &key, txn_id)? {
            apply_cow(&mut roots.node_store, &cow, txn_id, freed);
        }
        for tid in &node.type_labels {
            let tkey = serialization::encode_type_index_key(0x00, *tid, node.id.0);
            if let Some(cow) = engine.delete(roots.type_index, &tkey, txn_id)? {
                apply_cow(&mut roots.type_index, &cow, txn_id, freed);
            }
        }
    }

    Ok(())
}

/// Materializes edge inserts, updates, and deletes into the B-tree.
fn commit_edge_changes<B: crate::backend::StorageBackend>(
    buffer: &WriteBuffer,
    engine: &mut StorageEngine<B>,
    roots: &mut SnapshotRoots,
    txn_id: u64,
    freed: &mut Vec<(u64, PageId)>,
) -> Result<(), Error> {
    for edge in buffer.inserted_edges().values() {
        let key = serialization::encode_edge_key(edge.id);
        let props = serialization::serialize_properties(&edge.properties);
        let record = serialization::EdgeRecord::from_edge(edge, &props, None);
        let value = record.serialize();
        let cow = engine.insert(roots.edge_store, &key, &value, txn_id)?;
        apply_cow(&mut roots.edge_store, &cow, txn_id, freed);

        insert_edge_indexes(
            engine, roots, txn_id, freed,
            edge.id, edge.source, edge.target, &edge.type_labels,
        )?;
    }

    for (before, after) in buffer.updated_edges().values() {
        let key = serialization::encode_edge_key(after.id);
        let props = serialization::serialize_properties(&after.properties);
        let record = serialization::EdgeRecord::from_edge(after, &props, None);
        let value = record.serialize();
        let cow = engine.insert(roots.edge_store, &key, &value, txn_id)?;
        apply_cow(&mut roots.edge_store, &cow, txn_id, freed);

        let removed: Vec<TypeId> = before.type_labels.iter()
            .filter(|t| !after.type_labels.contains(t))
            .copied()
            .collect();
        let added: Vec<TypeId> = after.type_labels.iter()
            .filter(|t| !before.type_labels.contains(t))
            .copied()
            .collect();
        delete_edge_indexes(
            engine, roots, txn_id, freed,
            after.id, after.source, after.target, &removed,
        )?;
        insert_edge_indexes(
            engine, roots, txn_id, freed,
            after.id, after.source, after.target, &added,
        )?;
    }

    for edge in buffer.deleted_edge_ids().values() {
        let key = serialization::encode_edge_key(edge.id);
        if let Some(cow) = engine.delete(roots.edge_store, &key, txn_id)? {
            apply_cow(&mut roots.edge_store, &cow, txn_id, freed);
        }
        delete_edge_indexes(
            engine, roots, txn_id, freed,
            edge.id, edge.source, edge.target, &edge.type_labels,
        )?;
    }

    Ok(())
}

/// Deletes edge adjacency and type index entries for the given type labels.
#[allow(clippy::too_many_arguments)]
fn delete_edge_indexes<B: crate::backend::StorageBackend>(
    engine: &mut StorageEngine<B>,
    roots: &mut SnapshotRoots,
    txn_id: u64,
    freed: &mut Vec<(u64, PageId)>,
    edge_id: EdgeId,
    source: NodeId,
    target: NodeId,
    type_labels: &[TypeId],
) -> Result<(), Error> {
    for tid in type_labels {
        let okey = serialization::encode_outgoing_adj_key(source, *tid, edge_id);
        if let Some(cow) = engine.delete(roots.outgoing_adj, &okey, txn_id)? {
            apply_cow(&mut roots.outgoing_adj, &cow, txn_id, freed);
        }

        let ikey = serialization::encode_incoming_adj_key(target, *tid, edge_id);
        if let Some(cow) = engine.delete(roots.incoming_adj, &ikey, txn_id)? {
            apply_cow(&mut roots.incoming_adj, &cow, txn_id, freed);
        }

        let tkey = serialization::encode_type_index_key(0x01, *tid, edge_id.0);
        if let Some(cow) = engine.delete(roots.type_index, &tkey, txn_id)? {
            apply_cow(&mut roots.type_index, &cow, txn_id, freed);
        }
    }
    Ok(())
}
