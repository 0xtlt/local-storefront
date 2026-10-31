//! Connections: a page of a list of records (or of inline objects), as Relay's cursor
//! connections and Shopify's arguments define it.
//!
//! The list goes through the field's `query` ([`super::search`]), its `sortKey`
//! ([`super::sort`]) and `reverse`, then `after` and `before` cut it, then `first` or `last`.
//! `pageInfo` is exact: `hasPreviousPage` says whether an item comes before the page in the
//! list, `hasNextPage` whether one comes after it.
//!
//! What Shopify answers, word for word (each a field error, the connection `null`):
//!
//! - neither `first` nor `last`: `you must provide one of first or last` (a Shopify answer to
//!   `{ products { … } }`, quoted on zenn.dev);
//! - more than 250: `first cannot exceed 250. To query larger amounts of data with fewer limits,
//!   bulk operations should be used instead.\nSee https://shopify.dev/api/usage/bulk-operations/queries
//!   for usage details.\n` (an answer of the Admin API quoted on community.shopify.dev; `last`
//!   by analogy). shopify.dev's "Paginating results with GraphQL": "up to a maximum of 250";
//! - a cursor lsf did not give: `Malformed cursor`, with empty `extensions` (an Admin API answer
//!   quoted on community.shopify.dev).
//!
//! Cursors are opaque: the base64 of `{"position", "id"}`, the item's place in the list and its
//! GID (or the `id` of an inline object, when it has one). `after` finds the item by its id first,
//! so that a page follows its item even when the list changed before it, and by its position
//! otherwise.

use std::rc::Rc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value, json};

use super::{Inline, Obj, ShopResolver};
use crate::graphql::{FieldContext, FieldError, Output};

/// The most items a page may have.
pub const MAX_PAGE: i64 = 250;

/// The cursor of an item: the base64 of its position and its id.
pub fn cursor(position: usize, id: Option<&str>) -> String {
    STANDARD.encode(json!({"position": position, "id": id}).to_string())
}

/// The position and the id of a cursor, `None` when it is not one of lsf's.
pub fn decode(cursor: &str) -> Option<(usize, Option<String>)> {
    let bytes = STANDARD.decode(cursor.trim()).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let position = usize::try_from(value.get("position")?.as_u64()?).ok()?;
    let id = value.get("id").and_then(Value::as_str).map(str::to_string);
    Some((position, id))
}

/// Shopify's errors for the size of a page: one of `first` and `last` is required, neither
/// above 250.
pub fn check_arguments(
    ctx: &FieldContext<'_>,
) -> Result<(Option<usize>, Option<usize>), FieldError> {
    let first = ctx.arg_i64("first");
    let last = ctx.arg_i64("last");
    if first.is_none() && last.is_none() {
        return Err(FieldError::new("you must provide one of first or last"));
    }
    let size = |name: &str, value: Option<i64>| -> Result<Option<usize>, FieldError> {
        match value {
            Some(value) if value > MAX_PAGE => Err(FieldError::new(format!(
                "{name} cannot exceed {MAX_PAGE}. To query larger amounts of data with fewer limits, \
                 bulk operations should be used instead.\nSee \
                 https://shopify.dev/api/usage/bulk-operations/queries for usage details.\n"
            ))),
            Some(value) => Ok(Some(usize::try_from(value.max(0)).unwrap_or(0))),
            None => Ok(None),
        }
    };
    Ok((size("first", first)?, size("last", last)?))
}

fn malformed_cursor() -> FieldError {
    FieldError {
        message: "Malformed cursor".to_string(),
        extensions: Some(Map::new()),
    }
}

/// The id an item is found by: its GID, or the `id` of an inline object.
fn id_of(item: &Value) -> Option<&str> {
    match item {
        Value::String(key) => Some(key),
        Value::Object(map) => map.get("id").and_then(Value::as_str),
        _ => None,
    }
}

/// The index in `items` of the item a cursor names.
fn index_of(items: &[Value], cursor: &str) -> Result<usize, FieldError> {
    let (position, id) = decode(cursor).ok_or_else(malformed_cursor)?;
    if let Some(id) = &id
        && let Some(index) = items.iter().position(|item| id_of(item) == Some(id))
    {
        return Ok(index);
    }
    Ok(position.min(items.len()))
}

/// The connection of a field over `items`.
pub(super) fn page(
    resolver: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    items: Vec<Value>,
    reported: bool,
) -> Result<Output<Obj>, FieldError> {
    let (first, last) = if resolver.is_bulk() {
        // A bulk operation pages for the client: every item, whatever the size.
        let size = |name: &str| {
            ctx.arg_i64(name)
                .map(|n| usize::try_from(n.max(0)).unwrap_or(0))
        };
        (size("first"), size("last"))
    } else {
        check_arguments(ctx)?
    };
    let schema = ctx.schema();
    let connection_type = ctx.ty().named();
    let node_type = schema.connection_node(connection_type).unwrap_or_default();
    let mut items = resolver.search(ctx, node_type, items);
    super::sort::sort(resolver, ctx, node_type, &mut items);
    if ctx.arg_bool("reverse") == Some(true) {
        items.reverse();
    }
    let mut start = 0;
    let mut end = items.len();
    if let Some(after) = ctx.arg_str("after") {
        start = (index_of(&items, after)? + 1).min(items.len());
    }
    if let Some(before) = ctx.arg_str("before") {
        end = index_of(&items, before)?.max(start);
    }
    if let Some(first) = first {
        end = end.min(start + first);
    }
    if let Some(last) = last {
        start = start.max(end.saturating_sub(last));
    }
    let mut edges = Vec::with_capacity(end - start);
    let mut nodes = Vec::with_capacity(end - start);
    for (position, item) in items.iter().enumerate().take(end).skip(start) {
        let cursor = cursor(position, id_of(item));
        edges.push(json!({"cursor": cursor, "node": item}));
        nodes.push(item.clone());
    }
    let start_cursor = edges.first().map(|edge| edge["cursor"].clone());
    let end_cursor = edges.last().map(|edge| edge["cursor"].clone());
    let mut value = Map::new();
    value.insert("edges".into(), Value::Array(edges));
    value.insert("nodes".into(), Value::Array(nodes));
    value.insert(
        "pageInfo".into(),
        json!({
            "hasNextPage": end < items.len(),
            "hasPreviousPage": start > 0,
            "startCursor": start_cursor,
            "endCursor": end_cursor,
        }),
    );
    Ok(Output::Object(Obj::Inline(Rc::new(Inline {
        type_name: connection_type.to_string(),
        value,
        reported,
    }))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursors_round_trip() {
        let cursor = cursor(3, Some("gid://shopify/Product/9"));
        assert_eq!(
            decode(&cursor),
            Some((3, Some("gid://shopify/Product/9".to_string())))
        );
        assert_eq!(decode(&super::cursor(0, None)), Some((0, None)));
        assert_eq!(decode("not a cursor"), None);
        assert_eq!(decode(&STANDARD.encode("{}")), None);
    }

    #[test]
    fn a_cursor_finds_its_item_by_id_then_by_position() {
        let items = vec![
            json!("gid://shopify/Product/1"),
            json!("gid://shopify/Product/2"),
        ];
        let second = cursor(7, Some("gid://shopify/Product/2"));
        assert_eq!(index_of(&items, &second).unwrap(), 1);
        let gone = cursor(1, Some("gid://shopify/Product/5"));
        assert_eq!(index_of(&items, &gone).unwrap(), 1);
        assert_eq!(index_of(&items, &cursor(9, None)).unwrap(), 2);
        let error = index_of(&items, "x").unwrap_err();
        assert_eq!(error.message, "Malformed cursor");
        assert_eq!(error.extensions, Some(Map::new()));
    }
}
