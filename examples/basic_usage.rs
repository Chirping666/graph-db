//! Basic usage of the graph_db crate.
//!
//! Demonstrates: opening a database, registering types and property keys,
//! inserting nodes and edges, querying by type, and traversing edges.

use graph_db::db::{
    Database, DatabaseConfig, EdgeBuilder, NodeBuilder, TypeDefinitionBuilder,
};
use graph_db::{Error, Value};

fn main() -> Result<(), Error> {
    // --- 1. Open an in-memory database ---
    let db = Database::open(DatabaseConfig::in_memory())?;

    // --- 2. Register types and property keys ---
    // We create a small schema: Entity (base), Person (extends Entity),
    // Organization (extends Entity), and a KNOWS edge type.
    let mut wtx = db.write_txn()?;

    let entity_type = wtx.register_type(
        TypeDefinitionBuilder::node_type("Entity").open().build(),
    )?;
    let person_type = wtx.register_type(
        TypeDefinitionBuilder::node_type("Person")
            .supertype(entity_type)
            .open()
            .build(),
    )?;
    let org_type = wtx.register_type(
        TypeDefinitionBuilder::node_type("Organization")
            .supertype(entity_type)
            .open()
            .build(),
    )?;
    let knows_type = wtx.register_type(
        TypeDefinitionBuilder::edge_type("KNOWS").build(),
    )?;
    let works_at_type = wtx.register_type(
        TypeDefinitionBuilder::edge_type("WORKS_AT").build(),
    )?;

    // Register property keys.
    let name_key = wtx.get_or_create_property_key("name")?;
    let age_key = wtx.get_or_create_property_key("age")?;

    // --- 3. Insert nodes with properties ---
    let alice_id = wtx.insert_node(
        NodeBuilder::new()
            .type_label(person_type)
            .property(name_key, Value::String("Alice".into()))
            .property(age_key, Value::I64(30))
            .build(),
    )?;

    let bob_id = wtx.insert_node(
        NodeBuilder::new()
            .type_label(person_type)
            .property(name_key, Value::String("Bob".into()))
            .property(age_key, Value::I64(25))
            .build(),
    )?;

    let carol_id = wtx.insert_node(
        NodeBuilder::new()
            .type_label(person_type)
            .property(name_key, Value::String("Carol".into()))
            .property(age_key, Value::I64(35))
            .build(),
    )?;

    let acme_id = wtx.insert_node(
        NodeBuilder::new()
            .type_label(org_type)
            .property(name_key, Value::String("Acme Corp".into()))
            .build(),
    )?;

    // --- 4. Insert edges ---
    wtx.insert_edge(
        EdgeBuilder::new(alice_id, bob_id)
            .type_label(knows_type)
            .build(),
    )?;
    wtx.insert_edge(
        EdgeBuilder::new(alice_id, carol_id)
            .type_label(knows_type)
            .build(),
    )?;
    wtx.insert_edge(
        EdgeBuilder::new(bob_id, carol_id)
            .type_label(knows_type)
            .build(),
    )?;
    wtx.insert_edge(
        EdgeBuilder::new(alice_id, acme_id)
            .type_label(works_at_type)
            .build(),
    )?;

    wtx.commit()?;

    // --- 5. Query the graph ---
    let rtx = db.read_txn()?;

    // Count totals
    println!("Total nodes: {}", rtx.node_count()?);
    println!("Total edges: {}", rtx.edge_count()?);

    // Query all Person nodes (direct type match)
    let people = rtx.nodes_by_type(person_type, false)?;
    println!("\nPerson nodes ({}):", people.len());
    for person in &people {
        let name = person
            .properties
            .get(&name_key)
            .and_then(|v| v.as_str())
            .unwrap_or("(unnamed)");
        let age = person
            .properties
            .get(&age_key)
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        println!("  {} (age {})", name, age);
    }

    // Query all Entity nodes including subtypes — should include both
    // Person and Organization nodes.
    let all_entities = rtx.nodes_by_type(entity_type, true)?;
    println!(
        "\nAll Entity nodes (include_subtypes=true): {}",
        all_entities.len()
    );

    // --- 6. Traverse edges ---
    println!("\nAlice's outgoing KNOWS edges:");
    let alice_knows = rtx.outgoing_edges(alice_id, Some(knows_type))?;
    for edge in &alice_knows {
        if let Some(target) = rtx.get_node(edge.target)? {
            let name = target
                .properties
                .get(&name_key)
                .and_then(|v| v.as_str())
                .unwrap_or("(unnamed)");
            println!("  Alice --KNOWS--> {}", name);
        }
    }

    // Find neighbors (nodes reachable via outgoing edges)
    let alice_neighbors = rtx.neighbors(alice_id, None)?;
    println!(
        "\nAlice's neighbors (all edge types): {}",
        alice_neighbors.len()
    );

    // Incoming edges: who knows Carol?
    println!("\nIncoming KNOWS edges to Carol:");
    let carol_known_by = rtx.incoming_edges(carol_id, Some(knows_type))?;
    for edge in &carol_known_by {
        if let Some(source) = rtx.get_node(edge.source)? {
            let name = source
                .properties
                .get(&name_key)
                .and_then(|v| v.as_str())
                .unwrap_or("(unnamed)");
            println!("  {} --KNOWS--> Carol", name);
        }
    }

    rtx.finish();

    println!("\nDone.");
    Ok(())
}
