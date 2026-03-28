//! Integration tests for the database engine layer (Task 25, Phase 10).
//!
//! These tests exercise the full stack from public API through the storage engine.

use phonograph_std::constraint::{
    ChangeSet, ConstraintValidator, ConstraintViolation, ViolationSubject,
};
use phonograph_std::db::builders::{EdgeBuilder, NodeBuilder, TypeDefinitionBuilder};
use phonograph_std::error::Error;
use phonograph_std::schema::{GraphView, PropertyKeyRegistryView, TypeRegistryView};
use phonograph_std::types::{NodeId, PropertyKeyId, TypeId, Value};
use phonograph_std::FileDatabase;

use std::collections::HashSet;

/// Helper: creates a temp-dir database.
fn open_temp_db() -> (FileDatabase, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    let db = phonograph_std::open(&path).unwrap();
    (db, dir)
}

// =========================================================================
// 10.1 — Basic CRUD round-trip
// =========================================================================

#[test]
fn crud_round_trip() {
    let (db, _dir) = open_temp_db();

    // Write transaction: register types, insert nodes and edge
    let person_type;
    let org_type;
    let works_at_type;
    let name_key;
    let alice_id;
    let acme_id;
    let edge_id;

    {
        let mut wtx = db.write_txn().unwrap();

        person_type = wtx
            .register_type(TypeDefinitionBuilder::node_type("Person").build())
            .unwrap();
        org_type = wtx
            .register_type(TypeDefinitionBuilder::node_type("Organization").build())
            .unwrap();
        works_at_type = wtx
            .register_type(TypeDefinitionBuilder::edge_type("works_at").build())
            .unwrap();

        name_key = wtx.get_or_create_property_key("name").unwrap();

        alice_id = wtx
            .insert_node(
                NodeBuilder::new()
                    .type_label(person_type)
                    .property(name_key, Value::String("Alice".into()))
                    .build(),
            )
            .unwrap();

        acme_id = wtx
            .insert_node(
                NodeBuilder::new()
                    .type_label(org_type)
                    .property(name_key, Value::String("Acme".into()))
                    .build(),
            )
            .unwrap();

        edge_id = wtx
            .insert_edge(
                EdgeBuilder::new(alice_id, acme_id)
                    .type_label(works_at_type)
                    .build(),
            )
            .unwrap();

        wtx.commit().unwrap();
    }

    // Read transaction: verify everything
    {
        let rtx = db.read_txn().unwrap();

        // get_node
        let alice = rtx.get_node(alice_id).unwrap().unwrap();
        assert_eq!(
            alice.properties.get(&name_key),
            Some(&Value::String("Alice".into()))
        );
        assert!(alice.type_labels.contains(&person_type));

        // outgoing_edges
        let out_all = rtx.outgoing_edges(alice_id, None).unwrap();
        assert_eq!(out_all.len(), 1);
        assert_eq!(out_all[0].id, edge_id);

        let out_typed = rtx.outgoing_edges(alice_id, Some(works_at_type)).unwrap();
        assert_eq!(out_typed.len(), 1);

        let out_other = rtx.outgoing_edges(alice_id, Some(person_type)).unwrap();
        assert!(out_other.is_empty());

        // incoming_edges
        let inc = rtx.incoming_edges(acme_id, None).unwrap();
        assert_eq!(inc.len(), 1);
        assert_eq!(inc[0].source, alice_id);

        // neighbors
        let neighbors = rtx.neighbors(alice_id, Some(works_at_type)).unwrap();
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0].id, acme_id);

        // nodes_by_type
        let people = rtx.nodes_by_type(person_type, false).unwrap();
        assert_eq!(people.len(), 1);
        assert_eq!(people[0].id, alice_id);

        // edges_by_type
        let wa_edges = rtx.edges_by_type(works_at_type, false).unwrap();
        assert_eq!(wa_edges.len(), 1);

        // nodes_by_property
        let by_name = rtx
            .nodes_by_property(name_key, &Value::String("Alice".into()))
            .unwrap();
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name[0].id, alice_id);

        // counts
        assert_eq!(rtx.node_count().unwrap(), 2);
        assert_eq!(rtx.edge_count().unwrap(), 1);
    }
}

// =========================================================================
// 10.2 — Read-your-own-writes
// =========================================================================

#[test]
fn read_your_own_writes() {
    let (db, _dir) = open_temp_db();

    let mut wtx = db.write_txn().unwrap();
    let t = wtx
        .register_type(TypeDefinitionBuilder::node_type("Thing").build())
        .unwrap();
    let name_key = wtx.get_or_create_property_key("name").unwrap();

    // Insert A, read back
    let a_id = wtx
        .insert_node(
            NodeBuilder::new()
                .type_label(t)
                .property(name_key, Value::String("A".into()))
                .build(),
        )
        .unwrap();
    let a = wtx.get_node(a_id).unwrap().unwrap();
    assert_eq!(a.properties.get(&name_key), Some(&Value::String("A".into())));

    // Update A's property
    wtx.set_node_property(a_id, name_key, Value::String("A_updated".into()))
        .unwrap();
    let a2 = wtx.get_node(a_id).unwrap().unwrap();
    assert_eq!(
        a2.properties.get(&name_key),
        Some(&Value::String("A_updated".into()))
    );

    // Delete A
    wtx.delete_node(a_id).unwrap();
    assert!(wtx.get_node(a_id).unwrap().is_none());

    // Insert B, verify nodes_by_type
    let b_id = wtx
        .insert_node(NodeBuilder::new().type_label(t).build())
        .unwrap();
    let by_type = wtx.nodes_by_type(t, false).unwrap();
    assert!(by_type.iter().any(|n| n.id == b_id));
    assert!(!by_type.iter().any(|n| n.id == a_id));

    // Abort
    wtx.abort();

    // New read transaction: A and B should not exist
    let rtx = db.read_txn().unwrap();
    assert!(rtx.get_node(a_id).unwrap().is_none());
    assert!(rtx.get_node(b_id).unwrap().is_none());
}

// =========================================================================
// 10.3 — Cascading node deletion
// =========================================================================

#[test]
fn cascading_delete() {
    let (db, _dir) = open_temp_db();

    let a_id;
    let b_id;
    let c_id;
    let edge_type;

    {
        let mut wtx = db.write_txn().unwrap();
        let nt = wtx
            .register_type(TypeDefinitionBuilder::node_type("N").build())
            .unwrap();
        edge_type = wtx
            .register_type(TypeDefinitionBuilder::edge_type("E").build())
            .unwrap();

        a_id = wtx
            .insert_node(NodeBuilder::new().type_label(nt).build())
            .unwrap();
        b_id = wtx
            .insert_node(NodeBuilder::new().type_label(nt).build())
            .unwrap();
        c_id = wtx
            .insert_node(NodeBuilder::new().type_label(nt).build())
            .unwrap();

        // A → B, B → C, C → A
        wtx.insert_edge(
            EdgeBuilder::new(a_id, b_id).type_label(edge_type).build(),
        )
        .unwrap();
        wtx.insert_edge(
            EdgeBuilder::new(b_id, c_id).type_label(edge_type).build(),
        )
        .unwrap();
        wtx.insert_edge(
            EdgeBuilder::new(c_id, a_id).type_label(edge_type).build(),
        )
        .unwrap();

        // Delete B — should cascade to A→B and B→C edges
        wtx.delete_node(b_id).unwrap();

        // Within transaction: B is gone, A and C remain
        assert!(wtx.get_node(b_id).unwrap().is_none());
        assert!(wtx.get_node(a_id).unwrap().is_some());
        assert!(wtx.get_node(c_id).unwrap().is_some());

        // C→A edge should still exist
        let c_out = wtx.outgoing_edges(c_id, None).unwrap();
        assert_eq!(c_out.len(), 1);
        assert_eq!(c_out[0].target, a_id);

        // A should have no outgoing edges (A→B was deleted)
        let a_out = wtx.outgoing_edges(a_id, None).unwrap();
        assert!(a_out.is_empty());

        wtx.commit().unwrap();
    }

    // Verify via read transaction
    let rtx = db.read_txn().unwrap();
    assert!(rtx.get_node(b_id).unwrap().is_none());
    assert!(rtx.get_node(a_id).unwrap().is_some());
    assert!(rtx.get_node(c_id).unwrap().is_some());
    assert_eq!(rtx.edge_count().unwrap(), 1); // only C→A
}

// =========================================================================
// 10.4 — Type hierarchy and subtype query
// =========================================================================

#[test]
fn subtype_query() {
    let (db, _dir) = open_temp_db();

    let animal_type;
    let mammal_type;
    let dog_type;
    let cat_type;

    let fido_id;
    let whiskers_id;
    let generic_id;

    {
        let mut wtx = db.write_txn().unwrap();

        animal_type = wtx
            .register_type(TypeDefinitionBuilder::node_type("Animal").build())
            .unwrap();
        mammal_type = wtx
            .register_type(
                TypeDefinitionBuilder::node_type("Mammal")
                    .supertype(animal_type)
                    .build(),
            )
            .unwrap();
        dog_type = wtx
            .register_type(
                TypeDefinitionBuilder::node_type("Dog")
                    .supertype(mammal_type)
                    .build(),
            )
            .unwrap();
        cat_type = wtx
            .register_type(
                TypeDefinitionBuilder::node_type("Cat")
                    .supertype(mammal_type)
                    .build(),
            )
            .unwrap();

        fido_id = wtx
            .insert_node(NodeBuilder::new().type_label(dog_type).build())
            .unwrap();
        whiskers_id = wtx
            .insert_node(NodeBuilder::new().type_label(cat_type).build())
            .unwrap();
        generic_id = wtx
            .insert_node(NodeBuilder::new().type_label(animal_type).build())
            .unwrap();

        wtx.commit().unwrap();
    }

    let rtx = db.read_txn().unwrap();

    // Animal with subtypes → all three
    let all_animals = rtx.nodes_by_type(animal_type, true).unwrap();
    let ids: HashSet<NodeId> = all_animals.iter().map(|n| n.id).collect();
    assert!(ids.contains(&fido_id));
    assert!(ids.contains(&whiskers_id));
    assert!(ids.contains(&generic_id));

    // Mammal with subtypes → Fido and Whiskers
    let mammals = rtx.nodes_by_type(mammal_type, true).unwrap();
    let ids: HashSet<NodeId> = mammals.iter().map(|n| n.id).collect();
    assert!(ids.contains(&fido_id));
    assert!(ids.contains(&whiskers_id));
    assert!(!ids.contains(&generic_id));

    // Dog with subtypes → only Fido
    let dogs = rtx.nodes_by_type(dog_type, true).unwrap();
    assert_eq!(dogs.len(), 1);
    assert_eq!(dogs[0].id, fido_id);

    // Dog without subtypes → only Fido
    let dogs_exact = rtx.nodes_by_type(dog_type, false).unwrap();
    assert_eq!(dogs_exact.len(), 1);
    assert_eq!(dogs_exact[0].id, fido_id);
}

// =========================================================================
// 10.5 — Multi-hop traversal
// =========================================================================

#[test]
fn multi_hop_traversal() {
    let (db, _dir) = open_temp_db();

    // Register types
    let mut wtx = db.write_txn().unwrap();
    let person_t = wtx.register_type(TypeDefinitionBuilder::node_type("Person").build()).unwrap();
    let team_t = wtx.register_type(TypeDefinitionBuilder::node_type("Team").build()).unwrap();
    let project_t = wtx.register_type(TypeDefinitionBuilder::node_type("Project").build()).unwrap();
    let skill_t = wtx.register_type(TypeDefinitionBuilder::node_type("Skill").build()).unwrap();

    let member_of_t = wtx.register_type(TypeDefinitionBuilder::edge_type("member_of").build()).unwrap();
    let works_on_t = wtx.register_type(TypeDefinitionBuilder::edge_type("works_on").build()).unwrap();
    let requires_t = wtx.register_type(TypeDefinitionBuilder::edge_type("requires").build()).unwrap();
    let has_skill_t = wtx.register_type(TypeDefinitionBuilder::edge_type("has_skill").build()).unwrap();

    let name_key = wtx.get_or_create_property_key("name").unwrap();

    // Insert nodes
    let alice = wtx.insert_node(NodeBuilder::new().type_label(person_t).property(name_key, Value::String("Alice".into())).build()).unwrap();
    let bob = wtx.insert_node(NodeBuilder::new().type_label(person_t).property(name_key, Value::String("Bob".into())).build()).unwrap();
    let carol = wtx.insert_node(NodeBuilder::new().type_label(person_t).property(name_key, Value::String("Carol".into())).build()).unwrap();
    let engineering = wtx.insert_node(NodeBuilder::new().type_label(team_t).property(name_key, Value::String("Engineering".into())).build()).unwrap();
    let design = wtx.insert_node(NodeBuilder::new().type_label(team_t).property(name_key, Value::String("Design".into())).build()).unwrap();
    let project_x = wtx.insert_node(NodeBuilder::new().type_label(project_t).property(name_key, Value::String("ProjectX".into())).build()).unwrap();
    let project_y = wtx.insert_node(NodeBuilder::new().type_label(project_t).property(name_key, Value::String("ProjectY".into())).build()).unwrap();
    let rust_skill = wtx.insert_node(NodeBuilder::new().type_label(skill_t).property(name_key, Value::String("Rust".into())).build()).unwrap();
    let python_skill = wtx.insert_node(NodeBuilder::new().type_label(skill_t).property(name_key, Value::String("Python".into())).build()).unwrap();

    // Insert edges
    wtx.insert_edge(EdgeBuilder::new(alice, engineering).type_label(member_of_t).build()).unwrap();
    wtx.insert_edge(EdgeBuilder::new(alice, project_x).type_label(works_on_t).build()).unwrap();
    wtx.insert_edge(EdgeBuilder::new(alice, rust_skill).type_label(has_skill_t).build()).unwrap();
    wtx.insert_edge(EdgeBuilder::new(bob, engineering).type_label(member_of_t).build()).unwrap();
    wtx.insert_edge(EdgeBuilder::new(bob, project_y).type_label(works_on_t).build()).unwrap();
    wtx.insert_edge(EdgeBuilder::new(bob, python_skill).type_label(has_skill_t).build()).unwrap();
    wtx.insert_edge(EdgeBuilder::new(carol, design).type_label(member_of_t).build()).unwrap();
    wtx.insert_edge(EdgeBuilder::new(carol, project_x).type_label(works_on_t).build()).unwrap();
    wtx.insert_edge(EdgeBuilder::new(project_x, rust_skill).type_label(requires_t).build()).unwrap();
    wtx.insert_edge(EdgeBuilder::new(project_x, python_skill).type_label(requires_t).build()).unwrap();
    wtx.insert_edge(EdgeBuilder::new(project_y, python_skill).type_label(requires_t).build()).unwrap();

    wtx.commit().unwrap();

    let rtx = db.read_txn().unwrap();

    // Query 1: Skills required by projects Alice works on (2 hops)
    let alice_projects = rtx.outgoing_edges(alice, Some(works_on_t)).unwrap();
    let mut skills: HashSet<NodeId> = HashSet::new();
    for edge in &alice_projects {
        let req_edges = rtx.outgoing_edges(edge.target, Some(requires_t)).unwrap();
        for re in &req_edges {
            skills.insert(re.target);
        }
    }
    assert!(skills.contains(&rust_skill));
    assert!(skills.contains(&python_skill));
    assert_eq!(skills.len(), 2);

    // Query 2: People in same team as Alice (2 hops)
    let alice_teams = rtx.outgoing_edges(alice, Some(member_of_t)).unwrap();
    let mut teammates: HashSet<NodeId> = HashSet::new();
    for edge in &alice_teams {
        let members = rtx.incoming_edges(edge.target, Some(member_of_t)).unwrap();
        for m in &members {
            teammates.insert(m.source);
        }
    }
    assert!(teammates.contains(&alice));
    assert!(teammates.contains(&bob));
    assert_eq!(teammates.len(), 2);

    // Query 3: Projects requiring skills Alice has (3 hops via matching)
    let alice_skills = rtx.outgoing_edges(alice, Some(has_skill_t)).unwrap();
    let mut matching_projects: HashSet<NodeId> = HashSet::new();
    for skill_edge in &alice_skills {
        let projects_needing = rtx.incoming_edges(skill_edge.target, Some(requires_t)).unwrap();
        for pe in &projects_needing {
            matching_projects.insert(pe.source);
        }
    }
    assert!(matching_projects.contains(&project_x));
    assert_eq!(matching_projects.len(), 1);

    // Query 4: From Python, 4 hops → teams (Python→projects→people→teams)
    let py_projects = rtx.incoming_edges(python_skill, Some(requires_t)).unwrap();
    let mut all_teams: HashSet<NodeId> = HashSet::new();
    for pe in &py_projects {
        let workers = rtx.incoming_edges(pe.source, Some(works_on_t)).unwrap();
        for we in &workers {
            let teams = rtx.outgoing_edges(we.source, Some(member_of_t)).unwrap();
            for te in &teams {
                all_teams.insert(te.target);
            }
        }
    }
    assert!(all_teams.contains(&engineering));
    assert!(all_teams.contains(&design));
    assert_eq!(all_teams.len(), 2);
}

// =========================================================================
// 10.6 — Constraint validation at commit
// =========================================================================

struct RequireNameProperty {
    name_key: PropertyKeyId,
}

impl ConstraintValidator for RequireNameProperty {
    fn name(&self) -> &str {
        "RequireNameProperty"
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
                    message: format!("Node {:?} missing name", node.id),
                    subject: Some(ViolationSubject::Node(node.id)),
                });
            }
        }
        violations
    }
}

#[test]
fn constraint_validation_rejects_commit() {
    let (db, _dir) = open_temp_db();

    // Register type and get name key in a first transaction
    let (node_type, name_key) = {
        let mut wtx = db.write_txn().unwrap();
        let t = wtx.register_type(TypeDefinitionBuilder::node_type("Thing").build()).unwrap();
        let k = wtx.get_or_create_property_key("name").unwrap();
        wtx.commit().unwrap();
        (t, k)
    };

    // Register the validator
    db.register_constraint(Box::new(RequireNameProperty { name_key })).unwrap();

    // Insert node without name → commit should fail
    {
        let mut wtx = db.write_txn().unwrap();
        wtx.insert_node(NodeBuilder::new().type_label(node_type).build()).unwrap();
        let result = wtx.commit();
        assert!(matches!(result, Err(Error::ConstraintViolation(_))));
    }

    // Insert node with name → commit should succeed
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
// 10.7 — Empty transaction commit
// =========================================================================

#[test]
fn empty_commit() {
    let (db, _dir) = open_temp_db();
    let wtx = db.write_txn().unwrap();
    wtx.commit().unwrap(); // no mutations, should succeed
}

// =========================================================================
// 10.8 — Parallel edges
// =========================================================================

#[test]
fn parallel_edges() {
    let (db, _dir) = open_temp_db();

    let mut wtx = db.write_txn().unwrap();
    let nt = wtx.register_type(TypeDefinitionBuilder::node_type("N").build()).unwrap();
    let et = wtx.register_type(TypeDefinitionBuilder::edge_type("E").build()).unwrap();

    let a = wtx.insert_node(NodeBuilder::new().type_label(nt).build()).unwrap();
    let b = wtx.insert_node(NodeBuilder::new().type_label(nt).build()).unwrap();

    let e1 = wtx.insert_edge(EdgeBuilder::new(a, b).type_label(et).build()).unwrap();
    let e2 = wtx.insert_edge(EdgeBuilder::new(a, b).type_label(et).build()).unwrap();

    wtx.commit().unwrap();

    let rtx = db.read_txn().unwrap();
    let edges = rtx.outgoing_edges(a, Some(et)).unwrap();
    assert_eq!(edges.len(), 2);

    drop(rtx);

    // Delete one edge
    let mut wtx2 = db.write_txn().unwrap();
    wtx2.delete_edge(e1).unwrap();
    wtx2.commit().unwrap();

    let rtx2 = db.read_txn().unwrap();
    let edges2 = rtx2.outgoing_edges(a, Some(et)).unwrap();
    assert_eq!(edges2.len(), 1);
    assert_eq!(edges2[0].id, e2);
}

// =========================================================================
// 10.9 — Persistence round-trip
// =========================================================================

#[test]
fn persistence_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("persist.db");

    let node_type;
    let name_key;
    let alice_id;

    // Create and populate
    {
        let db = phonograph_std::open(&path).unwrap();
        let mut wtx = db.write_txn().unwrap();
        node_type = wtx.register_type(TypeDefinitionBuilder::node_type("Person").build()).unwrap();
        name_key = wtx.get_or_create_property_key("name").unwrap();
        alice_id = wtx.insert_node(
            NodeBuilder::new()
                .type_label(node_type)
                .property(name_key, Value::String("Alice".into()))
                .build(),
        ).unwrap();
        wtx.commit().unwrap();
        // db dropped here (close)
    }

    // Reopen and verify
    {
        let db = phonograph_std::open(&path).unwrap();
        let rtx = db.read_txn().unwrap();

        let alice = rtx.get_node(alice_id).unwrap().unwrap();
        assert_eq!(
            alice.properties.get(&name_key),
            Some(&Value::String("Alice".into()))
        );
        assert!(alice.type_labels.contains(&node_type));

        // Verify schema was reloaded
        let reg = rtx.type_registry();
        let td = reg.get_type(node_type).unwrap();
        assert_eq!(td.name, "Person");

        assert_eq!(rtx.node_count().unwrap(), 1);
    }
}
