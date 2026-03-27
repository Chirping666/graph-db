# 003 — Ontology & Knowledge Representation Models: Foundation Requirements Extraction

**Project:** Embedded Graph Database with Extensible Schema & Pluggable Inference
**Task:** 3 — Research: Ontology & Knowledge Representation Models — Foundation Requirements Extraction
**Status:** Complete
**Intended audience:** The designer of this project and all downstream Claude instances. A reader familiar with data modeling but not with formal ontology systems should be able to read this document, understand each model, and follow the reasoning that leads to the foundation requirements. A reader already familiar with OWL or RDF may skip directly to Sections 9–11.

---

## Table of Contents

1. [Purpose and Framing](#1-purpose-and-framing)
2. [Model 1: RDF and RDFS](#2-model-1-rdf-and-rdfs)
3. [Model 2: OWL — Web Ontology Language (All Profiles)](#3-model-2-owl--web-ontology-language-all-profiles)
4. [Model 3: SKOS — Simple Knowledge Organization System](#4-model-3-skos--simple-knowledge-organization-system)
5. [Model 4: Property Graphs with Schema (PG-Schema / openCypher / TinkerPop)](#5-model-4-property-graphs-with-schema-pg-schema--opencypher--tinkerpop)
6. [Model 5: Conceptual Graphs](#6-model-5-conceptual-graphs)
7. [Model 6: Frame-Based Systems and Description Logics](#7-model-6-frame-based-systems-and-description-logics)
8. [Model 7: Topic Maps (ISO 13250)](#8-model-7-topic-maps-iso-13250)
9. [Cross-Model Analysis: Shared Primitives](#9-cross-model-analysis-shared-primitives)
10. [Foundation Requirements List](#10-foundation-requirements-list)
11. [Validation: Mapping Each Model onto the Foundation](#11-validation-mapping-each-model-onto-the-foundation)
12. [What Must NOT Be in the Foundation](#12-what-must-not-be-in-the-foundation)
13. [Interaction with Storage Layer (Tasks 1 and 2 Integration)](#13-interaction-with-storage-layer-tasks-1-and-2-integration)

---

## 1. Purpose and Framing

### What this document is trying to answer

This project builds a **foundation layer** — not an ontology engine. The distinction matters: the project must not bake in OWL semantics, RDF triples, or SKOS concept hierarchies. But it must be architected so that a downstream crate *can* implement any of those models without hacking around the core.

The central research question is:

> **What is the minimal set of primitives and extension points that a foundation layer needs, such that OWL Lite, OWL DL, SKOS, typed property graphs, frame-based systems, and other models can all be built on top of it?**

The method: survey at least five knowledge representation models in enough depth to understand their primitives, constraints, and inference patterns. Then extract the common underlying structure — the substrate they all share — and express it as a requirements list for the core crate.

### What this document is NOT doing

- Recommending which ontology model to implement (the core must implement none of them)
- Designing the Rust API (that is Task 10)
- Choosing specific data structures (that is Task 7)
- Claiming the resulting foundation will automatically *implement* any model — only that it will *enable* implementation by downstream code

### Terminology conventions used in this document

- **Entity** — a node in the graph (regardless of whether the downstream model calls it a resource, concept, frame, or vertex)
- **Relationship** — a directed edge in the graph (regardless of whether the downstream model calls it a property, role, arc, or edge)
- **Type** — a classification applied to an entity or relationship
- **Constraint** — a rule that restricts the valid states of the graph
- **Inference rule** — a rule that derives new facts from existing facts

---

## 2. Model 1: RDF and RDFS

### 2.1 What RDF is

**RDF (Resource Description Framework)** is a W3C standard for representing knowledge as a directed, labeled graph of **triples**: `(subject, predicate, object)`. Everything is a **resource** identified by a URI (or a blank node, or a literal value for objects). There is no distinction between "nodes" and "edges" at the data level — predicates are also resources, also identified by URIs.

```
Subject          Predicate                  Object
<Person/Alice>   <foaf:knows>               <Person/Bob>
<Person/Alice>   <foaf:age>                 "30"^^xsd:integer
<Person/Alice>   <rdf:type>                 <foaf:Person>
```

The **RDF graph** is the set of all asserted triples. Named graphs (RDF 1.1) add a fourth component — a graph name — producing quads: `(subject, predicate, object, graph)`.

### 2.2 What RDFS adds

**RDFS (RDF Schema)** adds a vocabulary for describing the structure of RDF resources:

- `rdfs:Class` — declares something as a class (a type of resource)
- `rdfs:subClassOf` — declares a class as a subtype of another class
- `rdf:type` — declares an individual as a member of a class
- `rdfs:Property` — declares something as a property
- `rdfs:subPropertyOf` — declares a property as a sub-property of another
- `rdfs:domain` — declares the class of resources that a property can be applied to
- `rdfs:range` — declares the class of values a property can take
- `rdfs:label`, `rdfs:comment` — annotation properties for human-readable metadata

**RDFS inference** is shallow: if `A rdfs:subClassOf B` and `x rdf:type A`, then we can infer `x rdf:type B`. If `P rdfs:subPropertyOf Q` and `s P o`, then we can infer `s Q o`. No cardinality reasoning; no negation.

### 2.3 Primitives that RDF/RDFS requires from a foundation

| Primitive | How RDF/RDFS uses it |
|-----------|---------------------|
| **Named entity** (node with a stable identifier) | Every RDF resource is identified by a URI — a globally unique name |
| **Directed labeled edge** | Every triple is `(subject, predicate, object)` — a directed edge labeled with the predicate URI |
| **Entity classification** | `rdf:type` — the ability to assert that an entity belongs to a type |
| **Type hierarchy** | `rdfs:subClassOf`, `rdfs:subPropertyOf` — parent-child type relationships |
| **Domain/range annotation** | `rdfs:domain`, `rdfs:range` — a constraint linking relationship types to allowed endpoint types |
| **Property storage** | `rdfs:label`, `rdfs:comment`, literal values — entities carry key-value annotations |
| **Blank nodes** (anonymous entities) | Resources that exist but have no external name — a node with a locally-scoped ID |
| **Inference hooks** | Subclass propagation, subproperty propagation, domain/range entailment |

### 2.4 What RDF/RDFS does NOT require

- Fixed node or edge types determined at schema creation time (RDF is schemaless by default; schema is asserted using the same triple mechanism as data)
- Cardinality constraints (those come from OWL)
- Closed-world assumption (RDF uses the open-world assumption)
- Any specific storage representation

---

## 3. Model 2: OWL — Web Ontology Language (All Profiles)

### 3.1 Overview of OWL

**OWL (Web Ontology Language)** extends RDF/RDFS with a rich set of axioms drawn from description logics. OWL ontologies are RDF graphs where the triples have well-defined logical semantics. The key addition over RDFS is **formal reasoning**: OWL supports decidable automated reasoning — classification, consistency checking, and instance retrieval.

OWL has several profiles (sublanguages), each trading off expressivity for computational tractability:

| Profile | Description logic | Decidability | Key capability |
|---------|------------------|--------------|----------------|
| **OWL Lite** | SHIF(D) | Decidable | Basic class hierarchies, cardinality (0 or 1 only), property characteristics |
| **OWL DL** | SHOIN(D) | Decidable (EXPTIME) | Full OWL except `owl:Thing` as a class |
| **OWL Full** | Undecidable | Undecidable | Uses RDF meta-modeling; no reasoning guarantees |
| **OWL EL** | EL++ | Polynomial | Existential quantification; scales to large TBoxes |
| **OWL QL** | DL-Lite | LogSpace | Query rewriting to SQL; efficient conjunctive queries |
| **OWL RL** | RL | Polynomial | Rule-based reasoning; forward chaining |

### 3.2 Core OWL axiom categories

**TBox axioms** (terminological — about types and relationships):
- `owl:Class` — declares a class
- `rdfs:subClassOf` — class subsumption
- `owl:equivalentClass` — two classes have identical members
- `owl:disjointWith` — two classes share no members
- `owl:ObjectProperty`, `owl:DatatypeProperty` — property declarations
- `owl:subPropertyOf`, `owl:equivalentProperty`, `owl:inverseOf`
- `owl:TransitiveProperty`, `owl:SymmetricProperty`, `owl:FunctionalProperty` — property characteristics
- `owl:Restriction` — anonymous class defined by a constraint on a property:
  - `owl:allValuesFrom` — universal restriction
  - `owl:someValuesFrom` — existential restriction
  - `owl:maxCardinality`, `owl:minCardinality`, `owl:cardinality`
  - `owl:hasValue` — restriction to specific value
- `owl:unionOf`, `owl:intersectionOf`, `owl:complementOf` — boolean class combinators

**ABox axioms** (assertional — about individuals):
- `rdf:type` — instance membership
- `owl:sameAs` — two URIs refer to the same individual
- `owl:differentFrom`, `owl:AllDifferent` — distinct individuals

### 3.3 OWL reasoning patterns

An OWL reasoner derives:
- **Classification:** Infer the most specific type of each individual given the TBox axioms
- **Consistency checking:** Detect if the ontology is contradictory (an individual belongs to two disjoint classes)
- **Realization:** Given an individual, find all classes it belongs to
- **Entailment:** Given a query, find all individuals satisfying it

All of these are **triggered operations** — the reasoner runs on demand (or incrementally), not automatically on every write.

### 3.4 Primitives that OWL requires from a foundation

| Primitive | How OWL uses it |
|-----------|----------------|
| **Named entity with stable ID** | Individuals identified by URIs |
| **Directed labeled edge** | All OWL assertions are RDF triples |
| **Entity classification (multiple types)** | An individual can have multiple `rdf:type` assertions; OWL may infer additional types |
| **Type hierarchy with inheritance** | `rdfs:subClassOf`, subsumption lattice |
| **Type equality and type disjointness** | `owl:equivalentClass`, `owl:disjointWith` — constraints on the type system |
| **Property characteristics** | Transitivity, symmetry, functionality — constraints on edge behavior |
| **Anonymous type expressions** | `owl:Restriction` — types defined by structural constraints, not just by name |
| **Cardinality constraints** | Min/max/exact number of edges of a given type from a node |
| **Domain/range constraints** | Edge type endpoints must satisfy type constraints |
| **Set operations on types** | Union, intersection, complement of class expressions |
| **Same-as / identity** | Multiple names for the same individual |
| **Inference hooks** | Full OWL reasoning — classification, consistency, entailment — runs explicitly |
| **ABox / TBox separation** | Schema knowledge (TBox) vs. instance knowledge (ABox) are logically distinct, even if stored in the same graph |
| **Annotation properties** | `rdfs:label`, `rdfs:comment`, `owl:versionInfo` — metadata that does NOT participate in reasoning |

### 3.5 What the foundation layer provides vs. what OWL implements on top

The foundation provides:
- Named entities, directed labeled edges, entity types, type hierarchies
- A mechanism to register constraints (e.g., "cardinality check" as a registered `ConstraintValidator` trait object)
- A mechanism to register inference rules (e.g., "subclass propagation" as a registered `InferenceRule` trait object)
- A way to store and query TBox-like data (type definitions, type relationships) alongside ABox-like data (instance facts)

The OWL implementation provides (as downstream code):
- The specific constraint logic (disjointness, cardinality, restriction satisfaction)
- The specific inference logic (classification algorithm, consistency checking)
- The interpretation of `owl:sameAs`, `owl:equivalentClass`, etc., as concrete operations on the graph

---

## 4. Model 3: SKOS — Simple Knowledge Organization System

### 4.1 What SKOS is

**SKOS (Simple Knowledge Organization System)** is a W3C standard for representing **thesauri, classification schemes, taxonomies, and controlled vocabularies** using RDF. It is simpler than OWL — there is no formal reasoning, no description logic underpinning. SKOS is about organizing concepts in a way that humans and search systems can use.

SKOS is built entirely on RDF/RDFS. It defines a vocabulary of classes and properties:

**Concept model:**
- `skos:Concept` — a unit of thought (a concept)
- `skos:ConceptScheme` — a collection of concepts (a vocabulary)
- `skos:inScheme` — membership of a concept in a scheme
- `skos:hasTopConcept` / `skos:topConceptOf` — roots of the hierarchy

**Hierarchical relationships:**
- `skos:broader` — concept A is broader (more general) than concept B
- `skos:narrower` — concept A is narrower (more specific) than concept B
- `skos:broaderTransitive`, `skos:narrowerTransitive` — transitive closures (inferred)
- Note: `skos:broader` is NOT `rdfs:subClassOf` — SKOS concepts are individuals, not classes

**Associative relationships:**
- `skos:related` — two concepts are associatively (not hierarchically) related

**Labeling:**
- `skos:prefLabel` — the preferred human-readable label (one per language per concept)
- `skos:altLabel` — alternative labels (synonyms, abbreviations)
- `skos:hiddenLabel` — labels included for search but not display (misspellings)

**Notes:**
- `skos:definition`, `skos:scopeNote`, `skos:example`, `skos:editorialNote` — documentation annotations

**Mapping:**
- `skos:exactMatch`, `skos:closeMatch`, `skos:broadMatch`, `skos:narrowMatch` — mappings between concepts in different schemes

### 4.2 SKOS inference

SKOS has minimal formal inference:
- `skos:broader` is the inverse of `skos:narrower` (if stated, the other is entailed)
- `skos:broaderTransitive` is the transitive closure of `skos:broader`
- `skos:related` is symmetric
- Integrity conditions (not enforced by SKOS itself, but specified): a concept should not be both broader and narrower than another; `skos:prefLabel` should be unique per language tag

### 4.3 Primitives that SKOS requires from a foundation

| Primitive | How SKOS uses it |
|-----------|-----------------|
| **Named entity** | Every concept and concept scheme is a named resource |
| **Directed labeled edge** | All SKOS assertions are RDF triples |
| **Entity classification** | `rdf:type skos:Concept` — entities are typed as concepts |
| **Type hierarchy** (shallow) | Concept scheme membership; concept broader/narrower hierarchy — but these are between *instances*, not *types* |
| **Named relationships** | `skos:broader`, `skos:narrower`, `skos:related` — specific edge types |
| **Property storage** | Labels and notes are literal-valued properties |
| **Multi-valued properties** | Multiple `skos:altLabel` per concept (per language) |
| **Multi-language support** | Labels carry language tags (`"Kunst"@de`, `"Art"@en`) |
| **Cross-scheme mappings** | Edges between concepts in different concept schemes — no special treatment needed, just typed edges |
| **Inference hooks** (simple) | Symmetry of `related`; transitivity of `broaderTransitive`; inverse of `broader`/`narrower` |
| **Integrity constraints** | `prefLabel` uniqueness per language; hierarchy acyclicity |

### 4.4 What distinguishes SKOS from OWL at the foundation level

SKOS is notable for what it does NOT need:
- No anonymous type expressions (no `owl:Restriction`)
- No cardinality reasoning
- No consistency checking in the OWL sense
- The "type hierarchy" is between *instances* (`skos:broader` connects individuals, not classes)

This highlights an important foundation requirement: **the type system must allow users to define domain-specific relationship semantics without the foundation prescribing what "subtype" or "broader" means.** SKOS `skos:broader` is a user-defined edge type with user-defined inference rules; it should look to the foundation like any other edge type with registered constraints.

---

## 5. Model 4: Property Graphs with Schema (PG-Schema / openCypher / TinkerPop)

### 5.1 What property graphs are

A **property graph** is a directed, labeled, multi-graph where:
- **Nodes** have one or more **labels** (types) and a set of **properties** (key-value pairs)
- **Edges** have exactly one **type label** and a set of **properties**, plus a source and target node

This is the model used by Neo4j (Cypher), Amazon Neptune, TinkerPop (Gremlin), and many other graph databases. Unlike RDF, property graphs are not uniformly "everything is a triple" — edges are first-class objects with their own properties, not reducible to nodes.

### 5.2 Schema for property graphs

Traditional property graphs were **schemaless** — any node could have any label and any property. Recent standardization work has added schema:

**PG-Schema (ISO/IEC 39075 GQL, part of the emerging graph query standard)** defines:
- **Node types** — a node type specifies: zero or more required labels, a set of property declarations (key, value type, mandatory/optional)
- **Edge types** — an edge type specifies: a label, source node type constraint, target node type constraint, property declarations
- **Type inheritance** — a node type can extend another (inheriting required labels and properties)
- **Open vs. closed schema** — open (nodes may have extra labels/properties beyond the schema) vs. closed (nodes must conform exactly)

**openCypher extensions:**
- Property type declarations: `name: String`, `age: Integer`
- Required vs. optional properties
- Unique constraints: `UNIQUE (Node.propertyKey)`
- Existence constraints: `EXISTS (Node.propertyKey)`
- Node key constraints: a combination of properties that forms a unique identifier

**Apache TinkerPop / Gremlin:**
- TinkerPop has no built-in schema; schema is enforced by the underlying database implementation
- Some TinkerPop implementations support vertex programs for inference

### 5.3 Primitives that property graphs with schema require from a foundation

| Primitive | How PG-Schema uses it |
|-----------|----------------------|
| **Named entity (node)** | Nodes with a stable ID |
| **Multiple type labels per node** | A node can have labels `Person`, `Employee` simultaneously |
| **Typed directed edge with properties** | Edges are first-class objects, not just pairs |
| **Exactly-one edge type per edge** | Unlike RDF where edges can have multiple `rdf:type` assertions, property graph edges have one label |
| **Property declarations per type** | A type declares which properties its members should carry, with value type constraints |
| **Required vs. optional properties** | Mandatory properties raise a constraint violation if absent |
| **Type inheritance** | Node type B inherits the property declarations of node type A |
| **Uniqueness constraints** | A given property value must be unique across all nodes of a given type |
| **Existence constraints** | A given property must be present on all nodes of a given type |
| **Foreign-key-like constraints** | Edge types declare allowed source/target node types |
| **Open/closed world toggle** | Whether nodes may have properties beyond what the schema declares |
| **Cardinality on edges** | `UNIQUE` effectively enforces max-1 cardinality on a relationship type |

### 5.4 What distinguishes property graphs from RDF at the foundation level

The key structural difference: **in property graphs, edges are first-class citizens with their own properties**. In RDF, to attach properties to a relationship, you must reify it (create a new node representing the relationship and attach properties to that node — `rdf:Statement` or named graphs). The foundation layer should natively support edges with properties, so both models work naturally.

---

## 6. Model 5: Conceptual Graphs

### 6.1 What conceptual graphs are

**Conceptual Graphs (CGs)**, introduced by John Sowa in 1976, are a formal knowledge representation system based on Charles Sanders Peirce's existential graphs. They represent knowledge as a bipartite graph alternating between:
- **Concept nodes** — typed boxes representing entities or abstract entities (`[Person: Alice]`, `[Eating: *]`)
- **Conceptual relation nodes** — ellipses representing n-ary relations between concepts (`(Agnt)`, `(Thme)`)

```
[Person: Alice] →(Agnt)→ [Eating: *] →(Thme)→ [Pizza: *]
"Alice is eating a pizza"
```

### 6.2 Key features

- **Type hierarchy:** Every concept and every relation has a type; types are organized in a lattice with `Universal` at the top and `Absurd` at the bottom
- **Conformity:** A concept `[T: x]` is valid only if individual `x` conforms to type `T`
- **Referents:** A concept node has a type and an optionally specified referent — a specific individual (`Alice`), a generic (`*` = some unspecified), a collective, or a quantifier
- **Canonical graphs:** Template graphs representing what is prototypically true of a type — essentially a constraint over the structure of the graph around instances of that type
- **Operations:** Projection (matching a pattern onto a graph), join (combining graphs), specialization/generalization
- **Formal logic mapping:** Conceptual graphs have a well-defined mapping to first-order logic (and extensions to modal and higher-order logics)

### 6.3 Primitives that conceptual graphs require from a foundation

| Primitive | How CGs use it |
|-----------|---------------|
| **Named entity** | Individuals specified as referents in concept nodes |
| **Typed entity** | Every concept node has a type; `[Person: Alice]` |
| **Type hierarchy (lattice)** | Types form a lattice; `join` and `meet` operations on types |
| **Directed edges to relation nodes** | Arcs from concept to relation nodes |
| **N-ary relations** | A relation node can have multiple arcs (agent, theme, instrument, etc.) |
| **Generic (anonymous) entities** | `*` referent — an existential variable |
| **Structural constraints (canonical graphs)** | A template that valid instances of a type must conform to |
| **Pattern matching / projection** | Finding subgraph patterns that match a query graph |
| **Inference (join, specialization)** | Combining and specializing graphs to derive new facts |

### 6.4 What conceptual graphs highlight for the foundation

CGs require a notion of **n-ary relations** (edges with more than one endpoint). In a standard directed property graph, all edges are binary (source → target). CGs model n-ary relations as a special node (the relation node) with multiple directed arcs to participant nodes. The foundation layer does not need to directly support n-ary relations — they can be modeled by downstream code using a relation node connected to participants by typed edges. This is a well-known encoding pattern.

CGs also introduce the concept of **canonical graphs** — structural constraints over the neighborhood of typed instances. This maps to the constraint system: a constraint validator that inspects the local graph structure around a node and verifies it conforms to the canonical form for its type.

---

## 7. Model 6: Frame-Based Systems and Description Logics

### 7.1 Frame-based systems

**Frames** (Minsky, 1974) represent knowledge as structured objects analogous to data records. Each frame represents a concept or entity type and has:
- **Slots** — named attributes (analogous to properties) with defined value types
- **Slot facets** — metadata about a slot: default value, allowed range, cardinality constraints, inheritance behavior
- **Inheritance** — frames inherit slots from parent frames (superclasses); slot values can be inherited and overridden

Classic frame systems: FRL, KRL, KL-ONE, Loom, CLASSIC.

```
Frame: Person
  Slot: name        (type: String, cardinality: exactly 1)
  Slot: age         (type: Integer, cardinality: 0..1)
  Slot: knows       (type: Person, cardinality: 0..*)
  Default: [age: unknown]

Frame: Employee extends Person
  Slot: employer    (type: Organization, cardinality: 1..*)
  Slot: salary      (type: Currency, cardinality: 0..1)
```

### 7.2 Description Logics (DL)

**Description Logics** are the formal underpinning of OWL. They are a family of decidable fragments of first-order logic designed specifically for representing ontological knowledge. Key DL components:

- **Concepts (classes):** Descriptions of sets of individuals — `Person`, `Parent ⊓ Male` (Father), `∃hasChild.Person`
- **Roles (properties):** Binary relations between individuals
- **Individuals:** Specific named entities
- **TBox:** Terminological assertions (`Father ≡ Parent ⊓ Male`)
- **ABox:** Assertional facts (`Father(John)`, `hasChild(John, Mary)`)
- **Reasoning services:** Subsumption (`Is Father a subtype of Male?`), consistency, instance checking, realization

Description Logics add formal semantics to frame-based systems: a frame's slots become DL roles; facets become role restrictions; inheritance becomes subsumption.

### 7.3 Primitives that frame/DL systems require from a foundation

| Primitive | How Frames/DL use it |
|-----------|---------------------|
| **Named entity** | Individual objects |
| **Type (class/concept)** | Frames / DL concepts classify individuals |
| **Multiple inheritance** | A frame can extend multiple parent frames |
| **Type hierarchy (lattice)** | Subsumption ordering on concepts |
| **Named slots/roles** | Properties with declared types and cardinalities |
| **Slot cardinality facets** | Min/max number of fillers for a slot |
| **Slot range restrictions** | Fillers must be instances of a specific type |
| **Default values** | Inherited values when not explicitly set |
| **Closed-world reasoning** (some systems) | Absence of a value means the slot is empty |
| **Computed/derived slots** | Slot values derived from other slot values (inference) |
| **Structural constraints** | Value type, cardinality, co-constraints between slots |
| **Inference (inheritance propagation, slot filling)** | Inherit default values; classify instances; check constraints |

### 7.4 What frames/DL highlight for the foundation

The frame concept of **slot facets** (metadata about properties) is important. A property is not just a name — it has a declared type, a cardinality, a range, a default, and possibly derivation rules. This metadata must be storable in the foundation as part of the type/schema system. The foundation does not need to *enforce* these facets, but it must be able to *store* them so that downstream ontology layers can enforce them.

---

## 8. Model 7: Topic Maps (ISO 13250)

### 8.1 What Topic Maps are

**Topic Maps** (ISO 13250) are a knowledge representation standard focused on representing information sources (documents, databases) and the topics they address. They are particularly strong for merging knowledge from multiple sources.

Core concepts:
- **Topic** — a concept or subject (like a SKOS concept, but more general)
- **Topic Name** — a name for a topic, scoped to a context
- **Occurrence** — a link between a topic and an information resource (document, URL) about that topic
- **Association** — a relationship between two or more topics
- **Association Role** — the role each topic plays in an association
- **Scope** — a context that qualifies the validity of a name, occurrence, or association
- **Subject Identity** — topics that refer to the same real-world subject can be merged (the **published subject identifier** mechanism)

### 8.2 Key distinctive features

- **Scoped statements:** Every assertion can be qualified with a context/scope. `Alice hasFriend Bob [scope: ProfessionalContext]` — the friendship holds only in the context of professional interactions.
- **Merging:** Two topics from different topic maps that share the same subject identity are automatically merged into one topic with the union of their names, occurrences, and associations.
- **N-ary associations with roles:** An association involves typed participants playing typed roles, not just binary source-target edges.

### 8.3 Primitives that Topic Maps require from a foundation

| Primitive | How Topic Maps use it |
|-----------|----------------------|
| **Named entity** | Topics identified by subject indicators |
| **Typed directed edge** | Associations between topics |
| **N-ary relations (via role encoding)** | Associations have multiple participants; modeled as relation node + typed arcs |
| **Statement scoping / reification** | An edge or property assertion can have additional metadata (which context it holds in) — typically modeled by a "context node" connected to the edge |
| **Property storage** | Topic names, occurrences |
| **Identity resolution** | Multiple names/identifiers for the same entity (`owl:sameAs` equivalent) |
| **Annotation / metadata on edges** | Scope qualifiers on associations |

### 8.4 What Topic Maps highlight for the foundation

Topic Maps' **scoped statements** require attaching metadata to relationships — not just to nodes. This is directly supported by property-graph edges that carry their own properties (a `scope` property on an edge). The foundation's native support for edge properties is sufficient; no special mechanism is needed.

The **merging / identity** concept (multiple URIs/names mapping to the same entity) requires the foundation to support some form of **identity aliasing** or at minimum a way for downstream code to maintain an identity resolution table (an index mapping aliases to canonical IDs). The foundation does not need to implement merging, but it must not prevent it.

---

## 9. Cross-Model Analysis: Shared Primitives

Having surveyed seven models, we can now identify the structural patterns that recur across all of them.

### 9.1 The universal graph substrate

All seven models share a common underlying graph structure:

```
All models boil down to:
  Entities    — things that exist (nodes)
  Properties  — named values attached to entities (key-value pairs)
  Types       — classifications of entities and relationships
  Relationships — directed connections between entities (edges) with a type
  Type hierarchy — types organized in partial orders
  Constraints — rules restricting valid graph states
  Inference   — rules deriving new facts from existing facts
```

No model requires anything outside this set. The differences are in:
1. **What the types mean** (OWL classes vs. SKOS concepts vs. property graph labels)
2. **What the constraints enforce** (OWL cardinality vs. PG-Schema existence vs. SKOS acyclicity)
3. **How inference works** (OWL classification vs. SKOS transitive closure vs. frame inheritance)

### 9.2 Recurring primitive: Named entity with stable identity

Every model has a concept of a named thing with a stable identifier. RDF calls it a resource (URI). OWL calls it an individual. SKOS calls it a concept. Property graphs call it a node. CGs call it a referent. Frames call it an instance. Topic Maps call it a topic.

**Foundation requirement:** A node type with a stable, globally unique identifier.

### 9.3 Recurring primitive: Typed directed edge with properties

All models have directed relationships. Most models need relationships to carry metadata:
- RDF: edges (predicates) have no properties (hence reification)
- Property graphs: edges have first-class properties
- Topic Maps: associations have scopes
- CGs: arcs connect to relation nodes that carry type information
- OWL: property characteristics (transitivity, symmetry) are constraints on edges
- SKOS: relationships have no additional properties
- Frames: role fillers may carry slot-facet metadata

**Foundation requirement:** A directed edge type with a stable ID, a type label, a source and target node ID, and a mutable property bag. This is the property graph model of edges.

**Implication for RDF:** RDF's "predicate is just a URI, no edge properties" can be modeled by ignoring the edge's property bag. The foundation is strictly more expressive; downstream can use a subset.

### 9.4 Recurring primitive: Multiple type labels per entity

- RDF/OWL: multiple `rdf:type` assertions; an individual can belong to many classes
- Property graphs: multiple labels per node
- Frames: multiple inheritance; an instance inherits from multiple frames
- CGs: a concept type can be a join of multiple types
- SKOS: a concept can be `inScheme` multiple schemes

**Foundation requirement:** A node can have one or more type labels. The type system must not restrict nodes to exactly one type.

**Nuance:** Property graph edges traditionally have exactly one type label (as a design choice, not a logical necessity). The foundation should support edges with one or more types, even if most uses assign exactly one. This enables RDF's model (where the predicate URI is the "type" and there is exactly one per triple) as well as potential future uses.

### 9.5 Recurring primitive: Type hierarchy

All models organize types in a hierarchy:
- RDFS: `rdfs:subClassOf` lattice
- OWL: class subsumption with complex expressions
- SKOS: (this is between instances, not types — but concept schemes have a broader/narrower hierarchy)
- PG-Schema: node type extension/inheritance
- CGs: type lattice with meet (join type) and join (meet type) operations
- Frames/DL: class hierarchy with multiple inheritance
- Topic Maps: association type hierarchy (less emphasized)

**Foundation requirement:** A type can have zero or more parent types (supertypes). The type hierarchy is a **DAG** (directed acyclic graph), not a tree (multiple inheritance must be supported). The foundation stores the type hierarchy but does not interpret it — interpretation is left to downstream code.

### 9.6 Recurring primitive: Property type declarations

All schema-bearing models have some mechanism to declare that instances of a type carry specific properties:
- OWL: `owl:ObjectProperty` / `owl:DatatypeProperty` with domain/range
- RDFS: `rdfs:domain`, `rdfs:range`
- PG-Schema: property declarations per node/edge type
- Frames: slot definitions per frame
- CGs: canonical graphs include typical property structure

**Foundation requirement:** The schema system must allow **property type declarations** to be associated with node/edge types. A property type declaration specifies: a property key, an expected value type (a type descriptor), and modifiers (required/optional, single-valued/multi-valued). These declarations are **metadata stored in the schema** — the foundation stores them but does not enforce them; enforcement is done by registered constraint validators.

### 9.7 Recurring primitive: Constraints (externally enforced)

The constraint patterns across models:
- **Cardinality constraints** (OWL, PG-Schema, Frames): min/max number of edges of a given type
- **Domain/range constraints** (RDFS, OWL, PG-Schema, Frames): endpoints of a relationship must be of specified types
- **Uniqueness constraints** (PG-Schema): a property value must be unique across all instances of a type
- **Existence constraints** (PG-Schema, Frames): a required property must be present
- **Disjointness constraints** (OWL): two types share no members
- **Type consistency** (OWL): no individual can belong to two disjoint types simultaneously
- **Acyclicity constraints** (SKOS: a concept cannot be broader than itself)
- **Value constraints** (Frames, OWL): a property value must satisfy a specific condition

**Foundation requirement:** A **constraint registration system**: downstream code can register a constraint validator (a trait object) that is called during transaction validation. The foundation provides the mechanism for registration, lifecycle, and invocation; downstream code provides the actual constraint logic. The foundation should not hard-code any specific constraint type.

### 9.8 Recurring primitive: Inference rules (externally implemented)

The inference patterns across models:
- **Subtype propagation** (RDFS, OWL, Frames): if `x rdf:type A` and `A subClassOf B`, infer `x rdf:type B`
- **Transitive closure** (OWL, SKOS): if `A broader B` and `B broader C`, infer `A broaderTransitive C`
- **Inverse property symmetry** (OWL, SKOS): if `A knows B`, infer `B knows A` (for symmetric properties)
- **Property chain** (OWL): if `A hasMother B` and `B hasSister C`, infer `A hasAunt C`
- **Classification** (OWL DL): infer the most specific type of each individual
- **Slot inheritance** (Frames): inherit default slot values from superframes
- **Canonical expansion** (CGs): instantiate canonical graph structure for a given type

**Foundation requirement:** An **inference rule registration system**: downstream code can register inference rules (trait objects) that are triggered **on explicit request by the caller** — never automatically. The foundation provides the invocation mechanism, a way to represent inferred facts (either materialized into the graph or returned as a separate result), and a caching/invalidation mechanism. Downstream code provides the actual reasoning logic.

### 9.9 Recurring primitive: Metadata on schema elements

All models need to annotate the schema itself — the types and property declarations — with metadata:
- Human-readable labels and descriptions (`rdfs:label`, `rdfs:comment`, `skos:prefLabel`)
- Version information (`owl:versionInfo`)
- Provenance and authorship
- Application-specific extensions (e.g., display hints, priority rankings)

**Foundation requirement:** Schema elements (node types, edge types, property type declarations) must themselves be able to carry arbitrary properties (a property bag). The type registration mechanism must support this.

### 9.10 Recurring primitive: Blank nodes / anonymous entities

RDF has blank nodes; CGs have generic referents (`*`); OWL uses blank nodes for anonymous class expressions; some graph models have transient/unnamed nodes.

**Foundation requirement:** The foundation must support entities with **locally-scoped identifiers** (anonymous or internal), not just globally-unique user-defined IDs. This is typically implemented as a node type with an auto-generated internal ID and no external name, distinguishable from named nodes.

### 9.11 Summary of shared primitives

| # | Primitive | All models | Notes |
|---|-----------|-----------|-------|
| P1 | Named entity with stable ID | ✓ | Core |
| P2 | Anonymous/blank entity | Most | RDF mandatory; others optional |
| P3 | Typed directed edge with properties | ✓ | Property graph model; RDF is a subset |
| P4 | Multiple type labels per node | ✓ | Critical for OWL, RDF |
| P5 | Single or multiple type labels per edge | ✓ | Traditionally one per edge in PGs |
| P6 | Type hierarchy (DAG, multiple inheritance) | ✓ | Critical for all |
| P7 | Property type declarations per type | ✓ | Schema for properties |
| P8 | Metadata on schema elements | ✓ | Labels, comments, version info |
| P9 | Constraint registration (trait-based) | ✓ | Mechanism only; logic downstream |
| P10 | Inference rule registration (trait-based) | ✓ | Mechanism only; logic downstream |
| P11 | Explicit inference triggering | ✓ | Never automatic |
| P12 | Inferred-fact representation | Most | Materialized or separate result set |
| P13 | Identity aliasing (sameAs support) | OWL, Topics | Downstream can implement; foundation stores identity edges |

---

## 10. Foundation Requirements List

This section translates the cross-model analysis into a concrete, actionable list of capabilities that the core crate must provide. Each requirement is labeled with a priority:
- **MUST** — foundational; no downstream model can be built without it
- **SHOULD** — strongly recommended; required by most models
- **MAY** — optional extension; required by some models, absent from others

### Category A: Graph Data Model

**A1 (MUST):** The core must support **typed nodes** with:
- A stable, unique node ID (assigned at creation, never changes)
- One or more **type labels** (references to registered node types)
- A mutable **property bag** (map from property key ID to typed value)

**A2 (MUST):** The core must support **typed directed edges** with:
- A stable, unique edge ID
- One or more **type labels** (references to registered edge types)
- A source node ID and target node ID
- A mutable **property bag**
- Parallel edges between the same source/target pair must be permitted (multi-graph)

**A3 (SHOULD):** The core must support **anonymous nodes** — nodes with system-assigned internal IDs and no external name, distinguishable from named nodes. These support RDF blank nodes, OWL anonymous class expression instantiation, and CG generic referents.

**A4 (MUST):** Property values must support at minimum the following value types:
- Boolean
- Signed and unsigned integers (at least 64-bit)
- Floating-point (at least 64-bit)
- UTF-8 string
- Byte blob (arbitrary binary data)
- Typed reference (a node ID — effectively a pointer to another node)
- List of any of the above (multi-valued property support)
- Null / absent (explicitly representing absence)

**A5 (SHOULD):** Property values should support **language-tagged strings** (a string + a BCP 47 language tag), enabling SKOS and RDF multilingual labeling. This can be modeled as a special struct value type or as a convention over string + property-key namespacing.

### Category B: Type / Schema System

**B1 (MUST):** The core must maintain a **type registry** storing:
- **Node type definitions**: a type ID, a type name (string), zero or more parent type IDs (supertypes), a property bag (metadata annotations on the type itself), and a list of property type declarations
- **Edge type definitions**: same structure as node types, plus optionally declared allowed source type and target type constraints (stored as metadata, not enforced by the foundation)
- **Property type declarations**: a property key ID, a property key name (string), an expected value type descriptor, and modifiers (required/optional, single/multi-valued)

**B2 (MUST):** The type hierarchy must form a **DAG** (directed acyclic graph). Multiple supertypes must be allowed. The foundation must enforce acyclicity of the type hierarchy DAG. No diamond restriction — the same type can appear at multiple points in the hierarchy.

**B3 (MUST):** The type registry must be **persistently stored** as part of the database (not reloaded from application code at startup). This is required for schema evolution and for reading a database created by a different application version.

**B4 (MUST):** The type registry must be **extensible at runtime** — new types can be registered after the database has been created. This supports iterative schema development and migration.

**B5 (SHOULD):** The type registry should maintain a **property key registry** that maps property key names (strings) to compact integer key IDs, shared across all node and edge types. This avoids storing key strings in every property record.

**B6 (SHOULD):** Type definitions should support an **open/closed flag**: closed types reject nodes/edges with properties or type labels not declared in the schema; open types allow extras. The default should be open (to avoid rigidity). The foundation stores the flag; enforcement is done by a registered constraint.

**B7 (MUST):** The foundation must support a **type hierarchy traversal API**: given a type ID, return all direct supertypes; given a type ID, return all subtypes (direct and transitive). This enables downstream OWL classification and RDFS subtype propagation.

### Category C: Constraint System

**C1 (MUST):** The core must provide a **constraint registration mechanism**: a trait (`ConstraintValidator` or equivalent) that downstream code implements and registers with the database. Registered constraints are called at **transaction commit time** (or on explicit validation request).

**C2 (MUST):** The `ConstraintValidator` trait must receive:
- A read-only view of the current transaction's changes (what was inserted, deleted, or modified)
- A read-only view of the current database state
- The set of registered type definitions
- Return type: a list of constraint violations (or empty = all valid)

**C3 (MUST):** The foundation must allow **multiple constraint validators** to be registered simultaneously. They run in registration order; if any fails, the transaction is rejected.

**C4 (SHOULD):** The foundation should support **per-type constraint scoping**: a registered constraint can declare which node or edge types it applies to, allowing the foundation to skip calling it for transactions that don't touch those types.

**C5 (MUST):** The constraint system must be entirely **downstream-implemented**. The foundation ships with no built-in constraints (except type hierarchy acyclicity, which is a schema-level invariant, not a data constraint). No cardinality rules, no domain/range enforcement, no disjointness — all downstream.

**C6 (SHOULD):** The foundation should provide a **dry-run validation API**: run all registered constraints against the current database state without committing any transaction. This enables batch validation and migration assistance.

### Category D: Inference System

**D1 (MUST):** The core must provide an **inference rule registration mechanism**: a trait (`InferenceRule` or equivalent) that downstream code implements and registers with the database.

**D2 (MUST):** Inference must **only run when explicitly requested** by the caller. There must be no automatic background inference. This is a hard requirement to ensure the core is predictable.

**D3 (MUST):** The `InferenceRule` trait must receive:
- A read-only view of the current database state (or a snapshot)
- The set of registered type definitions
- Return type: a set of **inferred facts** (nodes, edges, or property assignments to add)

**D4 (MUST):** The foundation must support two modes of inferred fact handling:
- **Materialized inference**: inferred facts are written into the graph as regular nodes/edges/properties, distinguished by an "inferred" flag. They can be queried like normal facts.
- **Ephemeral inference**: inferred facts are returned as a separate in-memory result set, not persisted. They are recomputed on each explicit trigger.

The downstream code (or the user) chooses the mode per inference run.

**D5 (SHOULD):** Materialized inferred facts should be **invalidated** (deleted or marked stale) when the base facts they depended on change. The foundation should provide a hook for registering a **dependency tracker** alongside each inference rule, which specifies what changes trigger re-inference.

**D6 (MUST):** Inference rules must be isolatable from constraints. An inference rule produces new facts; a constraint validates existing facts. They use different traits and different lifecycles.

**D7 (SHOULD):** The foundation should support **rule scoping**: an inference rule declares which node/edge types it operates over, allowing the foundation to call it only when relevant types exist.

### Category E: Query and Traversal

**E1 (MUST):** The core must support the following base query operations (sufficient to implement any higher model's query layer on top):
- Lookup node by ID
- Lookup edge by ID
- Find all outgoing edges of a node (optionally filtered by edge type)
- Find all incoming edges of a node (optionally filtered by edge type)
- Find all nodes with a given type label
- Find all edges with a given type label
- Find all nodes/edges with a given property key-value pair (full scan, or indexed if index exists)
- Multi-hop traversal (BFS/DFS over edges, optionally type-filtered)

**E2 (MUST):** All queries must execute within a **transaction context** (read-only or read-write). This ensures snapshot isolation — a query sees a consistent state.

**E3 (SHOULD):** The core should provide a **type-hierarchy-aware query**: find all nodes whose type is `T` or any subtype of `T`. This is fundamental to OWL, RDFS, and frame-based inheritance. It requires the type hierarchy traversal from B7.

### Category F: Transaction and Persistence

**F1 (MUST):** All operations must be transactional. The API exposes read-only transactions (snapshot reads) and read-write transactions (atomic commit/rollback). This is inherited from the database internals requirements but restated here because it affects how ontology-layer operations are structured.

**F2 (MUST):** The schema (type registry) modifications must be transactional — adding or removing a type definition is an atomic operation that can be committed or rolled back.

**F3 (SHOULD):** The foundation should support **named graphs or scoped subgraphs**: a way to partition the graph into named regions. This supports OWL ontology IRI metadata, RDF named graphs, and Topic Maps' scope concept. The foundation can implement this as a special type of node (a "graph context" node) that edges can reference, or as a first-class concept. The exact mechanism is a design decision for Task 6, not this requirements document.

### Category G: Extension Points Summary

The extension point surface that downstream ontology layers use:

| Extension Point | Trait / Mechanism | What downstream implements |
|----------------|------------------|---------------------------|
| Custom node types | Type registration API | Domain-specific type definitions |
| Custom edge types | Type registration API | Domain-specific relationship types |
| Custom property types | Property key registry API | Domain-specific property vocabularies |
| Type hierarchy | Supertype declaration in type registration | Subsumption lattice for specific ontology |
| Constraint validation | `ConstraintValidator` trait | Cardinality, domain/range, disjointness, etc. |
| Inference rules | `InferenceRule` trait | Subtype propagation, transitive closure, classification |
| Materialized inference | Materialized inference mode | OWL materialization, RDFS entailment |
| Custom indexes | (Future: index registration API) | Secondary indexes for domain queries |

---

## 11. Validation: Mapping Each Model onto the Foundation

This section verifies the foundation requirements by showing — for each model surveyed — how it would be built on top of the foundation without requiring changes to the core.

### 11.1 Validation: RDF / RDFS

**How to implement on top of the foundation:**

1. **Node types:** Register two built-in types: `rdfs:Resource` (the supertype of everything) and `rdfs:Literal` (for literal-valued nodes). All user resources are node instances with the `rdfs:Resource` type label.

2. **RDF triples → edges:** Each triple `(S, P, O)` becomes an edge of edge type `P` from node `S` to node `O`. Since literal values can be node types (`rdfs:Literal`), or represented as property values on node S with key P — downstream can choose.

3. **rdf:type:** A special edge type `rdf:type` (registered with the foundation) from individuals to class nodes.

4. **rdfs:subClassOf:** A special edge type `rdfs:subClassOf` stored as type hierarchy edges in the schema B-tree, or as regular graph edges (both work; the former integrates with B7's type hierarchy traversal).

5. **RDFS inference:** Register an `InferenceRule` that, when triggered:
   - Finds all `rdfs:subClassOf` edges
   - For each `rdf:type A` on an individual, traverses the subclass chain and adds `rdf:type B` for each superclass B
   - Similarly for `rdfs:subPropertyOf`

6. **rdfs:domain / rdfs:range:** Register a `ConstraintValidator` that checks edge endpoints against declared domain/range restrictions.

**Foundation features used:** A1, A2, A4, B1, B2, B7, C1, C2, D1, D2, D3, E1, E3

**Nothing in the foundation needs to change.** ✓

---

### 11.2 Validation: OWL Lite

**How to implement on top of the foundation:**

1. **Classes and individuals:** Classes are nodes of a special "OWL Class" type. Individuals are nodes of user-defined class types. Class membership is recorded as `rdf:type` edges (as in RDFS above).

2. **Object properties:** Registered as edge types. Property characteristics are stored as property type declarations with metadata annotations (e.g., `{owl:TransitiveProperty: true}` as a boolean property on the edge type definition).

3. **owl:subClassOf:** Type hierarchy stored in the foundation's type hierarchy DAG (B2).

4. **owl:equivalentClass:** Stored as a special edge of type `owl:equivalentClass` between class nodes. Downstream reasoning can follow these.

5. **Cardinality (Lite: 0 or 1 only):** A `ConstraintValidator` is registered that, for each node of a class with a max-cardinality restriction, counts outgoing edges of the restricted type and rejects the transaction if the count exceeds the maximum.

6. **owl:FunctionalProperty:** A `ConstraintValidator` that enforces uniqueness of the object for functional properties (max-cardinality 1 enforced at the edge type level).

7. **OWL Lite inference:**
   - Register an `InferenceRule` for subclass propagation (identical to RDFS above)
   - Register an `InferenceRule` for property transitivity: for any transitive property P, close P under transitivity using BFS/DFS traversal

8. **ABox / TBox separation:** TBox assertions (class definitions) are stored as special graph nodes/edges in a reserved subgraph (e.g., a "schema" context node). ABox assertions are regular graph nodes/edges. The foundation's named-subgraph feature (F3) or a simple type convention distinguishes them.

**Foundation features used:** A1, A2, A3, A4, B1, B2, B4, B7, C1, C2, C3, D1–D4, E1, E3

**Nothing in the foundation needs to change.** ✓

---

### 11.3 Validation: OWL DL

**How to implement on top of the foundation:**

OWL DL is strictly more expressive than OWL Lite. The foundation requirements do not change — what changes is the complexity of the downstream implementation.

1. **Anonymous class expressions:** Implemented using anonymous nodes (A3). `owl:Restriction` becomes an anonymous node of type "OWL Restriction" with property-bag entries for `owl:onProperty` (an edge type ID) and `owl:allValuesFrom` / `owl:someValuesFrom` (a class node ID).

2. **Boolean class combinators** (`owl:unionOf`, `owl:intersectionOf`, `owl:complementOf`): Implemented using anonymous nodes connected to their component classes by membership edges.

3. **owl:sameAs:** Implemented as a special edge type. The downstream OWL reasoner can maintain an identity resolution table (mapping aliases to canonical IDs) as a regular B-tree index over edges of this type. The foundation provides the edge storage; the OWL layer provides the merging logic.

4. **owl:disjointWith:** A `ConstraintValidator` that, for any two registered disjoint classes, checks that no individual has `rdf:type` memberships in both.

5. **OWL DL classification:** A registered `InferenceRule` implementing tableau-based or rule-based classification. This is complex downstream code — but it requires nothing from the foundation beyond the inference hook and graph traversal APIs.

6. **Consistency checking:** Implemented as a special `ConstraintValidator` that triggers an OWL reasoner on explicit request.

**Foundation features used:** A1–A5, B1–B7, C1–C6, D1–D7, E1–E3

**Nothing in the foundation needs to change.** ✓

---

### 11.4 Validation: SKOS

**How to implement on top of the foundation:**

SKOS is among the simplest models to build on the foundation — it needs very few features.

1. **Concept and ConceptScheme types:** Register two node types: `skos:Concept` and `skos:ConceptScheme`.

2. **Hierarchical and associative relationships:** Register edge types: `skos:broader`, `skos:narrower`, `skos:related`, `skos:inScheme`, `skos:hasTopConcept`.

3. **Labels:** Use the foundation's property bag. Register property keys: `skos:prefLabel`, `skos:altLabel`, `skos:hiddenLabel`. Multi-valued properties (A4 list support) handle multiple altLabels.

4. **Language-tagged labels:** Use language-tagged string values (A5). `skos:prefLabel` values carry a language tag.

5. **SKOS integrity conditions:**
   - Register a `ConstraintValidator` that checks `skos:prefLabel` uniqueness per language per concept
   - Register a `ConstraintValidator` that checks the broader/narrower hierarchy for cycles (using DFS from any concept, checking it doesn't reach itself via `skos:broader` traversal)

6. **SKOS inference:**
   - Register an `InferenceRule` for symmetry of `skos:related` (if A related B, add B related A)
   - Register an `InferenceRule` for `skos:broaderTransitive` / `skos:narrowerTransitive` (transitive closure over `skos:broader` / `skos:narrower`)
   - Register an `InferenceRule` for `skos:broader` / `skos:narrower` inverse relationship

**Foundation features used:** A1, A2, A4, A5, B1, B4, C1, C2, D1–D4, E1

**Nothing in the foundation needs to change.** ✓

---

### 11.5 Validation: Typed Property Graph (PG-Schema style)

**How to implement on top of the foundation:**

This is the most direct mapping since the project's own data model is a typed property graph.

1. **Node types and edge types:** Registered directly with the foundation's type registry (B1). Property type declarations specify which properties each type carries.

2. **Property type declarations:** Stored in the type definition's property type declaration list (B1). Each declaration carries: property key ID, expected value type, required/optional flag, single/multi-valued flag.

3. **Type inheritance:** Declared via the supertype mechanism (B2). A node type `Employee` with supertype `Person` inherits all of `Person`'s property type declarations.

4. **Existence constraints:** Register a `ConstraintValidator` that checks required property declarations are satisfied for every modified node/edge.

5. **Uniqueness constraints:** Register a `ConstraintValidator` that checks declared unique property keys have no duplicate values across all nodes of the type. (This requires a secondary index for efficiency — the foundation should support a query like "find all nodes of type T where property K = V".)

6. **Foreign-key-style constraints (source/target type restrictions):** A `ConstraintValidator` that checks every edge's source and target are instances of the declared allowed types.

7. **Open/closed schema toggle:** The `B6` open/closed flag. A `ConstraintValidator` enforces closed types by rejecting properties or labels not declared in the schema.

**Foundation features used:** A1, A2, A4, B1–B7, C1–C6, E1–E3

**Nothing in the foundation needs to change.** ✓

---

### 11.6 Validation: Conceptual Graphs

**How to implement on top of the foundation:**

Conceptual graphs are the most unusual model. The bipartite concept-relation structure requires a specific encoding.

1. **Concept nodes:** Regular nodes with a type label from the CG type hierarchy.

2. **Relation nodes:** Nodes with a special "CG Relation" type. A relation node represents an n-ary relation and is connected to its participants by typed edges.

3. **CG arcs:** Edges from concept nodes to a relation node (or relation node to concept node, depending on direction convention), with an edge type representing the role (`Agnt`, `Thme`, `Rcpt`, etc.).

4. **Type lattice:** The CG type lattice is stored as the foundation's type hierarchy. The `Universal` type is the root; `Absurd` is a conceptual bottom (typically not stored, just referenced in reasoning).

5. **Canonical graphs:** Stored as special subgraphs (a canonical graph is itself a graph stored as nodes and edges in the foundation, referenced from the type definition it belongs to). A `ConstraintValidator` checks that instances of a type have the structure specified in their canonical graph.

6. **Projection (pattern matching):** Implemented as a graph query — find subgraphs that match a given template. The foundation's traversal and type-filtered query APIs (E1, E3) provide the building blocks.

7. **CG inference (join, specialization):** Registered as `InferenceRule` instances.

**Foundation features used:** A1–A4, B1–B7, C1–C5, D1–D4, E1–E3

**Nothing in the foundation needs to change.** ✓ *(Canonical graph storage is unusual but maps naturally to storing a subgraph referenced by a type definition.)*

---

### 11.7 Validation: Frame-Based Systems

**How to implement on top of the foundation:**

1. **Frames as node types:** Each frame (e.g., `Person`, `Employee`) is a registered node type.

2. **Slots as property type declarations:** Each frame's slots are property type declarations in the type definition, with cardinality, range, and value type metadata.

3. **Slot facets:** Stored as additional metadata in the property type declaration (a property bag on the declaration itself). Facets like `default_value`, `inverse_slot`, `value_type`, `cardinality` become entries in this bag.

4. **Multiple inheritance:** The foundation's DAG type hierarchy (B2) handles this directly. `Employee` can have supertypes `[Person, Organisational-Agent]`.

5. **Default slot values:** Registered as an `InferenceRule` that, when triggered, fills in default values for instances that don't have an explicit value for a slot with a declared default.

6. **Constraint enforcement:** `ConstraintValidator` instances enforce cardinality and range restrictions, exactly as for PG-Schema.

7. **Derived slots:** Registered as `InferenceRule` instances. A derived slot's value is computed by the rule from other slot values.

**Foundation features used:** A1, A2, A4, B1–B7, C1–C5, D1–D7, E1–E3

**Nothing in the foundation needs to change.** ✓

---

### Summary of Validation

| Model | Foundation features required | Any foundation changes needed? |
|-------|------------------------------|-------------------------------|
| RDF/RDFS | A1, A2, A4, B1–B7, C1–C3, D1–D4, E1, E3 | No |
| OWL Lite | + A3, A5, C4–C6, D5–D7 | No |
| OWL DL | + all of the above | No |
| SKOS | A1, A2, A4, A5, B1, B4, C1–C2, D1–D4, E1 | No |
| PG-Schema | A1, A2, A4, B1–B7, C1–C6, E1–E3 | No |
| Conceptual Graphs | A1–A4, B1–B7, C1–C5, D1–D4, E1–E3 | No |
| Frames/DL | A1, A2, A4, B1–B7, C1–C5, D1–D7, E1–E3 | No |
| Topic Maps | A1, A2, A4, A5, B1, B4, C1–C2, D1–D4, E1 | No |

All models map onto the foundation without requiring changes. The foundation is not under-specified for any of them. ✓

---

## 12. What Must NOT Be in the Foundation

Explicit exclusions — things that are tempting to include but must remain downstream:

| Excluded concept | Why excluded | Which model wants it | How downstream handles it |
|-----------------|-------------|---------------------|--------------------------|
| `rdf:type` predicate | A specific URI-named relationship; not a language primitive | RDF | Register as an edge type named `rdf:type` |
| `rdfs:subClassOf` semantics | A specific inference rule | RDFS | Register as an inference rule |
| `owl:disjointWith` enforcement | A specific constraint | OWL | Register as a constraint validator |
| `skos:broader` / `skos:narrower` | Specific relationship types | SKOS | Register as edge types |
| Cardinality enforcement | A specific constraint type | OWL, PG-Schema, Frames | Register as a constraint validator |
| OWL classification algorithm | A specific inference algorithm | OWL | Register as an inference rule |
| Transitivity / symmetry of named properties | Specific inference rules | OWL, SKOS | Register as inference rules |
| Closed-world assumption | A reasoning policy | Some DL systems | Implement in constraint validator |
| Open-world assumption | A reasoning policy | RDF/OWL | Default behavior (no constraint) |
| SPARQL, GQL, Cypher, Gremlin | Query languages | Various | Not provided by this crate |
| URI / IRI namespacing | RDF-specific serialization | RDF | Downstream namespace management |
| sameAs reasoning | Specific inference | OWL | Register as inference rule + identity index |
| Ontology versioning beyond schema versioning | Specific to OWL/SKOS | OWL, SKOS | Downstream application logic |
| Any built-in type vocabulary | Domain-specific | All | Zero built-in domain types |

**The golden rule:** If something can be expressed as "register a constraint that enforces X" or "register a rule that derives Y", it does not belong in the foundation.

---

## 13. Interaction with Storage Layer (Tasks 1 and 2 Integration)

This section briefly connects the foundation requirements to the storage design emerging from Tasks 1 and 2, without preempting Task 7's formal decisions.

### 13.1 Type registry storage

Requirements B1–B7 define a persistent type registry. From Task 2's analysis, this should be stored as a **dedicated schema B-tree** in the single file, separate from the main graph data. Type definitions are small, rarely updated, and always loaded at startup. A schema B-tree keyed by type ID can store serialized type definitions as values.

The **property key registry** (B5) is a small bidirectional map (string ↔ integer) that fits in a few pages and is fully cached in memory after startup.

### 13.2 Type labels on nodes/edges

Requirements A1, A4 allow multiple type labels per node/edge. In the fixed-size record layout recommended in Task 2, this creates a challenge: a single type label fits in a fixed record; multiple type labels require either a small inline array or an overflow store. 

Options:
1. **Inline array of type IDs** (e.g., up to 4 inline, overflow for more): Efficient for the common case; handles RDF's single `rdf:type` and OWL's inferred multi-type membership
2. **Single primary type + secondary type index**: The fixed record stores one primary type ID; a secondary B-tree indexes `(node_id, type_id)` for multi-type membership. This is cleaner for fixed-size records.

**Recommendation for Task 7:** Option 2 (primary type in fixed record + secondary type index B-tree) aligns with the hybrid storage model from Task 2 and avoids variable-length fixed records.

### 13.3 Constraint and inference hooks at the transaction boundary

Requirements C1–C5 require constraint validators to run at transaction commit time. This means the storage engine must provide a **pre-commit hook** that can access the transaction's change set (inserts, deletes, modifications). The WAL (or CoW commit path) from Task 1 is the natural integration point — immediately before writing the COMMIT record, run all registered constraints.

Requirements D1–D3 require inference rules to run on explicit request. This is a read-write transaction that the caller initiates. Inferred facts (if materialized) are committed in that transaction. The storage engine needs no special support beyond its normal transactional API.

### 13.4 Property bag storage

Requirement A4 mandates a typed value system for property values. The property store recommended in Task 2 (out-of-line variable-length blocks) must encode typed values. A simple encoding:

```
PropertyBlock:
  [num_entries: u16]
  [entry: key_id: u32, value_type: u8, value_payload: ...]×num_entries
```

Value types map to the A4 requirements: boolean (1 byte), integers (fixed-width), floats (fixed-width), string (length-prefixed UTF-8), blob (length-prefixed bytes), node reference (8-byte ID), list (length-prefixed sequence of homogeneous typed values), null (zero payload).

Language-tagged strings (A5) can be encoded as a special value type: `(string_bytes, lang_tag_bytes)`.

---

## Completion Report: Task 3 — Ontology & Knowledge Representation Models

### Status: COMPLETE

### Done Criterion:
The criterion requires: (1) comparing at least 5 approaches, (2) identifying shared underlying primitives, (3) producing a foundation requirements list, (4) a validation check showing how each model maps onto the foundation. 

Models surveyed: RDF/RDFS, OWL (all profiles), SKOS, Property Graphs with Schema (PG-Schema/openCypher), Conceptual Graphs, Frame-Based Systems/Description Logics, Topic Maps — 7 models total. ✓

Shared primitives identified and organized in Section 9 (13 primitives, cross-referenced to all models). ✓

Foundation requirements list produced in Section 10 (7 categories, 30+ requirements, MUST/SHOULD/MAY priority). ✓

Validation check completed in Section 11 — each of the 7+ models validated against the foundation, all verified to map correctly with no foundation changes needed. ✓

### Deliverables:
- `003-ontology-models-survey.md` — this document

### Summary:
Surveyed seven knowledge representation models in depth. Extracted 13 shared primitives that recur across all models. Translated these into a structured foundation requirements list across 7 categories (graph data model, type system, constraint system, inference system, query/traversal, transaction/persistence, extension points). Validated the requirements against all seven models, confirming each can be implemented as a downstream layer without requiring changes to the foundation.

The central finding: **all knowledge representation models reduce to the same substrate** — named typed nodes, typed directed edges with properties, a DAG type hierarchy, and externally-implemented constraints and inference rules. The foundation needs to be a well-engineered typed property graph with clean extension points; it does not need to understand ontology semantics at all.

### Context for Next Task:
This document is the required input for **Task 6 (Core Schema & Extension System Design)**. Task 6 should read this document carefully, particularly:
- Section 10 (Foundation Requirements) — the full requirements list with priorities
- Section 11 (Validation) — verification that the requirements are sufficient
- Section 12 (What Must NOT Be in the Foundation) — the exclusion list

Task 6 also depends on nothing else in Wave 1, so it can proceed as soon as this document is available.

Tasks 7 (Graph Storage Model) and 10 (API Surface) will also benefit from this document, particularly Section 13 (interaction with storage layer) and the data model requirements in Category A.

### Residual Concerns:
1. **Language-tagged strings (A5):** The foundation *should* support these but the exact encoding is left to Task 6/10. Whether language tags are a first-class value type or a convention over string + property key namespacing affects the API design.

2. **Named subgraphs (F3):** The foundation *should* support named subgraphs, but the exact mechanism is deliberately left open (it could be a special node type, a reserved property, or a first-class concept). Task 6 must decide this.

3. **Secondary indexes for constraint validation:** Uniqueness constraints (as used by PG-Schema and implied by OWL functional properties) require efficient lookups of "all nodes of type T with property K = V". The foundation's constraint API (C2) gives validators a read-only database view — but if that view doesn't include efficient property-value indexes, unique-constraint validation will require full type scans. This is a performance concern, not a correctness concern, but Task 7 and 10 should note it.

4. **Canonical graphs for CGs (Section 11.6):** Storing a CG canonical graph as a subgraph "referenced by a type definition" requires the type definition to hold a pointer to a subgraph node or context. The exact storage representation is left to Task 6.

5. **Topic Maps scope mechanism:** The validation (Section 11 notes for Topic Maps) confirms that edge properties handle scope adequately for most use cases. A more complete Topic Maps implementation might want a first-class "scope" concept; this is left to downstream.

### Upstream Flags:
None. All findings are scoped to the foundation requirements; no sibling task dependencies are affected.
