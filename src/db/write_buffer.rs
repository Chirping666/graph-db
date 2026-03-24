//! In-memory change tracking for write transactions.
//!
//! The `WriteBuffer` tracks all pending mutations within a write transaction.
//! It serves two purposes: (1) enabling read-your-own-writes by overlaying
//! pending changes on the base snapshot, and (2) producing the
//! `ChangeSet` at commit time.

use std::collections::BTreeMap;

use crate::constraint::{EdgeChange, NodeChange};
use crate::types::{Edge, EdgeId, Node, NodeId, TypeDefinition};

use super::schema_cache::PropertyKeyDefinition;

/// Tracks a schema change that must be persisted on commit.
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub(crate) enum SchemaChange {
    /// A new type definition was registered.
    TypeRegistered(TypeDefinition),
    /// A new property key was registered.
    PropertyKeyRegistered(PropertyKeyDefinition),
    /// An extension name was registered for persistence.
    ExtensionNameRegistered {
        /// `"constraint"` or `"inference"`.
        kind: &'static str,
        /// The extension name.
        name: String,
    },
    /// An extension name was unregistered.
    ExtensionNameUnregistered {
        /// `"constraint"` or `"inference"`.
        kind: &'static str,
        /// The extension name.
        name: String,
    },
}

/// In-memory buffer tracking all pending mutations in a write transaction.
///
/// Implements mutation collapsing so that the [`ChangeSet`] produced at
/// commit time is minimal and correct:
///
/// - Insert then update → single insert with final data
/// - Insert then delete → removed entirely (no changeset entry)
/// - Update then update → single update with original before, latest after
/// - Update then delete → single delete with original before
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct WriteBuffer {
    /// Pending node inserts: NodeId → Node.
    node_inserts: BTreeMap<NodeId, Node>,
    /// Pending node updates: NodeId → (before, after).
    node_updates: BTreeMap<NodeId, (Node, Node)>,
    /// Pending node deletes: NodeId → deleted Node (original state).
    node_deletes: BTreeMap<NodeId, Node>,

    /// Pending edge inserts: EdgeId → Edge.
    edge_inserts: BTreeMap<EdgeId, Edge>,
    /// Pending edge updates: EdgeId → (before, after).
    edge_updates: BTreeMap<EdgeId, (Edge, Edge)>,
    /// Pending edge deletes: EdgeId → deleted Edge (original state).
    edge_deletes: BTreeMap<EdgeId, Edge>,

    /// Schema changes to persist on commit.
    schema_changes: Vec<SchemaChange>,
}

#[allow(dead_code)]
impl WriteBuffer {
    /// Creates an empty write buffer.
    pub fn new() -> Self {
        Self {
            node_inserts: BTreeMap::new(),
            node_updates: BTreeMap::new(),
            node_deletes: BTreeMap::new(),
            edge_inserts: BTreeMap::new(),
            edge_updates: BTreeMap::new(),
            edge_deletes: BTreeMap::new(),
            schema_changes: Vec::new(),
        }
    }

    // ------------------------------------------------------------------
    // Node operations
    // ------------------------------------------------------------------

    /// Records a node insertion.
    pub fn insert_node(&mut self, node: Node) {
        self.node_inserts.insert(node.id, node);
    }

    /// Records a node update with collapsing.
    ///
    /// - If the node was previously inserted in this transaction,
    ///   the insert is replaced with the updated version.
    /// - If the node was previously updated, the "after" is replaced
    ///   while keeping the original "before".
    /// - Otherwise, a new update entry is created.
    pub fn update_node(&mut self, before: Node, after: Node) {
        let id = after.id;

        // insert → update: collapse to insert with updated data
        if let Some(existing) = self.node_inserts.get_mut(&id) {
            *existing = after;
            return;
        }

        // update → update: keep original before, use latest after
        if let Some(entry) = self.node_updates.get_mut(&id) {
            entry.1 = after;
            return;
        }

        self.node_updates.insert(id, (before, after));
    }

    /// Records a node deletion with collapsing.
    ///
    /// - If the node was previously inserted in this transaction,
    ///   it is removed entirely (no changeset entry).
    /// - If the node was previously updated, the update is removed
    ///   and a delete is recorded with the original "before" state.
    /// - Otherwise, a new delete entry is created.
    pub fn delete_node(&mut self, node: Node) {
        let id = node.id;

        // insert → delete: remove entirely
        if self.node_inserts.remove(&id).is_some() {
            return;
        }

        // update → delete: use original before
        if let Some((before, _)) = self.node_updates.remove(&id) {
            self.node_deletes.insert(id, before);
            return;
        }

        // Already deleted? No-op (handles duplicate cascade deletes).
        if self.node_deletes.contains_key(&id) {
            return;
        }

        self.node_deletes.insert(id, node);
    }

    /// Returns `true` if the node was inserted in this transaction.
    pub fn is_node_inserted(&self, id: NodeId) -> bool {
        self.node_inserts.contains_key(&id)
    }

    /// Returns `true` if the node was deleted in this transaction.
    pub fn is_node_deleted(&self, id: NodeId) -> bool {
        self.node_deletes.contains_key(&id)
    }

    /// Returns the latest pending version of a node (from inserts or updates).
    pub fn get_pending_node(&self, id: NodeId) -> Option<&Node> {
        if let Some(node) = self.node_inserts.get(&id) {
            return Some(node);
        }
        if let Some((_, after)) = self.node_updates.get(&id) {
            return Some(after);
        }
        None
    }

    // ------------------------------------------------------------------
    // Edge operations
    // ------------------------------------------------------------------

    /// Records an edge insertion.
    pub fn insert_edge(&mut self, edge: Edge) {
        self.edge_inserts.insert(edge.id, edge);
    }

    /// Records an edge update with collapsing.
    pub fn update_edge(&mut self, before: Edge, after: Edge) {
        let id = after.id;

        if let Some(existing) = self.edge_inserts.get_mut(&id) {
            *existing = after;
            return;
        }

        if let Some(entry) = self.edge_updates.get_mut(&id) {
            entry.1 = after;
            return;
        }

        self.edge_updates.insert(id, (before, after));
    }

    /// Records an edge deletion with collapsing.
    pub fn delete_edge(&mut self, edge: Edge) {
        let id = edge.id;

        if self.edge_inserts.remove(&id).is_some() {
            return;
        }

        if let Some((before, _)) = self.edge_updates.remove(&id) {
            self.edge_deletes.insert(id, before);
            return;
        }

        if self.edge_deletes.contains_key(&id) {
            return;
        }

        self.edge_deletes.insert(id, edge);
    }

    /// Returns `true` if the edge was deleted in this transaction.
    pub fn is_edge_deleted(&self, id: EdgeId) -> bool {
        self.edge_deletes.contains_key(&id)
    }

    /// Returns the latest pending version of an edge.
    pub fn get_pending_edge(&self, id: EdgeId) -> Option<&Edge> {
        if let Some(edge) = self.edge_inserts.get(&id) {
            return Some(edge);
        }
        if let Some((_, after)) = self.edge_updates.get(&id) {
            return Some(after);
        }
        None
    }

    /// Returns IDs of edges inserted in this transaction with the given source.
    pub fn inserted_edge_ids_for_source(&self, source: NodeId) -> Vec<EdgeId> {
        self.edge_inserts
            .values()
            .filter(|e| e.source == source)
            .map(|e| e.id)
            .collect()
    }

    /// Returns IDs of edges inserted in this transaction with the given target.
    pub fn inserted_edge_ids_for_target(&self, target: NodeId) -> Vec<EdgeId> {
        self.edge_inserts
            .values()
            .filter(|e| e.target == target)
            .map(|e| e.id)
            .collect()
    }

    /// Returns all inserted edges (for overlay queries).
    pub fn inserted_edges(&self) -> &BTreeMap<EdgeId, Edge> {
        &self.edge_inserts
    }

    /// Returns all deleted edges.
    pub fn deleted_edge_ids(&self) -> &BTreeMap<EdgeId, Edge> {
        &self.edge_deletes
    }

    /// Returns all inserted nodes (for overlay queries).
    pub fn inserted_nodes(&self) -> &BTreeMap<NodeId, Node> {
        &self.node_inserts
    }

    /// Returns all updated nodes.
    pub fn updated_nodes(&self) -> &BTreeMap<NodeId, (Node, Node)> {
        &self.node_updates
    }

    /// Returns all deleted nodes.
    pub fn deleted_nodes(&self) -> &BTreeMap<NodeId, Node> {
        &self.node_deletes
    }

    /// Returns all updated edges.
    pub fn updated_edges(&self) -> &BTreeMap<EdgeId, (Edge, Edge)> {
        &self.edge_updates
    }

    // ------------------------------------------------------------------
    // Schema operations
    // ------------------------------------------------------------------

    /// Records a schema change for persistence on commit.
    pub fn record_schema_change(&mut self, change: SchemaChange) {
        self.schema_changes.push(change);
    }

    /// Returns all recorded schema changes.
    pub fn schema_changes(&self) -> &[SchemaChange] {
        &self.schema_changes
    }

    /// Returns `true` if the buffer has no pending changes.
    pub fn is_empty(&self) -> bool {
        self.node_inserts.is_empty()
            && self.node_updates.is_empty()
            && self.node_deletes.is_empty()
            && self.edge_inserts.is_empty()
            && self.edge_updates.is_empty()
            && self.edge_deletes.is_empty()
            && self.schema_changes.is_empty()
    }

    // ------------------------------------------------------------------
    // ChangeSet production
    // ------------------------------------------------------------------

    /// Builds the changeset from the current buffer state.
    ///
    /// Returns owned vectors of node and edge changes. The caller must
    /// hold these vectors and borrow them into a [`ChangeSet`].
    pub fn build_changeset(&self) -> (Vec<NodeChange>, Vec<EdgeChange>) {
        let mut node_changes = Vec::new();
        for node in self.node_inserts.values() {
            node_changes.push(NodeChange::Inserted(node.clone()));
        }
        for (before, after) in self.node_updates.values() {
            node_changes.push(NodeChange::Modified {
                before: before.clone(),
                after: after.clone(),
            });
        }
        for node in self.node_deletes.values() {
            node_changes.push(NodeChange::Deleted(node.clone()));
        }

        let mut edge_changes = Vec::new();
        for edge in self.edge_inserts.values() {
            edge_changes.push(EdgeChange::Inserted(edge.clone()));
        }
        for (before, after) in self.edge_updates.values() {
            edge_changes.push(EdgeChange::Modified {
                before: before.clone(),
                after: after.clone(),
            });
        }
        for edge in self.edge_deletes.values() {
            edge_changes.push(EdgeChange::Deleted(edge.clone()));
        }

        (node_changes, edge_changes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{NodeId, EdgeId, TypeId, PropertyMap};

    fn make_node(id: u64) -> Node {
        Node {
            id: NodeId(id),
            type_labels: vec![TypeId(1)],
            properties: PropertyMap::new(),
            is_anonymous: false,
        }
    }

    fn make_edge(id: u64, src: u64, tgt: u64) -> Edge {
        Edge {
            id: EdgeId(id),
            type_labels: vec![TypeId(10)],
            source: NodeId(src),
            target: NodeId(tgt),
            properties: PropertyMap::new(),
        }
    }

    #[test]
    fn insert_node_and_get_pending() {
        let mut buf = WriteBuffer::new();
        let node = make_node(1);
        buf.insert_node(node.clone());
        assert!(buf.is_node_inserted(NodeId(1)));
        assert_eq!(buf.get_pending_node(NodeId(1)).unwrap().id, NodeId(1));
    }

    #[test]
    fn insert_then_update_collapses_to_insert() {
        let mut buf = WriteBuffer::new();
        let node = make_node(1);
        buf.insert_node(node.clone());

        let mut updated = node.clone();
        updated.type_labels.push(TypeId(2));
        buf.update_node(node, updated.clone());

        let (nc, _) = buf.build_changeset();
        assert_eq!(nc.len(), 1);
        match &nc[0] {
            NodeChange::Inserted(n) => assert_eq!(n.type_labels.len(), 2),
            _ => panic!("expected Inserted"),
        }
    }

    #[test]
    fn insert_then_delete_produces_nothing() {
        let mut buf = WriteBuffer::new();
        let node = make_node(1);
        buf.insert_node(node.clone());
        buf.delete_node(node);

        let (nc, _) = buf.build_changeset();
        assert!(nc.is_empty());
    }

    #[test]
    fn update_produces_modified() {
        let mut buf = WriteBuffer::new();
        let before = make_node(1);
        let mut after = before.clone();
        after.type_labels.push(TypeId(2));
        buf.update_node(before.clone(), after.clone());

        let (nc, _) = buf.build_changeset();
        assert_eq!(nc.len(), 1);
        match &nc[0] {
            NodeChange::Modified { before: b, after: a } => {
                assert_eq!(b.type_labels.len(), 1);
                assert_eq!(a.type_labels.len(), 2);
            }
            _ => panic!("expected Modified"),
        }
    }

    #[test]
    fn update_then_update_keeps_original_before() {
        let mut buf = WriteBuffer::new();
        let original = make_node(1);
        let mut mid = original.clone();
        mid.type_labels.push(TypeId(2));
        buf.update_node(original.clone(), mid.clone());

        let mut final_ver = mid.clone();
        final_ver.type_labels.push(TypeId(3));
        buf.update_node(mid, final_ver.clone());

        let (nc, _) = buf.build_changeset();
        assert_eq!(nc.len(), 1);
        match &nc[0] {
            NodeChange::Modified { before, after } => {
                assert_eq!(before.type_labels.len(), 1); // original
                assert_eq!(after.type_labels.len(), 3);  // latest
            }
            _ => panic!("expected Modified"),
        }
    }

    #[test]
    fn update_then_delete_uses_original_before() {
        let mut buf = WriteBuffer::new();
        let original = make_node(1);
        let mut updated = original.clone();
        updated.type_labels.push(TypeId(2));
        buf.update_node(original.clone(), updated.clone());
        buf.delete_node(updated);

        let (nc, _) = buf.build_changeset();
        assert_eq!(nc.len(), 1);
        match &nc[0] {
            NodeChange::Deleted(n) => assert_eq!(n.type_labels.len(), 1), // original
            _ => panic!("expected Deleted"),
        }
    }

    #[test]
    fn edge_insert_tracking() {
        let mut buf = WriteBuffer::new();
        let e1 = make_edge(1, 10, 20);
        let e2 = make_edge(2, 10, 30);
        let e3 = make_edge(3, 20, 30);
        buf.insert_edge(e1);
        buf.insert_edge(e2);
        buf.insert_edge(e3);

        let ids = buf.inserted_edge_ids_for_source(NodeId(10));
        assert_eq!(ids.len(), 2);

        let ids = buf.inserted_edge_ids_for_target(NodeId(30));
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn is_node_deleted_correct() {
        let mut buf = WriteBuffer::new();
        let node = make_node(1);
        assert!(!buf.is_node_deleted(NodeId(1)));
        buf.delete_node(node);
        assert!(buf.is_node_deleted(NodeId(1)));
    }

    #[test]
    fn schema_change_recording() {
        let mut buf = WriteBuffer::new();
        buf.record_schema_change(SchemaChange::ExtensionNameRegistered {
            kind: "constraint",
            name: "test".to_string(),
        });
        assert_eq!(buf.schema_changes().len(), 1);
    }

    #[test]
    fn is_empty_when_fresh() {
        let buf = WriteBuffer::new();
        assert!(buf.is_empty());
    }

    #[test]
    fn is_not_empty_after_insert() {
        let mut buf = WriteBuffer::new();
        buf.insert_node(make_node(1));
        assert!(!buf.is_empty());
    }

    #[test]
    fn edge_insert_then_delete_produces_nothing() {
        let mut buf = WriteBuffer::new();
        let edge = make_edge(1, 10, 20);
        buf.insert_edge(edge.clone());
        buf.delete_edge(edge);

        let (_, ec) = buf.build_changeset();
        assert!(ec.is_empty());
    }

    #[test]
    fn duplicate_delete_is_noop() {
        let mut buf = WriteBuffer::new();
        let node = make_node(1);
        buf.delete_node(node.clone());
        buf.delete_node(node); // duplicate — should not panic
        let (nc, _) = buf.build_changeset();
        assert_eq!(nc.len(), 1);
    }
}
