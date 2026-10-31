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
//! Cursors are opaque: the base64 of `{"position", "id", "sortKey", "value"}`, the item's place
//! in the list, its GID (or the `id` of an inline object, when it has one), and, when the list
//! was sorted, the key and the item's value under it. As Shopify's cursors, which hold the sort
//! value and the id of their item, a cursor resumes right after its item, even when the item is
//! gone (`after`: the first item that sorts strictly after the value and the id; `before`, the
//! last one strictly before): a page deleted while paging does not skip the next one. The item
//! itself, when it is still in the list, is found by its id. A list in its own order (a record's
//! array without `sortKey`), or one sorted by another key than the cursor's, falls back to the
//! position: the item that took the place of a removed one comes next.

use std::rc::Rc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value, json};

use super::sort::{Order, SortValue, tie_of};
use super::{Inline, Obj, ShopResolver};
use crate::graphql::{FieldContext, FieldError, Output};

/// The most items a page may have.
pub const MAX_PAGE: i64 = 250;

/// What a cursor says of its item.
#[derive(Clone, Debug, PartialEq)]
pub struct Cursor {
    /// Its index in the list it was given in.
    pub position: usize,
    /// Its GID, or the `id` of an inline object.
    pub id: Option<String>,
    /// The key the list was sorted by (`ID`, `TITLE`) and the item's value under it.
    pub sort: Option<(String, SortValue)>,
}

impl Cursor {
    /// The cursor as a connection answers it: base64 of JSON.
    pub fn encode(&self) -> String {
        let mut value = json!({"position": self.position, "id": self.id});
        if let Some((key, sorted)) = &self.sort {
            value["sortKey"] = json!(key);
            value["value"] = sorted.to_json();
        }
        STANDARD.encode(value.to_string())
    }

    /// The cursor `text` is, `None` when it is not one of lsf's.
    pub fn decode(text: &str) -> Option<Cursor> {
        let bytes = STANDARD.decode(text.trim()).ok()?;
        let value: Value = serde_json::from_slice(&bytes).ok()?;
        let position = usize::try_from(value.get("position")?.as_u64()?).ok()?;
        let id = value.get("id").and_then(Value::as_str).map(str::to_string);
        let sort = match value.get("sortKey") {
            None => None,
            Some(key) => Some((
                key.as_str()?.to_string(),
                SortValue::from_json(value.get("value")?)?,
            )),
        };
        Some(Cursor { position, id, sort })
    }
}

/// The cursor of the record `gid` in the `ID` order of its type, where it is at `position`: its
/// `defaultCursor`.
pub fn id_cursor(position: usize, gid: &str) -> String {
    Cursor {
        position,
        id: Some(gid.to_string()),
        sort: Some((
            "ID".to_string(),
            SortValue::Number(tie_of(Some(gid)) as f64),
        )),
    }
    .encode()
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

/// Where a cursor is in a list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Place {
    /// At its item, still in the list.
    At(usize),
    /// Between items: its item is gone, and this is the index of the first one after it.
    Before(usize),
}

impl Place {
    /// Where a page `after` the cursor starts.
    fn after(self, len: usize) -> usize {
        match self {
            Place::At(index) => (index + 1).min(len),
            Place::Before(index) => index.min(len),
        }
    }

    /// Where a page `before` the cursor ends.
    fn before(self, len: usize) -> usize {
        match self {
            Place::At(index) | Place::Before(index) => index.min(len),
        }
    }
}

/// Where in `items` (in the `order` they were sorted in, if any) the cursor `text` is.
fn place_of(items: &[Value], order: Option<&Order>, text: &str) -> Result<Place, FieldError> {
    let cursor = Cursor::decode(text).ok_or_else(malformed_cursor)?;
    if let Some(id) = &cursor.id
        && let Some(index) = items.iter().position(|item| id_of(item) == Some(id))
    {
        return Ok(Place::At(index));
    }
    if let (Some(order), Some((key, value))) = (order, &cursor.sort)
        && *key == order.key
    {
        let value = (value.clone(), tie_of(cursor.id.as_deref()));
        return Ok(Place::Before(order.before(&value)));
    }
    let position = cursor.position.min(items.len());
    Ok(match cursor.id {
        Some(_) => Place::Before(position),
        None => Place::At(position),
    })
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
    let mut order = super::sort::sort(resolver, ctx, node_type, &mut items);
    if ctx.arg_bool("reverse") == Some(true) {
        items.reverse();
        if let Some(order) = &mut order {
            order.reverse();
        }
    }
    let mut start = 0;
    let mut end = items.len();
    if let Some(after) = ctx.arg_str("after") {
        start = place_of(&items, order.as_ref(), after)?.after(items.len());
    }
    if let Some(before) = ctx.arg_str("before") {
        end = place_of(&items, order.as_ref(), before)?
            .before(items.len())
            .max(start);
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
        let cursor = Cursor {
            position,
            id: id_of(item).map(str::to_string),
            sort: order.as_ref().and_then(|order| {
                let (value, _) = order.values.get(position)?;
                Some((order.key.clone(), value.clone()))
            }),
        }
        .encode();
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

    fn cursor(position: usize, id: Option<&str>, sort: Option<(&str, SortValue)>) -> String {
        Cursor {
            position,
            id: id.map(str::to_string),
            sort: sort.map(|(key, value)| (key.to_string(), value)),
        }
        .encode()
    }

    #[test]
    fn cursors_round_trip() {
        for sort in [
            None,
            Some(("ID", SortValue::Number(9.0))),
            Some(("PRICE", SortValue::Number(0.1 + 0.2))),
            Some(("TITLE", SortValue::Text("hat".into()))),
            Some(("PUBLISHED_AT", SortValue::Missing)),
        ] {
            let text = cursor(3, Some("gid://shopify/Product/9"), sort.clone());
            assert_eq!(
                Cursor::decode(&text),
                Some(Cursor {
                    position: 3,
                    id: Some("gid://shopify/Product/9".to_string()),
                    sort: sort.map(|(key, value)| (key.to_string(), value)),
                })
            );
        }
        assert_eq!(
            Cursor::decode(&cursor(0, None, None)).map(|cursor| cursor.id),
            Some(None)
        );
        assert_eq!(Cursor::decode("not a cursor"), None);
        assert_eq!(Cursor::decode(&STANDARD.encode("{}")), None);
        let bad_value = STANDARD.encode(r#"{"position": 0, "sortKey": "ID", "value": 3}"#);
        assert_eq!(Cursor::decode(&bad_value), None);
    }

    #[test]
    fn a_cursor_finds_its_item_by_id_then_by_value_then_by_position() {
        let items = vec![
            json!("gid://shopify/Product/1"),
            json!("gid://shopify/Product/2"),
            json!("gid://shopify/Product/4"),
        ];
        let order = Order {
            key: "ID".to_string(),
            values: [1u32, 2, 4]
                .map(|id| (SortValue::Number(f64::from(id)), u64::from(id)))
                .to_vec(),
            reversed: false,
        };
        let second = cursor(7, Some("gid://shopify/Product/2"), None);
        assert_eq!(
            place_of(&items, Some(&order), &second).unwrap(),
            Place::At(1)
        );
        // Gone: after its value and its id, wherever it was.
        let gone = cursor(
            0,
            Some("gid://shopify/Product/3"),
            Some(("ID", SortValue::Number(3.0))),
        );
        assert_eq!(
            place_of(&items, Some(&order), &gone).unwrap(),
            Place::Before(2)
        );
        let mut reversed = order.clone();
        reversed.reverse();
        let backwards: Vec<Value> = items.iter().rev().cloned().collect();
        assert_eq!(
            place_of(&backwards, Some(&reversed), &gone).unwrap(),
            Place::Before(1)
        );
        // Another sort key, or no order: the position.
        let titled = cursor(
            1,
            Some("gid://shopify/Product/3"),
            Some(("TITLE", SortValue::Text("a".into()))),
        );
        assert_eq!(
            place_of(&items, Some(&order), &titled).unwrap(),
            Place::Before(1)
        );
        assert_eq!(
            place_of(
                &items,
                None,
                &cursor(1, Some("gid://shopify/Product/5"), None)
            )
            .unwrap(),
            Place::Before(1)
        );
        assert_eq!(
            place_of(&items, None, &cursor(9, None, None)).unwrap(),
            Place::At(3)
        );
        assert_eq!(Place::At(3).after(3), 3);
        let error = place_of(&items, None, "x").unwrap_err();
        assert_eq!(error.message, "Malformed cursor");
        assert_eq!(error.extensions, Some(Map::new()));
    }
}
