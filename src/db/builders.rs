//! Builder types for constructing nodes, edges, and type definitions.
//!
//! Builders produce entities with placeholder IDs (`0`). The database
//! assigns real IDs when the entity is inserted via a write transaction.

use crate::types::{
    Edge, EdgeId, Node, NodeId, PropertyDeclaration, PropertyKeyId, PropertyMap, TypeDefinition,
    TypeId, TypeKind, Value,
};

/// A builder for constructing [`Node`] values.
///
/// The built node has a placeholder `id` of `NodeId(0)`. The actual ID
/// is assigned by `WriteTransaction::insert_node`.
///
/// # Examples
///
/// ```
/// use graph_db::db::builders::NodeBuilder;
/// use graph_db::types::{TypeId, PropertyKeyId, Value};
///
/// let node = NodeBuilder::new()
///     .type_label(TypeId(1))
///     .property(PropertyKeyId(1), Value::String("Alice".into()))
///     .build();
/// ```
pub struct NodeBuilder {
    type_labels: Vec<TypeId>,
    properties: PropertyMap,
    is_anonymous: bool,
}

impl NodeBuilder {
    /// Creates a new, empty node builder.
    pub fn new() -> Self {
        Self {
            type_labels: Vec::new(),
            properties: PropertyMap::new(),
            is_anonymous: false,
        }
    }

    /// Adds a type label to the node.
    pub fn type_label(mut self, type_id: TypeId) -> Self {
        self.type_labels.push(type_id);
        self
    }

    /// Adds multiple type labels to the node.
    pub fn type_labels(mut self, ids: impl IntoIterator<Item = TypeId>) -> Self {
        self.type_labels.extend(ids);
        self
    }

    /// Sets a property on the node.
    pub fn property(mut self, key: PropertyKeyId, value: Value) -> Self {
        self.properties.insert(key, value);
        self
    }

    /// Marks this node as anonymous (a blank node / skolem).
    pub fn anonymous(mut self) -> Self {
        self.is_anonymous = true;
        self
    }

    /// Builds the [`Node`] with a placeholder ID of `NodeId(0)`.
    ///
    /// Type labels are sorted and deduplicated.
    pub fn build(mut self) -> Node {
        self.type_labels.sort();
        self.type_labels.dedup();
        Node {
            id: NodeId(0),
            type_labels: self.type_labels,
            properties: self.properties,
            is_anonymous: self.is_anonymous,
        }
    }
}

impl Default for NodeBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// A builder for constructing [`Edge`] values.
///
/// The built edge has a placeholder `id` of `EdgeId(0)`. The actual ID
/// is assigned by `WriteTransaction::insert_edge`.
///
/// # Examples
///
/// ```
/// use graph_db::db::builders::EdgeBuilder;
/// use graph_db::types::{NodeId, TypeId};
///
/// let edge = EdgeBuilder::new(NodeId(1), NodeId(2))
///     .type_label(TypeId(10))
///     .build();
/// ```
pub struct EdgeBuilder {
    source: NodeId,
    target: NodeId,
    type_labels: Vec<TypeId>,
    properties: PropertyMap,
}

impl EdgeBuilder {
    /// Creates a new edge builder with the given source and target nodes.
    pub fn new(source: NodeId, target: NodeId) -> Self {
        Self {
            source,
            target,
            type_labels: Vec::new(),
            properties: PropertyMap::new(),
        }
    }

    /// Adds a type label to the edge.
    pub fn type_label(mut self, type_id: TypeId) -> Self {
        self.type_labels.push(type_id);
        self
    }

    /// Adds multiple type labels to the edge.
    pub fn type_labels(mut self, ids: impl IntoIterator<Item = TypeId>) -> Self {
        self.type_labels.extend(ids);
        self
    }

    /// Sets a property on the edge.
    pub fn property(mut self, key: PropertyKeyId, value: Value) -> Self {
        self.properties.insert(key, value);
        self
    }

    /// Builds the [`Edge`] with a placeholder ID of `EdgeId(0)`.
    ///
    /// Type labels are sorted and deduplicated.
    pub fn build(mut self) -> Edge {
        self.type_labels.sort();
        self.type_labels.dedup();
        Edge {
            id: EdgeId(0),
            type_labels: self.type_labels,
            source: self.source,
            target: self.target,
            properties: self.properties,
        }
    }
}

/// A builder for constructing [`TypeDefinition`] values.
///
/// The built type definition has a placeholder `id` of `TypeId(0)`. The
/// actual ID is assigned by the schema cache during registration.
///
/// # Examples
///
/// ```
/// use graph_db::db::builders::TypeDefinitionBuilder;
///
/// let td = TypeDefinitionBuilder::node_type("Person")
///     .open()
///     .build();
/// ```
pub struct TypeDefinitionBuilder {
    name: String,
    kind: TypeKind,
    supertypes: Vec<TypeId>,
    property_declarations: Vec<PropertyDeclaration>,
    open: bool,
    metadata: PropertyMap,
}

impl TypeDefinitionBuilder {
    /// Creates a builder for a node type with the given name.
    pub fn node_type(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: TypeKind::Node,
            supertypes: Vec::new(),
            property_declarations: Vec::new(),
            open: false,
            metadata: PropertyMap::new(),
        }
    }

    /// Creates a builder for an edge type with the given name.
    pub fn edge_type(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            kind: TypeKind::Edge,
            supertypes: Vec::new(),
            property_declarations: Vec::new(),
            open: false,
            metadata: PropertyMap::new(),
        }
    }

    /// Adds a supertype to this type definition.
    pub fn supertype(mut self, id: TypeId) -> Self {
        self.supertypes.push(id);
        self
    }

    /// Adds a property declaration.
    pub fn property_declaration(mut self, decl: PropertyDeclaration) -> Self {
        self.property_declarations.push(decl);
        self
    }

    /// Marks this type as open (allows undeclared properties).
    pub fn open(mut self) -> Self {
        self.open = true;
        self
    }

    /// Marks this type as closed (default).
    pub fn closed(mut self) -> Self {
        self.open = false;
        self
    }

    /// Adds a metadata entry.
    pub fn metadata(mut self, key: PropertyKeyId, value: Value) -> Self {
        self.metadata.insert(key, value);
        self
    }

    /// Builds the [`TypeDefinition`] with a placeholder ID of `TypeId(0)`.
    pub fn build(self) -> TypeDefinition {
        TypeDefinition {
            id: TypeId(0),
            name: self.name,
            kind: self.kind,
            supertypes: self.supertypes,
            property_declarations: self.property_declarations,
            open: self.open,
            metadata: self.metadata,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_builder_basic() {
        let node = NodeBuilder::new()
            .type_label(TypeId(1))
            .property(PropertyKeyId(1), Value::String("test".into()))
            .build();
        assert_eq!(node.id, NodeId(0));
        assert_eq!(node.type_labels, vec![TypeId(1)]);
        assert_eq!(
            node.properties.get(&PropertyKeyId(1)),
            Some(&Value::String("test".into()))
        );
        assert!(!node.is_anonymous);
    }

    #[test]
    fn node_builder_anonymous() {
        let node = NodeBuilder::new().anonymous().build();
        assert!(node.is_anonymous);
    }

    #[test]
    fn node_builder_sorts_and_deduplicates_types() {
        let node = NodeBuilder::new()
            .type_label(TypeId(3))
            .type_label(TypeId(1))
            .type_label(TypeId(3))
            .type_label(TypeId(2))
            .build();
        assert_eq!(node.type_labels, vec![TypeId(1), TypeId(2), TypeId(3)]);
    }

    #[test]
    fn edge_builder_basic() {
        let edge = EdgeBuilder::new(NodeId(1), NodeId(2))
            .type_label(TypeId(10))
            .build();
        assert_eq!(edge.id, EdgeId(0));
        assert_eq!(edge.source, NodeId(1));
        assert_eq!(edge.target, NodeId(2));
        assert_eq!(edge.type_labels, vec![TypeId(10)]);
    }

    #[test]
    fn edge_builder_sorts_types() {
        let edge = EdgeBuilder::new(NodeId(1), NodeId(2))
            .type_label(TypeId(20))
            .type_label(TypeId(10))
            .build();
        assert_eq!(edge.type_labels, vec![TypeId(10), TypeId(20)]);
    }

    #[test]
    fn type_definition_builder_node() {
        let td = TypeDefinitionBuilder::node_type("Person")
            .supertype(TypeId(1))
            .open()
            .build();
        assert_eq!(td.id, TypeId(0));
        assert_eq!(td.name, "Person");
        assert_eq!(td.kind, TypeKind::Node);
        assert_eq!(td.supertypes, vec![TypeId(1)]);
        assert!(td.open);
    }

    #[test]
    fn type_definition_builder_edge() {
        let td = TypeDefinitionBuilder::edge_type("knows")
            .closed()
            .build();
        assert_eq!(td.kind, TypeKind::Edge);
        assert!(!td.open);
    }

    #[test]
    fn placeholder_ids_are_zero() {
        let node = NodeBuilder::new().build();
        assert_eq!(node.id.0, 0);

        let edge = EdgeBuilder::new(NodeId(1), NodeId(2)).build();
        assert_eq!(edge.id.0, 0);

        let td = TypeDefinitionBuilder::node_type("X").build();
        assert_eq!(td.id.0, 0);
    }
}
