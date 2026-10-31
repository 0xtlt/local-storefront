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

use serde_json::Value;

use super::{ShopResolver, camel_case};
use crate::graphql::FieldContext;
use crate::state::parse_gid;

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

    fn compare(&self, other: &SortValue) -> Ordering {
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
    key.and_then(parse_gid).map_or(0, |(_, id)| id)
}

/// Sorts the items of a connection by the field's `sortKey` (see the module's documentation).
pub(super) fn sort(
    resolver: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    node_type: &str,
    items: &mut [Value],
) {
    let root = ctx.parent_type() == ctx.schema().query_type();
    let default = ctx
        .definition()
        .arg("sortKey")
        .and_then(|arg| arg.default_value.as_ref())
        .and_then(Value::as_str);
    let key = match ctx.arg_str("sortKey") {
        Some(key) if root || Some(key) != default => key.to_string(),
        Some(_) => return,
        None if root && ctx.definition().arg("sortKey").is_none() => "ID".to_string(),
        None => return,
    };
    let key = key.to_ascii_uppercase();
    let custom = resolver.registry.sort_fn(node_type, &key);
    let field = camel_case(&key.to_ascii_lowercase());
    let readable = custom.is_some()
        || matches!(key.as_str(), "ID" | "RELEVANCE")
        || crate::schema::admin()
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
        return;
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
    for (slot, (_, _, item)) in items.iter_mut().zip(keyed) {
        *slot = item;
    }
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
