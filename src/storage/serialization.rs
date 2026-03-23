//! Record serialization, key encoding, and property storage.
//!
//! Implements binary serialization for NodeRecord, EdgeRecord,
//! PropertyMap values, and all B-tree key formats. Keys use
//! big-endian encoding; values use little-endian.
//! See `007-graph-storage-model.md` §§5–7 and `012-design-document.md` §19.

use crate::error::StorageError;
use crate::inference::ProvenanceRecord;
use crate::types::{
    Edge, EdgeId, Node, NodeId, PropertyDeclaration, PropertyKeyId, PropertyMap, TypeDefinition,
    TypeId, TypeKind, Value, ValueTypeDescriptor,
};

use super::page::PageId;

// ---------------------------------------------------------------------------
// B-tree key encoding (big-endian for lexicographic byte order)
// ---------------------------------------------------------------------------

/// Encodes a `NodeId` as an 8-byte big-endian key.
pub fn encode_node_key(id: NodeId) -> [u8; 8] {
    id.0.to_be_bytes()
}

/// Decodes a `NodeId` from an 8-byte big-endian key.
pub fn decode_node_key(key: &[u8]) -> NodeId {
    NodeId(u64::from_be_bytes(key[..8].try_into().unwrap()))
}

/// Encodes an `EdgeId` as an 8-byte big-endian key.
pub fn encode_edge_key(id: EdgeId) -> [u8; 8] {
    id.0.to_be_bytes()
}

/// Decodes an `EdgeId` from an 8-byte big-endian key.
pub fn decode_edge_key(key: &[u8]) -> EdgeId {
    EdgeId(u64::from_be_bytes(key[..8].try_into().unwrap()))
}

/// Encodes an outgoing adjacency key: `(NodeId, TypeId, EdgeId)` = 20 bytes BE.
pub fn encode_outgoing_adj_key(node: NodeId, type_id: TypeId, edge: EdgeId) -> [u8; 20] {
    let mut key = [0u8; 20];
    key[0..8].copy_from_slice(&node.0.to_be_bytes());
    key[8..12].copy_from_slice(&type_id.0.to_be_bytes());
    key[12..20].copy_from_slice(&edge.0.to_be_bytes());
    key
}

/// Decodes an outgoing adjacency key into `(NodeId, TypeId, EdgeId)`.
pub fn decode_outgoing_adj_key(key: &[u8]) -> (NodeId, TypeId, EdgeId) {
    let node = NodeId(u64::from_be_bytes(key[0..8].try_into().unwrap()));
    let type_id = TypeId(u32::from_be_bytes(key[8..12].try_into().unwrap()));
    let edge = EdgeId(u64::from_be_bytes(key[12..20].try_into().unwrap()));
    (node, type_id, edge)
}

/// Encodes an incoming adjacency key: `(NodeId, TypeId, EdgeId)` = 20 bytes BE.
pub fn encode_incoming_adj_key(node: NodeId, type_id: TypeId, edge: EdgeId) -> [u8; 20] {
    encode_outgoing_adj_key(node, type_id, edge)
}

/// Decodes an incoming adjacency key into `(NodeId, TypeId, EdgeId)`.
pub fn decode_incoming_adj_key(key: &[u8]) -> (NodeId, TypeId, EdgeId) {
    decode_outgoing_adj_key(key)
}

/// Encodes a type index key: `(TypeKindTag, TypeId, EntityId)` = 13 bytes BE.
///
/// `kind_tag`: 0x00 = node, 0x01 = edge.
pub fn encode_type_index_key(kind_tag: u8, type_id: TypeId, entity_id: u64) -> [u8; 13] {
    let mut key = [0u8; 13];
    key[0] = kind_tag;
    key[1..5].copy_from_slice(&type_id.0.to_be_bytes());
    key[5..13].copy_from_slice(&entity_id.to_be_bytes());
    key
}

/// Decodes a type index key into `(kind_tag, TypeId, entity_id)`.
pub fn decode_type_index_key(key: &[u8]) -> (u8, TypeId, u64) {
    let kind_tag = key[0];
    let type_id = TypeId(u32::from_be_bytes(key[1..5].try_into().unwrap()));
    let entity_id = u64::from_be_bytes(key[5..13].try_into().unwrap());
    (kind_tag, type_id, entity_id)
}

/// Encodes a page freelist key: `(FreedTxnId, PageId)` = 16 bytes BE.
pub fn encode_page_freelist_key(freed_txn_id: u64, page_id: PageId) -> [u8; 16] {
    let mut key = [0u8; 16];
    key[0..8].copy_from_slice(&freed_txn_id.to_be_bytes());
    key[8..16].copy_from_slice(&page_id.0.to_be_bytes());
    key
}

/// Decodes a page freelist key into `(freed_txn_id, PageId)`.
pub fn decode_page_freelist_key(key: &[u8]) -> (u64, PageId) {
    let freed_txn_id = u64::from_be_bytes(key[0..8].try_into().unwrap());
    let page_id = PageId(u64::from_be_bytes(key[8..16].try_into().unwrap()));
    (freed_txn_id, page_id)
}

/// Encodes an ID freelist key: `(EntityKindTag, EntityId)` = 9 bytes BE.
///
/// `kind_tag`: 0x00 = node, 0x01 = edge.
pub fn encode_id_freelist_key(kind_tag: u8, entity_id: u64) -> [u8; 9] {
    let mut key = [0u8; 9];
    key[0] = kind_tag;
    key[1..9].copy_from_slice(&entity_id.to_be_bytes());
    key
}

/// Decodes an ID freelist key into `(kind_tag, entity_id)`.
pub fn decode_id_freelist_key(key: &[u8]) -> (u8, u64) {
    (key[0], u64::from_be_bytes(key[1..9].try_into().unwrap()))
}

// ---------------------------------------------------------------------------
// Schema Store key encoding (variable-length with prefix discriminator)
// ---------------------------------------------------------------------------

/// Encodes a Schema Store key with prefix `0x01` (type definition).
pub fn encode_schema_type_key(type_id: TypeId) -> Vec<u8> {
    let mut key = Vec::with_capacity(5);
    key.push(0x01);
    key.extend_from_slice(&type_id.0.to_be_bytes());
    key
}

/// Encodes a Schema Store key with prefix `0x02` (property key).
pub fn encode_schema_property_key(key_id: PropertyKeyId) -> Vec<u8> {
    let mut key = Vec::with_capacity(5);
    key.push(0x02);
    key.extend_from_slice(&key_id.0.to_be_bytes());
    key
}

/// Encodes a Schema Store key with prefix `0x03` (counter).
///
/// `counter_name`: `0x01` = next NodeId, `0x02` = next EdgeId,
/// `0x03` = next TypeId, `0x04` = next PropertyKeyId.
pub fn encode_schema_counter_key(counter_name: u8) -> Vec<u8> {
    vec![0x03, counter_name]
}

/// Encodes a Schema Store key with prefix `0x04` (type hierarchy edge).
pub fn encode_schema_hierarchy_key(child: TypeId, parent: TypeId) -> Vec<u8> {
    let mut key = Vec::with_capacity(9);
    key.push(0x04);
    key.extend_from_slice(&child.0.to_be_bytes());
    key.extend_from_slice(&parent.0.to_be_bytes());
    key
}

/// Encodes a Schema Store key with prefix `0x05` (extension name).
///
/// `kind`: `0x01` = constraint validator, `0x02` = inference rule.
pub fn encode_schema_extension_key(kind: u8, name: &str) -> Vec<u8> {
    let name_bytes = name.as_bytes();
    let mut key = Vec::with_capacity(4 + name_bytes.len());
    key.push(0x05);
    key.push(kind);
    key.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
    key.extend_from_slice(name_bytes);
    key
}

/// Encodes a Schema Store key with prefix `0x06` (provenance).
pub fn encode_schema_provenance_key(entity_kind: u8, entity_id: u64, sub_id: u32) -> Vec<u8> {
    let mut key = Vec::with_capacity(14);
    key.push(0x06);
    key.push(entity_kind);
    key.extend_from_slice(&entity_id.to_be_bytes());
    key.extend_from_slice(&sub_id.to_be_bytes());
    key
}

// ---------------------------------------------------------------------------
// Value serialization
// ---------------------------------------------------------------------------

/// Value type tag constants.
const TAG_NULL: u8 = 0x00;
const TAG_BOOL: u8 = 0x01;
const TAG_I64: u8 = 0x02;
const TAG_U64: u8 = 0x03;
const TAG_F64: u8 = 0x04;
const TAG_STRING: u8 = 0x05;
const TAG_BYTES: u8 = 0x06;
const TAG_NODE_REF: u8 = 0x07;
const TAG_LANG_STRING: u8 = 0x08;
const TAG_LIST: u8 = 0x09;

/// Serializes a single [`Value`] to binary format.
///
/// Format: `[type_tag: u8] [payload]`.
pub fn serialize_value(value: &Value) -> Vec<u8> {
    let mut buf = Vec::new();
    write_value(&mut buf, value);
    buf
}

/// Writes a value into a buffer (recursive helper).
fn write_value(buf: &mut Vec<u8>, value: &Value) {
    match value {
        Value::Null => buf.push(TAG_NULL),
        Value::Bool(b) => {
            buf.push(TAG_BOOL);
            buf.push(if *b { 1 } else { 0 });
        }
        Value::I64(v) => {
            buf.push(TAG_I64);
            buf.extend_from_slice(&v.to_le_bytes());
        }
        Value::U64(v) => {
            buf.push(TAG_U64);
            buf.extend_from_slice(&v.to_le_bytes());
        }
        Value::F64(v) => {
            buf.push(TAG_F64);
            buf.extend_from_slice(&v.to_le_bytes());
        }
        Value::String(s) => {
            buf.push(TAG_STRING);
            buf.extend_from_slice(&(s.len() as u32).to_le_bytes());
            buf.extend_from_slice(s.as_bytes());
        }
        Value::Bytes(b) => {
            buf.push(TAG_BYTES);
            buf.extend_from_slice(&(b.len() as u32).to_le_bytes());
            buf.extend_from_slice(b);
        }
        Value::NodeRef(id) => {
            buf.push(TAG_NODE_REF);
            buf.extend_from_slice(&id.0.to_le_bytes());
        }
        Value::LangString { value: v, lang } => {
            buf.push(TAG_LANG_STRING);
            buf.extend_from_slice(&(v.len() as u32).to_le_bytes());
            buf.extend_from_slice(v.as_bytes());
            buf.extend_from_slice(&(lang.len() as u32).to_le_bytes());
            buf.extend_from_slice(lang.as_bytes());
        }
        Value::List(items) => {
            buf.push(TAG_LIST);
            buf.extend_from_slice(&(items.len() as u32).to_le_bytes());
            for item in items {
                write_value(buf, item);
            }
        }
    }
}

/// Deserializes a single [`Value`] from binary format.
///
/// Returns `(value, bytes_consumed)`.
///
/// # Errors
///
/// Returns an error if the data is malformed or truncated.
pub fn deserialize_value(data: &[u8]) -> Result<(Value, usize), StorageError> {
    if data.is_empty() {
        return Err(StorageError {
            message: "empty data for value deserialization".into(),
            source: None,
        });
    }

    let tag = data[0];
    let rest = &data[1..];

    match tag {
        TAG_NULL => Ok((Value::Null, 1)),
        TAG_BOOL => {
            check_len(rest, 1, "Bool")?;
            Ok((Value::Bool(rest[0] != 0), 2))
        }
        TAG_I64 => {
            check_len(rest, 8, "I64")?;
            let v = i64::from_le_bytes(rest[..8].try_into().unwrap());
            Ok((Value::I64(v), 9))
        }
        TAG_U64 => {
            check_len(rest, 8, "U64")?;
            let v = u64::from_le_bytes(rest[..8].try_into().unwrap());
            Ok((Value::U64(v), 9))
        }
        TAG_F64 => {
            check_len(rest, 8, "F64")?;
            let v = f64::from_le_bytes(rest[..8].try_into().unwrap());
            Ok((Value::F64(v), 9))
        }
        TAG_STRING => {
            check_len(rest, 4, "String length")?;
            let len = u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize;
            check_len(&rest[4..], len, "String data")?;
            let s = std::str::from_utf8(&rest[4..4 + len])
                .map_err(|e| StorageError {
                    message: format!("invalid UTF-8 in String value: {e}"),
                    source: None,
                })?
                .to_string();
            Ok((Value::String(s), 1 + 4 + len))
        }
        TAG_BYTES => {
            check_len(rest, 4, "Bytes length")?;
            let len = u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize;
            check_len(&rest[4..], len, "Bytes data")?;
            let b = rest[4..4 + len].to_vec();
            Ok((Value::Bytes(b), 1 + 4 + len))
        }
        TAG_NODE_REF => {
            check_len(rest, 8, "NodeRef")?;
            let id = NodeId(u64::from_le_bytes(rest[..8].try_into().unwrap()));
            Ok((Value::NodeRef(id), 9))
        }
        TAG_LANG_STRING => {
            check_len(rest, 4, "LangString value length")?;
            let val_len = u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize;
            check_len(&rest[4..], val_len, "LangString value")?;
            let value = std::str::from_utf8(&rest[4..4 + val_len])
                .map_err(|e| StorageError {
                    message: format!("invalid UTF-8 in LangString value: {e}"),
                    source: None,
                })?
                .to_string();
            let after_val = 4 + val_len;
            check_len(&rest[after_val..], 4, "LangString lang length")?;
            let lang_len =
                u32::from_le_bytes(rest[after_val..after_val + 4].try_into().unwrap()) as usize;
            check_len(&rest[after_val + 4..], lang_len, "LangString lang")?;
            let lang = std::str::from_utf8(&rest[after_val + 4..after_val + 4 + lang_len])
                .map_err(|e| StorageError {
                    message: format!("invalid UTF-8 in LangString lang: {e}"),
                    source: None,
                })?
                .to_string();
            Ok((
                Value::LangString { value, lang },
                1 + after_val + 4 + lang_len,
            ))
        }
        TAG_LIST => {
            check_len(rest, 4, "List count")?;
            let count = u32::from_le_bytes(rest[..4].try_into().unwrap()) as usize;
            let mut items = Vec::with_capacity(count);
            let mut offset = 1 + 4; // tag + count
            for _ in 0..count {
                let (item, consumed) = deserialize_value(&data[offset..])?;
                items.push(item);
                offset += consumed;
            }
            Ok((Value::List(items), offset))
        }
        _ => Err(StorageError {
            message: format!("unknown value type tag: {tag:#04x}"),
            source: None,
        }),
    }
}

fn check_len(data: &[u8], needed: usize, context: &str) -> Result<(), StorageError> {
    if data.len() < needed {
        Err(StorageError {
            message: format!(
                "{context}: need {needed} bytes, have {}",
                data.len()
            ),
            source: None,
        })
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Property serialization
// ---------------------------------------------------------------------------

/// Serializes a [`PropertyMap`] to binary format.
///
/// Format: `[entry_count: u16 LE] [entries...]`
/// Each entry: `[PropertyKeyId: u32 LE] [value_tag: u8] [payload]`
pub fn serialize_properties(props: &PropertyMap) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&(props.len() as u16).to_le_bytes());
    for (key_id, value) in props {
        buf.extend_from_slice(&key_id.0.to_le_bytes());
        write_value(&mut buf, value);
    }
    buf
}

/// Deserializes a [`PropertyMap`] from binary format.
///
/// # Errors
///
/// Returns an error if the data is malformed.
pub fn deserialize_properties(data: &[u8]) -> Result<PropertyMap, StorageError> {
    if data.is_empty() {
        return Ok(PropertyMap::new());
    }
    check_len(data, 2, "PropertyMap entry_count")?;
    let count = u16::from_le_bytes([data[0], data[1]]) as usize;
    let mut map = PropertyMap::new();
    let mut offset = 2;

    for _ in 0..count {
        check_len(&data[offset..], 4, "PropertyMap key_id")?;
        let key_id =
            PropertyKeyId(u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()));
        offset += 4;
        let (value, consumed) = deserialize_value(&data[offset..])?;
        offset += consumed;
        map.insert(key_id, value);
    }

    Ok(map)
}

// ---------------------------------------------------------------------------
// NodeRecord
// ---------------------------------------------------------------------------

/// Binary representation of a node in the Node Store B-tree.
///
/// # Layout (per `007-graph-storage-model.md` §5.1)
///
/// | Offset | Size | Field |
/// |--------|------|-------|
/// | 0      | 1    | flags (bit 0: is_anonymous) |
/// | 1      | 1    | type_count |
/// | 2      | 4    | primary_type (u32 LE) |
/// | 6      | 4    | property_size (u32 LE) |
/// | 10     | 8    | overflow_page_id (u64 LE) |
/// | 18     | 4*(N-1) | extra_types (u32 LE each) |
/// | 18+4*(N-1) | S | inline_properties |
#[derive(Clone, Debug)]
pub struct NodeRecord {
    /// Bit 0: is_anonymous.
    pub flags: u8,
    /// Number of type labels (0–255).
    pub type_count: u8,
    /// First type ID (or `TypeId(0)` if type_count == 0).
    pub primary_type: TypeId,
    /// Byte length of inline properties.
    pub property_size: u32,
    /// Overflow page ID (0 if properties are inline).
    pub overflow_page_id: PageId,
    /// Additional type IDs beyond the primary (if type_count > 1).
    pub extra_types: Vec<TypeId>,
    /// Inline serialized property bytes.
    pub inline_properties: Vec<u8>,
}

impl NodeRecord {
    /// Serializes this record to binary format (little-endian values).
    pub fn serialize(&self) -> Vec<u8> {
        let extra_count = self.extra_types.len();
        let total = 18 + extra_count * 4 + self.inline_properties.len();
        let mut buf = Vec::with_capacity(total);
        buf.push(self.flags);
        buf.push(self.type_count);
        buf.extend_from_slice(&self.primary_type.0.to_le_bytes());
        buf.extend_from_slice(&self.property_size.to_le_bytes());
        buf.extend_from_slice(&self.overflow_page_id.0.to_le_bytes());
        for tid in &self.extra_types {
            buf.extend_from_slice(&tid.0.to_le_bytes());
        }
        buf.extend_from_slice(&self.inline_properties);
        buf
    }

    /// Deserializes a `NodeRecord` from binary data.
    ///
    /// # Errors
    ///
    /// Returns an error if the data is too short or malformed.
    pub fn deserialize(data: &[u8]) -> Result<Self, StorageError> {
        check_len(data, 18, "NodeRecord minimum")?;
        let flags = data[0];
        let type_count = data[1];
        let primary_type = TypeId(u32::from_le_bytes(data[2..6].try_into().unwrap()));
        let property_size = u32::from_le_bytes(data[6..10].try_into().unwrap());
        let overflow_page_id = PageId(u64::from_le_bytes(data[10..18].try_into().unwrap()));

        let extra_count = if type_count > 1 {
            (type_count - 1) as usize
        } else {
            0
        };
        let extra_start = 18;
        let extra_end = extra_start + extra_count * 4;
        check_len(data, extra_end, "NodeRecord extra_types")?;

        let mut extra_types = Vec::with_capacity(extra_count);
        for i in 0..extra_count {
            let off = extra_start + i * 4;
            extra_types.push(TypeId(u32::from_le_bytes(
                data[off..off + 4].try_into().unwrap(),
            )));
        }

        let prop_end = extra_end + property_size as usize;
        check_len(data, prop_end, "NodeRecord inline_properties")?;
        let inline_properties = data[extra_end..prop_end].to_vec();

        Ok(Self {
            flags,
            type_count,
            primary_type,
            property_size,
            overflow_page_id,
            extra_types,
            inline_properties,
        })
    }

    /// Constructs a `NodeRecord` from a [`Node`] and pre-serialized property bytes.
    pub fn from_node(node: &Node, serialized_props: &[u8], overflow_page: Option<PageId>) -> Self {
        let type_count = node.type_labels.len().min(255) as u8;
        let primary_type = node.type_labels.first().copied().unwrap_or(TypeId::NULL);
        let extra_types: Vec<TypeId> = if node.type_labels.len() > 1 {
            node.type_labels[1..].to_vec()
        } else {
            Vec::new()
        };

        Self {
            flags: if node.is_anonymous { 0x01 } else { 0x00 },
            type_count,
            primary_type,
            property_size: serialized_props.len() as u32,
            overflow_page_id: overflow_page.unwrap_or(PageId::NULL),
            extra_types,
            inline_properties: serialized_props.to_vec(),
        }
    }

    /// Reconstructs a [`Node`] from this record.
    pub fn to_node(&self, node_id: NodeId, properties: PropertyMap) -> Node {
        let mut type_labels = Vec::with_capacity(self.type_count as usize);
        if self.type_count > 0 {
            type_labels.push(self.primary_type);
            type_labels.extend_from_slice(&self.extra_types);
        }

        Node {
            id: node_id,
            type_labels,
            properties,
            is_anonymous: (self.flags & 0x01) != 0,
        }
    }
}

// ---------------------------------------------------------------------------
// EdgeRecord
// ---------------------------------------------------------------------------

/// Binary representation of an edge in the Edge Store B-tree.
///
/// # Layout (per `007-graph-storage-model.md` §5.2)
///
/// | Offset | Size | Field |
/// |--------|------|-------|
/// | 0      | 1    | flags (reserved, must be 0) |
/// | 1      | 1    | type_count |
/// | 2      | 4    | primary_type (u32 LE) |
/// | 6      | 8    | source (u64 LE) |
/// | 14     | 8    | target (u64 LE) |
/// | 22     | 4    | property_size (u32 LE) |
/// | 26     | 8    | overflow_page_id (u64 LE) |
/// | 34     | 4*(N-1) | extra_types |
/// | 34+4*(N-1) | S | inline_properties |
#[derive(Clone, Debug)]
pub struct EdgeRecord {
    /// Reserved flags (must be 0).
    pub flags: u8,
    /// Number of type labels.
    pub type_count: u8,
    /// First type ID.
    pub primary_type: TypeId,
    /// Source node ID.
    pub source: NodeId,
    /// Target node ID.
    pub target: NodeId,
    /// Byte length of inline properties.
    pub property_size: u32,
    /// Overflow page ID (0 if inline).
    pub overflow_page_id: PageId,
    /// Additional type IDs.
    pub extra_types: Vec<TypeId>,
    /// Inline serialized property bytes.
    pub inline_properties: Vec<u8>,
}

impl EdgeRecord {
    /// Serializes this record to binary format.
    pub fn serialize(&self) -> Vec<u8> {
        let extra_count = self.extra_types.len();
        let total = 34 + extra_count * 4 + self.inline_properties.len();
        let mut buf = Vec::with_capacity(total);
        buf.push(self.flags);
        buf.push(self.type_count);
        buf.extend_from_slice(&self.primary_type.0.to_le_bytes());
        buf.extend_from_slice(&self.source.0.to_le_bytes());
        buf.extend_from_slice(&self.target.0.to_le_bytes());
        buf.extend_from_slice(&self.property_size.to_le_bytes());
        buf.extend_from_slice(&self.overflow_page_id.0.to_le_bytes());
        for tid in &self.extra_types {
            buf.extend_from_slice(&tid.0.to_le_bytes());
        }
        buf.extend_from_slice(&self.inline_properties);
        buf
    }

    /// Deserializes an `EdgeRecord` from binary data.
    ///
    /// # Errors
    ///
    /// Returns an error if the data is too short or malformed.
    pub fn deserialize(data: &[u8]) -> Result<Self, StorageError> {
        check_len(data, 34, "EdgeRecord minimum")?;
        let flags = data[0];
        let type_count = data[1];
        let primary_type = TypeId(u32::from_le_bytes(data[2..6].try_into().unwrap()));
        let source = NodeId(u64::from_le_bytes(data[6..14].try_into().unwrap()));
        let target = NodeId(u64::from_le_bytes(data[14..22].try_into().unwrap()));
        let property_size = u32::from_le_bytes(data[22..26].try_into().unwrap());
        let overflow_page_id = PageId(u64::from_le_bytes(data[26..34].try_into().unwrap()));

        let extra_count = if type_count > 1 {
            (type_count - 1) as usize
        } else {
            0
        };
        let extra_start = 34;
        let extra_end = extra_start + extra_count * 4;
        check_len(data, extra_end, "EdgeRecord extra_types")?;

        let mut extra_types = Vec::with_capacity(extra_count);
        for i in 0..extra_count {
            let off = extra_start + i * 4;
            extra_types.push(TypeId(u32::from_le_bytes(
                data[off..off + 4].try_into().unwrap(),
            )));
        }

        let prop_end = extra_end + property_size as usize;
        check_len(data, prop_end, "EdgeRecord inline_properties")?;
        let inline_properties = data[extra_end..prop_end].to_vec();

        Ok(Self {
            flags,
            type_count,
            primary_type,
            source,
            target,
            property_size,
            overflow_page_id,
            extra_types,
            inline_properties,
        })
    }

    /// Constructs an `EdgeRecord` from an [`Edge`] and pre-serialized property bytes.
    pub fn from_edge(edge: &Edge, serialized_props: &[u8], overflow_page: Option<PageId>) -> Self {
        let type_count = edge.type_labels.len().min(255) as u8;
        let primary_type = edge.type_labels.first().copied().unwrap_or(TypeId::NULL);
        let extra_types: Vec<TypeId> = if edge.type_labels.len() > 1 {
            edge.type_labels[1..].to_vec()
        } else {
            Vec::new()
        };

        Self {
            flags: 0,
            type_count,
            primary_type,
            source: edge.source,
            target: edge.target,
            property_size: serialized_props.len() as u32,
            overflow_page_id: overflow_page.unwrap_or(PageId::NULL),
            extra_types,
            inline_properties: serialized_props.to_vec(),
        }
    }

    /// Reconstructs an [`Edge`] from this record.
    pub fn to_edge(&self, edge_id: EdgeId, properties: PropertyMap) -> Edge {
        let mut type_labels = Vec::with_capacity(self.type_count as usize);
        if self.type_count > 0 {
            type_labels.push(self.primary_type);
            type_labels.extend_from_slice(&self.extra_types);
        }

        Edge {
            id: edge_id,
            type_labels,
            source: self.source,
            target: self.target,
            properties,
        }
    }
}

// ---------------------------------------------------------------------------
// Schema Store value serialization
// ---------------------------------------------------------------------------

/// Serializes a [`TypeDefinition`] for storage in the Schema Store.
pub fn serialize_type_definition(td: &TypeDefinition) -> Vec<u8> {
    let mut buf = Vec::new();
    // kind: u8 (0=Node, 1=Edge)
    buf.push(match td.kind {
        TypeKind::Node => 0x00,
        TypeKind::Edge => 0x01,
    });
    // open: u8
    buf.push(if td.open { 1 } else { 0 });
    // name: length-prefixed string
    buf.extend_from_slice(&(td.name.len() as u32).to_le_bytes());
    buf.extend_from_slice(td.name.as_bytes());
    // supertypes count + ids
    buf.extend_from_slice(&(td.supertypes.len() as u16).to_le_bytes());
    for st in &td.supertypes {
        buf.extend_from_slice(&st.0.to_le_bytes());
    }
    // property declarations count
    buf.extend_from_slice(&(td.property_declarations.len() as u16).to_le_bytes());
    for pd in &td.property_declarations {
        serialize_property_declaration(&mut buf, pd);
    }
    // metadata
    let meta_bytes = serialize_properties(&td.metadata);
    buf.extend_from_slice(&meta_bytes);
    buf
}

/// Deserializes a [`TypeDefinition`] from Schema Store bytes.
///
/// The `type_id` is provided separately (from the key).
///
/// # Errors
///
/// Returns an error if the data is malformed.
pub fn deserialize_type_definition(
    type_id: TypeId,
    data: &[u8],
) -> Result<TypeDefinition, StorageError> {
    check_len(data, 2, "TypeDefinition kind+open")?;
    let kind = match data[0] {
        0x00 => TypeKind::Node,
        0x01 => TypeKind::Edge,
        other => {
            return Err(StorageError {
                message: format!("unknown TypeKind tag: {other:#04x}"),
                source: None,
            })
        }
    };
    let open = data[1] != 0;
    let mut offset = 2;

    // name
    check_len(&data[offset..], 4, "TypeDefinition name length")?;
    let name_len = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
    offset += 4;
    check_len(&data[offset..], name_len, "TypeDefinition name")?;
    let name = std::str::from_utf8(&data[offset..offset + name_len])
        .map_err(|e| StorageError {
            message: format!("invalid UTF-8 in TypeDefinition name: {e}"),
            source: None,
        })?
        .to_string();
    offset += name_len;

    // supertypes
    check_len(&data[offset..], 2, "TypeDefinition supertypes count")?;
    let st_count = u16::from_le_bytes([data[offset], data[offset + 1]]) as usize;
    offset += 2;
    let mut supertypes = Vec::with_capacity(st_count);
    for _ in 0..st_count {
        check_len(&data[offset..], 4, "TypeDefinition supertype")?;
        supertypes.push(TypeId(u32::from_le_bytes(
            data[offset..offset + 4].try_into().unwrap(),
        )));
        offset += 4;
    }

    // property declarations
    check_len(&data[offset..], 2, "TypeDefinition prop decl count")?;
    let pd_count = u16::from_le_bytes([data[offset], data[offset + 1]]) as usize;
    offset += 2;
    let mut property_declarations = Vec::with_capacity(pd_count);
    for _ in 0..pd_count {
        let (pd, consumed) = deserialize_property_declaration(&data[offset..])?;
        property_declarations.push(pd);
        offset += consumed;
    }

    // metadata
    let metadata = deserialize_properties(&data[offset..])?;

    Ok(TypeDefinition {
        id: type_id,
        name,
        kind,
        supertypes,
        property_declarations,
        open,
        metadata,
    })
}

fn serialize_property_declaration(buf: &mut Vec<u8>, pd: &PropertyDeclaration) {
    buf.extend_from_slice(&pd.key.0.to_le_bytes());
    serialize_value_type_descriptor(buf, &pd.value_type);
    buf.push(if pd.required { 1 } else { 0 });
    buf.push(if pd.multi_valued { 1 } else { 0 });
    let meta = serialize_properties(&pd.metadata);
    buf.extend_from_slice(&meta);
}

fn deserialize_property_declaration(
    data: &[u8],
) -> Result<(PropertyDeclaration, usize), StorageError> {
    let mut offset = 0;
    check_len(data, 4, "PropertyDeclaration key")?;
    let key = PropertyKeyId(u32::from_le_bytes(data[0..4].try_into().unwrap()));
    offset += 4;

    let (value_type, vt_consumed) = deserialize_value_type_descriptor(&data[offset..])?;
    offset += vt_consumed;

    check_len(&data[offset..], 2, "PropertyDeclaration flags")?;
    let required = data[offset] != 0;
    let multi_valued = data[offset + 1] != 0;
    offset += 2;

    // Read metadata PropertyMap
    let meta_data = &data[offset..];
    let metadata = deserialize_properties(meta_data)?;
    // Calculate how many bytes the metadata consumed
    let meta_bytes = serialize_properties(&metadata);
    offset += meta_bytes.len();

    Ok((
        PropertyDeclaration {
            key,
            value_type,
            required,
            multi_valued,
            metadata,
        },
        offset,
    ))
}

fn serialize_value_type_descriptor(buf: &mut Vec<u8>, vtd: &ValueTypeDescriptor) {
    match vtd {
        ValueTypeDescriptor::Any => buf.push(0x00),
        ValueTypeDescriptor::Bool => buf.push(0x01),
        ValueTypeDescriptor::I64 => buf.push(0x02),
        ValueTypeDescriptor::U64 => buf.push(0x03),
        ValueTypeDescriptor::F64 => buf.push(0x04),
        ValueTypeDescriptor::String => buf.push(0x05),
        ValueTypeDescriptor::Bytes => buf.push(0x06),
        ValueTypeDescriptor::NodeRef => buf.push(0x07),
        ValueTypeDescriptor::LangString => buf.push(0x08),
        ValueTypeDescriptor::List(inner) => {
            buf.push(0x09);
            serialize_value_type_descriptor(buf, inner);
        }
    }
}

fn deserialize_value_type_descriptor(
    data: &[u8],
) -> Result<(ValueTypeDescriptor, usize), StorageError> {
    check_len(data, 1, "ValueTypeDescriptor tag")?;
    match data[0] {
        0x00 => Ok((ValueTypeDescriptor::Any, 1)),
        0x01 => Ok((ValueTypeDescriptor::Bool, 1)),
        0x02 => Ok((ValueTypeDescriptor::I64, 1)),
        0x03 => Ok((ValueTypeDescriptor::U64, 1)),
        0x04 => Ok((ValueTypeDescriptor::F64, 1)),
        0x05 => Ok((ValueTypeDescriptor::String, 1)),
        0x06 => Ok((ValueTypeDescriptor::Bytes, 1)),
        0x07 => Ok((ValueTypeDescriptor::NodeRef, 1)),
        0x08 => Ok((ValueTypeDescriptor::LangString, 1)),
        0x09 => {
            let (inner, consumed) = deserialize_value_type_descriptor(&data[1..])?;
            Ok((ValueTypeDescriptor::List(Box::new(inner)), 1 + consumed))
        }
        other => Err(StorageError {
            message: format!("unknown ValueTypeDescriptor tag: {other:#04x}"),
            source: None,
        }),
    }
}

/// Serializes a property key name (length-prefixed UTF-8 string).
pub fn serialize_property_key_name(name: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4 + name.len());
    buf.extend_from_slice(&(name.len() as u32).to_le_bytes());
    buf.extend_from_slice(name.as_bytes());
    buf
}

/// Deserializes a property key name.
///
/// # Errors
///
/// Returns an error if the data is malformed.
pub fn deserialize_property_key_name(data: &[u8]) -> Result<String, StorageError> {
    check_len(data, 4, "property key name length")?;
    let len = u32::from_le_bytes(data[..4].try_into().unwrap()) as usize;
    check_len(&data[4..], len, "property key name")?;
    std::str::from_utf8(&data[4..4 + len])
        .map(|s| s.to_string())
        .map_err(|e| StorageError {
            message: format!("invalid UTF-8 in property key name: {e}"),
            source: None,
        })
}

/// Serializes a counter value (u64 LE).
pub fn serialize_counter(value: u64) -> [u8; 8] {
    value.to_le_bytes()
}

/// Deserializes a counter value.
///
/// # Errors
///
/// Returns an error if the data is too short.
pub fn deserialize_counter(data: &[u8]) -> Result<u64, StorageError> {
    check_len(data, 8, "counter value")?;
    Ok(u64::from_le_bytes(data[..8].try_into().unwrap()))
}

/// Serializes a [`ProvenanceRecord`].
///
/// Format: `[txn_id: 8B LE] [rule_name_len: 2B LE] [rule_name: UTF-8]`
pub fn serialize_provenance(record: &ProvenanceRecord) -> Vec<u8> {
    let mut buf = Vec::with_capacity(10 + record.rule_name.len());
    buf.extend_from_slice(&record.materialized_at.to_le_bytes());
    buf.extend_from_slice(&(record.rule_name.len() as u16).to_le_bytes());
    buf.extend_from_slice(record.rule_name.as_bytes());
    buf
}

/// Deserializes a [`ProvenanceRecord`].
///
/// # Errors
///
/// Returns an error if the data is malformed.
pub fn deserialize_provenance(data: &[u8]) -> Result<ProvenanceRecord, StorageError> {
    check_len(data, 10, "ProvenanceRecord minimum")?;
    let materialized_at = u64::from_le_bytes(data[..8].try_into().unwrap());
    let name_len = u16::from_le_bytes([data[8], data[9]]) as usize;
    check_len(&data[10..], name_len, "ProvenanceRecord rule_name")?;
    let rule_name = std::str::from_utf8(&data[10..10 + name_len])
        .map_err(|e| StorageError {
            message: format!("invalid UTF-8 in provenance rule_name: {e}"),
            source: None,
        })?
        .to_string();
    Ok(ProvenanceRecord {
        rule_name,
        materialized_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    // -- Key encoding sort order tests --

    #[test]
    fn node_key_big_endian_sort_order() {
        let ids = [NodeId(1), NodeId(256), NodeId(0), NodeId(u64::MAX), NodeId(1000)];
        let mut encoded: Vec<([u8; 8], NodeId)> = ids
            .iter()
            .map(|&id| (encode_node_key(id), id))
            .collect();
        encoded.sort_by(|a, b| a.0.cmp(&b.0));
        let sorted_ids: Vec<NodeId> = encoded.iter().map(|(_, id)| *id).collect();
        // Should be in ascending NodeId order
        for i in 1..sorted_ids.len() {
            assert!(sorted_ids[i - 1].0 <= sorted_ids[i].0);
        }
        assert_eq!(sorted_ids[0], NodeId(0));
        assert_eq!(sorted_ids[4], NodeId(u64::MAX));
    }

    #[test]
    fn node_id_1_less_than_256_in_byte_order() {
        let k1 = encode_node_key(NodeId(1));
        let k256 = encode_node_key(NodeId(256));
        assert!(k1 < k256);
    }

    #[test]
    fn adjacency_key_sort_order() {
        let k1 = encode_outgoing_adj_key(NodeId(1), TypeId(1), EdgeId(1));
        let k2 = encode_outgoing_adj_key(NodeId(1), TypeId(2), EdgeId(1));
        let k3 = encode_outgoing_adj_key(NodeId(2), TypeId(1), EdgeId(1));
        assert!(k1 < k2); // same node, different type
        assert!(k2 < k3); // different node
    }

    #[test]
    fn type_index_key_sort_order() {
        let k1 = encode_type_index_key(0x00, TypeId(1), 100);
        let k2 = encode_type_index_key(0x00, TypeId(1), 200);
        let k3 = encode_type_index_key(0x01, TypeId(1), 1);
        assert!(k1 < k2); // same type, different entity
        assert!(k2 < k3); // node < edge kind
    }

    #[test]
    fn page_freelist_key_sort_order() {
        let k1 = encode_page_freelist_key(5, PageId(10));
        let k2 = encode_page_freelist_key(5, PageId(20));
        let k3 = encode_page_freelist_key(10, PageId(1));
        assert!(k1 < k2); // same txn, different page
        assert!(k2 < k3); // older txn < newer txn
    }

    #[test]
    fn id_freelist_key_round_trip() {
        let key = encode_id_freelist_key(0x01, 42);
        let (tag, id) = decode_id_freelist_key(&key);
        assert_eq!(tag, 0x01);
        assert_eq!(id, 42);
    }

    // -- NodeRecord tests --

    #[test]
    fn node_record_round_trip() {
        let mut props = PropertyMap::new();
        props.insert(PropertyKeyId(1), Value::String("hello".into()));
        props.insert(PropertyKeyId(2), Value::I64(42));
        props.insert(PropertyKeyId(3), Value::Bool(true));

        let serialized_props = serialize_properties(&props);

        let node = Node {
            id: NodeId(100),
            type_labels: vec![TypeId(5)],
            properties: props.clone(),
            is_anonymous: false,
        };
        let record = NodeRecord::from_node(&node, &serialized_props, None);
        let bytes = record.serialize();
        let rt = NodeRecord::deserialize(&bytes).unwrap();

        assert_eq!(rt.flags, 0);
        assert_eq!(rt.type_count, 1);
        assert_eq!(rt.primary_type, TypeId(5));
        assert!(rt.overflow_page_id.is_null());

        let rt_node = rt.to_node(NodeId(100), props.clone());
        assert_eq!(rt_node.id, NodeId(100));
        assert_eq!(rt_node.type_labels, vec![TypeId(5)]);
        assert!(!rt_node.is_anonymous);
    }

    #[test]
    fn node_record_with_overflow() {
        let record = NodeRecord {
            flags: 0x01,
            type_count: 0,
            primary_type: TypeId::NULL,
            property_size: 0,
            overflow_page_id: PageId(99),
            extra_types: vec![],
            inline_properties: vec![],
        };
        let bytes = record.serialize();
        let rt = NodeRecord::deserialize(&bytes).unwrap();
        assert_eq!(rt.overflow_page_id, PageId(99));
        assert_eq!(rt.flags, 0x01);
    }

    // -- EdgeRecord tests --

    #[test]
    fn edge_record_round_trip() {
        let mut props = PropertyMap::new();
        props.insert(PropertyKeyId(1), Value::F64(3.125));

        let serialized_props = serialize_properties(&props);
        let edge = Edge {
            id: EdgeId(200),
            type_labels: vec![TypeId(10)],
            source: NodeId(1),
            target: NodeId(2),
            properties: props.clone(),
        };
        let record = EdgeRecord::from_edge(&edge, &serialized_props, None);
        let bytes = record.serialize();
        let rt = EdgeRecord::deserialize(&bytes).unwrap();

        assert_eq!(rt.source, NodeId(1));
        assert_eq!(rt.target, NodeId(2));
        assert_eq!(rt.primary_type, TypeId(10));

        let rt_edge = rt.to_edge(EdgeId(200), props);
        assert_eq!(rt_edge.id, EdgeId(200));
        assert_eq!(rt_edge.source, NodeId(1));
        assert_eq!(rt_edge.target, NodeId(2));
    }

    // -- Property serialization tests --

    #[test]
    fn property_round_trip_all_types() {
        let mut props = PropertyMap::new();
        props.insert(PropertyKeyId(1), Value::Null);
        props.insert(PropertyKeyId(2), Value::Bool(true));
        props.insert(PropertyKeyId(3), Value::I64(-100));
        props.insert(PropertyKeyId(4), Value::U64(999));
        props.insert(PropertyKeyId(5), Value::F64(2.5));
        props.insert(PropertyKeyId(6), Value::String("test".into()));
        props.insert(PropertyKeyId(7), Value::Bytes(vec![0xDE, 0xAD]));
        props.insert(PropertyKeyId(8), Value::NodeRef(NodeId(42)));
        props.insert(
            PropertyKeyId(9),
            Value::LangString {
                value: "hello".into(),
                lang: "en".into(),
            },
        );
        props.insert(
            PropertyKeyId(10),
            Value::List(vec![Value::I64(1), Value::String("two".into())]),
        );

        let bytes = serialize_properties(&props);
        let rt = deserialize_properties(&bytes).unwrap();
        assert_eq!(rt.len(), props.len());
        for (k, v) in &props {
            assert_eq!(rt.get(k).unwrap(), v);
        }
    }

    #[test]
    fn empty_property_map() {
        let props = PropertyMap::new();
        let bytes = serialize_properties(&props);
        assert_eq!(bytes.len(), 2); // just the entry_count
        let rt = deserialize_properties(&bytes).unwrap();
        assert!(rt.is_empty());
    }

    #[test]
    fn empty_data_deserializes_to_empty_map() {
        let rt = deserialize_properties(&[]).unwrap();
        assert!(rt.is_empty());
    }

    #[test]
    fn nested_list_round_trip() {
        let nested = Value::List(vec![
            Value::List(vec![Value::I64(1), Value::I64(2)]),
            Value::String("x".into()),
        ]);
        let bytes = serialize_value(&nested);
        let (rt, consumed) = deserialize_value(&bytes).unwrap();
        assert_eq!(rt, nested);
        assert_eq!(consumed, bytes.len());
    }

    // -- Schema Store serialization tests --

    #[test]
    fn type_definition_round_trip() {
        let td = TypeDefinition {
            id: TypeId(5),
            name: "Person".into(),
            kind: TypeKind::Node,
            supertypes: vec![TypeId(1), TypeId(2)],
            property_declarations: vec![PropertyDeclaration {
                key: PropertyKeyId(10),
                value_type: ValueTypeDescriptor::String,
                required: true,
                multi_valued: false,
                metadata: BTreeMap::new(),
            }],
            open: true,
            metadata: BTreeMap::new(),
        };

        let bytes = serialize_type_definition(&td);
        let rt = deserialize_type_definition(TypeId(5), &bytes).unwrap();

        assert_eq!(rt.id, TypeId(5));
        assert_eq!(rt.name, "Person");
        assert_eq!(rt.kind, TypeKind::Node);
        assert!(rt.open);
        assert_eq!(rt.supertypes, vec![TypeId(1), TypeId(2)]);
        assert_eq!(rt.property_declarations.len(), 1);
        assert_eq!(rt.property_declarations[0].key, PropertyKeyId(10));
        assert_eq!(
            rt.property_declarations[0].value_type,
            ValueTypeDescriptor::String
        );
        assert!(rt.property_declarations[0].required);
    }

    #[test]
    fn property_key_name_round_trip() {
        let bytes = serialize_property_key_name("my_property");
        let rt = deserialize_property_key_name(&bytes).unwrap();
        assert_eq!(rt, "my_property");
    }

    #[test]
    fn counter_round_trip() {
        let bytes = serialize_counter(12345);
        let rt = deserialize_counter(&bytes).unwrap();
        assert_eq!(rt, 12345);
    }

    #[test]
    fn provenance_round_trip() {
        let record = ProvenanceRecord {
            rule_name: "test_rule".into(),
            materialized_at: 42,
        };
        let bytes = serialize_provenance(&record);
        let rt = deserialize_provenance(&bytes).unwrap();
        assert_eq!(rt.rule_name, "test_rule");
        assert_eq!(rt.materialized_at, 42);
    }

    #[test]
    fn node_key_edge_cases() {
        let k0 = encode_node_key(NodeId(0));
        assert_eq!(k0, [0, 0, 0, 0, 0, 0, 0, 0]);

        let k_max = encode_node_key(NodeId(u64::MAX));
        assert_eq!(k_max, [0xFF; 8]);
    }

    #[test]
    fn schema_key_prefixes() {
        assert_eq!(encode_schema_type_key(TypeId(1))[0], 0x01);
        assert_eq!(encode_schema_property_key(PropertyKeyId(1))[0], 0x02);
        assert_eq!(encode_schema_counter_key(0x01)[0], 0x03);
        assert_eq!(
            encode_schema_hierarchy_key(TypeId(1), TypeId(2))[0],
            0x04
        );
        assert_eq!(encode_schema_extension_key(0x01, "test")[0], 0x05);
        assert_eq!(
            encode_schema_provenance_key(0x01, 1, 0)[0],
            0x06
        );
    }
}
