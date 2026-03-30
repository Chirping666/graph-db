//! Overlay graph view for constraint validators and inference rules.
//!
//! `OverlayGraphView` implements [`GraphView`] by merging data from a
//! base snapshot with pending changes from a `WriteBuffer`. It owns all
//! data in internal maps so that trait methods can return borrowed references.

use alloc::vec::Vec;
use alloc::collections::BTreeMap;

use phonograph::schema::GraphView;
use phonograph::types::{Edge, EdgeId, Node, NodeId, PropertyKeyId, TypeId, Value};

use super::schema_cache::SchemaCache;
use super::write_buffer::WriteBuffer;

/// A read interface for the base snapshot, abstracted for testability.
///
/// In production, this is backed by the storage engine's B-tree reads.
/// In tests, a simple in-memory map implementation is used.
#[allow(dead_code)]
pub(crate) trait SnapshotReader {
    /// Returns the node with the given id from the base snapshot.
    fn get_node(&self, id: NodeId) -> Option<Node>;
    /// Returns the edge with the given id from the base snapshot.
    fn get_edge(&self, id: EdgeId) -> Option<Edge>;
    /// Returns all outgoing edges from the given node.
    fn outgoing_edges(&self, node: NodeId, edge_type: Option<TypeId>) -> Vec<Edge>;
    /// Returns all incoming edges to the given node.
    fn incoming_edges(&self, node: NodeId, edge_type: Option<TypeId>) -> Vec<Edge>;
    /// Returns all nodes.
    fn all_nodes(&self) -> Vec<Node>;
    /// Returns all edges.
    fn all_edges(&self) -> Vec<Edge>;

    /// Returns all nodes whose type labels overlap with any of the given type IDs.
    /// Used for changeset-scoped preloading.
    fn nodes_by_type_ids(&self, type_ids: &[TypeId]) -> Vec<Node> {
        self.all_nodes()
            .into_iter()
            .filter(|n| n.type_labels.iter().any(|t| type_ids.contains(t)))
            .collect()
    }

    /// Returns all edges whose type labels overlap with any of the given type IDs.
    /// Used for changeset-scoped preloading.
    fn edges_by_type_ids(&self, type_ids: &[TypeId]) -> Vec<Edge> {
        self.all_edges()
            .into_iter()
            .filter(|e| e.type_labels.iter().any(|t| type_ids.contains(t)))
            .collect()
    }
}

/// A [`GraphView`] implementation that overlays [`WriteBuffer`] changes
/// on top of a base snapshot.
///
/// Constructed at validation/commit time and owns all data so that
/// `GraphView` trait methods can return borrowed references.
#[allow(dead_code)]
pub(crate) struct OverlayGraphView<'s> {
    /// All nodes visible in the overlay (base + inserts - deletes + updates).
    nodes: BTreeMap<NodeId, Node>,
    /// All edges visible in the overlay.
    edges: BTreeMap<EdgeId, Edge>,
    /// Outgoing edge index: NodeId → list of EdgeIds.
    outgoing_index: BTreeMap<NodeId, Vec<EdgeId>>,
    /// Incoming edge index: NodeId → list of EdgeIds.
    incoming_index: BTreeMap<NodeId, Vec<EdgeId>>,
    /// Schema cache for subtype resolution in `nodes_by_type`/`edges_by_type`.
    schema: &'s SchemaCache,
}

#[allow(dead_code)]
impl<'s> OverlayGraphView<'s> {
    /// Constructs an overlay view by merging the base snapshot with
    /// the write buffer's pending changes.
    ///
    /// This loads nodes and edges from the base snapshot, applies the
    /// WriteBuffer overlay (inserts, updates, deletes), and builds
    /// adjacency indexes. The result is a self-contained snapshot of the
    /// "as if committed" state.
    ///
    /// When `affected_types` is `Some`, only base entities whose type labels
    /// overlap with the given type IDs are loaded, plus adjacency neighbors
    /// of changed nodes. When `None`, all base entities are loaded.
    pub fn build(
        base: &impl SnapshotReader,
        buffer: &WriteBuffer,
        schema: &'s SchemaCache,
        affected_types: Option<&[TypeId]>,
    ) -> Self {
        // Load nodes from base (scoped or full).
        let mut nodes: BTreeMap<NodeId, Node> = BTreeMap::new();
        let base_nodes = match affected_types {
            Some(type_ids) => base.nodes_by_type_ids(type_ids),
            None => base.all_nodes(),
        };
        for node in base_nodes {
            nodes.insert(node.id, node);
        }

        // Load edges from base (scoped or full).
        let mut edges: BTreeMap<EdgeId, Edge> = BTreeMap::new();
        let base_edges = match affected_types {
            Some(type_ids) => base.edges_by_type_ids(type_ids),
            None => base.all_edges(),
        };
        for edge in base_edges {
            edges.insert(edge.id, edge);
        }

        // When scoped, also load adjacency neighbors of changed nodes so
        // validators can traverse from changed entities.
        if affected_types.is_some() {
            for node_id in buffer.inserted_nodes().keys()
                .chain(buffer.updated_nodes().keys())
                .chain(buffer.deleted_nodes().keys())
            {
                for edge in base.outgoing_edges(*node_id, None) {
                    if let Some(target) = base.get_node(edge.target) {
                        nodes.entry(target.id).or_insert(target);
                    }
                    edges.entry(edge.id).or_insert(edge);
                }
                for edge in base.incoming_edges(*node_id, None) {
                    if let Some(source) = base.get_node(edge.source) {
                        nodes.entry(source.id).or_insert(source);
                    }
                    edges.entry(edge.id).or_insert(edge);
                }
            }
        }

        // Apply buffer overlay: deletes, updates, inserts.
        for &id in buffer.deleted_nodes().keys() {
            nodes.remove(&id);
        }
        for (id, (_, after)) in buffer.updated_nodes() {
            nodes.insert(*id, after.clone());
        }
        for (id, node) in buffer.inserted_nodes() {
            nodes.insert(*id, node.clone());
        }

        // Apply edge buffer overlay.
        for &id in buffer.deleted_edge_ids().keys() {
            edges.remove(&id);
        }
        for (id, (_, after)) in buffer.updated_edges() {
            edges.insert(*id, after.clone());
        }
        for (id, edge) in buffer.inserted_edges() {
            edges.insert(*id, edge.clone());
        }

        // Build adjacency indexes.
        let mut outgoing_index: BTreeMap<NodeId, Vec<EdgeId>> = BTreeMap::new();
        let mut incoming_index: BTreeMap<NodeId, Vec<EdgeId>> = BTreeMap::new();
        for edge in edges.values() {
            outgoing_index
                .entry(edge.source)
                .or_default()
                .push(edge.id);
            incoming_index
                .entry(edge.target)
                .or_default()
                .push(edge.id);
        }

        Self {
            nodes,
            edges,
            outgoing_index,
            incoming_index,
            schema,
        }
    }
}

impl GraphView for OverlayGraphView<'_> {
    fn get_node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    fn get_edge(&self, id: EdgeId) -> Option<&Edge> {
        self.edges.get(&id)
    }

    fn outgoing_edges(&self, node: NodeId, edge_type: Option<TypeId>) -> Vec<&Edge> {
        let edge_ids = match self.outgoing_index.get(&node) {
            Some(ids) => ids,
            None => return Vec::new(),
        };
        edge_ids
            .iter()
            .filter_map(|eid| self.edges.get(eid))
            .filter(|e| edge_type.is_none_or(|tid| e.type_labels.contains(&tid)))
            .collect()
    }

    fn incoming_edges(&self, node: NodeId, edge_type: Option<TypeId>) -> Vec<&Edge> {
        let edge_ids = match self.incoming_index.get(&node) {
            Some(ids) => ids,
            None => return Vec::new(),
        };
        edge_ids
            .iter()
            .filter_map(|eid| self.edges.get(eid))
            .filter(|e| edge_type.is_none_or(|tid| e.type_labels.contains(&tid)))
            .collect()
    }

    fn nodes_by_type(&self, type_id: TypeId, include_subtypes: bool) -> Vec<&Node> {
        use phonograph::schema::TypeRegistryView;

        let mut type_ids = alloc::vec![type_id];
        if include_subtypes {
            type_ids.extend(self.schema.all_subtypes(type_id));
        }
        self.nodes
            .values()
            .filter(|n| type_ids.iter().any(|t| n.type_labels.contains(t)))
            .collect()
    }

    fn edges_by_type(&self, type_id: TypeId, include_subtypes: bool) -> Vec<&Edge> {
        use phonograph::schema::TypeRegistryView;

        let mut type_ids = alloc::vec![type_id];
        if include_subtypes {
            type_ids.extend(self.schema.all_subtypes(type_id));
        }
        self.edges
            .values()
            .filter(|e| type_ids.iter().any(|t| e.type_labels.contains(t)))
            .collect()
    }

    fn nodes_by_property(&self, key: PropertyKeyId, value: &Value) -> Vec<&Node> {
        self.nodes
            .values()
            .filter(|n| n.properties.get(&key).is_some_and(|v| v.total_eq(value)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use phonograph::types::{PropertyMap, TypeId};

    /// A simple in-memory snapshot reader for testing.
    struct MockSnapshot {
        nodes: BTreeMap<NodeId, Node>,
        edges: BTreeMap<EdgeId, Edge>,
    }

    impl MockSnapshot {
        fn new() -> Self {
            Self {
                nodes: BTreeMap::new(),
                edges: BTreeMap::new(),
            }
        }
    }

    impl SnapshotReader for MockSnapshot {
        fn get_node(&self, id: NodeId) -> Option<Node> {
            self.nodes.get(&id).cloned()
        }

        fn get_edge(&self, id: EdgeId) -> Option<Edge> {
            self.edges.get(&id).cloned()
        }

        fn outgoing_edges(&self, node: NodeId, edge_type: Option<TypeId>) -> Vec<Edge> {
            self.edges
                .values()
                .filter(|e| {
                    e.source == node
                        && edge_type.is_none_or(|t| e.type_labels.contains(&t))
                })
                .cloned()
                .collect()
        }

        fn incoming_edges(&self, node: NodeId, edge_type: Option<TypeId>) -> Vec<Edge> {
            self.edges
                .values()
                .filter(|e| {
                    e.target == node
                        && edge_type.is_none_or(|t| e.type_labels.contains(&t))
                })
                .cloned()
                .collect()
        }

        fn all_nodes(&self) -> Vec<Node> {
            self.nodes.values().cloned().collect()
        }

        fn all_edges(&self) -> Vec<Edge> {
            self.edges.values().cloned().collect()
        }
    }

    fn make_node(id: u64, type_id: u32) -> Node {
        Node {
            id: NodeId(id),
            type_labels: vec![TypeId(type_id)],
            properties: PropertyMap::new(),
            is_anonymous: false,
        }
    }

    fn make_edge(id: u64, src: u64, tgt: u64, type_id: u32) -> Edge {
        Edge {
            id: EdgeId(id),
            type_labels: vec![TypeId(type_id)],
            source: NodeId(src),
            target: NodeId(tgt),
            properties: PropertyMap::new(),
        }
    }

    #[test]
    fn base_node_visible_without_changes() {
        let mut snap = MockSnapshot::new();
        snap.nodes.insert(NodeId(1), make_node(1, 1));
        let buf = WriteBuffer::new();
        let schema = SchemaCache::new();
        let view = OverlayGraphView::build(&snap, &buf, &schema, None);

        assert!(view.get_node(NodeId(1)).is_some());
    }

    #[test]
    fn deleted_node_not_visible() {
        let mut snap = MockSnapshot::new();
        let node = make_node(1, 1);
        snap.nodes.insert(NodeId(1), node.clone());
        let mut buf = WriteBuffer::new();
        buf.delete_node(node);
        let schema = SchemaCache::new();
        let view = OverlayGraphView::build(&snap, &buf, &schema, None);

        assert!(view.get_node(NodeId(1)).is_none());
    }

    #[test]
    fn inserted_node_visible() {
        let snap = MockSnapshot::new();
        let mut buf = WriteBuffer::new();
        buf.insert_node(make_node(2, 1));
        let schema = SchemaCache::new();
        let view = OverlayGraphView::build(&snap, &buf, &schema, None);

        assert!(view.get_node(NodeId(2)).is_some());
    }

    #[test]
    fn updated_node_returns_new_version() {
        let mut snap = MockSnapshot::new();
        let node = make_node(1, 1);
        snap.nodes.insert(NodeId(1), node.clone());

        let mut updated = node.clone();
        updated.type_labels.push(TypeId(2));
        let mut buf = WriteBuffer::new();
        buf.update_node(node, updated);
        let schema = SchemaCache::new();
        let view = OverlayGraphView::build(&snap, &buf, &schema, None);

        let n = view.get_node(NodeId(1)).unwrap();
        assert_eq!(n.type_labels.len(), 2);
    }

    #[test]
    fn outgoing_edges_merges_correctly() {
        let mut snap = MockSnapshot::new();
        snap.nodes.insert(NodeId(1), make_node(1, 1));
        snap.nodes.insert(NodeId(2), make_node(2, 1));
        let e1 = make_edge(1, 1, 2, 10);
        let e2 = make_edge(2, 1, 2, 10);
        snap.edges.insert(EdgeId(1), e1.clone());
        snap.edges.insert(EdgeId(2), e2.clone());

        let mut buf = WriteBuffer::new();
        // Delete e1, insert e3.
        buf.delete_edge(e1);
        buf.insert_edge(make_edge(3, 1, 2, 10));

        let schema = SchemaCache::new();
        let view = OverlayGraphView::build(&snap, &buf, &schema, None);

        let out = view.outgoing_edges(NodeId(1), None);
        assert_eq!(out.len(), 2); // e2 + e3 (e1 deleted)
        let ids: Vec<EdgeId> = out.iter().map(|e| e.id).collect();
        assert!(ids.contains(&EdgeId(2)));
        assert!(ids.contains(&EdgeId(3)));
        assert!(!ids.contains(&EdgeId(1)));
    }

    #[test]
    fn nodes_by_type_includes_inserted() {
        let mut snap = MockSnapshot::new();
        snap.nodes.insert(NodeId(1), make_node(1, 5));

        let mut buf = WriteBuffer::new();
        buf.insert_node(make_node(2, 5));

        let schema = SchemaCache::new();
        let view = OverlayGraphView::build(&snap, &buf, &schema, None);

        let result = view.nodes_by_type(TypeId(5), false);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn nodes_by_type_excludes_deleted() {
        let mut snap = MockSnapshot::new();
        let node = make_node(1, 5);
        snap.nodes.insert(NodeId(1), node.clone());

        let mut buf = WriteBuffer::new();
        buf.delete_node(node);

        let schema = SchemaCache::new();
        let view = OverlayGraphView::build(&snap, &buf, &schema, None);

        let result = view.nodes_by_type(TypeId(5), false);
        assert!(result.is_empty());
    }

    #[test]
    fn nodes_by_property_finds_buffered_insert() {
        use phonograph::types::{PropertyKeyId, Value};

        let snap = MockSnapshot::new();
        let mut buf = WriteBuffer::new();
        let mut node = make_node(1, 1);
        node.properties
            .insert(PropertyKeyId(1), Value::String("Alice".to_string()));
        buf.insert_node(node);

        let schema = SchemaCache::new();
        let view = OverlayGraphView::build(&snap, &buf, &schema, None);

        let result = view.nodes_by_property(
            PropertyKeyId(1),
            &Value::String("Alice".to_string()),
        );
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn nodes_by_type_with_subtypes() {
        use phonograph::types::{TypeDefinition, TypeKind};

        let mut schema = SchemaCache::new();
        // Register parent type "Animal" (gets TypeId assigned by schema cache).
        let animal_id = schema
            .register_type(TypeDefinition {
                id: TypeId(0), // overwritten by register_type
                name: "Animal".into(),
                kind: TypeKind::Node,
                supertypes: vec![],
                property_declarations: vec![],
                open: true,
                metadata: PropertyMap::new(),
            })
            .unwrap();
        // Register child type "Dog" with Animal as supertype.
        let dog_id = schema
            .register_type(TypeDefinition {
                id: TypeId(0),
                name: "Dog".into(),
                kind: TypeKind::Node,
                supertypes: vec![animal_id],
                property_declarations: vec![],
                open: true,
                metadata: PropertyMap::new(),
            })
            .unwrap();

        let mut snap = MockSnapshot::new();
        let dog_node = Node {
            id: NodeId(1),
            type_labels: vec![dog_id],
            properties: PropertyMap::new(),
            is_anonymous: false,
        };
        snap.nodes.insert(NodeId(1), dog_node);

        let buf = WriteBuffer::new();
        let view = OverlayGraphView::build(&snap, &buf, &schema, None);

        // With include_subtypes=true, querying Animal should find the Dog node.
        let result = view.nodes_by_type(animal_id, true);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, NodeId(1));

        // With include_subtypes=false, querying Animal should NOT find the Dog node.
        let result = view.nodes_by_type(animal_id, false);
        assert!(result.is_empty());
    }

    #[test]
    fn edges_by_type_with_subtypes() {
        use phonograph::types::{TypeDefinition, TypeKind};

        let mut schema = SchemaCache::new();
        let relationship_id = schema
            .register_type(TypeDefinition {
                id: TypeId(0),
                name: "Relationship".into(),
                kind: TypeKind::Edge,
                supertypes: vec![],
                property_declarations: vec![],
                open: true,
                metadata: PropertyMap::new(),
            })
            .unwrap();
        let friendship_id = schema
            .register_type(TypeDefinition {
                id: TypeId(0),
                name: "Friendship".into(),
                kind: TypeKind::Edge,
                supertypes: vec![relationship_id],
                property_declarations: vec![],
                open: true,
                metadata: PropertyMap::new(),
            })
            .unwrap();

        let mut snap = MockSnapshot::new();
        snap.nodes.insert(NodeId(1), make_node(1, 1));
        snap.nodes.insert(NodeId(2), make_node(2, 1));
        let edge = Edge {
            id: EdgeId(1),
            type_labels: vec![friendship_id],
            source: NodeId(1),
            target: NodeId(2),
            properties: PropertyMap::new(),
        };
        snap.edges.insert(EdgeId(1), edge);

        let buf = WriteBuffer::new();
        let view = OverlayGraphView::build(&snap, &buf, &schema, None);

        // With include_subtypes=true, querying Relationship should find the Friendship edge.
        let result = view.edges_by_type(relationship_id, true);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, EdgeId(1));

        // With include_subtypes=false, querying Relationship should NOT find the Friendship edge.
        let result = view.edges_by_type(relationship_id, false);
        assert!(result.is_empty());
    }

    #[test]
    fn scoped_preload_excludes_unrelated_types() {
        let type_a = TypeId(10);
        let type_b = TypeId(20);

        let mut snap = MockSnapshot::new();
        snap.nodes.insert(NodeId(1), make_node(1, type_a.0));
        snap.nodes.insert(NodeId(2), make_node(2, type_b.0));

        let buf = WriteBuffer::new();
        let schema = SchemaCache::new();
        let view = OverlayGraphView::build(&snap, &buf, &schema, Some(&[type_a]));

        assert!(view.get_node(NodeId(1)).is_some(), "type A node should be loaded");
        assert!(view.get_node(NodeId(2)).is_none(), "type B node should be excluded");
    }

    #[test]
    fn scoped_preload_adjacency_neighbors_included() {
        let type_x = TypeId(10);
        let type_y = TypeId(20);

        // Base snapshot: node 1 (type X) --edge--> node 2 (type Y).
        let mut snap = MockSnapshot::new();
        snap.nodes.insert(NodeId(1), make_node(1, type_x.0));
        snap.nodes.insert(NodeId(2), make_node(2, type_y.0));
        snap.edges.insert(EdgeId(1), make_edge(1, 1, 2, type_x.0));

        // Buffer has an update to node 1 (changed node).
        let mut buf = WriteBuffer::new();
        let before = make_node(1, type_x.0);
        let after = make_node(1, type_x.0); // same shape, simulates a property change
        buf.update_node(before, after);

        let schema = SchemaCache::new();
        // Scope to type_x only — node 2 is type_y, but is an adjacency neighbor.
        let view = OverlayGraphView::build(&snap, &buf, &schema, Some(&[type_x]));

        assert!(view.get_node(NodeId(1)).is_some(), "changed node should be loaded");
        assert!(view.get_node(NodeId(2)).is_some(), "adjacency neighbor should be loaded");
        assert!(view.get_edge(EdgeId(1)).is_some(), "connecting edge should be loaded");
    }
}
