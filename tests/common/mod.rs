//! Shared test helpers for integration tests (Task 28).

use graph_db::constraint::{
    ChangeSet, ConstraintValidator, ConstraintViolation, ViolationSubject,
};
use graph_db::db::builders::{EdgeBuilder, NodeBuilder, TypeDefinitionBuilder};
use graph_db::db::config::DatabaseConfig;
use graph_db::db::database::Database;
use graph_db::error::Error;
use graph_db::inference::{InferenceResult, InferenceRule, InferredFact};
use graph_db::schema::{GraphView, PropertyKeyRegistryView, TypeRegistryView};
use graph_db::types::{EdgeId, NodeId, PropertyKeyId, TypeId, Value};

// ---------------------------------------------------------------------------
// Database constructors
// ---------------------------------------------------------------------------

/// Opens a persistent database in a temporary directory.
pub fn open_temp_db() -> (Database, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    let db = Database::open(DatabaseConfig::persistent(&path)).unwrap();
    (db, dir)
}

/// Opens an in-memory database.
pub fn open_mem_db() -> Database {
    Database::open(DatabaseConfig::in_memory()).unwrap()
}

// ---------------------------------------------------------------------------
// RequiredPropertyValidator
// ---------------------------------------------------------------------------

/// Test-only constraint validator: requires that nodes of a specific type
/// have a specific property set to a non-null value.
pub struct RequiredPropertyValidator {
    pub target_type: TypeId,
    pub required_key: PropertyKeyId,
    pub validator_name: String,
}

impl ConstraintValidator for RequiredPropertyValidator {
    fn name(&self) -> &str {
        &self.validator_name
    }

    fn applies_to_types(&self) -> Option<Vec<TypeId>> {
        Some(vec![self.target_type])
    }

    fn validate(
        &self,
        changes: &ChangeSet<'_>,
        _graph: &dyn GraphView,
        _types: &dyn TypeRegistryView,
        _keys: &dyn PropertyKeyRegistryView,
    ) -> Vec<ConstraintViolation> {
        let mut violations = Vec::new();

        // Check inserted nodes
        for node in changes.inserted_nodes() {
            if node.type_labels.contains(&self.target_type)
                && (!node.properties.contains_key(&self.required_key)
                    || node.properties.get(&self.required_key) == Some(&Value::Null))
            {
                violations.push(ConstraintViolation {
                    violation_kind: "MissingRequiredProperty".into(),
                    message: format!(
                        "Node {:?} of type {:?} missing required property {:?}",
                        node.id, self.target_type, self.required_key
                    ),
                    subject: Some(ViolationSubject::Node(node.id)),
                });
            }
        }

        // Check modified nodes
        for (node, _before) in changes.modified_nodes() {
            if node.type_labels.contains(&self.target_type)
                && (!node.properties.contains_key(&self.required_key)
                    || node.properties.get(&self.required_key) == Some(&Value::Null))
            {
                violations.push(ConstraintViolation {
                    violation_kind: "MissingRequiredProperty".into(),
                    message: format!(
                        "Node {:?} of type {:?} missing required property {:?}",
                        node.id, self.target_type, self.required_key
                    ),
                    subject: Some(ViolationSubject::Node(node.id)),
                });
            }
        }

        violations
    }
}

// ---------------------------------------------------------------------------
// InverseEdgeRule
// ---------------------------------------------------------------------------

/// Test-only inference rule: for every edge of `source_edge_type` A→B,
/// infers a `inverse_edge_type` edge B→A.
pub struct InverseEdgeRule {
    pub source_edge_type: TypeId,
    pub inverse_edge_type: TypeId,
}

impl InferenceRule for InverseEdgeRule {
    fn name(&self) -> &str {
        "test::InverseEdge"
    }

    fn applies_to_types(&self) -> Option<Vec<TypeId>> {
        Some(vec![self.source_edge_type])
    }

    fn infer(
        &self,
        graph: &dyn GraphView,
        _types: &dyn TypeRegistryView,
        _keys: &dyn PropertyKeyRegistryView,
    ) -> InferenceResult {
        let mut facts = Vec::new();
        let edges = graph.edges_by_type(self.source_edge_type, false);
        for edge in edges {
            facts.push(InferredFact::NewEdge {
                type_labels: vec![self.inverse_edge_type],
                source: edge.target,
                target: edge.source,
                properties: Default::default(),
            });
        }
        InferenceResult {
            facts,
            rule_name: self.name().to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// TestGraph
// ---------------------------------------------------------------------------

/// Contains all IDs from the standard test graph.
#[allow(dead_code)]
pub struct TestGraph {
    // Type IDs
    pub entity_type: TypeId,
    pub person_type: TypeId,
    pub org_type: TypeId,
    pub project_type: TypeId,
    pub knows_type: TypeId,
    pub works_at_type: TypeId,
    pub leads_type: TypeId,
    // Property key IDs
    pub name_key: PropertyKeyId,
    pub age_key: PropertyKeyId,
    pub founded_key: PropertyKeyId,
    pub active_key: PropertyKeyId,
    // Node IDs
    pub alice: NodeId,
    pub bob: NodeId,
    pub carol: NodeId,
    pub acme: NodeId,
    pub globex: NodeId,
    pub proj_alpha: NodeId,
    pub proj_beta: NodeId,
    pub proj_gamma: NodeId,
    // Edge IDs
    pub alice_knows_bob: EdgeId,
    pub bob_knows_carol: EdgeId,
    pub alice_knows_carol: EdgeId,
    pub alice_works_at_acme: EdgeId,
    pub bob_works_at_acme: EdgeId,
    pub carol_works_at_globex: EdgeId,
    pub alice_leads_alpha: EdgeId,
    pub bob_leads_beta: EdgeId,
    pub carol_leads_gamma: EdgeId,
    pub acme_leads_alpha: EdgeId,
    pub acme_leads_beta: EdgeId,
    pub globex_leads_gamma: EdgeId,
}

/// Builds the standard test graph in the given database.
///
/// Creates 4 node types (Entity→Person, Entity→Organization, Project),
/// 3 edge types (knows, works_at, leads), 4 property keys,
/// 8 nodes, and 12 edges.
#[allow(dead_code)]
pub fn build_test_graph(db: &Database) -> Result<TestGraph, Error> {
    let mut wtx = db.write_txn()?;

    // Types
    let entity_type = wtx.register_type(TypeDefinitionBuilder::node_type("Entity").build())?;
    let person_type = wtx.register_type(
        TypeDefinitionBuilder::node_type("Person")
            .supertype(entity_type)
            .build(),
    )?;
    let org_type = wtx.register_type(
        TypeDefinitionBuilder::node_type("Organization")
            .supertype(entity_type)
            .build(),
    )?;
    let project_type = wtx.register_type(TypeDefinitionBuilder::node_type("Project").build())?;

    let knows_type = wtx.register_type(TypeDefinitionBuilder::edge_type("knows").build())?;
    let works_at_type = wtx.register_type(TypeDefinitionBuilder::edge_type("works_at").build())?;
    let leads_type = wtx.register_type(TypeDefinitionBuilder::edge_type("leads").build())?;

    // Property keys
    let name_key = wtx.get_or_create_property_key("name")?;
    let age_key = wtx.get_or_create_property_key("age")?;
    let founded_key = wtx.get_or_create_property_key("founded")?;
    let active_key = wtx.get_or_create_property_key("active")?;

    // Nodes
    let alice = wtx.insert_node(
        NodeBuilder::new()
            .type_label(person_type)
            .property(name_key, Value::String("Alice".into()))
            .property(age_key, Value::I64(30))
            .build(),
    )?;
    let bob = wtx.insert_node(
        NodeBuilder::new()
            .type_label(person_type)
            .property(name_key, Value::String("Bob".into()))
            .property(age_key, Value::I64(25))
            .build(),
    )?;
    let carol = wtx.insert_node(
        NodeBuilder::new()
            .type_label(person_type)
            .property(name_key, Value::String("Carol".into()))
            .property(age_key, Value::I64(35))
            .build(),
    )?;
    let acme = wtx.insert_node(
        NodeBuilder::new()
            .type_label(org_type)
            .property(name_key, Value::String("Acme Corp".into()))
            .property(founded_key, Value::I64(1990))
            .build(),
    )?;
    let globex = wtx.insert_node(
        NodeBuilder::new()
            .type_label(org_type)
            .property(name_key, Value::String("Globex Inc".into()))
            .property(founded_key, Value::I64(2005))
            .build(),
    )?;
    let proj_alpha = wtx.insert_node(
        NodeBuilder::new()
            .type_label(project_type)
            .property(name_key, Value::String("Alpha".into()))
            .property(active_key, Value::Bool(true))
            .build(),
    )?;
    let proj_beta = wtx.insert_node(
        NodeBuilder::new()
            .type_label(project_type)
            .property(name_key, Value::String("Beta".into()))
            .property(active_key, Value::Bool(true))
            .build(),
    )?;
    let proj_gamma = wtx.insert_node(
        NodeBuilder::new()
            .type_label(project_type)
            .property(name_key, Value::String("Gamma".into()))
            .property(active_key, Value::Bool(false))
            .build(),
    )?;

    // Edges
    let alice_knows_bob = wtx.insert_edge(
        EdgeBuilder::new(alice, bob).type_label(knows_type).build(),
    )?;
    let bob_knows_carol = wtx.insert_edge(
        EdgeBuilder::new(bob, carol).type_label(knows_type).build(),
    )?;
    let alice_knows_carol = wtx.insert_edge(
        EdgeBuilder::new(alice, carol).type_label(knows_type).build(),
    )?;
    let alice_works_at_acme = wtx.insert_edge(
        EdgeBuilder::new(alice, acme).type_label(works_at_type).build(),
    )?;
    let bob_works_at_acme = wtx.insert_edge(
        EdgeBuilder::new(bob, acme).type_label(works_at_type).build(),
    )?;
    let carol_works_at_globex = wtx.insert_edge(
        EdgeBuilder::new(carol, globex).type_label(works_at_type).build(),
    )?;
    let alice_leads_alpha = wtx.insert_edge(
        EdgeBuilder::new(alice, proj_alpha).type_label(leads_type).build(),
    )?;
    let bob_leads_beta = wtx.insert_edge(
        EdgeBuilder::new(bob, proj_beta).type_label(leads_type).build(),
    )?;
    let carol_leads_gamma = wtx.insert_edge(
        EdgeBuilder::new(carol, proj_gamma).type_label(leads_type).build(),
    )?;
    let acme_leads_alpha = wtx.insert_edge(
        EdgeBuilder::new(acme, proj_alpha).type_label(leads_type).build(),
    )?;
    let acme_leads_beta = wtx.insert_edge(
        EdgeBuilder::new(acme, proj_beta).type_label(leads_type).build(),
    )?;
    let globex_leads_gamma = wtx.insert_edge(
        EdgeBuilder::new(globex, proj_gamma).type_label(leads_type).build(),
    )?;

    wtx.commit()?;

    Ok(TestGraph {
        entity_type,
        person_type,
        org_type,
        project_type,
        knows_type,
        works_at_type,
        leads_type,
        name_key,
        age_key,
        founded_key,
        active_key,
        alice,
        bob,
        carol,
        acme,
        globex,
        proj_alpha,
        proj_beta,
        proj_gamma,
        alice_knows_bob,
        bob_knows_carol,
        alice_knows_carol,
        alice_works_at_acme,
        bob_works_at_acme,
        carol_works_at_globex,
        alice_leads_alpha,
        bob_leads_beta,
        carol_leads_gamma,
        acme_leads_alpha,
        acme_leads_beta,
        globex_leads_gamma,
    })
}
