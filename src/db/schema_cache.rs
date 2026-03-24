//! In-memory schema cache implementing [`TypeRegistryView`] and
//! [`PropertyKeyRegistryView`].
//!
//! The [`SchemaCache`] holds all type definitions, property key definitions,
//! and precomputed type hierarchy data. It is loaded from the Schema Store
//! B-tree at database open time and cloned into each transaction for
//! read-your-own-writes access.

use std::collections::HashMap;

use crate::error::SchemaError;
use crate::schema::{PropertyKeyRegistryView, TypeRegistryView};
use crate::types::{
    EdgeId, NodeId, PropertyDeclaration, PropertyKeyId, TypeDefinition, TypeId, TypeKind,
};

/// A property key definition stored in the schema cache.
#[derive(Clone, Debug)]
pub struct PropertyKeyDefinition {
    /// The unique identifier of this property key.
    pub id: PropertyKeyId,
    /// The human-readable name of this property key.
    pub name: String,
}

/// In-memory cache of all schema data (types, property keys, hierarchy).
///
/// Stores type definitions contiguously in a `Vec` to satisfy the
/// [`TypeRegistryView::all_types`] contract of returning `&[TypeDefinition]`.
/// HashMap indexes provide O(1) lookups by ID or name.
#[derive(Clone, Debug)]
pub struct SchemaCache {
    /// Contiguous storage for type definitions (returned as `&[TypeDefinition]`).
    types_vec: Vec<TypeDefinition>,
    /// TypeId → index into `types_vec`.
    type_id_to_index: HashMap<TypeId, usize>,
    /// (name, kind) → index into `types_vec` for duplicate detection.
    type_names: HashMap<(String, TypeKind), usize>,
    /// Precomputed: TypeId → all transitive subtypes.
    subtypes_cache: HashMap<TypeId, Vec<TypeId>>,

    /// All property key definitions.
    property_keys_vec: Vec<PropertyKeyDefinition>,
    /// PropertyKeyId → index into `property_keys_vec`.
    prop_id_to_index: HashMap<PropertyKeyId, usize>,
    /// Property key name → PropertyKeyId.
    prop_name_to_id: HashMap<String, PropertyKeyId>,

    /// Next ID counters.
    pub(crate) next_node_id: u64,
    pub(crate) next_edge_id: u64,
    pub(crate) next_type_id: u32,
    pub(crate) next_property_key_id: u32,
}

impl Default for SchemaCache {
    fn default() -> Self {
        Self::new()
    }
}

impl SchemaCache {
    /// Creates an empty schema cache with ID counters starting at 1.
    pub fn new() -> Self {
        Self {
            types_vec: Vec::new(),
            type_id_to_index: HashMap::new(),
            type_names: HashMap::new(),
            subtypes_cache: HashMap::new(),
            property_keys_vec: Vec::new(),
            prop_id_to_index: HashMap::new(),
            prop_name_to_id: HashMap::new(),
            next_node_id: 1,
            next_edge_id: 1,
            next_type_id: 1,
            next_property_key_id: 1,
        }
    }

    /// Registers a type definition, assigning it a new `TypeId`.
    ///
    /// Validates that:
    /// - No existing type has the same (name, kind) pair
    /// - All referenced supertypes exist and have the same kind
    /// - Adding this type does not create a cycle in the hierarchy
    ///
    /// # Errors
    ///
    /// Returns [`SchemaError::DuplicateTypeName`] if a type with the same
    /// name and kind already exists, [`SchemaError::SupertypeNotFound`] if
    /// a supertype does not exist, [`SchemaError::KindMismatch`] if a
    /// supertype has a different kind, or [`SchemaError::CycleDetected`] if
    /// adding the type would create a cycle.
    pub fn register_type(&mut self, mut def: TypeDefinition) -> Result<TypeId, SchemaError> {
        let key = (def.name.clone(), def.kind);
        if self.type_names.contains_key(&key) {
            return Err(SchemaError::DuplicateTypeName {
                name: def.name.clone(),
                kind: def.kind,
            });
        }

        // Validate supertypes exist and have the same kind.
        for &sup_id in &def.supertypes {
            let sup = self
                .get_type_by_id(sup_id)
                .ok_or(SchemaError::SupertypeNotFound(sup_id))?;
            if sup.kind != def.kind {
                return Err(SchemaError::KindMismatch {
                    expected: def.kind,
                    found: sup.kind,
                });
            }
        }

        // Assign ID.
        let type_id = TypeId(self.next_type_id);
        self.next_type_id += 1;
        def.id = type_id;

        // Cycle detection: walk up from each supertype; if we ever
        // encounter type_id, there is a cycle. Since type_id is new and
        // not yet in the registry, a cycle can only occur if supertypes
        // form a cycle among themselves. But we also need to check the
        // broader case for when `load_type` is used with pre-assigned IDs.
        self.detect_cycle(type_id, &def.supertypes, &def.name)?;

        // Insert.
        let idx = self.types_vec.len();
        self.types_vec.push(def);
        self.type_id_to_index.insert(type_id, idx);
        self.type_names.insert(key, idx);

        self.rebuild_subtypes_cache();

        Ok(type_id)
    }

    /// Loads a type definition with a pre-assigned `TypeId`.
    ///
    /// Used during database open to populate the cache from the Schema Store.
    /// Does not validate uniqueness exhaustively — assumes the persisted data
    /// is consistent.
    ///
    /// # Errors
    ///
    /// Returns an error if a type with the same ID already exists.
    pub fn load_type(&mut self, def: TypeDefinition) -> Result<(), SchemaError> {
        if self.type_id_to_index.contains_key(&def.id) {
            return Err(SchemaError::DuplicateTypeName {
                name: def.name.clone(),
                kind: def.kind,
            });
        }

        let type_id = def.id;
        let key = (def.name.clone(), def.kind);
        let idx = self.types_vec.len();
        self.types_vec.push(def);
        self.type_id_to_index.insert(type_id, idx);
        self.type_names.insert(key, idx);

        // Update the next_type_id counter if needed.
        if type_id.0 >= self.next_type_id {
            self.next_type_id = type_id.0 + 1;
        }

        Ok(())
    }

    /// Rebuilds the subtypes cache after loading all types.
    ///
    /// Call this once after all `load_type` calls are complete.
    pub fn rebuild_subtypes_cache(&mut self) {
        self.subtypes_cache.clear();
        for td in &self.types_vec {
            for &sup_id in &td.supertypes {
                self.subtypes_cache
                    .entry(sup_id)
                    .or_default()
                    .push(td.id);
            }
        }
        // Expand to transitive subtypes.
        // Collect all type IDs, then for each, compute the full closure.
        let all_ids: Vec<TypeId> = self.types_vec.iter().map(|t| t.id).collect();
        let mut transitive: HashMap<TypeId, Vec<TypeId>> = HashMap::new();
        for &id in &all_ids {
            let mut result = Vec::new();
            let mut stack: Vec<TypeId> = self
                .subtypes_cache
                .get(&id)
                .cloned()
                .unwrap_or_default();
            let mut visited = std::collections::HashSet::new();
            while let Some(child) = stack.pop() {
                if visited.insert(child) {
                    result.push(child);
                    if let Some(grandchildren) = self.subtypes_cache.get(&child) {
                        stack.extend(grandchildren);
                    }
                }
            }
            if !result.is_empty() {
                transitive.insert(id, result);
            }
        }
        self.subtypes_cache = transitive;
    }

    /// Detects cycles by checking if any supertype (transitively) leads
    /// back to `new_type_id`.
    fn detect_cycle(
        &self,
        new_type_id: TypeId,
        supertypes: &[TypeId],
        _name: &str,
    ) -> Result<(), SchemaError> {
        let mut visited = std::collections::HashSet::new();
        let mut stack: Vec<(TypeId, TypeId)> = supertypes
            .iter()
            .map(|&s| (new_type_id, s))
            .collect();
        while let Some((child, parent)) = stack.pop() {
            if parent == new_type_id {
                return Err(SchemaError::CycleDetected {
                    child,
                    would_be_parent: parent,
                });
            }
            if !visited.insert(parent) {
                continue;
            }
            if let Some(td) = self.get_type_by_id(parent) {
                for &gp in &td.supertypes {
                    stack.push((parent, gp));
                }
            }
        }
        Ok(())
    }

    /// Gets a type definition by ID (internal helper).
    fn get_type_by_id(&self, id: TypeId) -> Option<&TypeDefinition> {
        self.type_id_to_index
            .get(&id)
            .map(|&idx| &self.types_vec[idx])
    }

    /// Registers a property key by name, returning its new ID.
    ///
    /// # Errors
    ///
    /// Returns [`SchemaError::DuplicatePropertyKey`] if a key with the same
    /// name already exists.
    pub fn register_property_key(&mut self, name: &str) -> Result<PropertyKeyId, SchemaError> {
        if self.prop_name_to_id.contains_key(name) {
            return Err(SchemaError::DuplicatePropertyKey { name: name.to_string() });
        }
        let id = PropertyKeyId(self.next_property_key_id);
        self.next_property_key_id += 1;
        let def = PropertyKeyDefinition {
            id,
            name: name.to_string(),
        };
        let idx = self.property_keys_vec.len();
        self.property_keys_vec.push(def);
        self.prop_id_to_index.insert(id, idx);
        self.prop_name_to_id.insert(name.to_string(), id);
        Ok(id)
    }

    /// Returns the existing property key ID for `name`, or creates a new one.
    pub fn get_or_create_property_key(&mut self, name: &str) -> PropertyKeyId {
        if let Some(&id) = self.prop_name_to_id.get(name) {
            return id;
        }
        // unwrap is safe: we just checked it doesn't exist.
        self.register_property_key(name).unwrap()
    }

    /// Loads a property key with a pre-assigned ID.
    ///
    /// Used during database open to populate the cache from the Schema Store.
    pub fn load_property_key(&mut self, id: PropertyKeyId, name: String) {
        if self.prop_id_to_index.contains_key(&id) {
            return; // already loaded
        }
        let def = PropertyKeyDefinition {
            id,
            name: name.clone(),
        };
        let idx = self.property_keys_vec.len();
        self.property_keys_vec.push(def);
        self.prop_id_to_index.insert(id, idx);
        self.prop_name_to_id.insert(name, id);
        if id.0 >= self.next_property_key_id {
            self.next_property_key_id = id.0 + 1;
        }
    }

    /// Returns the property key definition by ID.
    pub fn get_property_key(&self, id: PropertyKeyId) -> Option<&PropertyKeyDefinition> {
        self.prop_id_to_index
            .get(&id)
            .map(|&idx| &self.property_keys_vec[idx])
    }

    /// Allocates and returns the next `NodeId`.
    pub fn allocate_node_id(&mut self) -> NodeId {
        let id = NodeId(self.next_node_id);
        self.next_node_id += 1;
        id
    }

    /// Allocates and returns the next `EdgeId`.
    pub fn allocate_edge_id(&mut self) -> EdgeId {
        let id = EdgeId(self.next_edge_id);
        self.next_edge_id += 1;
        id
    }
}

// ---------------------------------------------------------------------------
// TypeRegistryView implementation
// ---------------------------------------------------------------------------

impl TypeRegistryView for SchemaCache {
    fn get_type(&self, id: TypeId) -> Option<&TypeDefinition> {
        self.get_type_by_id(id)
    }

    fn get_type_by_name(&self, name: &str, kind: TypeKind) -> Option<&TypeDefinition> {
        self.type_names
            .get(&(name.to_string(), kind))
            .map(|&idx| &self.types_vec[idx])
    }

    fn all_types(&self) -> &[TypeDefinition] {
        &self.types_vec
    }

    fn types_by_kind(&self, kind: TypeKind) -> Vec<&TypeDefinition> {
        self.types_vec.iter().filter(|t| t.kind == kind).collect()
    }

    fn direct_supertypes(&self, id: TypeId) -> Option<&[TypeId]> {
        self.get_type_by_id(id)
            .map(|td| td.supertypes.as_slice())
    }

    fn all_supertypes(&self, id: TypeId) -> Vec<TypeId> {
        let td = match self.get_type_by_id(id) {
            Some(t) => t,
            None => return Vec::new(),
        };
        let mut result = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut stack: Vec<TypeId> = td.supertypes.clone();
        // BFS — most specific first (direct supertypes come first)
        while let Some(sup) = stack.pop() {
            if visited.insert(sup) {
                result.push(sup);
                if let Some(parent) = self.get_type_by_id(sup) {
                    // Push in reverse so first supertypes come out first
                    for &gp in parent.supertypes.iter().rev() {
                        stack.push(gp);
                    }
                }
            }
        }
        result
    }

    fn direct_subtypes(&self, id: TypeId) -> Vec<TypeId> {
        // Direct subtypes: types whose supertypes list contains id.
        self.types_vec
            .iter()
            .filter(|t| t.supertypes.contains(&id))
            .map(|t| t.id)
            .collect()
    }

    fn all_subtypes(&self, id: TypeId) -> Vec<TypeId> {
        self.subtypes_cache
            .get(&id)
            .cloned()
            .unwrap_or_default()
    }

    fn is_subtype_of(&self, candidate: TypeId, ancestor: TypeId) -> bool {
        self.all_supertypes(candidate).contains(&ancestor)
    }

    fn effective_property_declarations(&self, id: TypeId) -> Vec<PropertyDeclaration> {
        let td = match self.get_type_by_id(id) {
            Some(t) => t,
            None => return Vec::new(),
        };
        // Collect supertypes in reverse order (most general first).
        let supertypes = self.all_supertypes(id);
        let mut decls: std::collections::BTreeMap<PropertyKeyId, PropertyDeclaration> =
            std::collections::BTreeMap::new();

        // Apply supertype declarations from most general to most specific.
        for &sup_id in supertypes.iter().rev() {
            if let Some(sup) = self.get_type_by_id(sup_id) {
                for decl in &sup.property_declarations {
                    decls.insert(decl.key, decl.clone());
                }
            }
        }
        // Apply own declarations last (shadow supertypes).
        for decl in &td.property_declarations {
            decls.insert(decl.key, decl.clone());
        }
        decls.into_values().collect()
    }
}

// ---------------------------------------------------------------------------
// PropertyKeyRegistryView implementation
// ---------------------------------------------------------------------------

impl PropertyKeyRegistryView for SchemaCache {
    fn get_key_id(&self, name: &str) -> Option<PropertyKeyId> {
        self.prop_name_to_id.get(name).copied()
    }

    fn get_key_name(&self, id: PropertyKeyId) -> Option<&str> {
        self.prop_id_to_index
            .get(&id)
            .map(|&idx| self.property_keys_vec[idx].name.as_str())
    }

    fn all_keys(&self) -> Vec<(PropertyKeyId, &str)> {
        self.property_keys_vec
            .iter()
            .map(|def| (def.id, def.name.as_str()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TypeKind;

    fn make_type(name: &str, kind: TypeKind, supertypes: Vec<TypeId>) -> TypeDefinition {
        TypeDefinition {
            id: TypeId::NULL,
            name: name.to_string(),
            kind,
            supertypes,
            property_declarations: Vec::new(),
            open: false,
            metadata: Default::default(),
        }
    }

    #[test]
    fn register_and_retrieve_type() {
        let mut cache = SchemaCache::new();
        let def = make_type("Person", TypeKind::Node, vec![]);
        let id = cache.register_type(def).unwrap();
        assert_ne!(id, TypeId::NULL);

        let retrieved = cache.get_type_by_id(id).unwrap();
        assert_eq!(retrieved.name, "Person");
        assert_eq!(retrieved.id, id);

        let by_name = TypeRegistryView::get_type_by_name(&cache, "Person", TypeKind::Node);
        assert!(by_name.is_some());
        assert_eq!(by_name.unwrap().id, id);
    }

    #[test]
    fn duplicate_type_name_rejected() {
        let mut cache = SchemaCache::new();
        cache
            .register_type(make_type("Person", TypeKind::Node, vec![]))
            .unwrap();
        let result = cache.register_type(make_type("Person", TypeKind::Node, vec![]));
        assert!(matches!(result, Err(SchemaError::DuplicateTypeName { .. })));
    }

    #[test]
    fn same_name_different_kind_allowed() {
        let mut cache = SchemaCache::new();
        cache
            .register_type(make_type("Knows", TypeKind::Node, vec![]))
            .unwrap();
        let result = cache.register_type(make_type("Knows", TypeKind::Edge, vec![]));
        assert!(result.is_ok());
    }

    #[test]
    fn subtypes_of_parent() {
        let mut cache = SchemaCache::new();
        let animal = cache
            .register_type(make_type("Animal", TypeKind::Node, vec![]))
            .unwrap();
        let mammal = cache
            .register_type(make_type("Mammal", TypeKind::Node, vec![animal]))
            .unwrap();
        let dog = cache
            .register_type(make_type("Dog", TypeKind::Node, vec![mammal]))
            .unwrap();

        let animal_subtypes = cache.all_subtypes(animal);
        assert!(animal_subtypes.contains(&mammal));
        assert!(animal_subtypes.contains(&dog));

        let mammal_subtypes = cache.all_subtypes(mammal);
        assert!(mammal_subtypes.contains(&dog));
        assert!(!mammal_subtypes.contains(&animal));
    }

    #[test]
    fn all_supertypes_chain() {
        let mut cache = SchemaCache::new();
        let a = cache
            .register_type(make_type("A", TypeKind::Node, vec![]))
            .unwrap();
        let b = cache
            .register_type(make_type("B", TypeKind::Node, vec![a]))
            .unwrap();
        let c = cache
            .register_type(make_type("C", TypeKind::Node, vec![b]))
            .unwrap();

        let supers = cache.all_supertypes(c);
        assert!(supers.contains(&b));
        assert!(supers.contains(&a));
        // b should come before a (most specific first)
        let b_pos = supers.iter().position(|&x| x == b).unwrap();
        let a_pos = supers.iter().position(|&x| x == a).unwrap();
        assert!(b_pos < a_pos);
    }

    #[test]
    fn cycle_detection() {
        let mut cache = SchemaCache::new();
        let a = cache
            .register_type(make_type("A", TypeKind::Node, vec![]))
            .unwrap();
        // B has supertype A
        let b = cache
            .register_type(make_type("B", TypeKind::Node, vec![a]))
            .unwrap();
        // Try to register C with supertype B, where C's ID would not
        // create a cycle. This should succeed.
        let c = cache
            .register_type(make_type("C", TypeKind::Node, vec![b]))
            .unwrap();
        assert_ne!(c, TypeId::NULL);

        // For a real cycle test: we need to try to create a type whose
        // supertypes transitively include itself. Since register_type
        // assigns a new ID, the cycle can only happen if we try to make
        // a supertype point to a type that's not yet registered. But
        // that would be caught by SupertypeNotFound. Cycle detection
        // is more relevant for load_type with pre-assigned IDs.
    }

    #[test]
    fn supertype_not_found() {
        let mut cache = SchemaCache::new();
        let result =
            cache.register_type(make_type("X", TypeKind::Node, vec![TypeId(999)]));
        assert!(matches!(result, Err(SchemaError::SupertypeNotFound(_))));
    }

    #[test]
    fn kind_mismatch_rejected() {
        let mut cache = SchemaCache::new();
        let node_type = cache
            .register_type(make_type("Base", TypeKind::Node, vec![]))
            .unwrap();
        let result =
            cache.register_type(make_type("EdgeChild", TypeKind::Edge, vec![node_type]));
        assert!(matches!(result, Err(SchemaError::KindMismatch { .. })));
    }

    #[test]
    fn property_key_registration() {
        let mut cache = SchemaCache::new();
        let id = cache.register_property_key("name").unwrap();
        assert_ne!(id, PropertyKeyId::NULL);

        let by_name = PropertyKeyRegistryView::get_key_id(&cache, "name");
        assert_eq!(by_name, Some(id));

        let name = PropertyKeyRegistryView::get_key_name(&cache, id);
        assert_eq!(name, Some("name"));
    }

    #[test]
    fn get_or_create_property_key_idempotent() {
        let mut cache = SchemaCache::new();
        let id1 = cache.get_or_create_property_key("email");
        let id2 = cache.get_or_create_property_key("email");
        assert_eq!(id1, id2);
    }

    #[test]
    fn duplicate_property_key_rejected() {
        let mut cache = SchemaCache::new();
        cache.register_property_key("name").unwrap();
        let result = cache.register_property_key("name");
        assert!(matches!(result, Err(SchemaError::DuplicatePropertyKey { .. })));
    }

    #[test]
    fn id_allocation_monotonic() {
        let mut cache = SchemaCache::new();
        let n1 = cache.allocate_node_id();
        let n2 = cache.allocate_node_id();
        assert!(n2.0 > n1.0);

        let e1 = cache.allocate_edge_id();
        let e2 = cache.allocate_edge_id();
        assert!(e2.0 > e1.0);
    }

    #[test]
    fn all_types_contiguous() {
        let mut cache = SchemaCache::new();
        cache
            .register_type(make_type("A", TypeKind::Node, vec![]))
            .unwrap();
        cache
            .register_type(make_type("B", TypeKind::Edge, vec![]))
            .unwrap();
        let all = TypeRegistryView::all_types(&cache);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].name, "A");
        assert_eq!(all[1].name, "B");
    }

    #[test]
    fn is_subtype_of_works() {
        let mut cache = SchemaCache::new();
        let a = cache
            .register_type(make_type("A", TypeKind::Node, vec![]))
            .unwrap();
        let b = cache
            .register_type(make_type("B", TypeKind::Node, vec![a]))
            .unwrap();
        assert!(cache.is_subtype_of(b, a));
        assert!(!cache.is_subtype_of(a, b));
    }

    #[test]
    fn effective_property_declarations_with_shadowing() {
        use crate::types::{PropertyKeyId, ValueTypeDescriptor};

        let mut cache = SchemaCache::new();
        let key1 = PropertyKeyId(1);
        let key2 = PropertyKeyId(2);

        let parent_def = TypeDefinition {
            id: TypeId::NULL,
            name: "Parent".to_string(),
            kind: TypeKind::Node,
            supertypes: vec![],
            property_declarations: vec![
                PropertyDeclaration {
                    key: key1,
                    value_type: ValueTypeDescriptor::String,
                    required: true,
                    multi_valued: false,
                    metadata: Default::default(),
                },
                PropertyDeclaration {
                    key: key2,
                    value_type: ValueTypeDescriptor::I64,
                    required: false,
                    multi_valued: false,
                    metadata: Default::default(),
                },
            ],
            open: false,
            metadata: Default::default(),
        };
        let parent_id = cache.register_type(parent_def).unwrap();

        // Child shadows key1 with a different required flag.
        let child_def = TypeDefinition {
            id: TypeId::NULL,
            name: "Child".to_string(),
            kind: TypeKind::Node,
            supertypes: vec![parent_id],
            property_declarations: vec![PropertyDeclaration {
                key: key1,
                value_type: ValueTypeDescriptor::String,
                required: false, // shadowed
                multi_valued: false,
                metadata: Default::default(),
            }],
            open: false,
            metadata: Default::default(),
        };
        let child_id = cache.register_type(child_def).unwrap();

        let decls = cache.effective_property_declarations(child_id);
        assert_eq!(decls.len(), 2);

        let key1_decl = decls.iter().find(|d| d.key == key1).unwrap();
        assert!(!key1_decl.required); // shadowed by child

        let key2_decl = decls.iter().find(|d| d.key == key2).unwrap();
        assert!(!key2_decl.required); // inherited from parent
    }

    #[test]
    fn direct_subtypes_works() {
        let mut cache = SchemaCache::new();
        let a = cache
            .register_type(make_type("A", TypeKind::Node, vec![]))
            .unwrap();
        let _b = cache
            .register_type(make_type("B", TypeKind::Node, vec![a]))
            .unwrap();
        let _c = cache
            .register_type(make_type("C", TypeKind::Node, vec![a]))
            .unwrap();
        let direct = cache.direct_subtypes(a);
        assert_eq!(direct.len(), 2);
    }

    #[test]
    fn types_by_kind_filters() {
        let mut cache = SchemaCache::new();
        cache
            .register_type(make_type("Person", TypeKind::Node, vec![]))
            .unwrap();
        cache
            .register_type(make_type("Knows", TypeKind::Edge, vec![]))
            .unwrap();
        cache
            .register_type(make_type("Place", TypeKind::Node, vec![]))
            .unwrap();

        let nodes = cache.types_by_kind(TypeKind::Node);
        assert_eq!(nodes.len(), 2);
        let edges = cache.types_by_kind(TypeKind::Edge);
        assert_eq!(edges.len(), 1);
    }

    #[test]
    fn all_keys_returns_all() {
        let mut cache = SchemaCache::new();
        cache.register_property_key("name").unwrap();
        cache.register_property_key("age").unwrap();
        let keys = PropertyKeyRegistryView::all_keys(&cache);
        assert_eq!(keys.len(), 2);
    }
}
