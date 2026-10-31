//! The `filter` of a webhook subscription: Shopify's search syntax over the JSON payload of the
//! webhook, which decides whether it is delivered.
//!
//! The rules are shopify.dev's ("Filter webhook deliveries"):
//!
//! - fields are named explicitly, nested ones with dots (`variants.price:>=10.00`); a field of
//!   an array of objects matches when one of them does (`line_items.properties.name:x`);
//! - `:` is equality, and matching is **case-sensitive** (search is not);
//! - a field the payload does not have, or a value of another type, makes its term false: no
//!   delivery, unless the term is negated (`-invalid_field:*` is true, as shopify.dev's
//!   example shows); a term without a field is such a term;
//! - tags are a string in the payload (`"tags": "music, vinyl"`): `tags:vinyl` matches one of
//!   them;
//! - a filter that does not follow the grammar is refused when the subscription is created:
//!   [`Filter::parse`] fails.

use chrono::{DateTime, Utc};
use serde_json::Value as Json;

use super::{Comparator, Node, Options, Query, SyntaxError, Term, Value, date};

/// A parsed webhook filter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    pub query: Query,
}

impl Filter {
    /// Parses a filter; one that does not follow the grammar is an error, which
    /// `webhookSubscriptionCreate` reports and `shopify.app.toml` should not have.
    pub fn parse(filter: &str) -> Result<Filter, SyntaxError> {
        Ok(Filter {
            query: Query::parse_strict(filter)?,
        })
    }

    /// The payload fields the filter reads (`variants.price`), each once: with `include_fields`,
    /// they must be included too.
    pub fn fields(&self) -> Vec<String> {
        self.query.fields()
    }

    /// Whether the webhook with this payload is delivered.
    pub fn matches(&self, payload: &Json, options: &Options) -> bool {
        self.query
            .root
            .as_ref()
            .is_none_or(|root| evaluate(root, payload, options))
    }
}

/// Whether `filter` lets the webhook with `payload` through: [`Filter::parse`] then
/// [`Filter::matches`].
pub fn webhook_filter_matches(
    filter: &str,
    payload: &Json,
    options: &Options,
) -> Result<bool, SyntaxError> {
    Ok(Filter::parse(filter)?.matches(payload, options))
}

fn evaluate(node: &Node, payload: &Json, options: &Options) -> bool {
    match node {
        Node::Term(term) => matches_term(term, payload, options),
        Node::Not(inner) => !evaluate(inner, payload, options),
        Node::And(items) => items.iter().all(|item| evaluate(item, payload, options)),
        Node::Or(items) => items.iter().any(|item| evaluate(item, payload, options)),
    }
}

/// The scalar values at a dotted path, through arrays at any level.
fn values_at<'a>(payload: &'a Json, path: &str) -> Vec<&'a Json> {
    let mut current = vec![payload];
    for key in path.split('.') {
        let mut next = Vec::new();
        for value in current {
            collect(value, key, &mut next);
        }
        current = next;
    }
    let mut leaves = Vec::new();
    for value in current {
        flatten(value, &mut leaves);
    }
    leaves
}

fn collect<'a>(value: &'a Json, key: &str, out: &mut Vec<&'a Json>) {
    match value {
        Json::Object(fields) => {
            if let Some(field) = fields.get(key) {
                out.push(field);
            }
        }
        Json::Array(items) => {
            for item in items {
                collect(item, key, out);
            }
        }
        _ => {}
    }
}

fn flatten<'a>(value: &'a Json, out: &mut Vec<&'a Json>) {
    match value {
        Json::Array(items) => {
            for item in items {
                flatten(item, out);
            }
        }
        Json::Null => {}
        value => out.push(value),
    }
}

fn matches_term(term: &Term, payload: &Json, options: &Options) -> bool {
    let Some(path) = &term.field else {
        return false;
    };
    let values = values_at(payload, path);
    if term.value == Value::Any {
        return !values.is_empty();
    }
    let tags = path == "tags" || path.ends_with(".tags");
    values.into_iter().any(|value| match value {
        Json::String(text) if tags => text
            .split(',')
            .map(str::trim)
            .any(|tag| matches_string(term, tag, options)),
        Json::String(text) => matches_string(term, text, options),
        Json::Number(number) => number
            .as_f64()
            .is_some_and(|number| matches_number(term, number)),
        Json::Bool(flag) => {
            term.comparator == Comparator::Eq
                && matches!(term.value, Value::Word(_) | Value::Phrase(_))
                && term.value.text() == if *flag { "true" } else { "false" }
        }
        _ => false,
    })
}

fn matches_number(term: &Term, number: f64) -> bool {
    match &term.value {
        Value::Word(text) | Value::Phrase(text) => text
            .parse::<f64>()
            .is_ok_and(|wanted| super::compare_ordered(term.comparator, number, wanted)),
        _ => false,
    }
}

/// A string of the payload: compared as a number when both are numbers (`"price": "29.99"`
/// with `variants.price:>=10.00`), as a date when both are dates, as text otherwise, case
/// and all.
fn matches_string(term: &Term, text: &str, options: &Options) -> bool {
    let wanted = term.value.text();
    if let Value::Prefix(prefix) = &term.value {
        return term.comparator == Comparator::Eq && text.starts_with(prefix.as_str());
    }
    if let (Ok(number), Ok(wanted)) = (text.trim().parse::<f64>(), wanted.trim().parse::<f64>()) {
        return super::compare_ordered(term.comparator, number, wanted);
    }
    if term.comparator != Comparator::Eq
        && let Ok(at) = DateTime::parse_from_rfc3339(text)
        && let Some(period) = date::period(wanted, options.now, options.offset)
    {
        return period.compare(term.comparator, at.with_timezone(&Utc));
    }
    super::compare_ordered(term.comparator, text, wanted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn options() -> Options {
        Options::utc(
            DateTime::parse_from_rfc3339("2026-10-31T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        )
    }

    fn delivers(filter: &str, payload: &Json) -> bool {
        webhook_filter_matches(filter, payload, &options())
            .unwrap_or_else(|error| panic!("{filter}: {error}"))
    }

    /// The payload of shopify.dev's example.
    fn album() -> Json {
        json!({
            "id": 9554194432293_u64,
            "title": "Greatest Hits Collection",
            "status": "active",
            "product_type": "Music",
            "vendor": "My Store",
            "variants": [{
                "id": 123456789,
                "title": "Album Edition",
                "price": "129.99",
                "taxable": true,
                "weight": 0.2,
                "sku": "GHC-001",
            }],
            "tags": "music, vinyl",
            "updated_at": "2026-10-01T10:00:00-04:00",
        })
    }

    #[test]
    fn delivers_as_shopify_documents() {
        let filter = "id:* AND status:active AND (product_type:Music OR product_type:Movies) AND variants.taxable:true AND variants.weight:<5 AND variants.price:>=100 AND variants.title:Album*";
        assert!(delivers(filter, &album()));
        let with_invalid = "id:* AND status:active AND (product_type:Music OR product_type:Movies) AND -invalid_field:* AND variants.taxable:true AND variants.weight:<5 AND variants.price:>=100 AND variants.title:Album*";
        assert!(delivers(with_invalid, &album()));
        assert!(delivers("variants.price:>=10.00", &album()));

        let mut cheap = album();
        cheap["variants"][0]["price"] = json!("29.99");
        assert!(!delivers(filter, &cheap));
        assert!(delivers("variants.price:>=10.00", &cheap));
        cheap["variants"][0]["price"] = json!("9.99");
        assert!(!delivers("variants.price:>=10.00", &cheap));
    }

    #[test]
    fn is_case_sensitive_and_explicit() {
        assert!(delivers("product_type:Music", &album()));
        assert!(!delivers("product_type:music", &album()));
        assert!(!delivers("Music", &album()), "a term without a field");
        assert!(!delivers("invalid_field:x", &album()));
        assert!(delivers("-invalid_field:x", &album()));
        assert!(
            !delivers("variants.taxable:yes", &album()),
            "a type mismatch"
        );
        assert!(delivers("vendor:\"My Store\"", &album()));
    }

    #[test]
    fn matches_arrays_and_tags() {
        let order = json!({
            "line_items": [
                {"product_exists": true, "product_id": 9554194432293_u64, "properties": []},
                {"product_exists": true, "product_id": 9554194465061_u64,
                 "properties": [{"name": "_your_custom_property", "value": "some-value"}]},
            ],
        });
        assert!(delivers(
            "line_items.properties.name:_your_custom_property",
            &order
        ));
        assert!(!delivers("line_items.properties.name:other", &order));
        assert!(delivers("line_items.product_id:9554194465061", &order));
        assert!(delivers("tags:vinyl", &album()));
        assert!(delivers("tags:music", &album()));
        assert!(!delivers("tags:jazz", &album()));
        assert!(delivers(
            "type:banana OR type:apple",
            &json!({"type": "apple"})
        ));
    }

    #[test]
    fn compares_dates() {
        assert!(delivers("updated_at:>2026-09-30", &album()));
        assert!(!delivers("updated_at:>2026-10-01", &album()));
        assert!(delivers("updated_at:<now", &album()));
    }

    #[test]
    fn refuses_what_is_not_the_grammar() {
        for bad in [
            "status:active AND",
            "(status:active",
            "status:",
            "status:\"open",
        ] {
            assert!(Filter::parse(bad).is_err(), "{bad}");
        }
        let filter = Filter::parse("variants.price:>=10 AND (tags:a OR -status:draft)").unwrap();
        assert_eq!(filter.fields(), ["variants.price", "tags", "status"]);
        assert!(Filter::parse("").unwrap().matches(&album(), &options()));
    }
}
