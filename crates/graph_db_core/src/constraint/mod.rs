//! Constraint validation types and traits.
//!
//! This module defines the change tracking types ([`NodeChange`], [`EdgeChange`],
//! [`ChangeSet`]) and the [`ConstraintValidator`] trait for pluggable validation.
//! All types are `no_std + alloc` compatible.

#[cfg(feature = "alloc")]
use alloc::{collections::BTreeSet, string::String, vec::Vec};

use crate::schema::{GraphView, PropertyKeyRegistryView, TypeRegistryView};
use crate::types::{Edge, EdgeId, Node, NodeId, TypeId};

// ---------------------------------------------------------------------------
// Change tracking
// ---------------------------------------------------------------------------

/// Describes a change to a single node within a transaction.
///
/// # Examples
///
/// ```
/// use graph_db_core::constraint::NodeChange;
/// use graph_db_core::types::{Node, NodeId};
/// use std::collections::BTreeMap;
///
/// let node = Node {
///     id: NodeId(1),
///     type_labels: vec![],
///     properties: BTreeMap::new(),
///     is_anonymous: false,
/// };
/// let change = NodeChange::Inserted(node);
/// assert!(matches!(change, NodeChange::Inserted(_)));
/// ```
#[derive(Clone, Debug)]
pub enum NodeChange {
    /// A new node was inserted.
    Inserted(Node),
    /// An existing node was modified. Contains the state before and after.
    Modified {
        /// The node state before the modification.
        before: Node,
        /// The node state after the modification.
        after: Node,
    },
    /// A node was deleted. Contains the state before deletion.
    Deleted(Node),
}

/// Describes a change to a single edge within a transaction.
///
/// # Examples
///
/// ```
/// use graph_db_core::constraint::EdgeChange;
/// use graph_db_core::types::{Edge, EdgeId, NodeId};
/// use std::collections::BTreeMap;
///
/// let edge = Edge {
///     id: EdgeId(1),
///     type_labels: vec![],
///     source: NodeId(10),
///     target: NodeId(20),
///     properties: BTreeMap::new(),
/// };
/// let change = EdgeChange::Inserted(edge);
/// assert!(matches!(change, EdgeChange::Inserted(_)));
/// ```
#[derive(Clone, Debug)]
pub enum EdgeChange {
    /// A new edge was inserted.
    Inserted(Edge),
    /// An existing edge was modified. Contains the state before and after.
    Modified {
        /// The edge state before the modification.
        before: Edge,
        /// The edge state after the modification.
        after: Edge,
    },
    /// An edge was deleted. Contains the state before deletion.
    Deleted(Edge),
}

/// A set of node and edge changes accumulated during a transaction.
///
/// Passed to [`ConstraintValidator::validate`] so validators can inspect
/// what changed. Fields are private; use the accessor and iterator methods
/// to inspect the changes.
pub struct ChangeSet<'a> {
    node_changes: &'a [NodeChange],
    edge_changes: &'a [EdgeChange],
}

impl<'a> ChangeSet<'a> {
    /// Creates a new `ChangeSet` referencing the given change slices.
    pub fn new(node_changes: &'a [NodeChange], edge_changes: &'a [EdgeChange]) -> Self {
        Self {
            node_changes,
            edge_changes,
        }
    }

    /// Returns all node changes.
    pub fn node_changes(&self) -> &[NodeChange] {
        self.node_changes
    }

    /// Returns all edge changes.
    pub fn edge_changes(&self) -> &[EdgeChange] {
        self.edge_changes
    }

    /// Returns an iterator over nodes that were inserted.
    pub fn inserted_nodes(&self) -> impl Iterator<Item = &Node> + '_ {
        self.node_changes.iter().filter_map(|c| match c {
            NodeChange::Inserted(n) => Some(n),
            _ => None,
        })
    }

    /// Returns an iterator over `(before, after)` pairs for modified nodes.
    pub fn modified_nodes(&self) -> impl Iterator<Item = (&Node, &Node)> + '_ {
        self.node_changes.iter().filter_map(|c| match c {
            NodeChange::Modified { before, after } => Some((before, after)),
            _ => None,
        })
    }

    /// Returns an iterator over nodes that were deleted.
    pub fn deleted_nodes(&self) -> impl Iterator<Item = &Node> + '_ {
        self.node_changes.iter().filter_map(|c| match c {
            NodeChange::Deleted(n) => Some(n),
            _ => None,
        })
    }

    /// Returns an iterator over edges that were inserted.
    pub fn inserted_edges(&self) -> impl Iterator<Item = &Edge> + '_ {
        self.edge_changes.iter().filter_map(|c| match c {
            EdgeChange::Inserted(e) => Some(e),
            _ => None,
        })
    }

    /// Returns an iterator over `(before, after)` pairs for modified edges.
    pub fn modified_edges(&self) -> impl Iterator<Item = (&Edge, &Edge)> + '_ {
        self.edge_changes.iter().filter_map(|c| match c {
            EdgeChange::Modified { before, after } => Some((before, after)),
            _ => None,
        })
    }

    /// Returns an iterator over edges that were deleted.
    pub fn deleted_edges(&self) -> impl Iterator<Item = &Edge> + '_ {
        self.edge_changes.iter().filter_map(|c| match c {
            EdgeChange::Deleted(e) => Some(e),
            _ => None,
        })
    }

    /// Returns a deduplicated list of all type IDs referenced by any changed
    /// node or edge (from their `type_labels` fields).
    pub fn affected_types(&self) -> Vec<TypeId> {
        let mut set = BTreeSet::new();
        for nc in self.node_changes {
            match nc {
                NodeChange::Inserted(n) | NodeChange::Deleted(n) => {
                    set.extend(&n.type_labels);
                }
                NodeChange::Modified { before, after } => {
                    set.extend(&before.type_labels);
                    set.extend(&after.type_labels);
                }
            }
        }
        for ec in self.edge_changes {
            match ec {
                EdgeChange::Inserted(e) | EdgeChange::Deleted(e) => {
                    set.extend(&e.type_labels);
                }
                EdgeChange::Modified { before, after } => {
                    set.extend(&before.type_labels);
                    set.extend(&after.type_labels);
                }
            }
        }
        set.into_iter().collect()
    }
}

// ---------------------------------------------------------------------------
// Violations
// ---------------------------------------------------------------------------

/// A constraint violation produced by a [`ConstraintValidator`].
///
/// # Examples
///
/// ```
/// use graph_db_core::constraint::{ConstraintViolation, ViolationSubject};
/// use graph_db_core::NodeId;
///
/// let v = ConstraintViolation {
///     violation_kind: "missing_name".into(),
///     message: "Node 1 has no name".into(),
///     subject: Some(ViolationSubject::Node(NodeId(1))),
/// };
/// assert_eq!(v.violation_kind, "missing_name");
/// ```
#[derive(Clone, Debug)]
pub struct ConstraintViolation {
    /// A machine-readable kind identifier (e.g., `"required_property_missing"`).
    pub violation_kind: String,
    /// A human-readable description of the violation.
    pub message: String,
    /// The entity that caused the violation, if applicable.
    pub subject: Option<ViolationSubject>,
}

/// Identifies the entity that caused a [`ConstraintViolation`].
///
/// # Examples
///
/// ```
/// use graph_db_core::constraint::ViolationSubject;
/// use graph_db_core::NodeId;
///
/// let subject = ViolationSubject::Node(NodeId(5));
/// assert!(matches!(subject, ViolationSubject::Node(_)));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViolationSubject {
    /// A node caused the violation.
    Node(NodeId),
    /// An edge caused the violation.
    Edge(EdgeId),
    /// A type definition caused the violation.
    Type(TypeId),
}

// ---------------------------------------------------------------------------
// ConstraintValidator trait
// ---------------------------------------------------------------------------

/// A pluggable constraint validator.
///
/// Implementations inspect a [`ChangeSet`] and the current graph state to
/// produce zero or more [`ConstraintViolation`]s. The database engine calls
/// all registered validators before committing a write transaction.
///
/// # Lifecycle
///
/// Validators are registered once on the `Database` and invoked on every
/// write transaction commit. They must be stateless with respect to the
/// validation call (no side effects).
///
/// # Thread Safety
///
/// Validators must be `Send + Sync` because they are stored as
/// `Box<dyn ConstraintValidator>` inside the multi-threaded `Database`.
pub trait ConstraintValidator: Send + Sync {
    /// Returns the name of this validator (for error reporting and logging).
    fn name(&self) -> &str;

    /// Returns the type IDs this validator is interested in, or `None`
    /// if it applies to all types.
    ///
    /// This is an optimization hint: the engine may skip calling `validate`
    /// if none of the changed entities have matching type labels.
    fn applies_to_types(&self) -> Option<Vec<TypeId>>;

    /// Validates the given changes against the current graph state.
    ///
    /// Returns an empty `Vec` if all constraints are satisfied, or one
    /// or more violations otherwise.
    ///
    /// # Parameters
    ///
    /// - `changes` — the set of node and edge changes in the current transaction
    /// - `graph` — a read-only view of the graph (pre-commit state)
    /// - `types` — a read-only view of the type registry
    /// - `keys` — a read-only view of the property key registry
    fn validate(
        &self,
        changes: &ChangeSet<'_>,
        graph: &dyn GraphView,
        types: &dyn TypeRegistryView,
        keys: &dyn PropertyKeyRegistryView,
    ) -> Vec<ConstraintViolation>;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EdgeId, NodeId, PropertyMap, TypeId};

    fn make_node(id: u64, types: &[u32]) -> Node {
        Node {
            id: NodeId(id),
            type_labels: types.iter().map(|&t| TypeId(t)).collect(),
            properties: PropertyMap::new(),
            is_anonymous: false,
        }
    }

    fn make_edge(id: u64, types: &[u32], src: u64, tgt: u64) -> Edge {
        Edge {
            id: EdgeId(id),
            type_labels: types.iter().map(|&t| TypeId(t)).collect(),
            source: NodeId(src),
            target: NodeId(tgt),
            properties: PropertyMap::new(),
        }
    }

    #[test]
    fn node_change_variants() {
        let n = make_node(1, &[1]);
        let _ = NodeChange::Inserted(n.clone());
        let _ = NodeChange::Modified {
            before: n.clone(),
            after: n.clone(),
        };
        let _ = NodeChange::Deleted(n);
    }

    #[test]
    fn edge_change_variants() {
        let e = make_edge(1, &[1], 1, 2);
        let _ = EdgeChange::Inserted(e.clone());
        let _ = EdgeChange::Modified {
            before: e.clone(),
            after: e.clone(),
        };
        let _ = EdgeChange::Deleted(e);
    }

    #[test]
    fn changeset_inserted_nodes() {
        let n1 = make_node(1, &[1]);
        let n2 = make_node(2, &[2]);
        let changes = [
            NodeChange::Inserted(n1.clone()),
            NodeChange::Deleted(n2),
        ];
        let cs = ChangeSet::new(&changes, &[]);
        let inserted: Vec<_> = cs.inserted_nodes().collect();
        assert_eq!(inserted.len(), 1);
        assert_eq!(inserted[0].id, NodeId(1));
    }

    #[test]
    fn changeset_modified_nodes() {
        let before = make_node(1, &[1]);
        let after = make_node(1, &[1, 2]);
        let changes = [NodeChange::Modified {
            before: before.clone(),
            after: after.clone(),
        }];
        let cs = ChangeSet::new(&changes, &[]);
        let modified: Vec<_> = cs.modified_nodes().collect();
        assert_eq!(modified.len(), 1);
        assert_eq!(modified[0].0.id, NodeId(1));
        assert_eq!(modified[0].1.type_labels.len(), 2);
    }

    #[test]
    fn changeset_deleted_nodes() {
        let n = make_node(5, &[3]);
        let changes = [NodeChange::Deleted(n)];
        let cs = ChangeSet::new(&changes, &[]);
        let deleted: Vec<_> = cs.deleted_nodes().collect();
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0].id, NodeId(5));
    }

    #[test]
    fn changeset_edge_iterators() {
        let e1 = make_edge(1, &[10], 1, 2);
        let e2 = make_edge(2, &[20], 3, 4);
        let edge_changes = [
            EdgeChange::Inserted(e1),
            EdgeChange::Deleted(e2),
        ];
        let cs = ChangeSet::new(&[], &edge_changes);
        assert_eq!(cs.inserted_edges().count(), 1);
        assert_eq!(cs.deleted_edges().count(), 1);
        assert_eq!(cs.modified_edges().count(), 0);
    }

    #[test]
    fn affected_types_deduplicated() {
        let n1 = make_node(1, &[1, 2]);
        let n2 = make_node(2, &[2, 3]);
        let e1 = make_edge(1, &[3, 4], 1, 2);
        let node_changes = [
            NodeChange::Inserted(n1),
            NodeChange::Inserted(n2),
        ];
        let edge_changes = [EdgeChange::Inserted(e1)];
        let cs = ChangeSet::new(&node_changes, &edge_changes);
        let types = cs.affected_types();
        assert_eq!(types, vec![TypeId(1), TypeId(2), TypeId(3), TypeId(4)]);
    }

    #[test]
    fn constraint_violation_construction() {
        let v = ConstraintViolation {
            violation_kind: "test".into(),
            message: "test message".into(),
            subject: Some(ViolationSubject::Node(NodeId(1))),
        };
        assert_eq!(v.violation_kind, "test");
        assert_eq!(v.subject, Some(ViolationSubject::Node(NodeId(1))));

        let v2 = ConstraintViolation {
            violation_kind: "test".into(),
            message: "no subject".into(),
            subject: None,
        };
        assert!(v2.subject.is_none());
    }

    #[test]
    fn violation_subject_eq() {
        assert_eq!(ViolationSubject::Node(NodeId(1)), ViolationSubject::Node(NodeId(1)));
        assert_ne!(ViolationSubject::Node(NodeId(1)), ViolationSubject::Edge(EdgeId(1)));
    }

    // Object-safety assertion
    fn _assert_constraint_validator_object_safe(_: &dyn ConstraintValidator) {}
}
