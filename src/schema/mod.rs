//! Read-only schema view traits.
//!
//! This module defines traits for querying the graph, the type registry,
//! and the property key registry. All traits are object-safe and operate
//! under `no_std + alloc`.

#[cfg(feature = "alloc")]
use alloc::vec::Vec;

use crate::types::{
    Edge, EdgeId, Node, NodeId, PropertyDeclaration, PropertyKeyId, TypeDefinition, TypeId,
    TypeKind, Value,
};

/// A read-only view of the graph's nodes and edges.
///
/// Provides query methods for retrieving nodes and edges by id, type, or
/// property. All methods return borrowed references to data owned by the
/// underlying storage.
///
/// This trait is object-safe and can be used as `&dyn GraphView`.
pub trait GraphView {
    /// Returns the node with the given id, or `None` if not found.
    fn get_node(&self, id: NodeId) -> Option<&Node>;

    /// Returns the edge with the given id, or `None` if not found.
    fn get_edge(&self, id: EdgeId) -> Option<&Edge>;

    /// Returns all outgoing edges from the given node.
    ///
    /// If `edge_type` is `Some`, only edges with that type label are returned.
    fn outgoing_edges(&self, node: NodeId, edge_type: Option<TypeId>) -> Vec<&Edge>;

    /// Returns all incoming edges to the given node.
    ///
    /// If `edge_type` is `Some`, only edges with that type label are returned.
    fn incoming_edges(&self, node: NodeId, edge_type: Option<TypeId>) -> Vec<&Edge>;

    /// Returns all nodes with the given type label.
    ///
    /// If `include_subtypes` is `true`, nodes whose type labels include
    /// any subtype of `type_id` are also returned.
    fn nodes_by_type(&self, type_id: TypeId, include_subtypes: bool) -> Vec<&Node>;

    /// Returns all edges with the given type label.
    ///
    /// If `include_subtypes` is `true`, edges whose type labels include
    /// any subtype of `type_id` are also returned.
    fn edges_by_type(&self, type_id: TypeId, include_subtypes: bool) -> Vec<&Edge>;

    /// Returns all nodes that have the given property key set to the given value.
    fn nodes_by_property(&self, key: PropertyKeyId, value: &Value) -> Vec<&Node>;
}

/// A read-only view of the type registry.
///
/// Provides methods for querying type definitions and navigating the type
/// hierarchy. The type hierarchy forms a directed acyclic graph (DAG).
///
/// This trait is object-safe and can be used as `&dyn TypeRegistryView`.
pub trait TypeRegistryView {
    /// Returns the type definition with the given id, or `None` if not found.
    fn get_type(&self, id: TypeId) -> Option<&TypeDefinition>;

    /// Returns the type definition with the given name and kind, or `None`.
    fn get_type_by_name(&self, name: &str, kind: TypeKind) -> Option<&TypeDefinition>;

    /// Returns all registered type definitions as a contiguous slice.
    ///
    /// The implementation is expected to store types contiguously in memory.
    fn all_types(&self) -> &[TypeDefinition];

    /// Returns all type definitions of the given kind.
    fn types_by_kind(&self, kind: TypeKind) -> Vec<&TypeDefinition>;

    /// Returns the direct supertype ids of the given type, or `None` if
    /// the type is not found.
    fn direct_supertypes(&self, id: TypeId) -> Option<&[TypeId]>;

    /// Returns all supertypes of the given type, transitively, in
    /// topological order (most specific first).
    fn all_supertypes(&self, id: TypeId) -> Vec<TypeId>;

    /// Returns the direct subtypes of the given type.
    fn direct_subtypes(&self, id: TypeId) -> Vec<TypeId>;

    /// Returns all subtypes of the given type, transitively.
    fn all_subtypes(&self, id: TypeId) -> Vec<TypeId>;

    /// Returns `true` if `candidate` is a subtype of `ancestor`
    /// (directly or transitively).
    fn is_subtype_of(&self, candidate: TypeId, ancestor: TypeId) -> bool;

    /// Returns the effective property declarations for the given type,
    /// including declarations inherited from supertypes. Subtype declarations
    /// shadow (override) supertype declarations with the same key.
    fn effective_property_declarations(&self, id: TypeId) -> Vec<PropertyDeclaration>;
}

/// A read-only view of the property key registry.
///
/// Maps between human-readable property key names and their numeric
/// identifiers.
///
/// This trait is object-safe and can be used as `&dyn PropertyKeyRegistryView`.
pub trait PropertyKeyRegistryView {
    /// Returns the id of the property key with the given name, or `None`.
    fn get_key_id(&self, name: &str) -> Option<PropertyKeyId>;

    /// Returns the name of the property key with the given id, or `None`.
    fn get_key_name(&self, id: PropertyKeyId) -> Option<&str>;

    /// Returns all registered property keys as `(id, name)` pairs.
    fn all_keys(&self) -> Vec<(PropertyKeyId, &str)>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Object-safety compile-time assertions: these functions are never called,
    // but their existence proves the traits can be used as trait objects.
    fn _assert_graph_view_object_safe(_: &dyn GraphView) {}
    fn _assert_type_registry_view_object_safe(_: &dyn TypeRegistryView) {}
    fn _assert_property_key_registry_view_object_safe(_: &dyn PropertyKeyRegistryView) {}
}
