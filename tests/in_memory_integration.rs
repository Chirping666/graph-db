//! Integration tests for the in-memory backend (Task 27).
//!
//! These tests verify that the complete database functionality works
//! identically through `MemoryBackend`: schema operations, CRUD, queries,
//! traversals, constraint validation, inference, concurrency, and snapshot
//! round-trips.

use std::collections::HashSet;
use std::sync::Arc;

use graph_db::constraint::{
    ChangeSet, ConstraintValidator, ConstraintViolation, ViolationSubject,
};
use graph_db::db::builders::{EdgeBuilder, NodeBuilder, TypeDefinitionBuilder};
use graph_db::db::config::DatabaseConfig;
use graph_db::db::database::Database;
use graph_db::error::Error;
use graph_db::backend_mem::MemoryBackend;
use graph_db::inference::{InferenceMode, InferenceResult, InferenceRule, InferredFact};
use graph_db::schema::{GraphView, PropertyKeyRegistryView, TypeRegistryView};
use graph_db::types::{NodeId, PropertyKeyId, TypeId, Value};

/// Helper: opens an in-memory database.
fn open_mem_db() -> Database {
    Database::open(DatabaseConfig::in_memory()).unwrap()
}

// =========================================================================
// 7.1 — Schema operations
// =========================================================================

#[test]
fn in_memory_schema_operations() {
    let db = open_mem_db();

    let mut wtx = db.write_txn().unwrap();

    // Register type hierarchy: Entity -> Person, Entity -> Organization
    let entity_type = wtx
        .register_type(TypeDefinitionBuilder::node_type("Entity").build())
        .unwrap();
    let person_type = wtx
        .register_type(
            TypeDefinitionBuilder::node_type("Person")
                .supertype(entity_type)
                .build(),
        )
        .unwrap();
    let org_type = wtx
        .register_type(
            TypeDefinitionBuilder::node_type("Organization")
                .supertype(entity_type)
                .build(),
        )
        .unwrap();

    // Register edge types
    let knows_type = wtx
        .register_type(TypeDefinitionBuilder::edge_type("knows").build())
        .unwrap();
    let works_at_type = wtx
        .register_type(TypeDefinitionBuilder::edge_type("works_at").build())
        .unwrap();

    // Register property keys
    let name_key = wtx.get_or_create_property_key("name").unwrap();
    let age_key = wtx.get_or_create_property_key("age").unwrap();

    wtx.commit().unwrap();

    // Verify via read transaction
    let rtx = db.read_txn().unwrap();
    let reg = rtx.type_registry();

    let person_td = reg.get_type(person_type).unwrap();
    assert_eq!(person_td.name, "Person");
    assert!(person_td.supertypes.contains(&entity_type));

    let org_td = reg.get_type(org_type).unwrap();
    assert_eq!(org_td.name, "Organization");
    assert!(org_td.supertypes.contains(&entity_type));

    let knows_td = reg.get_type(knows_type).unwrap();
    assert_eq!(knows_td.name, "knows");

    let works_at_td = reg.get_type(works_at_type).unwrap();
    assert_eq!(works_at_td.name, "works_at");

    // Subtype relationships
    assert!(reg.is_subtype_of(person_type, entity_type));
    assert!(reg.is_subtype_of(org_type, entity_type));
    assert!(!reg.is_subtype_of(entity_type, person_type));

    // Property keys
    let key_reg = rtx.property_key_registry();
    assert_eq!(key_reg.get_key_name(name_key), Some("name"));
    assert_eq!(key_reg.get_key_name(age_key), Some("age"));
}

// =========================================================================
// 7.2 — Node and edge CRUD
// =========================================================================

#[test]
fn in_memory_crud() {
    let db = open_mem_db();

    let mut wtx = db.write_txn().unwrap();
    let person_t = wtx
        .register_type(TypeDefinitionBuilder::node_type("Person").build())
        .unwrap();
    let edge_t = wtx
        .register_type(TypeDefinitionBuilder::edge_type("knows").build())
        .unwrap();
    let name_key = wtx.get_or_create_property_key("name").unwrap();

    // Insert nodes
    let alice = wtx
        .insert_node(
            NodeBuilder::new()
                .type_label(person_t)
                .property(name_key, Value::String("Alice".into()))
                .build(),
        )
        .unwrap();
    let bob = wtx
        .insert_node(
            NodeBuilder::new()
                .type_label(person_t)
                .property(name_key, Value::String("Bob".into()))
                .build(),
        )
        .unwrap();
    let carol = wtx
        .insert_node(
            NodeBuilder::new()
                .type_label(person_t)
                .property(name_key, Value::String("Carol".into()))
                .build(),
        )
        .unwrap();

    // Insert edges
    let e1 = wtx
        .insert_edge(EdgeBuilder::new(alice, bob).type_label(edge_t).build())
        .unwrap();
    let e2 = wtx
        .insert_edge(EdgeBuilder::new(bob, carol).type_label(edge_t).build())
        .unwrap();

    wtx.commit().unwrap();

    // Read back
    {
        let rtx = db.read_txn().unwrap();
        assert_eq!(rtx.node_count().unwrap(), 3);
        assert_eq!(rtx.edge_count().unwrap(), 2);

        let alice_node = rtx.get_node(alice).unwrap().unwrap();
        assert_eq!(
            alice_node.properties.get(&name_key),
            Some(&Value::String("Alice".into()))
        );
    }

    // Update
    {
        let mut wtx = db.write_txn().unwrap();
        wtx.set_node_property(alice, name_key, Value::String("Alice Updated".into()))
            .unwrap();
        wtx.commit().unwrap();
    }

    {
        let rtx = db.read_txn().unwrap();
        let alice_node = rtx.get_node(alice).unwrap().unwrap();
        assert_eq!(
            alice_node.properties.get(&name_key),
            Some(&Value::String("Alice Updated".into()))
        );
    }

    // Delete edge, then delete node with cascading
    {
        let mut wtx = db.write_txn().unwrap();
        wtx.delete_edge(e1).unwrap();
        wtx.delete_node(bob).unwrap(); // cascades bob->carol edge
        wtx.commit().unwrap();
    }

    {
        let rtx = db.read_txn().unwrap();
        assert_eq!(rtx.node_count().unwrap(), 2); // alice and carol
        assert_eq!(rtx.edge_count().unwrap(), 0);
        assert!(rtx.get_node(bob).unwrap().is_none());
        assert!(rtx.get_edge(e1).unwrap().is_none());
        assert!(rtx.get_edge(e2).unwrap().is_none());
    }
}

// =========================================================================
// 7.3 — Query and traversal
// =========================================================================

#[test]
fn in_memory_traversal() {
    let db = open_mem_db();

    let mut wtx = db.write_txn().unwrap();
    let person_t = wtx
        .register_type(TypeDefinitionBuilder::node_type("Person").build())
        .unwrap();
    let team_t = wtx
        .register_type(TypeDefinitionBuilder::node_type("Team").build())
        .unwrap();
    let project_t = wtx
        .register_type(TypeDefinitionBuilder::node_type("Project").build())
        .unwrap();
    let skill_t = wtx
        .register_type(TypeDefinitionBuilder::node_type("Skill").build())
        .unwrap();

    let member_of_t = wtx
        .register_type(TypeDefinitionBuilder::edge_type("member_of").build())
        .unwrap();
    let works_on_t = wtx
        .register_type(TypeDefinitionBuilder::edge_type("works_on").build())
        .unwrap();
    let requires_t = wtx
        .register_type(TypeDefinitionBuilder::edge_type("requires").build())
        .unwrap();
    let has_skill_t = wtx
        .register_type(TypeDefinitionBuilder::edge_type("has_skill").build())
        .unwrap();

    let name_key = wtx.get_or_create_property_key("name").unwrap();

    // 9 nodes
    let alice = wtx
        .insert_node(
            NodeBuilder::new()
                .type_label(person_t)
                .property(name_key, Value::String("Alice".into()))
                .build(),
        )
        .unwrap();
    let bob = wtx
        .insert_node(
            NodeBuilder::new()
                .type_label(person_t)
                .property(name_key, Value::String("Bob".into()))
                .build(),
        )
        .unwrap();
    let carol = wtx
        .insert_node(
            NodeBuilder::new()
                .type_label(person_t)
                .property(name_key, Value::String("Carol".into()))
                .build(),
        )
        .unwrap();
    let engineering = wtx
        .insert_node(NodeBuilder::new().type_label(team_t).build())
        .unwrap();
    let design = wtx
        .insert_node(NodeBuilder::new().type_label(team_t).build())
        .unwrap();
    let proj_x = wtx
        .insert_node(NodeBuilder::new().type_label(project_t).build())
        .unwrap();
    let proj_y = wtx
        .insert_node(NodeBuilder::new().type_label(project_t).build())
        .unwrap();
    let rust_skill = wtx
        .insert_node(NodeBuilder::new().type_label(skill_t).build())
        .unwrap();
    let py_skill = wtx
        .insert_node(NodeBuilder::new().type_label(skill_t).build())
        .unwrap();

    // 11 edges
    wtx.insert_edge(EdgeBuilder::new(alice, engineering).type_label(member_of_t).build()).unwrap();
    wtx.insert_edge(EdgeBuilder::new(bob, engineering).type_label(member_of_t).build()).unwrap();
    wtx.insert_edge(EdgeBuilder::new(carol, design).type_label(member_of_t).build()).unwrap();
    wtx.insert_edge(EdgeBuilder::new(alice, proj_x).type_label(works_on_t).build()).unwrap();
    wtx.insert_edge(EdgeBuilder::new(bob, proj_y).type_label(works_on_t).build()).unwrap();
    wtx.insert_edge(EdgeBuilder::new(carol, proj_x).type_label(works_on_t).build()).unwrap();
    wtx.insert_edge(EdgeBuilder::new(alice, rust_skill).type_label(has_skill_t).build()).unwrap();
    wtx.insert_edge(EdgeBuilder::new(bob, py_skill).type_label(has_skill_t).build()).unwrap();
    wtx.insert_edge(EdgeBuilder::new(proj_x, rust_skill).type_label(requires_t).build()).unwrap();
    wtx.insert_edge(EdgeBuilder::new(proj_x, py_skill).type_label(requires_t).build()).unwrap();
    wtx.insert_edge(EdgeBuilder::new(proj_y, py_skill).type_label(requires_t).build()).unwrap();

    wtx.commit().unwrap();

    let rtx = db.read_txn().unwrap();

    // nodes_by_type
    let people = rtx.nodes_by_type(person_t, false).unwrap();
    assert_eq!(people.len(), 3);

    // edges_by_type
    let member_edges = rtx.edges_by_type(member_of_t, false).unwrap();
    assert_eq!(member_edges.len(), 3);

    // outgoing / incoming edges
    let alice_out = rtx.outgoing_edges(alice, Some(works_on_t)).unwrap();
    assert_eq!(alice_out.len(), 1);
    assert_eq!(alice_out[0].target, proj_x);

    let proj_x_incoming = rtx.incoming_edges(proj_x, Some(works_on_t)).unwrap();
    assert_eq!(proj_x_incoming.len(), 2); // alice and carol

    // Multi-hop: 4 hops from py_skill → projects → people → teams
    let py_projects = rtx.incoming_edges(py_skill, Some(requires_t)).unwrap();
    let mut teams: HashSet<NodeId> = HashSet::new();
    for pe in &py_projects {
        let workers = rtx.incoming_edges(pe.source, Some(works_on_t)).unwrap();
        for we in &workers {
            let t = rtx.outgoing_edges(we.source, Some(member_of_t)).unwrap();
            for te in &t {
                teams.insert(te.target);
            }
        }
    }
    assert!(teams.contains(&engineering));
    assert!(teams.contains(&design));
    assert_eq!(teams.len(), 2);
}

// =========================================================================
// 7.4 — Constraint validation
// =========================================================================

struct RequireNameConstraint {
    name_key: PropertyKeyId,
}

impl ConstraintValidator for RequireNameConstraint {
    fn name(&self) -> &str {
        "RequireNameConstraint"
    }
    fn applies_to_types(&self) -> Option<Vec<TypeId>> {
        None
    }
    fn validate(
        &self,
        changes: &ChangeSet<'_>,
        _graph: &dyn GraphView,
        _types: &dyn TypeRegistryView,
        _keys: &dyn PropertyKeyRegistryView,
    ) -> Vec<ConstraintViolation> {
        let mut violations = Vec::new();
        for node in changes.inserted_nodes() {
            if !node.properties.contains_key(&self.name_key) {
                violations.push(ConstraintViolation {
                    violation_kind: "MissingName".into(),
                    message: format!("Node {:?} missing 'name'", node.id),
                    subject: Some(ViolationSubject::Node(node.id)),
                });
            }
        }
        violations
    }
}

#[test]
fn in_memory_constraint_validation() {
    let db = open_mem_db();

    // Setup types + key
    let (node_type, name_key) = {
        let mut wtx = db.write_txn().unwrap();
        let t = wtx
            .register_type(TypeDefinitionBuilder::node_type("Thing").build())
            .unwrap();
        let k = wtx.get_or_create_property_key("name").unwrap();
        wtx.commit().unwrap();
        (t, k)
    };

    db.register_constraint(Box::new(RequireNameConstraint { name_key }))
        .unwrap();

    // Violate constraint
    {
        let mut wtx = db.write_txn().unwrap();
        wtx.insert_node(NodeBuilder::new().type_label(node_type).build())
            .unwrap();
        let result = wtx.commit();
        assert!(matches!(result, Err(Error::ConstraintViolation(_))));
    }

    // Satisfy constraint
    {
        let mut wtx = db.write_txn().unwrap();
        wtx.insert_node(
            NodeBuilder::new()
                .type_label(node_type)
                .property(name_key, Value::String("Valid".into()))
                .build(),
        )
        .unwrap();
        wtx.commit().unwrap();
    }
}

// =========================================================================
// 7.5 — Inference
// =========================================================================

struct SimpleInferenceRule {
    source_key: PropertyKeyId,
    inferred_key: PropertyKeyId,
}

impl InferenceRule for SimpleInferenceRule {
    fn name(&self) -> &str {
        "simple_infer"
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
        let sources = graph.nodes_by_property(self.source_key, &Value::Bool(true));
        for src in &sources {
            facts.push(InferredFact::NodePropertyUpdate {
                node: src.id,
                key: self.inferred_key,
                value: Value::String("inferred!".into()),
            });
        }
        InferenceResult {
            facts,
            rule_name: self.name().to_string(),
        }
    }
}

#[test]
fn in_memory_inference() {
    let db = open_mem_db();

    let mut wtx = db.write_txn().unwrap();
    let node_t = wtx
        .register_type(TypeDefinitionBuilder::node_type("Thing").build())
        .unwrap();
    let source_key = wtx.get_or_create_property_key("is_source").unwrap();
    let inferred_key = wtx.get_or_create_property_key("derived").unwrap();

    let n1 = wtx
        .insert_node(
            NodeBuilder::new()
                .type_label(node_t)
                .property(source_key, Value::Bool(true))
                .build(),
        )
        .unwrap();
    let _n2 = wtx
        .insert_node(
            NodeBuilder::new()
                .type_label(node_t)
                .property(source_key, Value::Bool(false))
                .build(),
        )
        .unwrap();

    wtx.commit().unwrap();

    db.register_inference_rule(Box::new(SimpleInferenceRule {
        source_key,
        inferred_key,
    }))
    .unwrap();

    // Ephemeral inference (via read transaction)
    {
        let rtx = db.read_txn().unwrap();
        let result = rtx.run_inference("simple_infer").unwrap();
        assert_eq!(result.facts.len(), 1);
        match &result.facts[0] {
            InferredFact::NodePropertyUpdate { node, key, value } => {
                assert_eq!(*node, n1);
                assert_eq!(*key, inferred_key);
                assert_eq!(*value, Value::String("inferred!".into()));
            }
            _ => panic!("unexpected fact type"),
        }
    }

    // Materialized inference (via write transaction)
    {
        let mut wtx = db.write_txn().unwrap();
        let mapping = wtx
            .run_inference("simple_infer", InferenceMode::Materialized)
            .unwrap();
        assert_eq!(mapping.facts.len(), 1);
        wtx.commit().unwrap();
    }

    // Verify materialized property is visible
    {
        let rtx = db.read_txn().unwrap();
        let node = rtx.get_node(n1).unwrap().unwrap();
        assert_eq!(
            node.properties.get(&inferred_key),
            Some(&Value::String("inferred!".into()))
        );
    }
}

// =========================================================================
// 7.6 — Concurrent access
// =========================================================================

#[test]
fn in_memory_concurrent_access() {
    let db = Arc::new(open_mem_db());

    // Setup
    {
        let mut wtx = db.write_txn().unwrap();
        let t = wtx
            .register_type(TypeDefinitionBuilder::node_type("N").build())
            .unwrap();
        let name_key = wtx.get_or_create_property_key("name").unwrap();
        for i in 0..10 {
            wtx.insert_node(
                NodeBuilder::new()
                    .type_label(t)
                    .property(name_key, Value::String(format!("node_{i}")))
                    .build(),
            )
            .unwrap();
        }
        wtx.commit().unwrap();
    }

    // Spawn reader threads
    let mut handles = Vec::new();
    for _ in 0..4 {
        let db_clone = Arc::clone(&db);
        handles.push(std::thread::spawn(move || {
            let rtx = db_clone.read_txn().unwrap();
            let count = rtx.node_count().unwrap();
            assert!(count >= 10);
        }));
    }

    // Writer thread
    {
        let db_clone = Arc::clone(&db);
        handles.push(std::thread::spawn(move || {
            let mut wtx = db_clone.write_txn().unwrap();
            let name_key = wtx.get_or_create_property_key("name").unwrap();
            let t = wtx
                .register_type(TypeDefinitionBuilder::node_type("M").build())
                .unwrap();
            wtx.insert_node(
                NodeBuilder::new()
                    .type_label(t)
                    .property(name_key, Value::String("writer_node".into()))
                    .build(),
            )
            .unwrap();
            wtx.commit().unwrap();
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    // Final check
    let rtx = db.read_txn().unwrap();
    assert!(rtx.node_count().unwrap() >= 11);
}

// =========================================================================
// 8.1 — Snapshot round-trip: in-memory → file → in-memory
// =========================================================================

#[test]
fn snapshot_in_memory_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let snap_path = dir.path().join("snapshot.db");

    let node_type;
    let name_key;
    let alice_id;

    // Create in-memory DB with data
    {
        let db = open_mem_db();
        let mut wtx = db.write_txn().unwrap();
        node_type = wtx
            .register_type(TypeDefinitionBuilder::node_type("Person").build())
            .unwrap();
        name_key = wtx.get_or_create_property_key("name").unwrap();
        alice_id = wtx
            .insert_node(
                NodeBuilder::new()
                    .type_label(node_type)
                    .property(name_key, Value::String("Alice".into()))
                    .build(),
            )
            .unwrap();
        wtx.commit().unwrap();

        // Snapshot to file
        db.save_to_file(&snap_path).unwrap();
    }

    // Load snapshot into new MemoryBackend and verify raw bytes
    let loaded = MemoryBackend::load_from_file(&snap_path).unwrap();
    assert!(!loaded.as_bytes().is_empty());

    // Open the snapshot file as persistent and verify data
    {
        let db2 = Database::open(DatabaseConfig::persistent(&snap_path)).unwrap();
        let rtx = db2.read_txn().unwrap();
        let alice = rtx.get_node(alice_id).unwrap().unwrap();
        assert_eq!(
            alice.properties.get(&name_key),
            Some(&Value::String("Alice".into()))
        );
        assert!(alice.type_labels.contains(&node_type));
    }
}

// =========================================================================
// 8.2 — Snapshot interop: in-memory → persistent
// =========================================================================

#[test]
fn snapshot_in_memory_to_persistent() {
    let dir = tempfile::tempdir().unwrap();
    let snap_path = dir.path().join("interop.db");

    let node_type;
    let edge_type;
    let name_key;
    let alice_id;
    let bob_id;

    {
        let db = open_mem_db();
        let mut wtx = db.write_txn().unwrap();
        node_type = wtx
            .register_type(TypeDefinitionBuilder::node_type("Person").build())
            .unwrap();
        edge_type = wtx
            .register_type(TypeDefinitionBuilder::edge_type("knows").build())
            .unwrap();
        name_key = wtx.get_or_create_property_key("name").unwrap();

        alice_id = wtx
            .insert_node(
                NodeBuilder::new()
                    .type_label(node_type)
                    .property(name_key, Value::String("Alice".into()))
                    .build(),
            )
            .unwrap();
        bob_id = wtx
            .insert_node(
                NodeBuilder::new()
                    .type_label(node_type)
                    .property(name_key, Value::String("Bob".into()))
                    .build(),
            )
            .unwrap();
        wtx.insert_edge(EdgeBuilder::new(alice_id, bob_id).type_label(edge_type).build())
            .unwrap();
        wtx.commit().unwrap();

        db.save_to_file(&snap_path).unwrap();
    }

    // Open as persistent database
    let db2 = Database::open(DatabaseConfig::persistent(&snap_path)).unwrap();
    let rtx = db2.read_txn().unwrap();

    assert_eq!(rtx.node_count().unwrap(), 2);
    assert_eq!(rtx.edge_count().unwrap(), 1);

    let alice = rtx.get_node(alice_id).unwrap().unwrap();
    assert_eq!(
        alice.properties.get(&name_key),
        Some(&Value::String("Alice".into()))
    );

    let edges = rtx.outgoing_edges(alice_id, Some(edge_type)).unwrap();
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].target, bob_id);

    // Schema survived
    let reg = rtx.type_registry();
    let td = reg.get_type(node_type).unwrap();
    assert_eq!(td.name, "Person");
}

// =========================================================================
// 8.3 — Snapshot interop: persistent → in-memory
// =========================================================================

#[test]
fn snapshot_persistent_to_in_memory() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("persist.db");

    let node_type;
    let name_key;
    let alice_id;

    // Create persistent database
    {
        let db = Database::open(DatabaseConfig::persistent(&db_path)).unwrap();
        let mut wtx = db.write_txn().unwrap();
        node_type = wtx
            .register_type(TypeDefinitionBuilder::node_type("Person").build())
            .unwrap();
        name_key = wtx.get_or_create_property_key("name").unwrap();
        alice_id = wtx
            .insert_node(
                NodeBuilder::new()
                    .type_label(node_type)
                    .property(name_key, Value::String("Alice".into()))
                    .build(),
            )
            .unwrap();
        wtx.commit().unwrap();
    }

    // Load the file into a MemoryBackend
    let loaded = MemoryBackend::load_from_file(&db_path).unwrap();
    assert!(!loaded.as_bytes().is_empty());

    // Save to a new temp path and open as persistent to verify
    // (We don't have a direct "open from MemoryBackend" API, so verify
    // via the file round-trip.)
    let snap_path = dir.path().join("from_persistent.db");
    loaded.save_to_file(&snap_path).unwrap();

    let db2 = Database::open(DatabaseConfig::persistent(&snap_path)).unwrap();
    let rtx = db2.read_txn().unwrap();

    let alice = rtx.get_node(alice_id).unwrap().unwrap();
    assert_eq!(
        alice.properties.get(&name_key),
        Some(&Value::String("Alice".into()))
    );
    assert!(alice.type_labels.contains(&node_type));
    assert_eq!(rtx.node_count().unwrap(), 1);
}
