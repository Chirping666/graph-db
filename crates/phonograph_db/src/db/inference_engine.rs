//! Inference engine internals: caching, provenance, and rule management.
//!
//! This module contains the `InferenceEngine` (the top-level coordinator),
//! `InferenceCache` (generation-keyed LRU result cache), and
//! `ProvenanceRegistry` (in-memory provenance index with persistence
//! support). These are internal types owned by the `Database`; transactions
//! access them through the engine's `Mutex`.

use alloc::{boxed::Box, string::{String, ToString}, vec, vec::Vec};
use alloc::collections::BTreeMap;

use phonograph::inference::{
    InferenceResult, InferenceRule, InferredEntity, ProvenanceRecord,
};
use crate::storage::serialization;
use phonograph::types::{EdgeId, NodeId, PropertyKeyId, TypeId};

// ---------------------------------------------------------------------------
// InferenceCache
// ---------------------------------------------------------------------------

/// A cached inference result together with its LRU access counter.
struct CacheEntry {
    result: InferenceResult,
    last_accessed: u64,
}

/// In-memory LRU cache for inference results.
///
/// Keyed by `(rule_name, data_generation)` where `data_generation` is the
/// snapshot's transaction ID. Bounded by `max_entries`. Not persisted to disk.
/// When the cache is full, the least recently used entry is evicted.
///
/// Uses a two-level map (`rule_name` → `generation` → entry) so that
/// lookups via `get()` can borrow the rule name (`&str`) without allocating.
pub(crate) struct InferenceCache {
    /// Two-level cache: `rule_name` → (`generation` → cached result + access counter).
    entries: BTreeMap<String, BTreeMap<u64, CacheEntry>>,
    /// Maximum number of entries (total across all rule names). 0 = caching disabled.
    max_entries: usize,
    /// Monotonically increasing counter for LRU ordering.
    access_counter: u64,
}

impl InferenceCache {
    /// Creates a new cache with the given capacity.
    ///
    /// Pass 0 to disable caching entirely.
    pub(crate) fn new(max_entries: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            max_entries,
            access_counter: 0,
        }
    }

    /// Looks up a cached result. Returns `Some(result.clone())` on hit,
    /// `None` on miss. Updates the LRU access counter on hit.
    ///
    /// This method borrows `rule_name` without allocating a `String`.
    pub(crate) fn get(&mut self, rule_name: &str, generation: u64) -> Option<InferenceResult> {
        if self.max_entries == 0 {
            return None;
        }
        if let Some(by_gen) = self.entries.get_mut(rule_name)
            && let Some(entry) = by_gen.get_mut(&generation)
        {
            self.access_counter += 1;
            entry.last_accessed = self.access_counter;
            return Some(entry.result.clone());
        }
        None
    }

    /// Inserts a result into the cache. If full, evicts the LRU entry.
    /// No-op if `max_entries == 0`.
    pub(crate) fn insert(
        &mut self,
        rule_name: String,
        generation: u64,
        result: InferenceResult,
    ) {
        if self.max_entries == 0 {
            return;
        }
        self.access_counter += 1;
        let entry = CacheEntry {
            result,
            last_accessed: self.access_counter,
        };
        // If the key already exists, replace it (no net change in count).
        if let Some(by_gen) = self.entries.get_mut(&rule_name)
            && let alloc::collections::btree_map::Entry::Occupied(mut e) =
                by_gen.entry(generation)
        {
            e.insert(entry);
            return;
        }
        // Evict LRU if at capacity (count total entries across all rule names).
        if self.total_entries() >= self.max_entries {
            self.evict_lru();
        }
        self.entries
            .entry(rule_name)
            .or_default()
            .insert(generation, entry);
    }

    /// Clears all entries.
    #[allow(dead_code)]
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }

    /// Returns the total number of cached entries across all rule names.
    fn total_entries(&self) -> usize {
        self.entries.values().map(|m| m.len()).sum()
    }

    /// Evicts the entry with the smallest `last_accessed` counter.
    fn evict_lru(&mut self) {
        let mut lru_key: Option<(String, u64)> = None;
        let mut lru_access = u64::MAX;
        for (rule, by_gen) in &self.entries {
            for (generation, entry) in by_gen {
                if entry.last_accessed < lru_access {
                    lru_access = entry.last_accessed;
                    lru_key = Some((rule.clone(), *generation));
                }
            }
        }
        if let Some((rule, generation)) = lru_key
            && let Some(by_gen) = self.entries.get_mut(&rule)
        {
            by_gen.remove(&generation);
            if by_gen.is_empty() {
                self.entries.remove(&rule);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ProvenanceRegistry
// ---------------------------------------------------------------------------

/// Tracks which entities in the graph were produced by inference rules.
///
/// Maintained in memory with persistence to the Schema Store B-tree.
/// Loaded from disk at database open; updated during materialization.
pub(crate) struct ProvenanceRegistry {
    /// Forward index: entity → provenance record.
    by_entity: BTreeMap<InferredEntity, ProvenanceRecord>,
    /// Reverse index: rule_name → entities produced by that rule.
    by_rule: BTreeMap<String, Vec<InferredEntity>>,
}

impl ProvenanceRegistry {
    /// Creates an empty provenance registry.
    pub(crate) fn new() -> Self {
        Self {
            by_entity: BTreeMap::new(),
            by_rule: BTreeMap::new(),
        }
    }

    /// Records that `entity` was produced by `rule_name` at `txn_id`.
    ///
    /// If the entity already has a provenance record (e.g. re-materialization
    /// without cleanup), the old record is replaced and the reverse index
    /// is updated.
    pub(crate) fn record(
        &mut self,
        entity: InferredEntity,
        rule_name: &str,
        txn_id: u64,
    ) {
        // If this entity was already recorded under a different (or same) rule,
        // remove it from the old reverse index entry.
        if let Some(old_record) = self.by_entity.get(&entity) {
            let old_rule = old_record.rule_name.clone();
            if let Some(entities) = self.by_rule.get_mut(&old_rule) {
                entities.retain(|e| e != &entity);
                if entities.is_empty() {
                    self.by_rule.remove(&old_rule);
                }
            }
        }

        let record = ProvenanceRecord {
            rule_name: rule_name.to_string(),
            materialized_at: txn_id,
        };
        self.by_entity.insert(entity.clone(), record);
        self.by_rule
            .entry(rule_name.to_string())
            .or_default()
            .push(entity);
    }

    /// Removes all provenance records for entities produced by `rule_name`.
    /// Returns the removed entities (for cleanup from the WriteBuffer).
    pub(crate) fn remove_by_rule(&mut self, rule_name: &str) -> Vec<InferredEntity> {
        let entities = self.by_rule.remove(rule_name).unwrap_or_default();
        for entity in &entities {
            self.by_entity.remove(entity);
        }
        entities
    }

    /// Looks up provenance of a specific entity. `None` if user-asserted.
    pub(crate) fn get(&self, entity: &InferredEntity) -> Option<&ProvenanceRecord> {
        self.by_entity.get(entity)
    }

    /// Checks whether a specific entity was produced by inference.
    pub(crate) fn is_inferred(&self, entity: &InferredEntity) -> bool {
        self.by_entity.contains_key(entity)
    }

    /// Returns all entities produced by a specific rule.
    ///
    /// Returns an empty slice if the rule has no recorded entities.
    #[allow(dead_code)]
    pub(crate) fn entities_by_rule(&self, rule_name: &str) -> &[InferredEntity] {
        self.by_rule
            .get(rule_name)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    // --- Persistence ---

    /// Encodes a single provenance entry as `(key_bytes, value_bytes)` for
    /// writing to the Schema Store B-tree.
    ///
    /// Key: `[0x06][entity_kind: 1B][entity_id: 8B BE][sub_id: 4B BE]` (14 bytes)
    /// Value: `[txn_id: 8B LE][rule_name_len: 2B LE][rule_name: UTF-8]`
    pub(crate) fn encode_entry(
        entity: &InferredEntity,
        record: &ProvenanceRecord,
    ) -> (Vec<u8>, Vec<u8>) {
        let mut key = vec![0u8; 14];
        key[0] = 0x06; // provenance prefix
        match entity {
            InferredEntity::Node(id) => {
                key[1] = 0x01;
                key[2..10].copy_from_slice(&id.0.to_be_bytes());
                // sub_id = 0
            }
            InferredEntity::Edge(id) => {
                key[1] = 0x02;
                key[2..10].copy_from_slice(&id.0.to_be_bytes());
            }
            InferredEntity::NodeProperty { node, key: pk } => {
                key[1] = 0x03;
                key[2..10].copy_from_slice(&node.0.to_be_bytes());
                key[10..14].copy_from_slice(&pk.0.to_be_bytes());
            }
            InferredEntity::EdgeProperty { edge, key: pk } => {
                key[1] = 0x04;
                key[2..10].copy_from_slice(&edge.0.to_be_bytes());
                key[10..14].copy_from_slice(&pk.0.to_be_bytes());
            }
            InferredEntity::NodeType { node, type_id } => {
                key[1] = 0x05;
                key[2..10].copy_from_slice(&node.0.to_be_bytes());
                key[10..14].copy_from_slice(&type_id.0.to_be_bytes());
            }
            InferredEntity::EdgeType { edge, type_id } => {
                key[1] = 0x06;
                key[2..10].copy_from_slice(&edge.0.to_be_bytes());
                key[10..14].copy_from_slice(&type_id.0.to_be_bytes());
            }
        }

        let value = serialization::serialize_provenance(record);
        (key, value)
    }

    /// Decodes a provenance entry from Schema Store key/value bytes.
    ///
    /// Returns `None` if the key does not start with prefix `0x06` or is
    /// malformed.
    pub(crate) fn decode_entry(
        key: &[u8],
        value: &[u8],
    ) -> Option<(InferredEntity, ProvenanceRecord)> {
        if key.len() != 14 || key[0] != 0x06 {
            return None;
        }
        let entity_id = u64::from_be_bytes(key[2..10].try_into().ok()?);
        let sub_id = u32::from_be_bytes(key[10..14].try_into().ok()?);

        let entity = match key[1] {
            0x01 => InferredEntity::Node(NodeId(entity_id)),
            0x02 => InferredEntity::Edge(EdgeId(entity_id)),
            0x03 => InferredEntity::NodeProperty {
                node: NodeId(entity_id),
                key: PropertyKeyId(sub_id),
            },
            0x04 => InferredEntity::EdgeProperty {
                edge: EdgeId(entity_id),
                key: PropertyKeyId(sub_id),
            },
            0x05 => InferredEntity::NodeType {
                node: NodeId(entity_id),
                type_id: TypeId(sub_id),
            },
            0x06 => InferredEntity::EdgeType {
                edge: EdgeId(entity_id),
                type_id: TypeId(sub_id),
            },
            _ => return None,
        };

        let record = serialization::deserialize_provenance(value).ok()?;
        Some((entity, record))
    }

    /// Populates this registry from an iterator of raw `(key, value)` pairs
    /// read from the Schema Store B-tree during database open.
    pub(crate) fn load_from_entries(
        &mut self,
        entries: impl Iterator<Item = (Vec<u8>, Vec<u8>)>,
    ) {
        for (key, value) in entries {
            if let Some((entity, record)) = Self::decode_entry(&key, &value) {
                let rule_name = record.rule_name.clone();
                self.by_entity.insert(entity.clone(), record);
                self.by_rule.entry(rule_name).or_default().push(entity);
            }
        }
    }

    /// Returns all entries as encoded `(key, value)` pairs for writing
    /// to the Schema Store B-tree.
    #[allow(dead_code)]
    pub(crate) fn to_entries(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.by_entity
            .iter()
            .map(|(entity, record)| Self::encode_entry(entity, record))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// InferenceEngine
// ---------------------------------------------------------------------------

/// The internal inference engine. Owned by the `Database`.
///
/// Manages rule registration (with insertion-order tracking), result caching,
/// and provenance tracking. The `Database` wraps this in a `Mutex`.
pub(crate) struct InferenceEngine {
    /// Registered rules, keyed by name for O(log N) lookup.
    rules: BTreeMap<String, Box<dyn InferenceRule>>,
    /// Rule names in registration (insertion) order.
    /// `run_all_inference` iterates in this order for deterministic chaining.
    rule_order: Vec<String>,
    /// Generation-keyed LRU result cache.
    cache: InferenceCache,
    /// In-memory provenance index.
    provenance: ProvenanceRegistry,
}

impl InferenceEngine {
    /// Creates a new engine with the given cache capacity.
    pub(crate) fn new(cache_size: usize) -> Self {
        Self {
            rules: BTreeMap::new(),
            rule_order: Vec::new(),
            cache: InferenceCache::new(cache_size),
            provenance: ProvenanceRegistry::new(),
        }
    }

    // --- Rule registry ---

    /// Registers an inference rule. Replaces any existing rule with the same
    /// name (preserving its position in the registration order).
    pub(crate) fn register_rule(&mut self, rule: Box<dyn InferenceRule>) {
        let name = rule.name().to_string();
        if !self.rules.contains_key(&name) {
            self.rule_order.push(name.clone());
        }
        self.rules.insert(name, rule);
    }

    /// Unregisters an inference rule. Returns `true` if found and removed.
    pub(crate) fn unregister_rule(&mut self, name: &str) -> bool {
        if self.rules.remove(name).is_some() {
            self.rule_order.retain(|n| n != name);
            true
        } else {
            false
        }
    }

    /// Returns the names of all registered rules in registration order.
    pub(crate) fn rule_names(&self) -> Vec<String> {
        self.rule_order.clone()
    }

    /// Looks up a rule by name. Returns `None` if not registered.
    pub(crate) fn get_rule(&self, name: &str) -> Option<&dyn InferenceRule> {
        self.rules.get(name).map(|r| r.as_ref())
    }

    // --- Cache delegation ---

    /// Checks the cache for a result matching `(rule_name, generation)`.
    pub(crate) fn cache_get(
        &mut self,
        rule_name: &str,
        generation: u64,
    ) -> Option<InferenceResult> {
        self.cache.get(rule_name, generation)
    }

    /// Inserts a result into the cache.
    pub(crate) fn cache_insert(
        &mut self,
        rule_name: String,
        generation: u64,
        result: InferenceResult,
    ) {
        self.cache.insert(rule_name, generation, result);
    }

    // --- Provenance delegation ---

    /// Returns a shared reference to the provenance registry.
    pub(crate) fn provenance(&self) -> &ProvenanceRegistry {
        &self.provenance
    }

    /// Returns a mutable reference to the provenance registry.
    pub(crate) fn provenance_mut(&mut self) -> &mut ProvenanceRegistry {
        &mut self.provenance
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use phonograph::inference::{InferenceResult, InferredEntity, InferredFact, ProvenanceRecord};
    use phonograph::types::{EdgeId, NodeId, PropertyKeyId, TypeId};

    // =======================================================================
    // InferenceCache tests
    // =======================================================================

    fn sample_result(rule: &str) -> InferenceResult {
        InferenceResult {
            facts: vec![InferredFact::NodeTypeAssignment {
                node: NodeId(1),
                type_id: TypeId(1),
            }],
            rule_name: rule.to_string(),
        }
    }

    #[test]
    fn cache_disabled_always_misses() {
        let mut cache = InferenceCache::new(0);
        cache.insert("r".into(), 1, sample_result("r"));
        assert!(cache.get("r", 1).is_none());
    }

    #[test]
    fn cache_hit_on_exact_key() {
        let mut cache = InferenceCache::new(2);
        cache.insert("r1".into(), 5, sample_result("r1"));
        cache.insert("r2".into(), 5, sample_result("r2"));
        assert!(cache.get("r1", 5).is_some());
        assert!(cache.get("r2", 5).is_some());
    }

    #[test]
    fn cache_miss_on_wrong_generation() {
        let mut cache = InferenceCache::new(2);
        cache.insert("r".into(), 5, sample_result("r"));
        assert!(cache.get("r", 6).is_none());
    }

    #[test]
    fn cache_miss_on_wrong_rule_name() {
        let mut cache = InferenceCache::new(2);
        cache.insert("r".into(), 5, sample_result("r"));
        assert!(cache.get("other", 5).is_none());
    }

    #[test]
    fn cache_lru_eviction() {
        let mut cache = InferenceCache::new(2);
        cache.insert("r1".into(), 1, sample_result("r1"));
        cache.insert("r2".into(), 1, sample_result("r2"));
        // r1 was inserted first with the lowest counter.
        cache.insert("r3".into(), 1, sample_result("r3"));
        // r1 should be evicted.
        assert!(cache.get("r1", 1).is_none());
        assert!(cache.get("r2", 1).is_some());
        assert!(cache.get("r3", 1).is_some());
    }

    #[test]
    fn cache_access_updates_lru_priority() {
        let mut cache = InferenceCache::new(2);
        cache.insert("r1".into(), 1, sample_result("r1"));
        cache.insert("r2".into(), 1, sample_result("r2"));
        // Access r1 to make it more recent than r2.
        let _ = cache.get("r1", 1);
        // Insert r3 — should evict r2 (not r1).
        cache.insert("r3".into(), 1, sample_result("r3"));
        assert!(cache.get("r1", 1).is_some());
        assert!(cache.get("r2", 1).is_none());
        assert!(cache.get("r3", 1).is_some());
    }

    #[test]
    fn cache_clear_empties_all() {
        let mut cache = InferenceCache::new(10);
        cache.insert("r1".into(), 1, sample_result("r1"));
        cache.insert("r2".into(), 1, sample_result("r2"));
        cache.clear();
        assert!(cache.get("r1", 1).is_none());
        assert!(cache.get("r2", 1).is_none());
    }

    // =======================================================================
    // ProvenanceRegistry tests
    // =======================================================================

    #[test]
    fn provenance_record_and_get() {
        let mut reg = ProvenanceRegistry::new();
        reg.record(InferredEntity::Node(NodeId(1)), "rule_a", 10);
        let p = reg.get(&InferredEntity::Node(NodeId(1)));
        assert!(p.is_some());
        assert_eq!(p.unwrap().rule_name, "rule_a");
        assert_eq!(p.unwrap().materialized_at, 10);
    }

    #[test]
    fn provenance_is_inferred() {
        let mut reg = ProvenanceRegistry::new();
        reg.record(InferredEntity::Edge(EdgeId(5)), "rule_b", 20);
        assert!(reg.is_inferred(&InferredEntity::Edge(EdgeId(5))));
        assert!(!reg.is_inferred(&InferredEntity::Node(NodeId(999))));
    }

    #[test]
    fn provenance_remove_by_rule() {
        let mut reg = ProvenanceRegistry::new();
        reg.record(InferredEntity::Node(NodeId(1)), "rule_a", 10);
        reg.record(InferredEntity::Node(NodeId(2)), "rule_a", 10);
        reg.record(InferredEntity::Edge(EdgeId(3)), "rule_b", 10);

        let removed = reg.remove_by_rule("rule_a");
        assert_eq!(removed.len(), 2);
        assert!(!reg.is_inferred(&InferredEntity::Node(NodeId(1))));
        assert!(!reg.is_inferred(&InferredEntity::Node(NodeId(2))));
        // rule_b unaffected
        assert!(reg.is_inferred(&InferredEntity::Edge(EdgeId(3))));
    }

    #[test]
    fn provenance_remove_by_rule_nonexistent() {
        let mut reg = ProvenanceRegistry::new();
        let removed = reg.remove_by_rule("nonexistent");
        assert!(removed.is_empty());
    }

    #[test]
    fn provenance_entities_by_rule() {
        let mut reg = ProvenanceRegistry::new();
        reg.record(InferredEntity::Node(NodeId(1)), "rule_a", 10);
        reg.record(InferredEntity::Node(NodeId(2)), "rule_a", 10);

        let entities = reg.entities_by_rule("rule_a");
        assert_eq!(entities.len(), 2);

        // Unknown rule returns empty.
        let empty = reg.entities_by_rule("unknown");
        assert!(empty.is_empty());
    }

    #[test]
    fn provenance_replace_existing_entity() {
        let mut reg = ProvenanceRegistry::new();
        reg.record(InferredEntity::Node(NodeId(1)), "rule_a", 10);
        // Re-record same entity under same rule — should replace, not duplicate.
        reg.record(InferredEntity::Node(NodeId(1)), "rule_a", 20);
        assert_eq!(
            reg.get(&InferredEntity::Node(NodeId(1)))
                .unwrap()
                .materialized_at,
            20
        );
        assert_eq!(reg.entities_by_rule("rule_a").len(), 1);
    }

    #[test]
    fn provenance_replace_entity_different_rule() {
        let mut reg = ProvenanceRegistry::new();
        reg.record(InferredEntity::Node(NodeId(1)), "rule_a", 10);
        // Re-record same entity under a different rule.
        reg.record(InferredEntity::Node(NodeId(1)), "rule_b", 20);
        assert_eq!(
            reg.get(&InferredEntity::Node(NodeId(1)))
                .unwrap()
                .rule_name,
            "rule_b"
        );
        assert!(reg.entities_by_rule("rule_a").is_empty());
        assert_eq!(reg.entities_by_rule("rule_b").len(), 1);
    }

    // =======================================================================
    // Provenance persistence (encode/decode) tests
    // =======================================================================

    #[test]
    fn encode_decode_node_round_trip() {
        let entity = InferredEntity::Node(NodeId(42));
        let record = ProvenanceRecord {
            rule_name: "test".into(),
            materialized_at: 100,
        };
        let (key, value) = ProvenanceRegistry::encode_entry(&entity, &record);
        assert_eq!(key.len(), 14);
        assert_eq!(key[0], 0x06);
        assert_eq!(key[1], 0x01);

        let (e2, r2) = ProvenanceRegistry::decode_entry(&key, &value).unwrap();
        assert_eq!(e2, entity);
        assert_eq!(r2, record);
    }

    #[test]
    fn encode_decode_edge_round_trip() {
        let entity = InferredEntity::Edge(EdgeId(99));
        let record = ProvenanceRecord {
            rule_name: "r2".into(),
            materialized_at: 200,
        };
        let (key, value) = ProvenanceRegistry::encode_entry(&entity, &record);
        let (e2, r2) = ProvenanceRegistry::decode_entry(&key, &value).unwrap();
        assert_eq!(e2, entity);
        assert_eq!(r2, record);
    }

    #[test]
    fn encode_decode_node_property_round_trip() {
        let entity = InferredEntity::NodeProperty {
            node: NodeId(10),
            key: PropertyKeyId(5),
        };
        let record = ProvenanceRecord {
            rule_name: "prop_rule".into(),
            materialized_at: 50,
        };
        let (key, value) = ProvenanceRegistry::encode_entry(&entity, &record);
        assert_eq!(key[1], 0x03);
        let (e2, r2) = ProvenanceRegistry::decode_entry(&key, &value).unwrap();
        assert_eq!(e2, entity);
        assert_eq!(r2, record);
    }

    #[test]
    fn encode_decode_edge_property_round_trip() {
        let entity = InferredEntity::EdgeProperty {
            edge: EdgeId(7),
            key: PropertyKeyId(3),
        };
        let record = ProvenanceRecord {
            rule_name: "ep".into(),
            materialized_at: 1,
        };
        let (key, value) = ProvenanceRegistry::encode_entry(&entity, &record);
        assert_eq!(key[1], 0x04);
        let (e2, r2) = ProvenanceRegistry::decode_entry(&key, &value).unwrap();
        assert_eq!(e2, entity);
        assert_eq!(r2, record);
    }

    #[test]
    fn encode_decode_node_type_round_trip() {
        let entity = InferredEntity::NodeType {
            node: NodeId(1),
            type_id: TypeId(8),
        };
        let record = ProvenanceRecord {
            rule_name: "nt".into(),
            materialized_at: 5,
        };
        let (key, value) = ProvenanceRegistry::encode_entry(&entity, &record);
        assert_eq!(key[1], 0x05);
        let (e2, r2) = ProvenanceRegistry::decode_entry(&key, &value).unwrap();
        assert_eq!(e2, entity);
        assert_eq!(r2, record);
    }

    #[test]
    fn encode_decode_edge_type_round_trip() {
        let entity = InferredEntity::EdgeType {
            edge: EdgeId(2),
            type_id: TypeId(9),
        };
        let record = ProvenanceRecord {
            rule_name: "et".into(),
            materialized_at: 3,
        };
        let (key, value) = ProvenanceRegistry::encode_entry(&entity, &record);
        assert_eq!(key[1], 0x06);
        let (e2, r2) = ProvenanceRegistry::decode_entry(&key, &value).unwrap();
        assert_eq!(e2, entity);
        assert_eq!(r2, record);
    }

    #[test]
    fn keys_sort_correctly() {
        // Node(1) < Edge(1) < NodeProperty(1, 1) per entity_kind byte ordering.
        let n = InferredEntity::Node(NodeId(1));
        let e = InferredEntity::Edge(EdgeId(1));
        let np = InferredEntity::NodeProperty {
            node: NodeId(1),
            key: PropertyKeyId(1),
        };
        let record = ProvenanceRecord {
            rule_name: "r".into(),
            materialized_at: 1,
        };
        let (kn, _) = ProvenanceRegistry::encode_entry(&n, &record);
        let (ke, _) = ProvenanceRegistry::encode_entry(&e, &record);
        let (knp, _) = ProvenanceRegistry::encode_entry(&np, &record);
        assert!(kn < ke);
        assert!(ke < knp);
    }

    #[test]
    fn decode_entry_wrong_prefix_returns_none() {
        let mut key = vec![0u8; 14];
        key[0] = 0x05; // not 0x06
        assert!(ProvenanceRegistry::decode_entry(&key, &[]).is_none());
    }

    #[test]
    fn decode_entry_truncated_key_returns_none() {
        let key = vec![0x06, 0x01, 0, 0, 0]; // too short
        assert!(ProvenanceRegistry::decode_entry(&key, &[]).is_none());
    }

    #[test]
    fn decode_entry_unknown_kind_returns_none() {
        let mut key = vec![0u8; 14];
        key[0] = 0x06;
        key[1] = 0xFF; // unknown entity kind
        let value = serialization::serialize_provenance(&ProvenanceRecord {
            rule_name: "r".into(),
            materialized_at: 1,
        });
        assert!(ProvenanceRegistry::decode_entry(&key, &value).is_none());
    }

    #[test]
    fn load_from_entries_populates_both_indexes() {
        let mut reg = ProvenanceRegistry::new();
        let n = InferredEntity::Node(NodeId(1));
        let e = InferredEntity::Edge(EdgeId(2));
        let record_a = ProvenanceRecord {
            rule_name: "r".into(),
            materialized_at: 1,
        };
        let record_b = ProvenanceRecord {
            rule_name: "r".into(),
            materialized_at: 1,
        };
        let entries = vec![
            ProvenanceRegistry::encode_entry(&n, &record_a),
            ProvenanceRegistry::encode_entry(&e, &record_b),
        ];
        reg.load_from_entries(entries.into_iter());
        assert!(reg.is_inferred(&n));
        assert!(reg.is_inferred(&e));
        assert_eq!(reg.entities_by_rule("r").len(), 2);
    }

    #[test]
    fn to_entries_load_from_entries_round_trip() {
        let mut reg1 = ProvenanceRegistry::new();
        reg1.record(InferredEntity::Node(NodeId(10)), "rule_x", 100);
        reg1.record(InferredEntity::Edge(EdgeId(20)), "rule_y", 200);
        reg1.record(
            InferredEntity::NodeProperty {
                node: NodeId(10),
                key: PropertyKeyId(5),
            },
            "rule_x",
            100,
        );

        let entries = reg1.to_entries();
        let mut reg2 = ProvenanceRegistry::new();
        reg2.load_from_entries(entries.into_iter());

        // Verify all entities transferred.
        assert!(reg2.is_inferred(&InferredEntity::Node(NodeId(10))));
        assert!(reg2.is_inferred(&InferredEntity::Edge(EdgeId(20))));
        assert!(reg2.is_inferred(&InferredEntity::NodeProperty {
            node: NodeId(10),
            key: PropertyKeyId(5),
        }));
        assert_eq!(reg2.entities_by_rule("rule_x").len(), 2);
        assert_eq!(reg2.entities_by_rule("rule_y").len(), 1);
    }

    // =======================================================================
    // InferenceEngine tests
    // =======================================================================

    // A minimal rule for engine tests.
    struct DummyRule {
        rule_name: String,
    }

    impl InferenceRule for DummyRule {
        fn name(&self) -> &str {
            &self.rule_name
        }

        fn applies_to_types(&self) -> Option<Vec<TypeId>> {
            None
        }

        fn infer(
            &self,
            _graph: &dyn phonograph::schema::GraphView,
            _types: &dyn phonograph::schema::TypeRegistryView,
            _keys: &dyn phonograph::schema::PropertyKeyRegistryView,
        ) -> InferenceResult {
            InferenceResult {
                facts: vec![],
                rule_name: self.rule_name.clone(),
            }
        }
    }

    #[test]
    fn engine_register_and_lookup() {
        let mut engine = InferenceEngine::new(0);
        engine.register_rule(Box::new(DummyRule {
            rule_name: "alpha".into(),
        }));
        assert!(engine.get_rule("alpha").is_some());
        assert!(engine.get_rule("beta").is_none());
    }

    #[test]
    fn engine_unregister() {
        let mut engine = InferenceEngine::new(0);
        engine.register_rule(Box::new(DummyRule {
            rule_name: "alpha".into(),
        }));
        assert!(engine.unregister_rule("alpha"));
        assert!(!engine.unregister_rule("alpha"));
        assert!(engine.get_rule("alpha").is_none());
    }

    #[test]
    fn engine_rule_names_in_insertion_order() {
        let mut engine = InferenceEngine::new(0);
        engine.register_rule(Box::new(DummyRule {
            rule_name: "beta".into(),
        }));
        engine.register_rule(Box::new(DummyRule {
            rule_name: "alpha".into(),
        }));
        let names = engine.rule_names();
        assert_eq!(names, vec!["beta", "alpha"]);
    }

    #[test]
    fn engine_replace_rule_preserves_order() {
        let mut engine = InferenceEngine::new(0);
        engine.register_rule(Box::new(DummyRule {
            rule_name: "a".into(),
        }));
        engine.register_rule(Box::new(DummyRule {
            rule_name: "b".into(),
        }));
        // Replace "a" — should keep its position.
        engine.register_rule(Box::new(DummyRule {
            rule_name: "a".into(),
        }));
        let names = engine.rule_names();
        assert_eq!(names, vec!["a", "b"]);
    }
}
