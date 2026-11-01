//! The `sortKey` of a connection.
//!
//! Root connections sort by their `sortKey`, the schema's default included (`products` by `ID`,
//! `orders` by `PROCESSED_AT`, `locations` by `NAME`), as Shopify's do; a root connection
//! without a `sortKey` argument sorts by `ID`. The array of a record keeps its order (Shopify's:
//! variants by position, the products of a collection in its order) unless the query asks for
//! another `sortKey`.
//!
//! A key is read generically: `ID` (and `RELEVANCE`, which without a ranking is the order of
//! the ids) by the number of the GID; any other by the record's field of the same name in
//! camelCase (`TITLE` → `title`, `CREATED_AT` → `createdAt`, `PRODUCT_TYPE` → `productType`,
//! `metaobjects(sortKey: "updated_at")` → `updatedAt`): numbers, and decimals written as strings,
//! by value, text ignoring case. Ties keep the order of the ids. A domain registers the keys that
//! do not follow (`INVENTORY_TOTAL` of products is `totalInventory`): [`super::Registry::sort_key`].
//! A key lsf cannot read keeps the order, and says so in `extensions.lsf.notes`.

use std::cmp::Ordering;

use serde_json::{Value, json};

use super::{ShopResolver, camel_case};
use crate::graphql::FieldContext;
use crate::state::gid_number;

/// What a record is sorted by.
#[derive(Clone, Debug, PartialEq)]
pub enum SortValue {
    /// No value: first in ascending order.
    Missing,
    Number(f64),
    /// Compared ignoring case.
    Text(String),
}

impl SortValue {
    /// The sort value of a JSON value: a number, a decimal string, the `amount` of a `MoneyV2`,
    /// else its text.
    pub fn of(value: Option<&Value>) -> SortValue {
        match value {
            None | Some(Value::Null) => SortValue::Missing,
            Some(Value::Bool(flag)) => SortValue::Number(f64::from(u8::from(*flag))),
            Some(value) => match super::search::number(Some(value)) {
                Some(number) => SortValue::Number(number),
                None => match value {
                    Value::String(text) => SortValue::Text(text.to_lowercase()),
                    other => SortValue::Text(other.to_string()),
                },
            },
        }
    }

    /// The value as a cursor keeps it: `null`, `{"number": "19.5"}` (a string, so that any
    /// `f64` comes back the same), `{"text": "hat"}`.
    pub(super) fn to_json(&self) -> Value {
        match self {
            SortValue::Missing => Value::Null,
            SortValue::Number(number) => json!({"number": number.to_string()}),
            SortValue::Text(text) => json!({"text": text}),
        }
    }

    /// The value [`SortValue::to_json`] wrote, `None` for anything else.
    pub(super) fn from_json(value: &Value) -> Option<SortValue> {
        match value {
            Value::Null => Some(SortValue::Missing),
            Value::Object(map) => match (map.get("number"), map.get("text")) {
                (Some(Value::String(number)), None) => number.parse().ok().map(SortValue::Number),
                (None, Some(Value::String(text))) => Some(SortValue::Text(text.clone())),
                _ => None,
            },
            _ => None,
        }
    }

    pub(super) fn compare(&self, other: &SortValue) -> Ordering {
        match (self, other) {
            (SortValue::Missing, SortValue::Missing) => Ordering::Equal,
            (SortValue::Missing, _) => Ordering::Less,
            (_, SortValue::Missing) => Ordering::Greater,
            (SortValue::Number(a), SortValue::Number(b)) => a.total_cmp(b),
            (SortValue::Text(a), SortValue::Text(b)) => a.cmp(b),
            (SortValue::Number(_), SortValue::Text(_)) => Ordering::Less,
            (SortValue::Text(_), SortValue::Number(_)) => Ordering::Greater,
        }
    }
}

/// The number of an item's GID, for `ID` and for ties.
fn id_of(resolver: &ShopResolver<'_>, item: &Value) -> u64 {
    let key = match item {
        Value::String(key) => Some(key.as_str()),
        other => resolver
            .item_map(other)
            .and_then(|map| map.get("id"))
            .and_then(Value::as_str),
    };
    tie_of(key)
}

/// The number of a GID, which breaks ties (0 without one).
pub(super) fn tie_of(gid: Option<&str>) -> u64 {
    gid.map_or(0, gid_number)
}

/// How an item compares with another in a sorted list: its value, then its id.
pub(super) fn compare(a: &(SortValue, u64), b: &(SortValue, u64)) -> Ordering {
    a.0.compare(&b.0).then(a.1.cmp(&b.1))
}

/// The order [`sort`] put a list in: its key (`ID`, `TITLE`), and the value and the id number of
/// each item, in the order of the list. Cursors keep an item's, to resume after it when it is
/// gone.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Order {
    pub key: String,
    pub values: Vec<(SortValue, u64)>,
    /// Sorted from the last to the first (`reverse: true`).
    pub reversed: bool,
}

impl Order {
    /// The order of `values`, flipped.
    pub fn reverse(&mut self) {
        self.values.reverse();
        self.reversed = !self.reversed;
    }

    /// How many items of the list come strictly before an item with `value`: the index of the
    /// first one after it.
    pub fn before(&self, value: &(SortValue, u64)) -> usize {
        self.values.partition_point(|item| {
            let ordering = compare(item, value);
            if self.reversed {
                ordering == Ordering::Greater
            } else {
                ordering == Ordering::Less
            }
        })
    }
}

/// Sorts the items of a connection by the field's `sortKey` (see the module's documentation),
/// and answers the order it put them in; `None` when the list keeps its own order.
pub(super) fn sort(
    resolver: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    node_type: &str,
    items: &mut [Value],
) -> Option<Order> {
    let root = ctx.parent_type() == ctx.schema().query_type();
    let default = ctx
        .definition()
        .arg("sortKey")
        .and_then(|arg| arg.default_value.as_ref())
        .and_then(Value::as_str);
    let key = match ctx.arg_str("sortKey") {
        Some(key) if root || Some(key) != default => key.to_string(),
        Some(_) => return None,
        None if root && ctx.definition().arg("sortKey").is_none() => "ID".to_string(),
        None => return None,
    };
    let key = key.to_ascii_uppercase();
    let custom = resolver.registry.sort_fn(node_type, &key);
    let field = camel_case(&key.to_ascii_lowercase());
    let readable = custom.is_some()
        || matches!(key.as_str(), "ID" | "RELEVANCE")
        || ctx
            .schema()
            .get(node_type)
            .is_some_and(|def| def.fields.contains_key(&field))
        || items.iter().any(|item| {
            resolver
                .item_map(item)
                .is_some_and(|map| map.contains_key(&field))
        });
    if !readable {
        resolver.note(format!(
            "lsf does not sort {node_type} by {key}: the order is the order of creation"
        ));
        return None;
    }
    let mut keyed: Vec<(SortValue, u64, Value)> = items
        .iter()
        .map(|item| {
            let id = id_of(resolver, item);
            let value = match (&custom, key.as_str()) {
                (Some(read), _) => read(resolver, item),
                (None, "ID" | "RELEVANCE") => SortValue::Number(id as f64),
                (None, _) => SortValue::of(resolver.item_map(item).and_then(|map| map.get(&field))),
            };
            (value, id, item.clone())
        })
        .collect();
    keyed.sort_by(|(a, a_id, _), (b, b_id, _)| a.compare(b).then(a_id.cmp(b_id)));
    let mut values = Vec::with_capacity(keyed.len());
    for (slot, (value, id, item)) in items.iter_mut().zip(keyed) {
        *slot = item;
        values.push((value, id));
    }
    Some(Order {
        key,
        values,
        reversed: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compares_numbers_decimals_and_text() {
        assert_eq!(
            SortValue::of(Some(&json!("19.00"))),
            SortValue::Number(19.0)
        );
        assert_eq!(
            SortValue::of(Some(&json!({"amount": "5.0"}))),
            SortValue::Number(5.0)
        );
        assert_eq!(
            SortValue::of(Some(&json!("Hat"))),
            SortValue::Text("hat".into())
        );
        assert_eq!(SortValue::of(None), SortValue::Missing);
        assert_eq!(
            SortValue::of(Some(&json!("2024-01-01T00:00:00Z")))
                .compare(&SortValue::of(Some(&json!("2025-01-01T00:00:00Z")))),
            Ordering::Less
        );
        assert_eq!(
            SortValue::Missing.compare(&SortValue::Number(0.0)),
            Ordering::Less
        );
    }
}
