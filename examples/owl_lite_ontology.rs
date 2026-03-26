//! Minimal OWL Lite ontology layer on top of graph_db.
//!
//! Demonstrates how to use the extension system to build an ontology layer:
//!
//! - Register OWL-inspired types (Class, Individual, rdf:type, rdfs:subClassOf)
//! - Implement a custom `ConstraintValidator` (max-cardinality check)
//! - Implement a custom `InferenceRule` (subclass propagation)
//! - Run inference and query inferred facts
//!
//! All OWL-specific types are defined locally in this example — the crate
//! itself has no built-in ontology vocabulary.

use std::collections::BTreeMap;

use graph_db::constraint::{
    ChangeSet, ConstraintValidator, ConstraintViolation, ViolationSubject,
};
use graph_db::db::{
    Database, DatabaseConfig, EdgeBuilder, NodeBuilder, TypeDefinitionBuilder,
};
use graph_db::inference::{InferenceMode, InferenceResult, InferenceRule, InferredFact};
use graph_db::schema::{GraphView, PropertyKeyRegistryView, TypeRegistryView};
use graph_db::types::{PropertyKeyId, TypeId, Value};
use graph_db::Error;

// ---------------------------------------------------------------------------
// Custom constraint validator: MaxCardinalityValidator
// ---------------------------------------------------------------------------

/// Enforces a maximum cardinality constraint on outgoing edges of a given type.
///
/// For example: "An Individual may have at most N outgoing `rdf:type` edges."
/// This models OWL's `owl:maxCardinality` restriction in a simplified form.
struct MaxCardinalityValidator {
    /// The node type this constraint applies to (e.g., Individual).
    node_type: TypeId,
    /// The edge type to count (e.g., rdf:type).
    edge_type: TypeId,
    /// Maximum number of outgoing edges of `edge_type` allowed.
    max: usize,
}

impl ConstraintValidator for MaxCardinalityValidator {
    fn name(&self) -> &str {
        "owl:MaxCardinality"
    }

    fn applies_to_types(&self) -> Option<Vec<TypeId>> {
        // Must include the edge type so the engine dispatches us when
        // edges of that type are inserted (affected_types is derived
        // from the changeset's type labels).
        Some(vec![self.node_type, self.edge_type])
    }

    fn validate(
        &self,
        changes: &ChangeSet<'_>,
        graph: &dyn GraphView,
        _types: &dyn TypeRegistryView,
        _keys: &dyn PropertyKeyRegistryView,
    ) -> Vec<ConstraintViolation> {
        let mut violations = Vec::new();

        // Collect all node IDs that need cardinality checks:
        // - source nodes of inserted edges of the constrained type
        // - inserted or modified nodes of the constrained node type
        let mut affected_sources = std::collections::HashSet::new();
        for edge in changes.inserted_edges() {
            if edge.type_labels.contains(&self.edge_type) {
                affected_sources.insert(edge.source);
            }
        }
        for node in changes.inserted_nodes().chain(
            changes.modified_nodes().map(|(after, _before)| after),
        ) {
            if node.type_labels.contains(&self.node_type) {
                affected_sources.insert(node.id);
            }
        }

        // The `graph` view already includes pending changes (it is an
        // overlay), so `outgoing_edges` returns the post-commit count.
        for node_id in affected_sources {
            let node = match graph.get_node(node_id) {
                Some(n) => n,
                None => continue,
            };
            if !node.type_labels.contains(&self.node_type) {
                continue;
            }

            let total = graph.outgoing_edges(node_id, Some(self.edge_type)).len();
            if total > self.max {
                violations.push(ConstraintViolation {
                    violation_kind: "MaxCardinalityExceeded".into(),
                    message: format!(
                        "Node {:?} has {} outgoing edges of the constrained type \
                         (max {})",
                        node_id, total, self.max,
                    ),
                    subject: Some(ViolationSubject::Node(node_id)),
                });
            }
        }

        violations
    }
}

// ---------------------------------------------------------------------------
// Custom inference rule: SubclassPropagationRule
// ---------------------------------------------------------------------------

/// For every node N with a `rdf:type` edge to class C, if C has a
/// `rdfs:subClassOf` edge to class P, infers a new `rdf:type` edge N → P.
///
/// This implements transitive class membership through the subclass hierarchy,
/// a fundamental OWL/RDFS inference pattern.
struct SubclassPropagationRule {
    /// Edge type representing class membership (rdf:type).
    rdf_type_edge: TypeId,
    /// Edge type representing the subclass relationship (rdfs:subClassOf).
    subclass_of_edge: TypeId,
}

impl InferenceRule for SubclassPropagationRule {
    fn name(&self) -> &str {
        "rdfs:SubclassPropagation"
    }

    fn applies_to_types(&self) -> Option<Vec<TypeId>> {
        // We care about rdf:type and rdfs:subClassOf edges.
        Some(vec![self.rdf_type_edge, self.subclass_of_edge])
    }

    fn infer(
        &self,
        graph: &dyn GraphView,
        _types: &dyn TypeRegistryView,
        _keys: &dyn PropertyKeyRegistryView,
    ) -> InferenceResult {
        let mut facts = Vec::new();

        // For every rdf:type edge (individual → class), check if the class
        // has superclasses via rdfs:subClassOf.
        let type_edges = graph.edges_by_type(self.rdf_type_edge, false);

        for type_edge in &type_edges {
            let individual = type_edge.source;
            let class_node = type_edge.target;

            // Walk the subClassOf chain upward.
            let mut frontier = vec![class_node];
            let mut visited = std::collections::HashSet::new();
            visited.insert(class_node);

            while let Some(current_class) = frontier.pop() {
                let superclass_edges =
                    graph.outgoing_edges(current_class, Some(self.subclass_of_edge));

                for sc_edge in &superclass_edges {
                    let parent_class = sc_edge.target;
                    if !visited.insert(parent_class) {
                        continue; // Already visited — avoid cycles.
                    }
                    frontier.push(parent_class);

                    // Check if the individual already has a rdf:type edge
                    // to this parent class.
                    let existing = graph.outgoing_edges(individual, Some(self.rdf_type_edge));
                    let already_typed = existing.iter().any(|e| e.target == parent_class);

                    if !already_typed {
                        facts.push(InferredFact::NewEdge {
                            type_labels: vec![self.rdf_type_edge],
                            source: individual,
                            target: parent_class,
                            properties: BTreeMap::new(),
                        });
                    }
                }
            }
        }

        InferenceResult {
            facts,
            rule_name: self.name().to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Main: build a small ontology and demonstrate inference + validation
// ---------------------------------------------------------------------------

fn main() -> Result<(), Error> {
    println!("=== OWL Lite Ontology Example ===\n");

    let db = Database::open(DatabaseConfig::in_memory())?;

    // --- Step 1: Register OWL-inspired types ---
    let mut wtx = db.write_txn()?;

    // Node types
    let class_type = wtx.register_type(
        TypeDefinitionBuilder::node_type("owl:Class").open().build(),
    )?;
    let individual_type = wtx.register_type(
        TypeDefinitionBuilder::node_type("owl:Individual").open().build(),
    )?;

    // Edge types
    let rdf_type_edge = wtx.register_type(
        TypeDefinitionBuilder::edge_type("rdf:type").build(),
    )?;
    let subclass_of_edge = wtx.register_type(
        TypeDefinitionBuilder::edge_type("rdfs:subClassOf").build(),
    )?;

    // Property key for labels
    let label_key = wtx.get_or_create_property_key("rdfs:label")?;

    // --- Step 2: Build a class hierarchy: Animal → Mammal → Dog ---
    let animal_class = wtx.insert_node(
        NodeBuilder::new()
            .type_label(class_type)
            .property(label_key, Value::String("Animal".into()))
            .build(),
    )?;
    let mammal_class = wtx.insert_node(
        NodeBuilder::new()
            .type_label(class_type)
            .property(label_key, Value::String("Mammal".into()))
            .build(),
    )?;
    let dog_class = wtx.insert_node(
        NodeBuilder::new()
            .type_label(class_type)
            .property(label_key, Value::String("Dog".into()))
            .build(),
    )?;

    // Mammal rdfs:subClassOf Animal
    wtx.insert_edge(
        EdgeBuilder::new(mammal_class, animal_class)
            .type_label(subclass_of_edge)
            .build(),
    )?;
    // Dog rdfs:subClassOf Mammal
    wtx.insert_edge(
        EdgeBuilder::new(dog_class, mammal_class)
            .type_label(subclass_of_edge)
            .build(),
    )?;

    // --- Step 3: Create individuals of type Dog ---
    let rex = wtx.insert_node(
        NodeBuilder::new()
            .type_label(individual_type)
            .property(label_key, Value::String("Rex".into()))
            .build(),
    )?;
    let buddy = wtx.insert_node(
        NodeBuilder::new()
            .type_label(individual_type)
            .property(label_key, Value::String("Buddy".into()))
            .build(),
    )?;

    // Rex rdf:type Dog
    wtx.insert_edge(
        EdgeBuilder::new(rex, dog_class)
            .type_label(rdf_type_edge)
            .build(),
    )?;
    // Buddy rdf:type Dog
    wtx.insert_edge(
        EdgeBuilder::new(buddy, dog_class)
            .type_label(rdf_type_edge)
            .build(),
    )?;

    wtx.commit()?;

    // Print the initial state.
    {
        let rtx = db.read_txn()?;
        println!("--- Initial State ---");
        print_individual_types(&rtx, rex, rdf_type_edge, label_key, "Rex")?;
        print_individual_types(&rtx, buddy, rdf_type_edge, label_key, "Buddy")?;
        rtx.finish();
    }

    // --- Step 4: Register the inference rule and run inference ---
    db.register_inference_rule(Box::new(SubclassPropagationRule {
        rdf_type_edge,
        subclass_of_edge,
    }))?;

    {
        let mut wtx = db.write_txn()?;

        println!("\n--- Running Subclass Propagation Inference ---");
        let result = wtx.run_inference("rdfs:SubclassPropagation", InferenceMode::Materialized)?;
        println!(
            "Inference rule '{}' produced {} new facts.",
            result.rule_name,
            result.facts.len()
        );

        wtx.commit()?;
    }

    // Query the inferred state.
    {
        let rtx = db.read_txn()?;
        println!("\n--- After Inference ---");
        print_individual_types(&rtx, rex, rdf_type_edge, label_key, "Rex")?;
        print_individual_types(&rtx, buddy, rdf_type_edge, label_key, "Buddy")?;
        rtx.finish();
    }

    // --- Step 5: Demonstrate constraint validation ---
    // Register a max-cardinality constraint: Individuals may have at most
    // 3 rdf:type edges (direct type + 2 inferred supertypes for Dog).
    db.register_constraint(Box::new(MaxCardinalityValidator {
        node_type: individual_type,
        edge_type: rdf_type_edge,
        max: 3,
    }))?;

    println!("\n--- Constraint Validation Demo ---");
    println!("Registered max-cardinality constraint: at most 3 rdf:type edges per Individual.");

    // This should succeed — Rex currently has 3 rdf:type edges (Dog, Mammal, Animal).
    {
        let mut wtx = db.write_txn()?;
        // Add a harmless property change to trigger validation at commit.
        wtx.set_node_property(rex, label_key, Value::String("Rex the Dog".into()))?;
        let violations = wtx.validate()?;
        println!(
            "Validation after property update: {} violation(s)",
            violations.len()
        );
        wtx.commit()?;
    }

    // Now add a 4th rdf:type edge — this should cause a violation at commit.
    let bird_class = {
        let mut wtx = db.write_txn()?;
        let bird = wtx.insert_node(
            NodeBuilder::new()
                .type_label(class_type)
                .property(label_key, Value::String("Bird".into()))
                .build(),
        )?;
        wtx.commit()?;
        bird
    };

    {
        let mut wtx = db.write_txn()?;
        // Try to also make Rex a Bird — this would give Rex 4 rdf:type edges.
        wtx.insert_edge(
            EdgeBuilder::new(rex, bird_class)
                .type_label(rdf_type_edge)
                .build(),
        )?;
        match wtx.commit() {
            Ok(()) => println!("Commit succeeded (unexpected)."),
            Err(Error::ConstraintViolation(violations)) => {
                println!("\nCommit rejected with {} violation(s):", violations.len());
                for v in &violations {
                    println!("  [{}] {}", v.violation_kind, v.message);
                }
            }
            Err(e) => println!("Unexpected error: {e}"),
        }
    }

    println!("\nDone.");
    Ok(())
}

/// Helper: print the rdf:type edges for an individual, showing class labels.
fn print_individual_types(
    rtx: &graph_db::db::ReadTransaction<'_>,
    individual_id: graph_db::NodeId,
    rdf_type_edge: TypeId,
    label_key: PropertyKeyId,
    name: &str,
) -> Result<(), Error> {
    let type_edges = rtx.outgoing_edges(individual_id, Some(rdf_type_edge))?;
    let class_names: Vec<String> = type_edges
        .iter()
        .filter_map(|e| {
            rtx.get_node(e.target)
                .ok()
                .flatten()
                .and_then(|n| {
                    n.properties
                        .get(&label_key)
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                })
        })
        .collect();
    println!("  {} rdf:type {:?}", name, class_names);
    Ok(())
}
