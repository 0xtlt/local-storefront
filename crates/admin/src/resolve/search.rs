//! The `query` argument of a connection over records: [`crate::search`]'s syntax, matched
//! against each record through a [`Searchable`] that reads the filters the connection documents
//! (`schemas/2026-10/search.json`).
//!
//! A documented filter is read from the record generically, by its documented type, when its
//! name says where the value is:
//!
//! | Filter | Read from |
//! |---|---|
//! | `id` | the number of the record's GID |
//! | `<x>_id` (`customer_id`, `collection_id`) | the GID in `x`, or the GIDs in `xs` |
//! | `tag` | each of `tags`; `tag_not:x` is `NOT tag:x` (any `<f>_not` whose `<f>` is documented) |
//! | a `time` or `date` (`created_at`) | `createdAt` |
//! | a `boolean` (`gift_card`) | `giftCard`, else `isGiftCard` |
//! | a number | the number (or the decimal string) in the camelCase field |
//! | a `string` | the camelCase field (or its plural), tokenized text for `title`, `name`, `body`, `description`, `note`, a whole keyword otherwise |
//! | `metafields.{namespace}.{key}` | the `value` of the record's metafield |
//! | `fields.{key}` | the `value` of the record's field (a metaobject's) |
//! | `default` (a term without a field) | the title, name, handle, email, vendor, product type, SKU and tags |
//!
//! A domain registers what does not follow (`sku` of a product is in its variants):
//! [`super::Registry::search_field`] and [`super::Registry::default_text`].
//!
//! As shopify.dev says, a field the connection does not document is ignored, with the warning
//! Shopify returns in `extensions.search`: `[{"path", "query", "parsed", "warnings": [{"field",
//! "message": "Invalid search field for this query."}]}]` (the keys of shopify.dev's "Search
//! syntax" example, in its order). A documented filter lsf cannot read is ignored too, and said
//! in `extensions.lsf.notes`.

use serde_json::{Value, json};

use super::{ShopResolver, camel_case};
use crate::graphql::FieldContext;
use crate::search::{self, Node, Query, SearchValue, Searchable, Term};
use crate::state::{Record, parse_gid};

/// The string filters that are tokenized text, not a keyword matched whole.
const TEXT_FILTERS: &[&str] = &[
    "title",
    "name",
    "body",
    "description",
    "note",
    "display_name",
];

/// The fields a term without a field is searched in, when the domain gives none.
const DEFAULT_TEXT: &[&str] = &[
    "title",
    "name",
    "displayName",
    "handle",
    "email",
    "vendor",
    "productType",
    "sku",
];

/// The items that match the field's `query`, with its warnings in `extensions.search`.
pub(super) fn filter(
    resolver: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    node_type: &str,
    items: Vec<Value>,
) -> Vec<Value> {
    if ctx.definition().arg("query").is_none() {
        return items;
    }
    let Some(text) = ctx.arg_str("query").filter(|text| !text.trim().is_empty()) else {
        return items;
    };
    let documented = search::documented_filters(ctx.parent_type(), ctx.field_name()).unwrap_or(&[]);
    let query = Query::parse(text);
    let warnings = query.invalid_fields(|field| documented_type(documented, field).is_some());
    if !warnings.is_empty() {
        resolver.report_search(json!({
            "path": ctx.path(),
            "query": text,
            "parsed": query.parsed(),
            "warnings": warnings.iter().map(search::Warning::to_json).collect::<Vec<_>>(),
        }));
    }
    matching(resolver, node_type, documented, query, items)
}

/// The items of a search, by the filters a connection documents (`documented_filters(parent,
/// field)`), outside of a field's `query`: what a bulk mutation's `search` selects.
pub(super) fn filter_text(
    resolver: &ShopResolver<'_>,
    node_type: &str,
    (parent, field): (&str, &str),
    text: &str,
    items: Vec<Value>,
) -> Vec<Value> {
    let documented = search::documented_filters(parent, field).unwrap_or(&[]);
    matching(resolver, node_type, documented, Query::parse(text), items)
}

/// The items a parsed query matches.
fn matching(
    resolver: &ShopResolver<'_>,
    node_type: &str,
    documented: &[(String, String)],
    mut query: Query,
    items: Vec<Value>,
) -> Vec<Value> {
    if let Some(root) = query.root.take() {
        query.root = Some(negations(root, documented));
    }
    let options = resolver.search_options();
    items
        .into_iter()
        .filter(|item| {
            let Some(record) = resolver.item_map(item) else {
                return false;
            };
            let searched = Searched {
                resolver,
                node_type,
                item,
                record,
                documented,
            };
            query.matches(&searched, &options)
        })
        .collect()
}

/// The type a connection documents for a filter (`time` for `created_at`), a `{placeholder}` of
/// a documented name standing for one segment (`metafields.custom.color`).
fn documented_type<'d>(documented: &'d [(String, String)], filter: &str) -> Option<&'d str> {
    let filter = filter.to_ascii_lowercase();
    let parts: Vec<&str> = filter.split('.').collect();
    documented.iter().find_map(|(name, kind)| {
        let pattern: Vec<&str> = name.split('.').collect();
        (pattern.len() == parts.len()
            && pattern
                .iter()
                .zip(&parts)
                .all(|(pattern, part)| pattern.starts_with('{') || pattern == part))
        .then_some(kind.as_str())
    })
}

/// `tag_not:x` as `NOT tag:x`: a `<filter>_not` whose `<filter>` the connection documents.
fn negations(node: Node, documented: &[(String, String)]) -> Node {
    match node {
        Node::Term(term) => {
            let base = term
                .field
                .as_deref()
                .and_then(|field| {
                    field
                        .to_ascii_lowercase()
                        .strip_suffix("_not")
                        .map(str::to_string)
                })
                .filter(|base| documented_type(documented, base).is_some());
            match base {
                Some(base) => Node::Not(Box::new(Node::Term(Term {
                    field: Some(base),
                    ..term
                }))),
                None => Node::Term(term),
            }
        }
        Node::Not(inner) => Node::Not(Box::new(negations(*inner, documented))),
        Node::And(items) => Node::And(
            items
                .into_iter()
                .map(|n| negations(n, documented))
                .collect(),
        ),
        Node::Or(items) => Node::Or(
            items
                .into_iter()
                .map(|n| negations(n, documented))
                .collect(),
        ),
    }
}

/// A record as a search sees it.
struct Searched<'r, 'a> {
    resolver: &'r ShopResolver<'a>,
    node_type: &'r str,
    item: &'r Value,
    record: &'r Record,
    documented: &'r [(String, String)],
}

impl Searchable for Searched<'_, '_> {
    fn values(&self, field: &str) -> Option<Vec<SearchValue>> {
        let kind = documented_type(self.documented, field)?;
        if let Some(read) = self.resolver.registry.search_fn(self.node_type, field) {
            return read(self.resolver, self.item, self.record);
        }
        let values = generic(
            self.resolver,
            self.node_type,
            self.item,
            self.record,
            field,
            kind,
        );
        if values.is_none() {
            self.resolver.note(format!(
                "lsf does not read the search filter `{field}` of {}: the term was ignored",
                self.node_type
            ));
        }
        values
    }

    fn default_text(&self) -> Vec<String> {
        if let Some(read) = self.resolver.registry.text_fn(self.node_type) {
            return read(self.resolver, self.record);
        }
        let mut texts: Vec<String> = DEFAULT_TEXT
            .iter()
            .filter_map(|field| self.record.get(*field).and_then(Value::as_str))
            .map(str::to_string)
            .collect();
        texts.extend(strings(self.record.get("tags")));
        texts
    }
}

/// The strings of a value: itself, or the strings of an array.
fn strings(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(text)) => vec![text.clone()],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// The value of a documented filter, read generically (see the module's table). `None` when
/// lsf does not know where it is.
fn generic(
    resolver: &ShopResolver<'_>,
    node_type: &str,
    item: &Value,
    record: &Record,
    field: &str,
    kind: &str,
) -> Option<Vec<SearchValue>> {
    let field = field.to_ascii_lowercase();
    let schema = crate::schema::admin();
    let has_field = |name: &str| {
        record.contains_key(name)
            || schema
                .get(node_type)
                .is_some_and(|def| def.fields.contains_key(name))
    };
    if field == "id" {
        let key = item
            .as_str()
            .or_else(|| record.get("id").and_then(Value::as_str));
        return Some(
            key.and_then(parse_gid)
                .map(|(_, id)| SearchValue::Id(id))
                .into_iter()
                .collect(),
        );
    }
    if let Some(rest) = field.strip_prefix("metafields.") {
        let (namespace, key) = rest.split_once('.')?;
        let namespace = resolver.app_namespace(namespace);
        let value = record
            .get("metafields")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| resolver.item_map(item))
            .find(|metafield| {
                metafield.get("namespace").and_then(Value::as_str) == Some(namespace.as_str())
                    && metafield.get("key").and_then(Value::as_str) == Some(key)
            })
            .and_then(|metafield| metafield.get("value").and_then(Value::as_str));
        return Some(
            value
                .map(|value| SearchValue::Keyword(value.to_string()))
                .into_iter()
                .collect(),
        );
    }
    // `fields.{key}` of `metaobjects`: the `value` of the entry's field.
    if let Some(key) = field.strip_prefix("fields.") {
        let value = record
            .get("fields")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|entry| entry.get("key").and_then(Value::as_str) == Some(key))
            .and_then(|entry| entry.get("value").and_then(Value::as_str));
        return Some(
            value
                .map(|value| SearchValue::Keyword(value.to_string()))
                .into_iter()
                .collect(),
        );
    }
    if let Some(base) = field.strip_suffix("_id") {
        let name = camel_case(base);
        let plural = format!("{name}s");
        let source = [name.as_str(), plural.as_str()]
            .into_iter()
            .find(|name| has_field(name))?;
        return Some(
            strings(record.get(source))
                .iter()
                .filter_map(|gid| parse_gid(gid))
                .map(|(_, id)| SearchValue::Id(id))
                .collect(),
        );
    }
    let name = camel_case(&field);
    let candidates: Vec<String> = match kind {
        "boolean" => {
            let mut pascal = name.clone();
            if let Some(first) = pascal.get_mut(0..1) {
                first.make_ascii_uppercase();
            }
            vec![name.clone(), format!("is{pascal}")]
        }
        "string" => vec![name.clone(), format!("{name}s")],
        _ => vec![name.clone()],
    };
    let source = candidates.iter().find(|name| has_field(name))?;
    let value = record.get(source.as_str());
    Some(match kind {
        "time" | "date" => strings(value)
            .iter()
            .filter_map(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
            .map(|date| SearchValue::Date(date.with_timezone(&chrono::Utc)))
            .collect(),
        "boolean" => value
            .and_then(Value::as_bool)
            .map(SearchValue::Bool)
            .into_iter()
            .collect(),
        "integer" | "float" | "bigdecimal" => {
            number(value).map(SearchValue::Number).into_iter().collect()
        }
        _ => {
            let text = TEXT_FILTERS.contains(&field.as_str());
            strings(value)
                .into_iter()
                .map(|value| {
                    if text {
                        SearchValue::Text(value)
                    } else {
                        SearchValue::Keyword(value)
                    }
                })
                .collect()
        }
    })
}

/// A number, or a decimal written as a string (`"19.00"`), or the `amount` of a `MoneyV2`.
pub(super) fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        Value::Object(map) => number(map.get("amount")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::{Comparator, Value as Word};

    fn term(field: Option<&str>, value: &str) -> Term {
        Term {
            field: field.map(str::to_string),
            comparator: Comparator::Eq,
            value: Word::Word(value.to_string()),
        }
    }

    fn documented() -> Vec<(String, String)> {
        [
            ("tag", "string"),
            ("tag_not", "string"),
            ("created_at", "time"),
            ("metafields.{namespace}.{key}", "mixed"),
        ]
        .iter()
        .map(|(name, kind)| (name.to_string(), kind.to_string()))
        .collect()
    }

    #[test]
    fn reads_documented_types_with_placeholders() {
        let documented = documented();
        assert_eq!(documented_type(&documented, "created_at"), Some("time"));
        assert_eq!(documented_type(&documented, "Created_At"), Some("time"));
        assert_eq!(
            documented_type(&documented, "metafields.custom.color"),
            Some("mixed")
        );
        assert_eq!(documented_type(&documented, "metafields.custom"), None);
        assert_eq!(documented_type(&documented, "vendor"), None);
    }

    #[test]
    fn a_not_filter_is_the_negation_of_its_filter() {
        let documented = documented();
        let rewritten = negations(Node::Term(term(Some("tag_not"), "sale")), &documented);
        assert_eq!(
            rewritten,
            Node::Not(Box::new(Node::Term(term(Some("tag"), "sale"))))
        );
        // An undocumented base is left alone.
        let left = negations(Node::Term(term(Some("vendor_not"), "x")), &documented);
        assert_eq!(left, Node::Term(term(Some("vendor_not"), "x")));
    }
}
