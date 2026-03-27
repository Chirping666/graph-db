//! The [`GraphReader`] trait — a public, fallible, owned-return read interface.
//!
//! `GraphReader` is implemented by both [`ReadTransaction`] and
//! [`WriteTransaction`], providing a unified read API for code that
//! is agnostic to the transaction type.

use alloc::vec::Vec;
use crate::error::Error;
use phonograph::schema::{PropertyKeyRegistryView, TypeRegistryView};
use phonograph::types::{Edge, EdgeId, Node, NodeId, PropertyKeyId, TypeId, Value};

use super::read_txn::ReadTransaction;
use super::write_txn::WriteTransaction;

/// A unified read interface for graph queries.
///
/// Unlike [`GraphView`](phonograph::schema::GraphView) (which returns borrowed
/// references and is infallible), `GraphReader` returns owned values and
/// is fallible, making it suitable for the public API.
///
/// Implemented by both [`ReadTransaction`] and [`WriteTransaction`].
pub trait GraphReader {
    /// Returns the node with the given id, or `None` if not found.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    fn get_node(&self, id: NodeId) -> Result<Option<Node>, Error>;

    /// Returns the edge with the given id, or `None` if not found.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    fn get_edge(&self, id: EdgeId) -> Result<Option<Edge>, Error>;

    /// Returns all outgoing edges from the given node.
    ///
    /// If `edge_type` is `Some`, only edges with that type label are returned.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    fn outgoing_edges(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<Vec<Edge>, Error>;

    /// Returns all incoming edges to the given node.
    ///
    /// If `edge_type` is `Some`, only edges with that type label are returned.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    fn incoming_edges(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<Vec<Edge>, Error>;

    /// Returns all neighbor nodes reachable via outgoing edges.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    fn neighbors(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<Vec<Node>, Error>;

    /// Returns all nodes with the given type label.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    fn nodes_by_type(
        &self,
        type_id: TypeId,
        include_subtypes: bool,
    ) -> Result<Vec<Node>, Error>;

    /// Returns all edges with the given type label.
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    fn edges_by_type(
        &self,
        type_id: TypeId,
        include_subtypes: bool,
    ) -> Result<Vec<Edge>, Error>;

    /// Returns all nodes with the given property value.
    ///
    /// Full scan in v1 (no property index).
    ///
    /// # Errors
    ///
    /// Returns an error on storage I/O failure.
    fn nodes_by_property(
        &self,
        key: PropertyKeyId,
        value: &Value,
    ) -> Result<Vec<Node>, Error>;

    /// Returns a reference to the type registry.
    fn type_registry(&self) -> &dyn TypeRegistryView;

    /// Returns a reference to the property key registry.
    fn property_key_registry(&self) -> &dyn PropertyKeyRegistryView;
}

impl<B: crate::backend::StorageBackend> GraphReader for ReadTransaction<'_, B> {
    fn get_node(&self, id: NodeId) -> Result<Option<Node>, Error> {
        self.get_node(id)
    }
    fn get_edge(&self, id: EdgeId) -> Result<Option<Edge>, Error> {
        self.get_edge(id)
    }
    fn outgoing_edges(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<Vec<Edge>, Error> {
        self.outgoing_edges(node, edge_type)
    }
    fn incoming_edges(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<Vec<Edge>, Error> {
        self.incoming_edges(node, edge_type)
    }
    fn neighbors(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<Vec<Node>, Error> {
        self.neighbors(node, edge_type)
    }
    fn nodes_by_type(
        &self,
        type_id: TypeId,
        include_subtypes: bool,
    ) -> Result<Vec<Node>, Error> {
        self.nodes_by_type(type_id, include_subtypes)
    }
    fn edges_by_type(
        &self,
        type_id: TypeId,
        include_subtypes: bool,
    ) -> Result<Vec<Edge>, Error> {
        self.edges_by_type(type_id, include_subtypes)
    }
    fn nodes_by_property(
        &self,
        key: PropertyKeyId,
        value: &Value,
    ) -> Result<Vec<Node>, Error> {
        self.nodes_by_property(key, value)
    }
    fn type_registry(&self) -> &dyn TypeRegistryView {
        self.type_registry()
    }
    fn property_key_registry(&self) -> &dyn PropertyKeyRegistryView {
        self.property_key_registry()
    }
}

impl<B: crate::backend::StorageBackend> GraphReader for WriteTransaction<'_, B> {
    fn get_node(&self, id: NodeId) -> Result<Option<Node>, Error> {
        self.get_node(id)
    }
    fn get_edge(&self, id: EdgeId) -> Result<Option<Edge>, Error> {
        self.get_edge(id)
    }
    fn outgoing_edges(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<Vec<Edge>, Error> {
        self.outgoing_edges(node, edge_type)
    }
    fn incoming_edges(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<Vec<Edge>, Error> {
        self.incoming_edges(node, edge_type)
    }
    fn neighbors(
        &self,
        node: NodeId,
        edge_type: Option<TypeId>,
    ) -> Result<Vec<Node>, Error> {
        self.neighbors(node, edge_type)
    }
    fn nodes_by_type(
        &self,
        type_id: TypeId,
        include_subtypes: bool,
    ) -> Result<Vec<Node>, Error> {
        self.nodes_by_type(type_id, include_subtypes)
    }
    fn edges_by_type(
        &self,
        type_id: TypeId,
        include_subtypes: bool,
    ) -> Result<Vec<Edge>, Error> {
        self.edges_by_type(type_id, include_subtypes)
    }
    fn nodes_by_property(
        &self,
        key: PropertyKeyId,
        value: &Value,
    ) -> Result<Vec<Node>, Error> {
        self.nodes_by_property(key, value)
    }
    fn type_registry(&self) -> &dyn TypeRegistryView {
        self.type_registry()
    }
    fn property_key_registry(&self) -> &dyn PropertyKeyRegistryView {
        self.property_key_registry()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Object-safety assertion.
    fn _assert_graph_reader_object_safe(_: &dyn GraphReader) {}

    // Send/Sync assertions on Database.
    fn _assert_database_send_sync<T: Send + Sync>() {}
    fn _verify_database_send_sync() {
        _assert_database_send_sync::<super::super::Database>();
    }

    // !Send, !Sync on transactions (compile-time: these should NOT compile
    // if uncommented, but we verify the PhantomData marker is present).
}
