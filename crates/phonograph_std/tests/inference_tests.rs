//! Integration tests for the inference hook infrastructure (Task 26).
//!
//! These tests exercise the full inference lifecycle: registration, ephemeral
//! dispatch, materialized dispatch, caching, provenance queries, rule chaining,
//! constraint interaction, and provenance persistence.

use phonograph_std::constraint::{
    ChangeSet, ConstraintValidator, ConstraintViolation, ViolationSubject,
};
use phonograph_std::db::builders::{NodeBuilder, TypeDefinitionBuilder};
use phonograph_std::error::Error;
use phonograph_std::InferenceError;
use phonograph_std::inference::{
    InferenceMode, InferenceResult, InferenceRule, InferredFact,
};
use phonograph_std::schema::{GraphView, PropertyKeyRegistryView, TypeRegistryView};
use phonograph_std::types::{EdgeId, NodeId, PropertyKeyId, TypeId, Value};
use phonograph_std::FileDatabase;

use std::sync::atomic::{AtomicU64, Ordering};

/// Helper: creates a temp-dir database.
fn open_temp_db() -> (FileDatabase, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    let db = phonograph_std::open(&path).unwrap();
    (db, dir)
}

// =========================================================================
// Test example rules
// =========================================================================

/// A minimal inference rule for testing the infrastructure.
///
/// For every node with property `source_key = true`, infers:
/// - A `NewEdge` from that node to every node with `target_key = true`
/// - A `NodePropertyUpdate` setting `inferred_key = true` on the source node
///
/// Also tracks invocation count for cache testing.
struct TestInferenceRule {
    edge_type_id: TypeId,
    source_key: PropertyKeyId,
    target_key: PropertyKeyId,
    inferred_key: PropertyKeyId,
    invocation_count: AtomicU64,
}

impl InferenceRule for TestInferenceRule {
    fn name(&self) -> &str {
        "test_inference_rule"
    }

    fn applies_to_types(&self) -> Option<Vec<TypeId>> {
        None
    }

    fn infer(
        &self,
        graph: &dyn GraphView,
        _types: &dyn TypeRegistryView,
        _keys: &dyn PropertyKeyRegistryView,
    ) -> InferenceResult {
        self.invocation_count.fetch_add(1, Ordering::Relaxed);

        let mut facts = Vec::new();

        // Find source nodes (property source_key = Bool(true))
        let sources = graph.nodes_by_property(self.source_key, &Value::Bool(true));
        // Find target nodes (property target_key = Bool(true))
        let targets = graph.nodes_by_property(self.target_key, &Value::Bool(true));

        for src in &sources {
            for tgt in &targets {
                if src.id != tgt.id {
                    facts.push(InferredFact::NewEdge {
                        type_labels: vec![self.edge_type_id],
                        source: src.id,
                        target: tgt.id,
                        properties: Default::default(),
                    });
                }
            }
            // Mark source as inferred
            facts.push(InferredFact::NodePropertyUpdate {
                node: src.id,
                key: self.inferred_key,
                value: Value::Bool(true),
            });
        }

        InferenceResult {
            facts,
            rule_name: self.name().to_string(),
        }
    }
}

/// A chaining rule that reads the "inferred" property set by TestInferenceRule
/// and creates a new summary node for each such source node.
struct ChainingTestRule {
    node_type_id: TypeId,
    inferred_key: PropertyKeyId,
    summary_key: PropertyKeyId,
}

impl InferenceRule for ChainingTestRule {
    fn name(&self) -> &str {
        "chaining_test_rule"
    }

    fn applies_to_types(&self) -> Option<Vec<TypeId>> {
        None
    }

    fn infer(
        &self,
        graph: &dyn GraphView,
        _types: &dyn TypeRegistryView,
        _keys: &dyn PropertyKeyRegistryView,
    ) -> InferenceResult {
        let mut facts = Vec::new();
        let inferred_nodes =
            graph.nodes_by_property(self.inferred_key, &Value::Bool(true));
        for node in &inferred_nodes {
            facts.push(InferredFact::NewNode {
                type_labels: vec![self.node_type_id],
                properties: {
                    let mut props = phonograph_std::types::PropertyMap::new();
                    props.insert(
                        self.summary_key,
                        Value::String(format!("summary_of_{}", node.id.0)),
                    );
                    props
                },
                is_anonymous: true,
            });
        }

        InferenceResult {
            facts,
            rule_name: self.name().to_string(),
        }
    }
}

/// A rule that produces an invalid edge (referencing non-existent source node).
struct InvalidFactRule;

impl InferenceRule for InvalidFactRule {
    fn name(&self) -> &str {
        "invalid_fact_rule"
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
            facts: vec![InferredFact::NewEdge {
                type_labels: vec![TypeId(9999)], // unregistered type
                source: NodeId(99999),           // non-existent
                target: NodeId(99998),
                properties: Default::default(),
            }],
            rule_name: "invalid_fact_rule".to_string(),
        }
    }
}

/// Helper to set up a database with types and seed data for inference tests.
/// Returns (db, dir, node_type, edge_type, source_key, target_key, inferred_key,
///          source_node_id, target_node_id).
#[allow(clippy::type_complexity)]
fn setup_inference_db() -> (
    FileDatabase,
    tempfile::TempDir,
    TypeId,
    TypeId,
    PropertyKeyId,
    PropertyKeyId,
    PropertyKeyId,
    NodeId,
    NodeId,
) {
    let (db, dir) = open_temp_db();

    let mut wtx = db.write_txn().unwrap();

    let node_type = wtx
        .register_type(TypeDefinitionBuilder::node_type("TestNode").build())
        .unwrap();
    let edge_type = wtx
        .register_type(TypeDefinitionBuilder::edge_type("inferred_link").build())
        .unwrap();

    let source_key = wtx.get_or_create_property_key("source").unwrap();
    let target_key = wtx.get_or_create_property_key("target").unwrap();
    let inferred_key = wtx.get_or_create_property_key("inferred").unwrap();

    let src_id = wtx
        .insert_node(
            NodeBuilder::new()
                .type_label(node_type)
                .property(source_key, Value::Bool(true))
                .build(),
        )
        .unwrap();
    let tgt_id = wtx
        .insert_node(
            NodeBuilder::new()
                .type_label(node_type)
                .property(target_key, Value::Bool(true))
                .build(),
        )
        .unwrap();

    wtx.commit().unwrap();

    (
        db,
        dir,
        node_type,
        edge_type,
        source_key,
        target_key,
        inferred_key,
        src_id,
        tgt_id,
    )
}

// =========================================================================
// Phase 5.3 — Ephemeral inference tests
// =========================================================================

#[test]
fn ephemeral_inference_returns_facts() {
    let (db, _dir, _nt, et, sk, tk, ik, _src, _tgt) = setup_inference_db();

    db.register_inference_rule(Box::new(TestInferenceRule {
        edge_type_id: et,
        source_key: sk,
        target_key: tk,
        inferred_key: ik,
        invocation_count: AtomicU64::new(0),
    }))
    .unwrap();

    let rtx = db.read_txn().unwrap();
    let result = rtx.run_inference("test_inference_rule").unwrap();

    // Should have at least 1 NewEdge + 1 NodePropertyUpdate
    assert!(!result.facts.is_empty());
    assert_eq!(result.rule_name, "test_inference_rule");

    // Verify no side effects: node count should be unchanged.
    assert_eq!(rtx.node_count().unwrap(), 2);
}

#[test]
fn ephemeral_inference_unknown_rule_returns_error() {
    let (db, _dir) = open_temp_db();
    let rtx = db.read_txn().unwrap();
    let err = rtx.run_inference("nonexistent").unwrap_err();
    assert!(matches!(err, Error::Inference(InferenceError::RuleNotFound(_))));
}

#[test]
fn run_all_inference_ephemeral() {
    let (db, _dir, nt, et, sk, tk, ik, _src, _tgt) = setup_inference_db();

    // Also register summary_key for chaining rule.
    let summary_key = {
        let mut wtx = db.write_txn().unwrap();
        let k = wtx.get_or_create_property_key("summary").unwrap();
        wtx.commit().unwrap();
        k
    };

    db.register_inference_rule(Box::new(TestInferenceRule {
        edge_type_id: et,
        source_key: sk,
        target_key: tk,
        inferred_key: ik,
        invocation_count: AtomicU64::new(0),
    }))
    .unwrap();
    db.register_inference_rule(Box::new(ChainingTestRule {
        node_type_id: nt,
        inferred_key: ik,
        summary_key,
    }))
    .unwrap();

    let rtx = db.read_txn().unwrap();
    let results = rtx.run_all_inference().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].rule_name, "test_inference_rule");
    assert_eq!(results[1].rule_name, "chaining_test_rule");
}

// =========================================================================
// Phase 6.4 — Materialized inference tests
// =========================================================================

#[test]
fn materialized_inference_writes_to_graph() {
    let (db, _dir, _nt, et, sk, tk, ik, src, tgt) = setup_inference_db();

    db.register_inference_rule(Box::new(TestInferenceRule {
        edge_type_id: et,
        source_key: sk,
        target_key: tk,
        inferred_key: ik,
        invocation_count: AtomicU64::new(0),
    }))
    .unwrap();

    {
        let mut wtx = db.write_txn().unwrap();
        let result = wtx
            .run_inference("test_inference_rule", InferenceMode::Materialized)
            .unwrap();
        assert!(!result.facts.is_empty());

        // Inferred edges should be visible in the write transaction.
        let edges = wtx.outgoing_edges(src, Some(et)).unwrap();
        assert!(!edges.is_empty());
        let edge = &edges[0];
        assert_eq!(edge.source, src);
        assert_eq!(edge.target, tgt);

        // Source node should have "inferred" property.
        let node = wtx.get_node(src).unwrap().unwrap();
        assert_eq!(node.properties.get(&ik), Some(&Value::Bool(true)));

        wtx.commit().unwrap();
    }

    // Verify persistence: read transaction sees materialized data.
    let rtx = db.read_txn().unwrap();
    let edges = rtx.outgoing_edges(src, Some(et)).unwrap();
    assert!(!edges.is_empty());
}

#[test]
fn materialized_mapping_has_assigned_ids() {
    let (db, _dir, _nt, et, sk, tk, ik, _src, _tgt) = setup_inference_db();

    db.register_inference_rule(Box::new(TestInferenceRule {
        edge_type_id: et,
        source_key: sk,
        target_key: tk,
        inferred_key: ik,
        invocation_count: AtomicU64::new(0),
    }))
    .unwrap();

    let mut wtx = db.write_txn().unwrap();
    wtx.run_inference("test_inference_rule", InferenceMode::Materialized)
        .unwrap();

    let mapping = wtx.last_materialization_mapping().unwrap();
    // Should have at least one new edge ID assigned.
    assert!(!mapping.new_edge_ids.is_empty());
    for &(_, eid) in &mapping.new_edge_ids {
        assert_ne!(eid, EdgeId(0));
    }
}

#[test]
fn re_inference_cleans_up_old_facts() {
    let (db, _dir, _nt, et, sk, tk, ik, src, tgt) = setup_inference_db();

    db.register_inference_rule(Box::new(TestInferenceRule {
        edge_type_id: et,
        source_key: sk,
        target_key: tk,
        inferred_key: ik,
        invocation_count: AtomicU64::new(0),
    }))
    .unwrap();

    let mut wtx = db.write_txn().unwrap();

    // First materialization.
    wtx.run_inference("test_inference_rule", InferenceMode::Materialized)
        .unwrap();
    let first_edges = wtx.outgoing_edges(src, Some(et)).unwrap();
    let first_edge_ids: Vec<_> = first_edges.iter().map(|e| e.id).collect();
    assert!(!first_edge_ids.is_empty());

    // Modify seed data (add another target).
    let extra_tgt = wtx
        .insert_node(
            NodeBuilder::new()
                .property(tk, Value::Bool(true))
                .build(),
        )
        .unwrap();

    // Re-run inference.
    wtx.run_inference("test_inference_rule", InferenceMode::Materialized)
        .unwrap();
    let second_edges = wtx.outgoing_edges(src, Some(et)).unwrap();

    // Old edges should be gone; new edges point to both targets.
    let second_edge_ids: Vec<_> = second_edges.iter().map(|e| e.id).collect();
    for old_id in &first_edge_ids {
        assert!(!second_edge_ids.contains(old_id), "old edge should be removed");
    }

    // Should now have edges to both tgt and extra_tgt.
    let targets: Vec<_> = second_edges.iter().map(|e| e.target).collect();
    assert!(targets.contains(&tgt));
    assert!(targets.contains(&extra_tgt));
}

#[test]
fn ephemeral_mode_in_write_transaction() {
    let (db, _dir, _nt, et, sk, tk, ik, src, _tgt) = setup_inference_db();

    db.register_inference_rule(Box::new(TestInferenceRule {
        edge_type_id: et,
        source_key: sk,
        target_key: tk,
        inferred_key: ik,
        invocation_count: AtomicU64::new(0),
    }))
    .unwrap();

    let mut wtx = db.write_txn().unwrap();
    let result = wtx
        .run_inference("test_inference_rule", InferenceMode::Ephemeral)
        .unwrap();
    assert!(!result.facts.is_empty());

    // No new edges should appear.
    let edges = wtx.outgoing_edges(src, Some(et)).unwrap();
    assert!(edges.is_empty());
}

#[test]
fn invalid_fact_returns_error() {
    let (db, _dir) = open_temp_db();

    db.register_inference_rule(Box::new(InvalidFactRule))
        .unwrap();

    let mut wtx = db.write_txn().unwrap();
    let err = wtx
        .run_inference("invalid_fact_rule", InferenceMode::Materialized)
        .unwrap_err();
    assert!(matches!(
        err,
        Error::Inference(InferenceError::InvalidFact { .. })
    ));
}

#[test]
fn dirty_transaction_bypasses_cache() {
    let (db, _dir, _nt, et, sk, tk, ik, _src, _tgt) = setup_inference_db();

    let invocation_count = std::sync::Arc::new(AtomicU64::new(0));
    let count_clone = invocation_count.clone();

    db.register_inference_rule(Box::new(TestInferenceRule {
        edge_type_id: et,
        source_key: sk,
        target_key: tk,
        inferred_key: ik,
        invocation_count: AtomicU64::new(0),
    }))
    .unwrap();

    // We need a rule that we can observe invocation count on.
    // Use a different approach: check that after mutation the result changes.
    let mut wtx = db.write_txn().unwrap();

    // First run (clean transaction) — should be cached.
    let result1 = wtx
        .run_inference("test_inference_rule", InferenceMode::Ephemeral)
        .unwrap();
    let edge_count1 = result1
        .facts
        .iter()
        .filter(|f| matches!(f, InferredFact::NewEdge { .. }))
        .count();

    // Insert another target node — makes transaction dirty.
    let _ = wtx
        .insert_node(
            NodeBuilder::new()
                .property(tk, Value::Bool(true))
                .build(),
        )
        .unwrap();

    // Second run should re-invoke the rule (seeing the new target).
    let result2 = wtx
        .run_inference("test_inference_rule", InferenceMode::Ephemeral)
        .unwrap();
    let edge_count2 = result2
        .facts
        .iter()
        .filter(|f| matches!(f, InferredFact::NewEdge { .. }))
        .count();

    // Should have more edges now (new target visible).
    assert!(edge_count2 > edge_count1, "dirty bypass should re-invoke rule");
    drop(count_clone);
}

// =========================================================================
// Phase 7.3 — Provenance query tests
// =========================================================================

#[test]
fn provenance_for_inferred_node_and_edge() {
    let (db, _dir, _nt, et, sk, tk, ik, src, _tgt) = setup_inference_db();

    db.register_inference_rule(Box::new(TestInferenceRule {
        edge_type_id: et,
        source_key: sk,
        target_key: tk,
        inferred_key: ik,
        invocation_count: AtomicU64::new(0),
    }))
    .unwrap();

    {
        let mut wtx = db.write_txn().unwrap();
        wtx.run_inference("test_inference_rule", InferenceMode::Materialized)
            .unwrap();

        // Get an inferred edge ID.
        let edges = wtx.outgoing_edges(src, Some(et)).unwrap();
        assert!(!edges.is_empty());
        let inferred_edge_id = edges[0].id;

        // Provenance queries within the write transaction.
        assert!(wtx.is_inferred_edge(inferred_edge_id).unwrap());
        assert!(!wtx.is_inferred_node(src).unwrap()); // src was user-created

        let prov = wtx.edge_provenance(inferred_edge_id).unwrap().unwrap();
        assert_eq!(prov.rule_name, "test_inference_rule");

        assert!(wtx.node_provenance(src).unwrap().is_none());

        wtx.commit().unwrap();
    }

    // Provenance in a read transaction after commit.
    let rtx = db.read_txn().unwrap();
    let edges = rtx.outgoing_edges(src, Some(et)).unwrap();
    let inferred_edge_id = edges[0].id;
    assert!(rtx.is_inferred_edge(inferred_edge_id).unwrap());
    let prov = rtx.edge_provenance(inferred_edge_id).unwrap().unwrap();
    assert_eq!(prov.rule_name, "test_inference_rule");
}

#[test]
fn provenance_persists_across_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");

    let inferred_edge_id;
    let et;
    let src;

    {
        let db = phonograph_std::open(&path).unwrap();
        let mut wtx = db.write_txn().unwrap();

        let node_type = wtx
            .register_type(TypeDefinitionBuilder::node_type("TN").build())
            .unwrap();
        et = wtx
            .register_type(TypeDefinitionBuilder::edge_type("IL").build())
            .unwrap();
        let sk = wtx.get_or_create_property_key("source").unwrap();
        let tk = wtx.get_or_create_property_key("target").unwrap();
        let ik = wtx.get_or_create_property_key("inferred").unwrap();

        src = wtx
            .insert_node(
                NodeBuilder::new()
                    .type_label(node_type)
                    .property(sk, Value::Bool(true))
                    .build(),
            )
            .unwrap();
        let _tgt = wtx
            .insert_node(
                NodeBuilder::new()
                    .type_label(node_type)
                    .property(tk, Value::Bool(true))
                    .build(),
            )
            .unwrap();
        wtx.commit().unwrap();

        db.register_inference_rule(Box::new(TestInferenceRule {
            edge_type_id: et,
            source_key: sk,
            target_key: tk,
            inferred_key: ik,
            invocation_count: AtomicU64::new(0),
        }))
        .unwrap();

        let mut wtx = db.write_txn().unwrap();
        wtx.run_inference("test_inference_rule", InferenceMode::Materialized)
            .unwrap();
        let edges = wtx.outgoing_edges(src, Some(et)).unwrap();
        inferred_edge_id = edges[0].id;
        wtx.commit().unwrap();
    }

    // Reopen without re-registering the rule.
    let db2 = phonograph_std::open(&path).unwrap();
    let rtx = db2.read_txn().unwrap();
    assert!(rtx.is_inferred_edge(inferred_edge_id).unwrap());
    let prov = rtx.edge_provenance(inferred_edge_id).unwrap().unwrap();
    assert_eq!(prov.rule_name, "test_inference_rule");
}

#[test]
fn provenance_after_re_inference() {
    let (db, _dir, _nt, et, sk, tk, ik, src, _tgt) = setup_inference_db();

    db.register_inference_rule(Box::new(TestInferenceRule {
        edge_type_id: et,
        source_key: sk,
        target_key: tk,
        inferred_key: ik,
        invocation_count: AtomicU64::new(0),
    }))
    .unwrap();

    // First materialization, commit.
    {
        let mut wtx = db.write_txn().unwrap();
        wtx.run_inference("test_inference_rule", InferenceMode::Materialized)
            .unwrap();
        wtx.commit().unwrap();
    }

    let old_edge_id = {
        let rtx = db.read_txn().unwrap();
        let edges = rtx.outgoing_edges(src, Some(et)).unwrap();
        edges[0].id
    };

    // Second materialization in a new transaction.
    {
        let mut wtx = db.write_txn().unwrap();
        wtx.run_inference("test_inference_rule", InferenceMode::Materialized)
            .unwrap();

        // Old edge should no longer have provenance; new one should.
        let new_edges = wtx.outgoing_edges(src, Some(et)).unwrap();
        assert!(!new_edges.is_empty());
        let new_edge_id = new_edges[0].id;
        assert_ne!(old_edge_id, new_edge_id);

        assert!(wtx.is_inferred_edge(new_edge_id).unwrap());

        wtx.commit().unwrap();
    }
}

// =========================================================================
// Phase 9 — No automatic inference triggers
// =========================================================================

#[test]
fn no_automatic_inference_on_insert() {
    let (db, _dir, _nt, et, sk, tk, ik, src, _tgt) = setup_inference_db();

    db.register_inference_rule(Box::new(TestInferenceRule {
        edge_type_id: et,
        source_key: sk,
        target_key: tk,
        inferred_key: ik,
        invocation_count: AtomicU64::new(0),
    }))
    .unwrap();

    // Insert more data but don't call run_inference.
    {
        let mut wtx = db.write_txn().unwrap();
        wtx.insert_node(
            NodeBuilder::new()
                .property(sk, Value::Bool(true))
                .build(),
        )
        .unwrap();
        wtx.commit().unwrap();
    }

    // No inferred edges should exist.
    let rtx = db.read_txn().unwrap();
    let edges = rtx.outgoing_edges(src, Some(et)).unwrap();
    assert!(edges.is_empty());
}

#[test]
fn no_automatic_inference_on_commit() {
    let (db, _dir, _nt, et, sk, tk, ik, src, _tgt) = setup_inference_db();

    db.register_inference_rule(Box::new(TestInferenceRule {
        edge_type_id: et,
        source_key: sk,
        target_key: tk,
        inferred_key: ik,
        invocation_count: AtomicU64::new(0),
    }))
    .unwrap();

    let rtx = db.read_txn().unwrap();
    let edges = rtx.outgoing_edges(src, Some(et)).unwrap();
    assert!(edges.is_empty(), "no inferred edges without explicit call");
}

#[test]
fn no_automatic_inference_on_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");

    {
        let db = phonograph_std::open(&path).unwrap();
        let mut wtx = db.write_txn().unwrap();
        let nt = wtx
            .register_type(TypeDefinitionBuilder::node_type("N").build())
            .unwrap();
        let et = wtx
            .register_type(TypeDefinitionBuilder::edge_type("E").build())
            .unwrap();
        let sk = wtx.get_or_create_property_key("source").unwrap();
        let tk = wtx.get_or_create_property_key("target").unwrap();
        let ik = wtx.get_or_create_property_key("inferred").unwrap();
        wtx.insert_node(
            NodeBuilder::new()
                .type_label(nt)
                .property(sk, Value::Bool(true))
                .build(),
        )
        .unwrap();
        wtx.insert_node(
            NodeBuilder::new()
                .type_label(nt)
                .property(tk, Value::Bool(true))
                .build(),
        )
        .unwrap();
        wtx.commit().unwrap();

        db.register_inference_rule(Box::new(TestInferenceRule {
            edge_type_id: et,
            source_key: sk,
            target_key: tk,
            inferred_key: ik,
            invocation_count: AtomicU64::new(0),
        }))
        .unwrap();
    }

    // Reopen — no inference should run automatically.
    let db2 = phonograph_std::open(&path).unwrap();
    let rtx = db2.read_txn().unwrap();
    assert_eq!(rtx.edge_count().unwrap(), 0);
}

// =========================================================================
// Phase 10 — Constraint validation interaction
// =========================================================================

/// A validator that requires every edge to have source and target with a specific type.
struct TypeCheckValidator {
    required_node_type: TypeId,
}

impl ConstraintValidator for TypeCheckValidator {
    fn name(&self) -> &str {
        "type_check"
    }

    fn applies_to_types(&self) -> Option<Vec<TypeId>> {
        None
    }

    fn validate(
        &self,
        changes: &ChangeSet<'_>,
        graph: &dyn GraphView,
        _types: &dyn TypeRegistryView,
        _keys: &dyn PropertyKeyRegistryView,
    ) -> Vec<ConstraintViolation> {
        let mut violations = Vec::new();
        for change in changes.edge_changes() {
            if let phonograph_std::constraint::EdgeChange::Inserted(edge) = change {
                // Check that source has the required type.
                if let Some(src) = graph.get_node(edge.source) && !src.type_labels.contains(&self.required_node_type) {
                    violations.push(ConstraintViolation {
                        violation_kind: "type_check".to_string(),
                        message: "source missing required type".to_string(),
                        subject: Some(ViolationSubject::Edge(edge.id)),
                    });
                }
            }
        }
        violations
    }
}

#[test]
fn materialized_facts_pass_constraint_validation() {
    let (db, _dir, nt, et, sk, tk, ik, _src, _tgt) = setup_inference_db();

    db.register_constraint(Box::new(TypeCheckValidator {
        required_node_type: nt,
    }))
    .unwrap();

    db.register_inference_rule(Box::new(TestInferenceRule {
        edge_type_id: et,
        source_key: sk,
        target_key: tk,
        inferred_key: ik,
        invocation_count: AtomicU64::new(0),
    }))
    .unwrap();

    let mut wtx = db.write_txn().unwrap();
    wtx.run_inference("test_inference_rule", InferenceMode::Materialized)
        .unwrap();
    // Source nodes have the TestNode type, so validation should pass.
    wtx.commit().unwrap();
}

// =========================================================================
// Phase 11 — Sequential rule execution order
// =========================================================================

#[test]
fn rule_chaining_in_run_all_inference() {
    let (db, _dir, nt, et, sk, tk, ik, _src, _tgt) = setup_inference_db();

    let summary_key = {
        let mut wtx = db.write_txn().unwrap();
        let k = wtx.get_or_create_property_key("summary").unwrap();
        wtx.commit().unwrap();
        k
    };

    // Register rules in order: test_inference_rule first, then chaining.
    db.register_inference_rule(Box::new(TestInferenceRule {
        edge_type_id: et,
        source_key: sk,
        target_key: tk,
        inferred_key: ik,
        invocation_count: AtomicU64::new(0),
    }))
    .unwrap();
    db.register_inference_rule(Box::new(ChainingTestRule {
        node_type_id: nt,
        inferred_key: ik,
        summary_key,
    }))
    .unwrap();

    let mut wtx = db.write_txn().unwrap();
    let results = wtx
        .run_all_inference(InferenceMode::Materialized)
        .unwrap();

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].rule_name, "test_inference_rule");
    assert_eq!(results[1].rule_name, "chaining_test_rule");

    // ChainingTestRule should have produced summary nodes — verifying it saw
    // the "inferred" property set by TestInferenceRule.
    assert!(!results[1].facts.is_empty(), "chaining rule should see first rule's output");

    wtx.commit().unwrap();
}

#[test]
fn run_all_inference_registration_order_not_alphabetical() {
    let (db, _dir, nt, et, sk, tk, ik, _src, _tgt) = setup_inference_db();

    let summary_key = {
        let mut wtx = db.write_txn().unwrap();
        let k = wtx.get_or_create_property_key("summary").unwrap();
        wtx.commit().unwrap();
        k
    };

    // Register "chaining_test_rule" (alphabetically first) AFTER "test_inference_rule".
    // Execution order should be registration order, not alphabetical.
    db.register_inference_rule(Box::new(TestInferenceRule {
        edge_type_id: et,
        source_key: sk,
        target_key: tk,
        inferred_key: ik,
        invocation_count: AtomicU64::new(0),
    }))
    .unwrap();
    db.register_inference_rule(Box::new(ChainingTestRule {
        node_type_id: nt,
        inferred_key: ik,
        summary_key,
    }))
    .unwrap();

    let names = db.inference_rule_names();
    assert_eq!(
        names,
        vec!["test_inference_rule", "chaining_test_rule"],
        "should be in registration order"
    );
}
