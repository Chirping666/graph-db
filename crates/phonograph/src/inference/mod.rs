//! Inference rule types and traits.
//!
//! This module defines the types for representing inferred facts, inference
//! results, provenance tracking, and the [`InferenceRule`] trait for pluggable
//! inference. All types are `no_std + alloc` compatible.

use alloc::{string::String, vec::Vec};

use crate::schema::{GraphView, PropertyKeyRegistryView, TypeRegistryView};
use crate::types::{EdgeId, NodeId, PropertyKeyId, PropertyMap, TypeId, Value};

// ---------------------------------------------------------------------------
// Inferred facts
// ---------------------------------------------------------------------------

/// A single fact produced by an [`InferenceRule`].
///
/// Each variant describes a different kind of graph mutation that the
/// inference engine should apply (or return ephemerally).
///
/// Does **not** implement `Eq` because several variants contain
/// [`PropertyMap`] or [`Value`], which transitively contain `f64`.
///
/// # Examples
///
/// ```
/// use phonograph::inference::InferredFact;
/// use phonograph::{NodeId, TypeId, PropertyKeyId, Value};
/// use std::collections::BTreeMap;
///
/// let fact = InferredFact::NewEdge {
///     type_labels: vec![TypeId(1)],
///     source: NodeId(10),
///     target: NodeId(20),
///     properties: BTreeMap::new(),
/// };
/// assert!(matches!(fact, InferredFact::NewEdge { .. }));
/// ```
#[derive(Clone, Debug)]
pub enum InferredFact {
    /// Infer a new node to be inserted into the graph.
    NewNode {
        /// Type labels for the new node.
        type_labels: Vec<TypeId>,
        /// Properties for the new node.
        properties: PropertyMap,
        /// Whether the new node is anonymous (blank node / skolem).
        is_anonymous: bool,
    },
    /// Infer a new edge to be inserted into the graph.
    NewEdge {
        /// Type labels for the new edge.
        type_labels: Vec<TypeId>,
        /// Source node of the new edge.
        source: NodeId,
        /// Target node of the new edge.
        target: NodeId,
        /// Properties for the new edge.
        properties: PropertyMap,
    },
    /// Update (or add) a property on an existing node.
    NodePropertyUpdate {
        /// The node to update.
        node: NodeId,
        /// The property key to set.
        key: PropertyKeyId,
        /// The new value.
        value: Value,
    },
    /// Update (or add) a property on an existing edge.
    EdgePropertyUpdate {
        /// The edge to update.
        edge: EdgeId,
        /// The property key to set.
        key: PropertyKeyId,
        /// The new value.
        value: Value,
    },
    /// Assign an additional type label to an existing node.
    NodeTypeAssignment {
        /// The node to assign the type to.
        node: NodeId,
        /// The type to assign.
        type_id: TypeId,
    },
    /// Assign an additional type label to an existing edge.
    EdgeTypeAssignment {
        /// The edge to assign the type to.
        edge: EdgeId,
        /// The type to assign.
        type_id: TypeId,
    },
}

/// The result of running an [`InferenceRule`].
///
/// Contains a list of inferred facts and the name of the rule that
/// produced them.
///
/// # Examples
///
/// ```
/// use phonograph::inference::InferenceResult;
///
/// let result = InferenceResult {
///     facts: vec![],
///     rule_name: "my_rule".into(),
/// };
/// assert!(result.facts.is_empty());
/// ```
#[derive(Clone, Debug)]
pub struct InferenceResult {
    /// The inferred facts produced by the rule.
    pub facts: Vec<InferredFact>,
    /// The name of the rule that produced these facts.
    pub rule_name: String,
}

// ---------------------------------------------------------------------------
// Inference mode
// ---------------------------------------------------------------------------

/// Controls whether inference results are written to the graph or returned
/// without persisting.
///
/// # Examples
///
/// ```
/// use phonograph::InferenceMode;
///
/// let mode = InferenceMode::Ephemeral;
/// assert_ne!(mode, InferenceMode::Materialized);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InferenceMode {
    /// Return inferred facts without writing them to the graph.
    Ephemeral,
    /// Write inferred facts to the graph as materialized data.
    Materialized,
}

// ---------------------------------------------------------------------------
// Provenance
// ---------------------------------------------------------------------------

/// Records which rule produced a materialized inference result and when.
///
/// # Examples
///
/// ```
/// use phonograph::ProvenanceRecord;
///
/// let rec = ProvenanceRecord {
///     rule_name: "inverse_edge".into(),
///     materialized_at: 5,
/// };
/// assert_eq!(rec.rule_name, "inverse_edge");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProvenanceRecord {
    /// The name of the inference rule that produced this result.
    pub rule_name: String,
    /// The transaction ID at which this result was materialized.
    pub materialized_at: u64,
}

/// Identifies a specific entity or sub-entity produced by inference.
///
/// Used as a key in the provenance registry to track which inferred
/// entities came from which rules.
///
/// Derives `Eq`, `Ord`, and `Hash` because it does not contain [`Value`]
/// and is used as a `BTreeMap` key.
///
/// # Examples
///
/// ```
/// use phonograph::inference::InferredEntity;
/// use phonograph::NodeId;
///
/// let entity = InferredEntity::Node(NodeId(42));
/// assert!(matches!(entity, InferredEntity::Node(_)));
/// ```
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InferredEntity {
    /// An inferred node.
    Node(NodeId),
    /// An inferred edge.
    Edge(EdgeId),
    /// An inferred property on a node.
    NodeProperty {
        /// The node carrying the inferred property.
        node: NodeId,
        /// The property key.
        key: PropertyKeyId,
    },
    /// An inferred property on an edge.
    EdgeProperty {
        /// The edge carrying the inferred property.
        edge: EdgeId,
        /// The property key.
        key: PropertyKeyId,
    },
    /// An inferred type assignment on a node.
    NodeType {
        /// The node receiving the type assignment.
        node: NodeId,
        /// The assigned type.
        type_id: TypeId,
    },
    /// An inferred type assignment on an edge.
    EdgeType {
        /// The edge receiving the type assignment.
        edge: EdgeId,
        /// The assigned type.
        type_id: TypeId,
    },
}

/// Maps indices in an [`InferenceResult::facts`] vector to the actual IDs
/// assigned when those facts were materialized.
///
/// # Examples
///
/// ```
/// use phonograph::MaterializedMapping;
///
/// let mapping = MaterializedMapping {
///     new_node_ids: vec![],
///     new_edge_ids: vec![],
/// };
/// assert!(mapping.new_node_ids.is_empty());
/// ```
#[derive(Clone, Debug)]
pub struct MaterializedMapping {
    /// Pairs of `(fact_index, assigned_node_id)` for `NewNode` facts.
    pub new_node_ids: Vec<(usize, NodeId)>,
    /// Pairs of `(fact_index, assigned_edge_id)` for `NewEdge` facts.
    pub new_edge_ids: Vec<(usize, EdgeId)>,
}

// ---------------------------------------------------------------------------
// InferenceRule trait
// ---------------------------------------------------------------------------

/// A pluggable inference rule.
///
/// Implementations inspect the current graph state and produce zero or more
/// [`InferredFact`]s. The database engine calls registered rules according
/// to the configured [`InferenceMode`].
///
/// # Lifecycle
///
/// Rules are registered once on the `Database` and may be invoked on every
/// write transaction commit (materialized mode) or on-demand (ephemeral mode).
/// They must be stateless with respect to the inference call (no side effects).
///
/// # Thread Safety
///
/// Rules must be `Send + Sync` because they are stored as
/// `Box<dyn InferenceRule>` inside the multi-threaded `Database`.
pub trait InferenceRule: Send + Sync {
    /// Returns the name of this rule (for provenance tracking and logging).
    fn name(&self) -> &str;

    /// Returns the type IDs this rule is interested in, or `None`
    /// if it applies to all types.
    ///
    /// This is an optimization hint: the engine may skip calling `infer`
    /// if none of the changed entities have matching type labels.
    fn applies_to_types(&self) -> Option<Vec<TypeId>>;

    /// Runs inference over the current graph state and returns any inferred facts.
    ///
    /// # Parameters
    ///
    /// - `graph` — a read-only view of the graph
    /// - `types` — a read-only view of the type registry
    /// - `keys` — a read-only view of the property key registry
    fn infer(
        &self,
        graph: &dyn GraphView,
        types: &dyn TypeRegistryView,
        keys: &dyn PropertyKeyRegistryView,
    ) -> InferenceResult;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::collections::BTreeMap;

    #[test]
    fn inferred_fact_new_node() {
        let fact = InferredFact::NewNode {
            type_labels: vec![TypeId(1)],
            properties: PropertyMap::new(),
            is_anonymous: false,
        };
        assert!(matches!(fact, InferredFact::NewNode { .. }));
    }

    #[test]
    fn inferred_fact_new_edge() {
        let fact = InferredFact::NewEdge {
            type_labels: vec![TypeId(2)],
            source: NodeId(1),
            target: NodeId(2),
            properties: PropertyMap::new(),
        };
        assert!(matches!(fact, InferredFact::NewEdge { .. }));
    }

    #[test]
    fn inferred_fact_property_updates() {
        let _ = InferredFact::NodePropertyUpdate {
            node: NodeId(1),
            key: PropertyKeyId(1),
            value: Value::I64(42),
        };
        let _ = InferredFact::EdgePropertyUpdate {
            edge: EdgeId(1),
            key: PropertyKeyId(2),
            value: Value::String("test".into()),
        };
    }

    #[test]
    fn inferred_fact_type_assignments() {
        let _ = InferredFact::NodeTypeAssignment {
            node: NodeId(1),
            type_id: TypeId(5),
        };
        let _ = InferredFact::EdgeTypeAssignment {
            edge: EdgeId(1),
            type_id: TypeId(6),
        };
    }

    #[test]
    fn inference_result_construction() {
        let result = InferenceResult {
            facts: vec![InferredFact::NodeTypeAssignment {
                node: NodeId(1),
                type_id: TypeId(5),
            }],
            rule_name: "test_rule".into(),
        };
        assert_eq!(result.facts.len(), 1);
        assert_eq!(result.rule_name, "test_rule");
    }

    #[test]
    fn inference_mode_eq() {
        assert_eq!(InferenceMode::Ephemeral, InferenceMode::Ephemeral);
        assert_eq!(InferenceMode::Materialized, InferenceMode::Materialized);
        assert_ne!(InferenceMode::Ephemeral, InferenceMode::Materialized);
    }

    #[test]
    fn provenance_record_eq() {
        let a = ProvenanceRecord {
            rule_name: "rule_a".into(),
            materialized_at: 100,
        };
        let b = ProvenanceRecord {
            rule_name: "rule_a".into(),
            materialized_at: 100,
        };
        assert_eq!(a, b);
    }

    #[test]
    fn inferred_entity_ordering() {
        // Derived Ord: variant order is Node < Edge < NodeProperty < ...
        assert!(InferredEntity::Node(NodeId(1)) < InferredEntity::Node(NodeId(2)));
        assert!(InferredEntity::Node(NodeId(1)) < InferredEntity::Edge(EdgeId(1)));
    }

    #[test]
    fn inferred_entity_as_btreemap_key() {
        let mut map = BTreeMap::new();
        map.insert(
            InferredEntity::Node(NodeId(1)),
            ProvenanceRecord {
                rule_name: "r1".into(),
                materialized_at: 1,
            },
        );
        map.insert(
            InferredEntity::EdgeProperty {
                edge: EdgeId(2),
                key: PropertyKeyId(3),
            },
            ProvenanceRecord {
                rule_name: "r2".into(),
                materialized_at: 2,
            },
        );
        assert_eq!(map.len(), 2);
        assert!(map.contains_key(&InferredEntity::Node(NodeId(1))));
    }

    #[test]
    fn materialized_mapping_construction() {
        let mapping = MaterializedMapping {
            new_node_ids: vec![(0, NodeId(100)), (2, NodeId(101))],
            new_edge_ids: vec![(1, EdgeId(200))],
        };
        assert_eq!(mapping.new_node_ids.len(), 2);
        assert_eq!(mapping.new_edge_ids.len(), 1);
    }

    // Object-safety assertion
    fn _assert_inference_rule_object_safe(_: &dyn InferenceRule) {}

    // Send + Sync assertions
    fn _assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn inference_rule_is_send_sync() {
        _assert_send_sync::<Box<dyn InferenceRule>>();
    }
}
