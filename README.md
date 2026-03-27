# Phonograph

An embedded graph database with extensible schema, pluggable constraint validation, and pluggable inference hooks.

## Overview

Phonograph is an embedded typed property graph database in Rust. It stores typed nodes and edges with arbitrary properties in a single file (or in memory), providing ACID transactions with snapshot isolation.

It is not an ontology engine — it is the layer *beneath* one. The crate provides **mechanism** for types, constraints, and inference but does not prescribe which types, constraints, or inference rules exist. There are no built-in OWL, RDF, or SKOS vocabularies. You define your domain model by registering types, implementing constraint validators, and implementing inference rules.

Target audience: developers building knowledge graphs, ontology systems (OWL, SKOS, custom models), or typed graph applications that need an embedded storage engine with extensibility hooks.

## Workspace Structure

| Crate | Purpose | `no_std`? |
|-------|---------|-----------|
| [`phonograph`](crates/phonograph/) | Graph vocabulary: core types, traits, errors | yes (`no_std + alloc`) |
| [`phonograph_db`](crates/phonograph_db/) | Database engine: storage, B+ trees, transactions, buffer pool | yes (`no_std + alloc`) |
| [`phonograph_std`](crates/phonograph_std/) | OS/platform layer: `FileBackend`, file locking, convenience API | no (always `std`) |

Most users should depend on `phonograph_std`. For `no_std` environments, depend on `phonograph_db` directly.

## Features

- **Typed property graph** — typed nodes with properties, typed directed edges with properties
- **Full persistence with crash safety** — single-file format, dual-superblock atomic commit protocol
- **MVCC concurrency** — single-writer, multiple-reader, snapshot isolation
- **Extensible type/schema system** — user-defined node types, edge types, type hierarchies, property declarations
- **Pluggable constraint validation** — implement the `ConstraintValidator` trait to enforce domain rules at commit time
- **Pluggable inference hooks** — implement the `InferenceRule` trait to derive new facts, with materialized and ephemeral modes
- **`no_std + alloc` database engine** — the core engine runs on bare metal with a heap allocator
- **In-memory backend** — for testing or non-persistent use cases, with optional snapshot-to-disk
- **Pure Rust** — no external database dependencies; the entire storage engine is implemented from scratch
- **Explicit transaction model** — `read_txn()` / `write_txn()` / `commit()`

## Quick Start

Add `phonograph_std` to your `Cargo.toml`:

```toml
[dependencies]
phonograph_std = "0.1"
```

```rust
use phonograph_std::db::{NodeBuilder, TypeDefinitionBuilder};
use phonograph_std::error::Error;
use phonograph_std::types::Value;

fn main() -> Result<(), Error> {
    // Open an in-memory database
    let db = phonograph_std::open_in_memory()?;

    // Register a type and property key, then insert a node
    let (person_type, name_key) = {
        let mut wtx = db.write_txn()?;
        let person = wtx.register_type(
            TypeDefinitionBuilder::node_type("Person").build(),
        )?;
        let name = wtx.get_or_create_property_key("name")?;
        wtx.insert_node(
            NodeBuilder::new()
                .type_label(person)
                .property(name, Value::String("Alice".into()))
                .build(),
        )?;
        wtx.commit()?;
        (person, name)
    };

    // Query it back
    let rtx = db.read_txn()?;
    let people = rtx.nodes_by_type(person_type, false)?;
    assert_eq!(people.len(), 1);
    assert_eq!(
        people[0].properties.get(&name_key),
        Some(&Value::String("Alice".into())),
    );
    Ok(())
}
```

## Architecture

```text
+-------------------------------------+
|  Application / Downstream Crate     |
+-------------------------------------+
|  phonograph_std (convenience API)   |
+-------------------------------------+
|  phonograph_db (database engine)    |
+-------------------------------------+
|  phonograph (graph vocabulary)      |
+-------------------------------------+
|  Storage Backend Traits (HAL)       |
+--------------+----------------------+
|  FileBackend |  MemoryBackend       |
+--------------+----------------------+
```

- **phonograph** — Core types (`Node`, `Edge`, `Value`, `TypeDefinition`), traits (`ConstraintValidator`, `InferenceRule`, `GraphView`), vocabulary error types
- **phonograph_db** — Copy-on-Write B+ trees, buffer pool, page allocator, dual-superblock atomic commit, `Database<B>` generic over storage backend
- **phonograph_std** — `FileBackend` with OS file locking, `AnyBackend` enum dispatch, `open()` / `open_in_memory()` convenience constructors

## Extension System

Build ontology layers on top of the crate:

1. **Register custom types** to model your domain (e.g., `owl:Class`, `rdfs:subClassOf`)
2. **Implement `ConstraintValidator`** to enforce domain rules (e.g., cardinality constraints, required properties)
3. **Implement `InferenceRule`** to derive new facts (e.g., subclass propagation, inverse edges)

See [`crates/phonograph_std/examples/owl_lite_ontology.rs`](crates/phonograph_std/examples/owl_lite_ontology.rs) for a complete worked example building a minimal OWL Lite ontology layer.

## `no_std` Usage

The database engine (`phonograph_db`) compiles under `no_std + alloc`:

```toml
[dependencies]
phonograph_db = { version = "0.1", default-features = false }
```

```rust
use phonograph_db::{Database, DatabaseConfig};
use phonograph_db::backend_mem::MemoryBackend;

let db = Database::create(MemoryBackend::new(), DatabaseConfig::default())?;
```

## Known Limitations

- `nodes_by_property()` performs a full scan (no property value index in v0.1)
- Query methods return owned `Vec`s (no streaming iterator API)
- No batch insert API
- `write_txn()` blocks indefinitely when another write transaction is active (no timeout)

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT License ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.
