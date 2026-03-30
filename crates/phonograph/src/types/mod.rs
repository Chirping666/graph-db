//! Core data types for the graph database.
//!
//! This module defines the foundational types used throughout the crate:
//! identity newtypes, the dynamically-typed value system, property maps,
//! node and edge structs, and the type/schema system.

use alloc::{
    boxed::Box,
    collections::BTreeMap,
    string::String,
    vec::Vec,
};

use core::fmt;

// ---------------------------------------------------------------------------
// Identity types
// ---------------------------------------------------------------------------

/// Unique identifier for a node in the graph.
///
/// Wraps a `u64`. The value `0` is reserved as a null sentinel
/// (see [`NodeId::NULL`] and [`NodeId::is_null`]).
///
/// # Examples
///
/// ```
/// use phonograph::NodeId;
///
/// let id = NodeId(42);
/// assert_eq!(id.0, 42);
/// assert!(!id.is_null());
/// assert!(NodeId::NULL.is_null());
/// assert_eq!(format!("{id}"), "42");
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct NodeId(pub u64);

impl NodeId {
    /// The null sentinel value. A `NodeId` of `0` indicates "no node".
    pub const NULL: NodeId = NodeId(0);

    /// Returns `true` if this identifier is the null sentinel.
    pub fn is_null(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for an edge in the graph.
///
/// Wraps a `u64`. The value `0` is reserved as a null sentinel
/// (see [`EdgeId::NULL`] and [`EdgeId::is_null`]).
///
/// # Examples
///
/// ```
/// use phonograph::EdgeId;
///
/// let id = EdgeId(7);
/// assert!(!id.is_null());
/// assert!(EdgeId::NULL.is_null());
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct EdgeId(pub u64);

impl EdgeId {
    /// The null sentinel value. An `EdgeId` of `0` indicates "no edge".
    pub const NULL: EdgeId = EdgeId(0);

    /// Returns `true` if this identifier is the null sentinel.
    pub fn is_null(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for EdgeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for a type definition in the schema.
///
/// Wraps a `u32`. The value `0` is reserved as a null sentinel
/// (see [`TypeId::NULL`] and [`TypeId::is_null`]).
///
/// # Examples
///
/// ```
/// use phonograph::TypeId;
///
/// let id = TypeId(1);
/// assert!(!id.is_null());
/// assert!(TypeId::NULL.is_null());
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct TypeId(pub u32);

impl TypeId {
    /// The null sentinel value. A `TypeId` of `0` indicates "no type".
    pub const NULL: TypeId = TypeId(0);

    /// Returns `true` if this identifier is the null sentinel.
    pub fn is_null(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for TypeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for a property key in the property key registry.
///
/// Wraps a `u32`. The value `0` is reserved as a null sentinel
/// (see [`PropertyKeyId::NULL`] and [`PropertyKeyId::is_null`]).
///
/// # Examples
///
/// ```
/// use phonograph::PropertyKeyId;
///
/// let id = PropertyKeyId(3);
/// assert!(!id.is_null());
/// assert!(PropertyKeyId::NULL.is_null());
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct PropertyKeyId(pub u32);

impl PropertyKeyId {
    /// The null sentinel value. A `PropertyKeyId` of `0` indicates "no key".
    pub const NULL: PropertyKeyId = PropertyKeyId(0);

    /// Returns `true` if this identifier is the null sentinel.
    pub fn is_null(self) -> bool {
        self.0 == 0
    }
}

impl fmt::Display for PropertyKeyId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Value system
// ---------------------------------------------------------------------------

/// A dynamically-typed property value.
///
/// `Value` represents any value that can be stored as a property on a node
/// or edge. It does **not** implement `Eq` because the `F64` variant
/// contains an `f64`, which follows IEEE 754 semantics (NaN ≠ NaN).
///
/// # Examples
///
/// ```
/// use phonograph::{Value, NodeId};
///
/// let s = Value::String("hello".into());
/// assert_eq!(s.as_str(), Some("hello"));
///
/// let n = Value::I64(42);
/// assert_eq!(n.as_i64(), Some(42));
/// assert!(!n.is_null());
///
/// assert!(Value::Null.is_null());
///
/// let r = Value::NodeRef(NodeId(7));
/// assert_eq!(r.as_node_ref(), Some(NodeId(7)));
/// ```
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// The null/absent value.
    Null,
    /// A boolean value.
    Bool(bool),
    /// A signed 64-bit integer.
    I64(i64),
    /// An unsigned 64-bit integer.
    U64(u64),
    /// A 64-bit floating-point number (IEEE 754).
    F64(f64),
    /// A UTF-8 string.
    String(String),
    /// An arbitrary byte sequence.
    Bytes(Vec<u8>),
    /// A reference to another node in the graph.
    NodeRef(NodeId),
    /// A language-tagged string (e.g., `"hello"@en`).
    LangString {
        /// The string value.
        value: String,
        /// The BCP-47 language tag.
        lang: String,
    },
    /// An ordered list of values.
    List(Vec<Value>),
}

impl Value {
    /// Returns `true` if this value is [`Value::Null`].
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Returns the inner `bool` if this is a [`Value::Bool`], or `None`.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(v) => Some(*v),
            _ => None,
        }
    }

    /// Returns the inner `i64` if this is a [`Value::I64`], or `None`.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::I64(v) => Some(*v),
            _ => None,
        }
    }

    /// Returns the inner `u64` if this is a [`Value::U64`], or `None`.
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::U64(v) => Some(*v),
            _ => None,
        }
    }

    /// Returns the inner `f64` if this is a [`Value::F64`], or `None`.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::F64(v) => Some(*v),
            _ => None,
        }
    }

    /// Returns a string slice if this is a [`Value::String`], or `None`.
    ///
    /// Does **not** match [`Value::LangString`] — use pattern matching
    /// to access language-tagged strings.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(v) => Some(v.as_str()),
            _ => None,
        }
    }

    /// Returns a byte slice if this is a [`Value::Bytes`], or `None`.
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(v) => Some(v.as_slice()),
            _ => None,
        }
    }

    /// Returns the referenced [`NodeId`] if this is a [`Value::NodeRef`], or `None`.
    pub fn as_node_ref(&self) -> Option<NodeId> {
        match self {
            Value::NodeRef(id) => Some(*id),
            _ => None,
        }
    }

    /// Deterministic equality comparison that handles `f64` correctly.
    ///
    /// Unlike [`PartialEq`], this method uses [`f64::total_cmp`] semantics for
    /// [`Value::F64`] variants, meaning:
    /// - `NaN == NaN` → `true`
    /// - `0.0 != -0.0`
    ///
    /// For all other variants, this delegates to [`PartialEq`].
    ///
    /// # Examples
    ///
    /// ```
    /// use phonograph::Value;
    ///
    /// // NaN equals NaN under total_eq
    /// assert!(Value::F64(f64::NAN).total_eq(&Value::F64(f64::NAN)));
    ///
    /// // 0.0 and -0.0 are distinct under total_eq
    /// assert!(!Value::F64(0.0).total_eq(&Value::F64(-0.0)));
    ///
    /// // Non-float variants delegate to PartialEq
    /// assert!(Value::I64(42).total_eq(&Value::I64(42)));
    /// ```
    pub fn total_eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::F64(a), Value::F64(b)) => a.total_cmp(b).is_eq(),
            (Value::List(a), Value::List(b)) => {
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.total_eq(y))
            }
            (Value::LangString { value: v1, lang: l1 }, Value::LangString { value: v2, lang: l2 }) => {
                v1 == v2 && l1 == l2
            }
            _ => self == other,
        }
    }

    /// Returns `true` if this value matches the given type descriptor.
    ///
    /// # Matching rules
    ///
    /// - [`ValueTypeDescriptor::Any`] matches every value (including `Null`).
    /// - [`Value::Null`] matches only `Any`.
    /// - [`ValueTypeDescriptor::String`] matches both [`Value::String`] and
    ///   [`Value::LangString`].
    /// - [`ValueTypeDescriptor::List`] matches [`Value::List`] if every item
    ///   in the list matches the inner descriptor. An empty list matches any
    ///   `List(...)` descriptor.
    /// - All other descriptors match the correspondingly named `Value` variant.
    pub fn matches_descriptor(&self, descriptor: &ValueTypeDescriptor) -> bool {
        match descriptor {
            ValueTypeDescriptor::Any => true,
            _ if matches!(self, Value::Null) => false,
            ValueTypeDescriptor::Bool => matches!(self, Value::Bool(_)),
            ValueTypeDescriptor::I64 => matches!(self, Value::I64(_)),
            ValueTypeDescriptor::U64 => matches!(self, Value::U64(_)),
            ValueTypeDescriptor::F64 => matches!(self, Value::F64(_)),
            ValueTypeDescriptor::String => {
                matches!(self, Value::String(_) | Value::LangString { .. })
            }
            ValueTypeDescriptor::Bytes => matches!(self, Value::Bytes(_)),
            ValueTypeDescriptor::NodeRef => matches!(self, Value::NodeRef(_)),
            ValueTypeDescriptor::LangString => matches!(self, Value::LangString { .. }),
            ValueTypeDescriptor::List(inner) => match self {
                Value::List(items) => items.iter().all(|item| item.matches_descriptor(inner)),
                _ => false,
            },
        }
    }
}

/// Describes the expected type of a [`Value`].
///
/// Used in [`PropertyDeclaration`] to specify what kind of value a property
/// expects. Unlike [`Value`], this enum is `Eq` because it contains no
/// floating-point data.
///
/// # Examples
///
/// ```
/// use phonograph::{Value, ValueTypeDescriptor};
///
/// let desc = ValueTypeDescriptor::I64;
/// assert!(Value::I64(42).matches_descriptor(&desc));
/// assert!(!Value::String("hi".into()).matches_descriptor(&desc));
///
/// // Any matches everything
/// assert!(Value::Null.matches_descriptor(&ValueTypeDescriptor::Any));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValueTypeDescriptor {
    /// Matches any value, including [`Value::Null`].
    Any,
    /// Matches [`Value::Bool`].
    Bool,
    /// Matches [`Value::I64`].
    I64,
    /// Matches [`Value::U64`].
    U64,
    /// Matches [`Value::F64`].
    F64,
    /// Matches [`Value::String`] and [`Value::LangString`].
    String,
    /// Matches [`Value::Bytes`].
    Bytes,
    /// Matches [`Value::NodeRef`].
    NodeRef,
    /// Matches [`Value::LangString`] only.
    LangString,
    /// Matches [`Value::List`] where every element matches the inner descriptor.
    List(Box<ValueTypeDescriptor>),
}

// ---------------------------------------------------------------------------
// PropertyMap, Node, Edge
// ---------------------------------------------------------------------------

/// A map from property key identifiers to dynamically-typed values.
///
/// Uses `BTreeMap` rather than `HashMap` to remain `no_std`-compatible
/// and to provide deterministic iteration order.
pub type PropertyMap = BTreeMap<PropertyKeyId, Value>;

/// Deterministic equality comparison for two [`PropertyMap`]s.
///
/// Uses [`Value::total_eq`] for each value, so `f64` comparisons follow
/// [`f64::total_cmp`] semantics (NaN == NaN, 0.0 ≠ −0.0).
///
/// # Examples
///
/// ```
/// use phonograph::{PropertyKeyId, Value, property_map_total_eq};
/// use std::collections::BTreeMap;
///
/// let mut a = BTreeMap::new();
/// a.insert(PropertyKeyId(1), Value::F64(f64::NAN));
///
/// let mut b = BTreeMap::new();
/// b.insert(PropertyKeyId(1), Value::F64(f64::NAN));
///
/// // PartialEq would return false because NaN != NaN
/// assert_ne!(a, b);
/// // total_eq returns true
/// assert!(property_map_total_eq(&a, &b));
/// ```
pub fn property_map_total_eq(a: &PropertyMap, b: &PropertyMap) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|((k1, v1), (k2, v2))| k1 == k2 && v1.total_eq(v2))
}

/// A node in the graph.
///
/// Nodes carry zero or more type labels and a bag of properties.
/// The `type_labels` vector should be kept sorted by the caller;
/// the struct does not enforce this invariant at construction time.
///
/// Does **not** implement `Eq` because [`PropertyMap`] contains [`Value`]
/// which contains `f64`.
///
/// # Examples
///
/// ```
/// use phonograph::{Node, NodeId, TypeId, Value, PropertyKeyId};
/// use std::collections::BTreeMap;
///
/// let mut props = BTreeMap::new();
/// props.insert(PropertyKeyId(1), Value::String("Alice".into()));
///
/// let node = Node {
///     id: NodeId(1),
///     type_labels: vec![TypeId(1)],
///     properties: props,
///     is_anonymous: false,
/// };
/// assert_eq!(node.id, NodeId(1));
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// The unique identifier of this node.
    pub id: NodeId,
    /// The type labels assigned to this node, sorted by `TypeId`.
    pub type_labels: Vec<TypeId>,
    /// The properties stored on this node.
    pub properties: PropertyMap,
    /// Whether this node is anonymous (a blank node / skolem).
    pub is_anonymous: bool,
}

/// An edge in the graph, connecting a source node to a target node.
///
/// Edges carry zero or more type labels and a bag of properties.
/// The `type_labels` vector should be kept sorted by the caller.
///
/// Does **not** implement `Eq` because [`PropertyMap`] contains [`Value`]
/// which contains `f64`.
///
/// # Examples
///
/// ```
/// use phonograph::{Edge, EdgeId, NodeId, TypeId};
/// use std::collections::BTreeMap;
///
/// let edge = Edge {
///     id: EdgeId(1),
///     type_labels: vec![TypeId(1)],
///     source: NodeId(10),
///     target: NodeId(20),
///     properties: BTreeMap::new(),
/// };
/// assert_eq!(edge.source, NodeId(10));
/// assert_eq!(edge.target, NodeId(20));
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    /// The unique identifier of this edge.
    pub id: EdgeId,
    /// The type labels assigned to this edge, sorted by `TypeId`.
    pub type_labels: Vec<TypeId>,
    /// The source (origin) node of this edge.
    pub source: NodeId,
    /// The target (destination) node of this edge.
    pub target: NodeId,
    /// The properties stored on this edge.
    pub properties: PropertyMap,
}

// ---------------------------------------------------------------------------
// Type system
// ---------------------------------------------------------------------------

/// Distinguishes whether a type definition applies to nodes or edges.
///
/// # Examples
///
/// ```
/// use phonograph::TypeKind;
///
/// let k = TypeKind::Node;
/// assert_eq!(format!("{k}"), "Node");
/// assert_ne!(TypeKind::Node, TypeKind::Edge);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TypeKind {
    /// A type that classifies nodes.
    Node,
    /// A type that classifies edges.
    Edge,
}

impl fmt::Display for TypeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypeKind::Node => write!(f, "Node"),
            TypeKind::Edge => write!(f, "Edge"),
        }
    }
}

/// Declares a property that a typed node or edge is expected to carry.
///
/// The core database stores these declarations but does **not** enforce them.
/// Enforcement is performed by downstream [`ConstraintValidator`](crate::constraint::ConstraintValidator)
/// implementations.
///
/// Does **not** implement `Eq` because `metadata` is a [`PropertyMap`]
/// which may contain `f64` values.
///
/// # Examples
///
/// ```
/// use phonograph::{PropertyDeclaration, PropertyKeyId, ValueTypeDescriptor};
/// use std::collections::BTreeMap;
///
/// let decl = PropertyDeclaration {
///     key: PropertyKeyId(1),
///     value_type: ValueTypeDescriptor::String,
///     required: true,
///     multi_valued: false,
///     metadata: BTreeMap::new(),
/// };
/// assert!(decl.required);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct PropertyDeclaration {
    /// The property key this declaration is about.
    pub key: PropertyKeyId,
    /// The expected type of the property value.
    pub value_type: ValueTypeDescriptor,
    /// Whether this property is required on instances of the owning type.
    pub required: bool,
    /// Whether this property may hold multiple values (as a `List`).
    pub multi_valued: bool,
    /// Arbitrary metadata associated with this declaration (e.g., defaults, facets).
    pub metadata: PropertyMap,
}

/// A type definition in the schema.
///
/// Type definitions form a directed acyclic graph (DAG) via the `supertypes`
/// field. Property declarations are inherited from supertypes and may be
/// overridden (shadowed) by subtypes.
///
/// Does **not** implement `Eq` because `metadata` and `property_declarations`
/// transitively contain [`Value`] which contains `f64`.
///
/// # Examples
///
/// ```
/// use phonograph::{TypeDefinition, TypeId, TypeKind};
/// use std::collections::BTreeMap;
///
/// let td = TypeDefinition {
///     id: TypeId(1),
///     name: "Person".into(),
///     kind: TypeKind::Node,
///     supertypes: vec![],
///     property_declarations: vec![],
///     open: true,
///     metadata: BTreeMap::new(),
/// };
/// assert_eq!(td.name, "Person");
/// assert_eq!(td.kind, TypeKind::Node);
/// ```
#[derive(Clone, Debug, PartialEq)]
pub struct TypeDefinition {
    /// The unique identifier of this type.
    pub id: TypeId,
    /// The name of this type, unique within its [`TypeKind`].
    pub name: String,
    /// Whether this type classifies nodes or edges.
    pub kind: TypeKind,
    /// The direct supertypes of this type in the type hierarchy DAG.
    pub supertypes: Vec<TypeId>,
    /// The property declarations owned by this type (not including inherited ones).
    pub property_declarations: Vec<PropertyDeclaration>,
    /// Whether instances may carry undeclared properties beyond those
    /// in `property_declarations` and inherited declarations.
    pub open: bool,
    /// Arbitrary metadata associated with this type definition.
    pub metadata: PropertyMap,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // === ID type tests ===

    macro_rules! test_id_type {
        ($name:ident, $ty:ident, $inner:ty, $val:expr) => {
            mod $name {
                use super::*;

                #[test]
                fn construction() {
                    let id = $ty($val);
                    assert_eq!(id.0, $val);
                }

                #[test]
                fn null_is_null() {
                    assert!($ty::NULL.is_null());
                }

                #[test]
                fn non_null_is_not_null() {
                    assert!(!$ty(1).is_null());
                }

                #[test]
                fn ordering() {
                    assert!($ty(1) < $ty(2));
                    assert!($ty(2) > $ty(1));
                    assert_eq!($ty(5), $ty(5));
                }

                #[test]
                fn display() {
                    assert_eq!(format!("{}", $ty($val)), format!("{}", $val));
                }
            }
        };
    }

    test_id_type!(node_id, NodeId, u64, 42u64);
    test_id_type!(edge_id, EdgeId, u64, 99u64);
    test_id_type!(type_id, TypeId, u32, 7u32);
    test_id_type!(property_key_id, PropertyKeyId, u32, 13u32);

    // === Value tests ===

    #[test]
    fn value_null() {
        let v = Value::Null;
        assert!(v.is_null());
    }

    #[test]
    fn value_bool() {
        let v = Value::Bool(true);
        assert_eq!(v.as_bool(), Some(true));
        assert!(!v.is_null());
    }

    #[test]
    fn value_i64() {
        let v = Value::I64(-42);
        assert_eq!(v.as_i64(), Some(-42));
    }

    #[test]
    fn value_u64() {
        let v = Value::U64(100);
        assert_eq!(v.as_u64(), Some(100));
    }

    #[test]
    fn value_f64() {
        let v = Value::F64(2.72);
        assert_eq!(v.as_f64(), Some(2.72));
    }

    #[test]
    fn value_string() {
        let v = Value::String("hello".into());
        assert_eq!(v.as_str(), Some("hello"));
    }

    #[test]
    fn value_bytes() {
        let v = Value::Bytes(vec![1, 2, 3]);
        assert_eq!(v.as_bytes(), Some(&[1, 2, 3][..]));
    }

    #[test]
    fn value_node_ref() {
        let v = Value::NodeRef(NodeId(7));
        assert_eq!(v.as_node_ref(), Some(NodeId(7)));
    }

    #[test]
    fn value_lang_string() {
        let v = Value::LangString {
            value: "bonjour".into(),
            lang: "fr".into(),
        };
        assert!(v.as_str().is_none());
        assert!(v.as_bool().is_none());
    }

    #[test]
    fn value_list() {
        let v = Value::List(vec![Value::I64(1), Value::I64(2)]);
        assert!(v.as_i64().is_none());
    }

    #[test]
    fn value_helpers_return_none_on_mismatch() {
        let v = Value::Bool(true);
        assert!(v.as_i64().is_none());
        assert!(v.as_u64().is_none());
        assert!(v.as_f64().is_none());
        assert!(v.as_str().is_none());
        assert!(v.as_bytes().is_none());
        assert!(v.as_node_ref().is_none());
    }

    // === matches_descriptor tests ===

    #[test]
    fn any_matches_everything() {
        let cases = [
            Value::Null,
            Value::Bool(false),
            Value::I64(0),
            Value::U64(0),
            Value::F64(0.0),
            Value::String("".into()),
            Value::Bytes(vec![]),
            Value::NodeRef(NodeId(1)),
            Value::LangString { value: "x".into(), lang: "en".into() },
            Value::List(vec![]),
        ];
        for v in &cases {
            assert!(v.matches_descriptor(&ValueTypeDescriptor::Any), "Any should match {:?}", v);
        }
    }

    #[test]
    fn null_matches_only_any() {
        let descriptors = [
            ValueTypeDescriptor::Bool,
            ValueTypeDescriptor::I64,
            ValueTypeDescriptor::U64,
            ValueTypeDescriptor::F64,
            ValueTypeDescriptor::String,
            ValueTypeDescriptor::Bytes,
            ValueTypeDescriptor::NodeRef,
            ValueTypeDescriptor::LangString,
            ValueTypeDescriptor::List(Box::new(ValueTypeDescriptor::Any)),
        ];
        for d in &descriptors {
            assert!(!Value::Null.matches_descriptor(d), "Null should not match {:?}", d);
        }
    }

    #[test]
    fn specific_matches() {
        assert!(Value::Bool(true).matches_descriptor(&ValueTypeDescriptor::Bool));
        assert!(Value::I64(1).matches_descriptor(&ValueTypeDescriptor::I64));
        assert!(Value::U64(1).matches_descriptor(&ValueTypeDescriptor::U64));
        assert!(Value::F64(1.0).matches_descriptor(&ValueTypeDescriptor::F64));
        assert!(Value::String("x".into()).matches_descriptor(&ValueTypeDescriptor::String));
        assert!(Value::Bytes(vec![1]).matches_descriptor(&ValueTypeDescriptor::Bytes));
        assert!(Value::NodeRef(NodeId(1)).matches_descriptor(&ValueTypeDescriptor::NodeRef));
    }

    #[test]
    fn string_descriptor_matches_lang_string() {
        let v = Value::LangString { value: "hi".into(), lang: "en".into() };
        assert!(v.matches_descriptor(&ValueTypeDescriptor::String));
    }

    #[test]
    fn lang_string_descriptor_matches_lang_string_only() {
        let v = Value::LangString { value: "hi".into(), lang: "en".into() };
        assert!(v.matches_descriptor(&ValueTypeDescriptor::LangString));
        assert!(!Value::String("hi".into()).matches_descriptor(&ValueTypeDescriptor::LangString));
    }

    #[test]
    fn list_homogeneous_match() {
        let v = Value::List(vec![Value::I64(1), Value::I64(2)]);
        assert!(v.matches_descriptor(&ValueTypeDescriptor::List(Box::new(ValueTypeDescriptor::I64))));
    }

    #[test]
    fn list_heterogeneous_no_match() {
        let v = Value::List(vec![Value::I64(1), Value::String("x".into())]);
        assert!(!v.matches_descriptor(&ValueTypeDescriptor::List(Box::new(ValueTypeDescriptor::I64))));
    }

    #[test]
    fn empty_list_matches_any_list_descriptor() {
        let v = Value::List(vec![]);
        assert!(v.matches_descriptor(&ValueTypeDescriptor::List(Box::new(ValueTypeDescriptor::Any))));
        assert!(v.matches_descriptor(&ValueTypeDescriptor::List(Box::new(ValueTypeDescriptor::I64))));
        assert!(v.matches_descriptor(&ValueTypeDescriptor::List(Box::new(ValueTypeDescriptor::Bool))));
    }

    #[test]
    fn non_matching_pairs() {
        assert!(!Value::Bool(true).matches_descriptor(&ValueTypeDescriptor::I64));
        assert!(!Value::I64(1).matches_descriptor(&ValueTypeDescriptor::Bool));
        assert!(!Value::String("x".into()).matches_descriptor(&ValueTypeDescriptor::Bytes));
        assert!(!Value::I64(1).matches_descriptor(&ValueTypeDescriptor::List(Box::new(ValueTypeDescriptor::I64))));
    }

    // Note: Value does NOT implement Eq. This is a compile-time property —
    // if someone adds `Eq` to Value's derives, tests that rely on `PartialEq`
    // will still pass, but the design intent is that Eq is NOT derived.

    // === total_eq tests ===

    #[test]
    fn total_eq_nan_equals_nan() {
        assert!(Value::F64(f64::NAN).total_eq(&Value::F64(f64::NAN)));
    }

    #[test]
    fn total_eq_zero_not_equal_neg_zero() {
        assert!(!Value::F64(0.0).total_eq(&Value::F64(-0.0)));
    }

    #[test]
    fn total_eq_float_not_equal_integer() {
        assert!(!Value::F64(1.0).total_eq(&Value::I64(1)));
    }

    #[test]
    fn total_eq_non_float_delegates_to_partial_eq() {
        assert!(Value::Null.total_eq(&Value::Null));
        assert!(Value::Bool(true).total_eq(&Value::Bool(true)));
        assert!(!Value::Bool(true).total_eq(&Value::Bool(false)));
        assert!(Value::I64(42).total_eq(&Value::I64(42)));
        assert!(!Value::I64(1).total_eq(&Value::I64(2)));
        assert!(Value::U64(10).total_eq(&Value::U64(10)));
        assert!(Value::String("hi".into()).total_eq(&Value::String("hi".into())));
        assert!(!Value::String("a".into()).total_eq(&Value::String("b".into())));
        assert!(Value::Bytes(vec![1, 2]).total_eq(&Value::Bytes(vec![1, 2])));
        assert!(Value::NodeRef(NodeId(5)).total_eq(&Value::NodeRef(NodeId(5))));
    }

    #[test]
    fn total_eq_lang_string() {
        let a = Value::LangString { value: "hello".into(), lang: "en".into() };
        let b = Value::LangString { value: "hello".into(), lang: "en".into() };
        let c = Value::LangString { value: "hello".into(), lang: "fr".into() };
        assert!(a.total_eq(&b));
        assert!(!a.total_eq(&c));
    }

    #[test]
    fn total_eq_list_with_nan() {
        let a = Value::List(vec![Value::F64(f64::NAN), Value::I64(1)]);
        let b = Value::List(vec![Value::F64(f64::NAN), Value::I64(1)]);
        assert!(a.total_eq(&b));
    }

    #[test]
    fn total_eq_list_length_mismatch() {
        let a = Value::List(vec![Value::I64(1)]);
        let b = Value::List(vec![Value::I64(1), Value::I64(2)]);
        assert!(!a.total_eq(&b));
    }

    #[test]
    fn total_eq_property_map() {
        let mut a = PropertyMap::new();
        a.insert(PropertyKeyId(1), Value::F64(f64::NAN));
        a.insert(PropertyKeyId(2), Value::String("x".into()));

        let mut b = PropertyMap::new();
        b.insert(PropertyKeyId(1), Value::F64(f64::NAN));
        b.insert(PropertyKeyId(2), Value::String("x".into()));

        // PartialEq fails on NaN
        assert_ne!(a, b);
        // total_eq succeeds
        assert!(property_map_total_eq(&a, &b));
    }

    #[test]
    fn total_eq_property_map_different_keys() {
        let mut a = PropertyMap::new();
        a.insert(PropertyKeyId(1), Value::I64(1));

        let mut b = PropertyMap::new();
        b.insert(PropertyKeyId(2), Value::I64(1));

        assert!(!property_map_total_eq(&a, &b));
    }

    #[test]
    fn total_eq_property_map_different_lengths() {
        let mut a = PropertyMap::new();
        a.insert(PropertyKeyId(1), Value::I64(1));

        let b = PropertyMap::new();

        assert!(!property_map_total_eq(&a, &b));
    }

    // === ValueTypeDescriptor Eq test ===

    #[test]
    fn value_type_descriptor_eq() {
        // ValueTypeDescriptor implements Eq (no f64)
        let a = ValueTypeDescriptor::I64;
        let b = ValueTypeDescriptor::I64;
        assert_eq!(a, b);

        let c = ValueTypeDescriptor::List(Box::new(ValueTypeDescriptor::String));
        let d = ValueTypeDescriptor::List(Box::new(ValueTypeDescriptor::String));
        assert_eq!(c, d);
    }

    // === Node tests ===

    #[test]
    fn node_construction() {
        let node = Node {
            id: NodeId(1),
            type_labels: vec![TypeId(1), TypeId(2)],
            properties: PropertyMap::new(),
            is_anonymous: false,
        };
        assert_eq!(node.id, NodeId(1));
        assert_eq!(node.type_labels.len(), 2);
        assert!(!node.is_anonymous);
    }

    #[test]
    fn node_equality() {
        let a = Node {
            id: NodeId(1),
            type_labels: vec![],
            properties: PropertyMap::new(),
            is_anonymous: false,
        };
        let b = a.clone();
        assert_eq!(a, b);

        let c = Node {
            id: NodeId(2),
            type_labels: vec![],
            properties: PropertyMap::new(),
            is_anonymous: false,
        };
        assert_ne!(a, c);
    }

    // === Edge tests ===

    #[test]
    fn edge_construction() {
        let edge = Edge {
            id: EdgeId(10),
            type_labels: vec![TypeId(3)],
            source: NodeId(1),
            target: NodeId(2),
            properties: PropertyMap::new(),
        };
        assert_eq!(edge.id, EdgeId(10));
        assert_eq!(edge.source, NodeId(1));
        assert_eq!(edge.target, NodeId(2));
    }

    #[test]
    fn edge_equality() {
        let a = Edge {
            id: EdgeId(1),
            type_labels: vec![],
            source: NodeId(1),
            target: NodeId(2),
            properties: PropertyMap::new(),
        };
        let b = a.clone();
        assert_eq!(a, b);

        let c = Edge {
            id: EdgeId(1),
            type_labels: vec![],
            source: NodeId(1),
            target: NodeId(3),
            properties: PropertyMap::new(),
        };
        assert_ne!(a, c);
    }

    // === TypeKind tests ===

    #[test]
    fn type_kind_eq() {
        assert_eq!(TypeKind::Node, TypeKind::Node);
        assert_ne!(TypeKind::Node, TypeKind::Edge);
        // TypeKind implements Eq (compile-time verification through use)
        fn _assert_eq<T: Eq>() {}
        _assert_eq::<TypeKind>();
    }

    #[test]
    fn type_kind_display() {
        assert_eq!(format!("{}", TypeKind::Node), "Node");
        assert_eq!(format!("{}", TypeKind::Edge), "Edge");
    }

    // === PropertyDeclaration tests ===

    #[test]
    fn property_declaration_construction() {
        let decl = PropertyDeclaration {
            key: PropertyKeyId(1),
            value_type: ValueTypeDescriptor::String,
            required: true,
            multi_valued: false,
            metadata: PropertyMap::new(),
        };
        assert_eq!(decl.key, PropertyKeyId(1));
        assert!(decl.required);
        assert!(!decl.multi_valued);
    }

    // === TypeDefinition tests ===

    #[test]
    fn type_definition_construction() {
        let decl = PropertyDeclaration {
            key: PropertyKeyId(1),
            value_type: ValueTypeDescriptor::String,
            required: true,
            multi_valued: false,
            metadata: PropertyMap::new(),
        };
        let typedef = TypeDefinition {
            id: TypeId(1),
            name: "Person".into(),
            kind: TypeKind::Node,
            supertypes: vec![TypeId(2)],
            property_declarations: vec![decl],
            open: false,
            metadata: PropertyMap::new(),
        };
        assert_eq!(typedef.id, TypeId(1));
        assert_eq!(typedef.name, "Person");
        assert_eq!(typedef.kind, TypeKind::Node);
        assert_eq!(typedef.supertypes, vec![TypeId(2)]);
        assert_eq!(typedef.property_declarations.len(), 1);
        assert!(!typedef.open);
    }

    #[test]
    fn type_definition_equality() {
        let make = || TypeDefinition {
            id: TypeId(1),
            name: "Thing".into(),
            kind: TypeKind::Node,
            supertypes: vec![],
            property_declarations: vec![],
            open: true,
            metadata: PropertyMap::new(),
        };
        assert_eq!(make(), make());
    }
}
