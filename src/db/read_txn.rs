//! Read-only transactions with snapshot isolation.
//!
//! A [`ReadTransaction`] sees a consistent snapshot of the database at the
//! time it was created. It does not block writers and multiple read
//! transactions can coexist.

use std::marker::PhantomData;
use std::sync::Arc;

use crate::error::{Error, InferenceError};
use crate::inference::{InferenceResult, ProvenanceRecord};
use crate::schema::{PropertyKeyRegistryView, TypeRegistryView};
use crate::storage::page::PageId;
use crate::storage::serialization;
use crate::storage::snapshot::Snapshot;
use crate::types::{Edge, EdgeId, Node, NodeId, PropertyKeyId, TypeId, Value};

use super::database::DatabaseInner;
use super::schema_cache::SchemaCache;

/// A read-only transaction providing snapshot-isolated reads.
///
/// Created via [`Database::read_txn`](super::Database::read_txn). Sees a consistent snapshot of the
/// database and does not block other readers or writers.
///
/// `ReadTransaction` is `!Send` and `!Sync` per design decision A12.
pub struct ReadTransaction<'db> {
    pub(crate) inner: &'db DatabaseInner,
    pub(crate) snapshot: Arc<Snapshot>,
    pub(crate) schema_cache: SchemaCache,
    /// Makes this type `!Send` and `!Sync`.
    _not_send: PhantomData<*const ()>,
}

impl<'db> ReadTransaction<'db> {
    /// Creates a new read transaction (called by `Database::read_txn`).
    pub(crate) fn new(
        inner: &'db DatabaseInner,
        snapshot: Arc<Snapshot>,
        schema_cache: SchemaCache,
    ) -> Self {
        Self {
            inner,
            snapshot,
            schema_cache,
            _not_send: PhantomData,
        }
    }

    // ------------------------------------------------------------------
    // Storage helpers
    // ------------------------------------------------------------------

    /// Point lookup in a B-tree.
    pub(crate) fn storage_search(
        &self,
        root: PageId,
        key: &[u8],
    ) -> Result<Option<Vec<u8>>, Error> {
        let mut engine = self.inner.storage.lock().unwrap();
        engine.search(root, key).map_err(Error::Storage)
    }

    /// Range scan collecting all results.
    #[allow(clippy::type_complexity)]
    pub(crate) fn storage_range_scan(
        &self,
        root: PageId,
        start_key: &[u8],
        end_key: Option<&[u8]>,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>, Error> {
        let mut engine = self.inner.storage.lock().unwrap();
        engine.range_scan(root, start_key, end_key).map_err(Error::Storage)
    }

    /// Deserializes a Node from a NodeRecord stored in the Node Store B-tree.
    pub(crate) fn deserialize_node(node_id: NodeId, value: &[u8]) -> Result<Node, Error> {
        let record = serialization::NodeRecord::deserialize(value)?;
        let properties = if record.property_size > 0 {
            serialization::deserialize_properties(&record.inline_properties)?
        } else {
            Default::default()
        };
        Ok(record.to_node(node_id, properties))
    }

    /// Deserializes an Edge from an EdgeRecord stored in the Edge Store B-tree.
    pub(crate) fn deserialize_edge(edge_id: EdgeId, value: &[u8]) -> Result<Edge, Error> {
        let record = serialization::EdgeRecord::deserialize(value)?;
        let properties = if record.property_size > 0 {
            serialization::deserialize_properties(&record.inline_properties)?
        } else {
            Default::default()
        };
        Ok(record.to_edge(edge_id, properties))
    }

    // ------------------------------------------------------------------
    // Node/edge lookups
    // ------------------------------------------------------------------

    /// Returns the node with the given id, or `None` if not found.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn get_node(&self, id: NodeId) -> Result<Option<Node>, Error> {
        let key = serialization::encode_node_key(id);
        match self.storage_search(self.snapshot.roots.node_store, &key)? {
            Some(value) => Ok(Some(Self::deserialize_node(id, &value)?)),
            None => Ok(None),
        }
    }

    /// Returns the edge with the given id, or `None` if not found.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn get_edge(&self, id: EdgeId) -> Result<Option<Edge>, Error> {
        let key = serialization::encode_edge_key(id);
        match self.storage_search(self.snapshot.roots.edge_store, &key)? {
            Some(value) => Ok(Some(Self::deserialize_edge(id, &value)?)),
            None => Ok(None),
        }
    }

    /// Returns all nodes in the database.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn all_nodes(&self) -> Result<Vec<Node>, Error> {
        let start = [0u8; 8];
        let entries =
            self.storage_range_scan(self.snapshot.roots.node_store, &start, None)?;
        let mut nodes = Vec::with_capacity(entries.len());
        for (key, value) in &entries {
            let node_id = serialization::decode_node_key(key);
            nodes.push(Self::deserialize_node(node_id, value)?);
        }
        Ok(nodes)
    }

    // ------------------------------------------------------------------
    // Traversal methods
    // ------------------------------------------------------------------

    /// Returns all outgoing edges from the given node.
    ///
    /// If `edge_type` is `Some`, only edges with that type label are returned.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn outgoing_edges(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<Vec<Edge>, Error> {
        let (start_key, end_key) = adj_key_range(node, edge_type);
        let entries = self.storage_range_scan(
            self.snapshot.roots.outgoing_adj,
            &start_key,
            Some(&end_key),
        )?;
        let mut edges = Vec::with_capacity(entries.len());
        for (key, _) in &entries {
            let (_, _, edge_id) = serialization::decode_outgoing_adj_key(key);
            if let Some(edge) = self.get_edge(edge_id)? {
                edges.push(edge);
            }
        }
        Ok(edges)
    }

    /// Returns all incoming edges to the given node.
    ///
    /// If `edge_type` is `Some`, only edges with that type label are returned.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn incoming_edges(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<Vec<Edge>, Error> {
        let (start_key, end_key) = adj_key_range(node, edge_type);
        let entries = self.storage_range_scan(
            self.snapshot.roots.incoming_adj,
            &start_key,
            Some(&end_key),
        )?;
        let mut edges = Vec::with_capacity(entries.len());
        for (key, _) in &entries {
            let (_, _, edge_id) = serialization::decode_incoming_adj_key(key);
            if let Some(edge) = self.get_edge(edge_id)? {
                edges.push(edge);
            }
        }
        Ok(edges)
    }

    /// Returns all neighbor nodes reachable via outgoing edges.
    ///
    /// If `edge_type` is `Some`, only follows edges with that type label.
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

    // ------------------------------------------------------------------
    // Type-based and property-based queries
    // ------------------------------------------------------------------

    /// Returns all nodes with the given type label.
    ///
    /// If `include_subtypes` is `true`, also includes nodes of subtypes.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn nodes_by_type(
        &self,
        type_id: TypeId,
        include_subtypes: bool,
    ) -> Result<Vec<Node>, Error> {
        let mut type_ids = vec![type_id];
        if include_subtypes {
            type_ids.extend(self.schema_cache.all_subtypes(type_id));
        }
        let mut result = Vec::new();
        for tid in type_ids {
            let start = serialization::encode_type_index_key(0x00, tid, 0);
            let end = serialization::encode_type_index_key(0x00, tid, u64::MAX);
            let entries = self.storage_range_scan(
                self.snapshot.roots.type_index,
                &start,
                Some(&end),
            )?;
            for (key, _) in &entries {
                let (_, _, entity_id) = serialization::decode_type_index_key(key);
                let node_id = NodeId(entity_id);
                if let Some(node) = self.get_node(node_id)? {
                    result.push(node);
                }
            }
        }
        Ok(result)
    }

    /// Returns all edges with the given type label.
    ///
    /// If `include_subtypes` is `true`, also includes edges of subtypes.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn edges_by_type(
        &self,
        type_id: TypeId,
        include_subtypes: bool,
    ) -> Result<Vec<Edge>, Error> {
        let mut type_ids = vec![type_id];
        if include_subtypes {
            type_ids.extend(self.schema_cache.all_subtypes(type_id));
        }
        let mut result = Vec::new();
        for tid in type_ids {
            let start = serialization::encode_type_index_key(0x01, tid, 0);
            let end = serialization::encode_type_index_key(0x01, tid, u64::MAX);
            let entries = self.storage_range_scan(
                self.snapshot.roots.type_index,
                &start,
                Some(&end),
            )?;
            for (key, _) in &entries {
                let (_, _, entity_id) = serialization::decode_type_index_key(key);
                let edge_id = EdgeId(entity_id);
                if let Some(edge) = self.get_edge(edge_id)? {
                    result.push(edge);
                }
            }
        }
        Ok(result)
    }

    /// Returns all nodes that have the given property set to the given value.
    ///
    /// This is a full scan of the Node Store (no property index in v1).
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

    // ------------------------------------------------------------------
    // Counting methods
    // ------------------------------------------------------------------

    /// Returns the total number of nodes in the database.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn node_count(&self) -> Result<u64, Error> {
        let start = [0u8; 8];
        let entries =
            self.storage_range_scan(self.snapshot.roots.node_store, &start, None)?;
        Ok(entries.len() as u64)
    }

    /// Returns the total number of edges in the database.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn edge_count(&self) -> Result<u64, Error> {
        let start = [0u8; 8];
        let entries =
            self.storage_range_scan(self.snapshot.roots.edge_store, &start, None)?;
        Ok(entries.len() as u64)
    }

    /// Returns the number of outgoing edges from a node.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn outgoing_edge_count(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<u64, Error> {
        let (start_key, end_key) = adj_key_range(node, edge_type);
        let entries = self.storage_range_scan(
            self.snapshot.roots.outgoing_adj,
            &start_key,
            Some(&end_key),
        )?;
        Ok(entries.len() as u64)
    }

    /// Returns the number of incoming edges to a node.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    pub fn incoming_edge_count(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<u64, Error> {
        let (start_key, end_key) = adj_key_range(node, edge_type);
        let entries = self.storage_range_scan(
            self.snapshot.roots.incoming_adj,
            &start_key,
            Some(&end_key),
        )?;
        Ok(entries.len() as u64)
    }

    // ------------------------------------------------------------------
    // Schema access
    // ------------------------------------------------------------------

    /// Returns a reference to the type registry view.
    pub fn type_registry(&self) -> &dyn TypeRegistryView {
        &self.schema_cache
    }

    /// Returns the property key ID for the given name, if registered.
    pub fn get_property_key(&self, name: &str) -> Option<PropertyKeyId> {
        self.schema_cache.get_key_id(name)
    }

    /// Returns the property key name for the given ID, if registered.
    pub fn get_property_key_name(&self, id: PropertyKeyId) -> Option<&str> {
        self.schema_cache.get_key_name(id)
    }

    /// Returns a reference to the property key registry view.
    pub fn property_key_registry(&self) -> &dyn PropertyKeyRegistryView {
        &self.schema_cache
    }

    // ------------------------------------------------------------------
    // Inference stubs (Task 26)
    // ------------------------------------------------------------------

    /// Runs a named inference rule in ephemeral mode.
    ///
    /// **Stub:** Returns `Error::Inference(RuleNotFound)` until Task 26.
    ///
    /// # Errors
    ///
    /// Always returns `Error::Inference(InferenceError::RuleNotFound)`.
    pub fn run_inference(&self, rule_name: &str) -> Result<InferenceResult, Error> {
        Err(Error::Inference(InferenceError::RuleNotFound(
            rule_name.to_string(),
        )))
    }

    /// Runs all registered inference rules.
    ///
    /// **Stub:** Returns an empty vector until Task 26.
    ///
    /// # Errors
    ///
    /// Returns an error on failure.
    pub fn run_all_inference(&self) -> Result<Vec<InferenceResult>, Error> {
        Ok(Vec::new())
    }

    // ------------------------------------------------------------------
    // Provenance stubs (Task 26)
    // ------------------------------------------------------------------

    /// Returns whether a node was inferred.
    ///
    /// **Stub:** Always returns `false` until Task 26.
    ///
    /// # Errors
    ///
    /// Returns an error on storage failure.
    pub fn is_inferred_node(&self, _id: NodeId) -> Result<bool, Error> {
        Ok(false)
    }

    /// Returns whether an edge was inferred.
    ///
    /// **Stub:** Always returns `false` until Task 26.
    ///
    /// # Errors
    ///
    /// Returns an error on storage failure.
    pub fn is_inferred_edge(&self, _id: EdgeId) -> Result<bool, Error> {
        Ok(false)
    }

    /// Returns the provenance record for a node.
    ///
    /// **Stub:** Always returns `None` until Task 26.
    ///
    /// # Errors
    ///
    /// Returns an error on storage failure.
    pub fn node_provenance(
        &self,
        _id: NodeId,
    ) -> Result<Option<ProvenanceRecord>, Error> {
        Ok(None)
    }

    /// Returns the provenance record for an edge.
    ///
    /// **Stub:** Always returns `None` until Task 26.
    ///
    /// # Errors
    ///
    /// Returns an error on storage failure.
    pub fn edge_provenance(
        &self,
        _id: EdgeId,
    ) -> Result<Option<ProvenanceRecord>, Error> {
        Ok(None)
    }

    /// Explicitly finishes this read transaction, releasing the snapshot.
    pub fn finish(self) {
        // Consumed by value; snapshot Arc dropped automatically.
    }
}

/// Computes the adjacency key range for a given node and optional type filter.
fn adj_key_range(node: NodeId, edge_type: Option<TypeId>) -> (Vec<u8>, Vec<u8>) {
    match edge_type {
        Some(tid) => {
            let start =
                serialization::encode_outgoing_adj_key(node, tid, EdgeId(0));
            let end = serialization::encode_outgoing_adj_key(
                node,
                tid,
                EdgeId(u64::MAX),
            );
            (start.to_vec(), end.to_vec())
        }
        None => {
            let start = serialization::encode_outgoing_adj_key(
                node,
                TypeId(0),
                EdgeId(0),
            );
            let end = serialization::encode_outgoing_adj_key(
                node,
                TypeId(u32::MAX),
                EdgeId(u64::MAX),
            );
            (start.to_vec(), end.to_vec())
        }
    }
}
