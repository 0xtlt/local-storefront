//! The dynamic value model shared by the parser, the runtime and filters.
//!
//! The variants map one-to-one onto the Ruby types Shopify's Liquid operates on, because a lot of
//! observable behaviour (truthiness, `to_s`, `inspect`, comparison errors) is defined in terms of
//! those types.

use std::any::Any;
use std::borrow::Cow;
use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;

use indexmap::IndexMap;

use crate::number::float_to_s;

/// An insertion-ordered string-keyed map, the equivalent of a Ruby `Hash` with string keys.
pub type Hash = IndexMap<String, Value>;

/// A Liquid value.
#[derive(Clone, Default)]
pub enum Value {
    #[default]
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Arc<str>),
    Array(Arc<Vec<Value>>),
    Hash(Arc<Hash>),
    /// An inclusive integer range, as produced by `(1..5)`.
    Range(i64, i64),
    /// A "drop": an object whose properties are resolved lazily by Rust code.
    Object(Arc<dyn Object>),
}

/// A lazily resolved object exposed to templates (Liquid calls these "drops").
///
/// Every method has a default so that implementors only describe what they support.
pub trait Object: Send + Sync + 'static {
    /// The Liquid object type, e.g. `product`. Used for diagnostics and by filters that accept
    /// several object types.
    fn type_name(&self) -> &str;

    /// Resolves a property. `None` means "no such property" and evaluates to `nil`.
    fn get(&self, _key: &str) -> Option<Value> {
        None
    }

    /// Resolves an integer index, for array-like objects.
    fn index(&self, _index: i64) -> Option<Value> {
        None
    }

    /// The items to iterate over when the object is used in a `for` loop or an array filter.
    fn items(&self) -> Option<Arc<Vec<Value>>> {
        None
    }

    /// The number of items, when the object is a collection.
    fn size(&self) -> Option<usize> {
        self.items().map(|items| items.len())
    }

    /// The scalar this object stands for in conditions, comparisons and lookups
    /// (Ruby Liquid's `to_liquid_value`).
    fn to_value(&self) -> Option<Value> {
        None
    }

    /// What `{{ object }}` prints.
    fn render(&self) -> Cow<'_, str> {
        match self.to_value() {
            Some(value) => Cow::Owned(value.to_str().into_owned()),
            None => Cow::Owned(format!("{}Drop", camelize(self.type_name()))),
        }
    }

    /// The JSON representation used by the `json` filter.
    fn to_json(&self) -> serde_json::Value {
        match self.to_value() {
            Some(value) => value.to_json(),
            None => serde_json::Value::Null,
        }
    }

    /// A key identifying the underlying entity. Two objects with equal keys compare equal.
    fn identity(&self) -> Option<String> {
        None
    }

    fn as_any(&self) -> &dyn Any;
}

fn camelize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper = true;
    for c in name.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

impl Value {
    pub fn str(s: impl AsRef<str>) -> Value {
        Value::Str(Arc::from(s.as_ref()))
    }

    pub fn array(items: Vec<Value>) -> Value {
        Value::Array(Arc::new(items))
    }

    pub fn hash(map: Hash) -> Value {
        Value::Hash(Arc::new(map))
    }

    pub fn object(object: impl Object) -> Value {
        Value::Object(Arc::new(object))
    }

    pub fn empty_string() -> Value {
        Value::Str(Arc::from(""))
    }

    pub fn is_nil(&self) -> bool {
        matches!(self, Value::Nil)
    }

    /// Replaces an object by the scalar it stands for, if any.
    pub fn to_liquid_value(&self) -> Value {
        match self {
            Value::Object(object) => object.to_value().unwrap_or_else(|| self.clone()),
            other => other.clone(),
        }
    }

    /// Only `nil` and `false` are falsy in Liquid.
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Nil | Value::Bool(false) => false,
            Value::Object(object) => match object.to_value() {
                Some(value) => value.is_truthy(),
                None => true,
            },
            _ => true,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Arc<Vec<Value>>> {
        match self {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_hash(&self) -> Option<&Arc<Hash>> {
        match self {
            Value::Hash(map) => Some(map),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&Arc<dyn Object>> {
        match self {
            Value::Object(object) => Some(object),
            _ => None,
        }
    }

    /// Downcasts an object value to a concrete drop type.
    pub fn downcast<T: Object>(&self) -> Option<&T> {
        match self {
            Value::Object(object) => object.as_any().downcast_ref::<T>(),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// A numeric view of ints and floats only (no string coercion).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The Ruby class name, used in error messages.
    pub fn class_name(&self) -> &'static str {
        match self {
            Value::Nil => "NilClass",
            Value::Bool(true) => "TrueClass",
            Value::Bool(false) => "FalseClass",
            Value::Int(_) => "Integer",
            Value::Float(_) => "Float",
            Value::Str(_) => "String",
            Value::Array(_) => "Array",
            Value::Hash(_) => "Hash",
            Value::Range(..) => "Range",
            Value::Object(_) => "Liquid::Drop",
        }
    }

    /// Looks up a property the way `object[key]` does for hashes and drops.
    pub fn get(&self, key: &str) -> Value {
        match self {
            Value::Hash(map) => map.get(key).cloned().unwrap_or(Value::Nil),
            Value::Object(object) => object.get(key).unwrap_or(Value::Nil),
            _ => Value::Nil,
        }
    }

    /// The items of an array-like value. Used by `for` and by array filters.
    pub fn items(&self) -> Option<Arc<Vec<Value>>> {
        match self {
            Value::Array(items) => Some(items.clone()),
            Value::Object(object) => object.items(),
            _ => None,
        }
    }

    /// `value.size` for the types that respond to it.
    pub fn size(&self) -> Option<usize> {
        match self {
            Value::Str(s) => Some(s.chars().count()),
            Value::Array(items) => Some(items.len()),
            Value::Hash(map) => Some(map.len()),
            Value::Range(from, to) => Some(range_len(*from, *to)),
            Value::Object(object) => object.size(),
            // Ruby's Integer#size is the byte width of the machine word.
            Value::Int(_) => Some(8),
            _ => None,
        }
    }

    /// `value.empty?`, for the types that respond to it.
    pub fn is_empty(&self) -> Option<bool> {
        match self {
            Value::Str(s) => Some(s.is_empty()),
            Value::Array(items) => Some(items.is_empty()),
            Value::Hash(map) => Some(map.is_empty()),
            Value::Object(object) => object.size().map(|size| size == 0),
            _ => None,
        }
    }

    /// `value == blank`.
    pub fn is_blank(&self) -> bool {
        match self {
            Value::Nil | Value::Bool(false) => true,
            Value::Bool(true) | Value::Int(_) | Value::Float(_) => false,
            Value::Str(s) => s.chars().all(char::is_whitespace),
            Value::Array(items) => items.is_empty(),
            Value::Hash(map) => map.is_empty(),
            Value::Range(..) => false,
            Value::Object(object) => match object.to_value() {
                Some(value) => value.is_blank(),
                None => object.size().is_some_and(|size| size == 0),
            },
        }
    }

    /// The string form used when a value is piped into a string filter (Ruby's `Utils.to_s`).
    pub fn to_str(&self) -> Cow<'_, str> {
        match self {
            Value::Nil => Cow::Borrowed(""),
            Value::Bool(true) => Cow::Borrowed("true"),
            Value::Bool(false) => Cow::Borrowed("false"),
            Value::Int(i) => Cow::Owned(i.to_string()),
            Value::Float(f) => Cow::Owned(float_to_s(*f)),
            Value::Str(s) => Cow::Borrowed(s),
            Value::Array(_) | Value::Hash(_) => Cow::Owned(self.inspect()),
            Value::Range(from, to) => Cow::Owned(format!("{from}..{to}")),
            Value::Object(object) => object.render(),
        }
    }

    /// Appends the value the way `{{ value }}` prints it: arrays are flattened and concatenated.
    pub fn render_to(&self, out: &mut String) {
        match self {
            Value::Nil => {}
            Value::Str(s) => out.push_str(s),
            Value::Array(items) => {
                for item in items.iter() {
                    item.render_to(out);
                }
            }
            other => out.push_str(&other.to_str()),
        }
    }

    /// Ruby's `inspect`, which is what nested values look like once stringified.
    pub fn inspect(&self) -> String {
        let mut out = String::new();
        self.inspect_to(&mut out);
        out
    }

    fn inspect_to(&self, out: &mut String) {
        match self {
            Value::Nil => out.push_str("nil"),
            Value::Str(s) => inspect_str(s, out),
            Value::Array(items) => {
                out.push('[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    item.inspect_to(out);
                }
                out.push(']');
            }
            Value::Hash(map) => {
                out.push('{');
                for (i, (key, value)) in map.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    inspect_str(key, out);
                    out.push_str("=>");
                    value.inspect_to(out);
                }
                out.push('}');
            }
            other => out.push_str(&other.to_str()),
        }
    }

    /// Ruby's `==`.
    pub fn ruby_eq(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Int(a), Value::Float(b)) | (Value::Float(b), Value::Int(a)) => *a as f64 == *b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Array(a), Value::Array(b)) => {
                a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x.ruby_eq(y))
            }
            (Value::Hash(a), Value::Hash(b)) => {
                a.len() == b.len()
                    && a.iter()
                        .all(|(key, value)| b.get(key).is_some_and(|other| value.ruby_eq(other)))
            }
            (Value::Range(a1, a2), Value::Range(b1, b2)) => a1 == b1 && a2 == b2,
            (Value::Object(a), Value::Object(b)) => {
                if Arc::ptr_eq(a, b) {
                    return true;
                }
                match (a.identity(), b.identity()) {
                    (Some(x), Some(y)) => x == y,
                    _ => match (a.to_value(), b.to_value()) {
                        (Some(x), Some(y)) => x.ruby_eq(&y),
                        _ => false,
                    },
                }
            }
            (Value::Object(a), b) | (b, Value::Object(a)) => {
                a.to_value().is_some_and(|scalar| scalar.ruby_eq(b))
            }
            _ => false,
        }
    }

    /// Ruby's `<=>`: `None` when the two values are not comparable.
    pub fn ruby_cmp(&self, other: &Value) -> Option<Ordering> {
        match (self, other) {
            (Value::Nil, Value::Nil) => Some(Ordering::Equal),
            (Value::Bool(a), Value::Bool(b)) if a == b => Some(Ordering::Equal),
            (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
            (Value::Int(a), Value::Float(b)) => (*a as f64).partial_cmp(b),
            (Value::Float(a), Value::Int(b)) => a.partial_cmp(&(*b as f64)),
            (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
            (Value::Str(a), Value::Str(b)) => Some(a.as_bytes().cmp(b.as_bytes())),
            (Value::Array(a), Value::Array(b)) => {
                for (x, y) in a.iter().zip(b.iter()) {
                    match x.ruby_cmp(y)? {
                        Ordering::Equal => {}
                        other => return Some(other),
                    }
                }
                Some(a.len().cmp(&b.len()))
            }
            (Value::Object(a), b) => a.to_value()?.ruby_cmp(b),
            (a, Value::Object(b)) => a.ruby_cmp(&b.to_value()?),
            _ => None,
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::Value as J;
        match self {
            Value::Nil => J::Null,
            Value::Bool(b) => J::Bool(*b),
            Value::Int(i) => J::from(*i),
            Value::Float(f) => serde_json::Number::from_f64(*f).map_or(J::Null, J::Number),
            Value::Str(s) => J::String(s.to_string()),
            Value::Array(items) => J::Array(items.iter().map(Value::to_json).collect()),
            Value::Hash(map) => J::Object(
                map.iter()
                    .map(|(key, value)| (key.clone(), value.to_json()))
                    .collect(),
            ),
            Value::Range(from, to) => J::Array((*from..=*to).map(J::from).collect()),
            Value::Object(object) => object.to_json(),
        }
    }
}

pub(crate) fn range_len(from: i64, to: i64) -> usize {
    if to < from {
        0
    } else {
        (to - from) as usize + 1
    }
}

fn inspect_str(s: &str, out: &mut String) {
    out.push('"');
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\u{1b}' => out.push_str("\\e"),
            '\u{0c}' => out.push_str("\\f"),
            '\u{0b}' => out.push_str("\\v"),
            '\u{08}' => out.push_str("\\b"),
            '\u{07}' => out.push_str("\\a"),
            '#' if matches!(chars.peek(), Some('{' | '$' | '@')) => out.push_str("\\#"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Object(object) => write!(f, "#<{}>", object.type_name()),
            other => f.write_str(&other.inspect()),
        }
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::str(s)
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Str(Arc::from(s))
    }
}

impl From<&String> for Value {
    fn from(s: &String) -> Self {
        Value::str(s)
    }
}

impl From<Arc<str>> for Value {
    fn from(s: Arc<str>) -> Self {
        Value::Str(s)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

impl From<i64> for Value {
    fn from(i: i64) -> Self {
        Value::Int(i)
    }
}

impl From<i32> for Value {
    fn from(i: i32) -> Self {
        Value::Int(i64::from(i))
    }
}

impl From<u32> for Value {
    fn from(i: u32) -> Self {
        Value::Int(i64::from(i))
    }
}

impl From<usize> for Value {
    fn from(i: usize) -> Self {
        Value::Int(i as i64)
    }
}

impl From<f64> for Value {
    fn from(f: f64) -> Self {
        Value::Float(f)
    }
}

impl From<Vec<Value>> for Value {
    fn from(items: Vec<Value>) -> Self {
        Value::array(items)
    }
}

impl From<Hash> for Value {
    fn from(map: Hash) -> Self {
        Value::hash(map)
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(option: Option<T>) -> Self {
        option.map_or(Value::Nil, Into::into)
    }
}

impl From<&serde_json::Value> for Value {
    fn from(json: &serde_json::Value) -> Self {
        use serde_json::Value as J;
        match json {
            J::Null => Value::Nil,
            J::Bool(b) => Value::Bool(*b),
            J::Number(n) => match n.as_i64() {
                Some(i) => Value::Int(i),
                None => Value::Float(n.as_f64().unwrap_or(0.0)),
            },
            J::String(s) => Value::str(s),
            J::Array(items) => Value::array(items.iter().map(Value::from).collect()),
            J::Object(map) => Value::hash(
                map.iter()
                    .map(|(key, value)| (key.clone(), Value::from(value)))
                    .collect(),
            ),
        }
    }
}

impl From<serde_json::Value> for Value {
    fn from(json: serde_json::Value) -> Self {
        Value::from(&json)
    }
}
