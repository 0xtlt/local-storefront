//! Shopify's search syntax: the `query` argument of connections (`products(query: "title:shirt
//! AND -tag:sale")`), and the `filter` of webhook subscriptions ([`filter`]).
//!
//! The grammar ([`syntax`]) is shopify.dev's "Search query syntax": terms, `field:value`, the
//! comparators `:<`, `:<=`, `:>`, `:>=`, the connectives `AND` and `OR` (`AND` when there is
//! none; `OR` binds tighter: `bob OR norman Shopify` is `(bob OR norman) AND Shopify`), the
//! modifiers `NOT` and `-`, parentheses, quoted phrases, `prefix*`, and `field:*` for a field
//! that has a value.
//!
//! **Matching a record.** [`Query::matches`] asks a [`Searchable`] record for the values of each
//! field the query names; the type of a value says how it compares ([`SearchValue`]). As
//! shopify.dev says:
//!
//! - a term without a field searches the record's default text, case-insensitively;
//! - `:` on a tokenized field (`Text`) finds the term among its words, on any other field it is
//!   equality; `:` ignores case, and an unquoted value with commas is a list of alternatives
//!   (`handle:a,b`, `channel:web,pos`), as several filters document;
//! - dates are [`date`]'s periods (`created_at:<=2024`, `updated_at:>'2020-10-21T23:39:20Z'`,
//!   `created_at:<now`);
//! - **a field the type does not know is ignored**: "If you specify an invalid field, then the
//!   query is ignored and all results are returned." The term drops out of the query (`a AND
//!   bad:x` is `a`, a query of only unknown fields matches everything), and
//!   [`Query::invalid_fields`] gives the warnings Shopify returns in `extensions.search`
//!   ("Invalid search field for this query.");
//! - **a malformed query is read as far as it goes**, with warnings, never refused: Shopify
//!   answers it with results and "warnings in the `extensions` section".
//!
//! **The filters of each connection.** The reference of each version documents, in the
//! description of each `query` argument, the filters it accepts and their type (`created_at`:
//! `time`); `tools/admin/access_scopes.mjs --version <v>` writes them into
//! `schemas/<v>/search.json`, and [`for_version`] reads them: what a [`Searchable`] should
//! answer, and what [`Query::invalid_fields`] can be given to warn about the others.

pub mod date;
pub mod filter;
mod syntax;

use std::collections::HashMap;
use std::sync::OnceLock;

use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::{Value as Json, json};

use crate::version::{self, ApiVersion};

pub use filter::Filter;

/// How a term compares a field with its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Comparator {
    /// `:`
    Eq,
    /// `:<`
    Lt,
    /// `:<=`
    Le,
    /// `:>`
    Gt,
    /// `:>=`
    Ge,
}

/// The value of a term.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// Unquoted: `Bob`, `2024-01-01`, `gid://shopify/Product/1`, `a,b`.
    Word(String),
    /// Quoted: `"Bob Norman"`, `'2020-10-21T23:39:20Z'`.
    Phrase(String),
    /// `norm*`: what starts with `norm`.
    Prefix(String),
    /// `*`: any value; `field:*` matches a field that has one.
    Any,
}

impl Value {
    /// The text of the value (empty for `*`).
    pub fn text(&self) -> &str {
        match self {
            Value::Word(text) | Value::Phrase(text) | Value::Prefix(text) => text,
            Value::Any => "",
        }
    }
}

/// `field:value`, or a value alone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Term {
    /// The field, as written; `None` for a term of the default search.
    pub field: Option<String>,
    pub comparator: Comparator,
    pub value: Value,
}

/// A query, as a tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Node {
    Term(Term),
    Not(Box<Node>),
    And(Vec<Node>),
    Or(Vec<Node>),
}

/// Something a query does that Shopify warns about: a field it does not know for this
/// connection, or a piece of the query it could not read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Warning {
    pub field: Option<String>,
    pub message: String,
}

impl Warning {
    fn new(field: Option<String>, message: &str) -> Warning {
        Warning {
            field,
            message: message.to_string(),
        }
    }

    /// As Shopify writes it in `extensions.search[].warnings`: `{"field", "message"}`.
    pub fn to_json(&self) -> Json {
        match &self.field {
            Some(field) => json!({"field": field, "message": self.message}),
            None => json!({"message": self.message}),
        }
    }
}

/// A query that does not follow the grammar, where only correct ones are accepted (a webhook
/// filter).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxError {
    pub message: String,
    /// The token (not the character) where it was found.
    pub position: usize,
}

impl SyntaxError {
    fn new(message: impl Into<String>, position: usize) -> SyntaxError {
        SyntaxError {
            message: message.into(),
            position,
        }
    }
}

impl std::fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SyntaxError {}

/// A parsed query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    /// The query as it was sent.
    pub source: String,
    /// `None` for an empty query, which matches everything.
    pub root: Option<Node>,
    /// What could not be read.
    pub warnings: Vec<Warning>,
}

impl Query {
    /// Parses a `query` argument. Never fails: what does not follow the grammar becomes a
    /// warning, and the rest is kept.
    pub fn parse(query: &str) -> Query {
        let (root, warnings) = syntax::parse(query, false).unwrap_or_else(|_| (None, Vec::new()));
        Query {
            source: query.to_string(),
            root,
            warnings,
        }
    }

    /// Parses a query that must follow the grammar.
    pub fn parse_strict(query: &str) -> Result<Query, SyntaxError> {
        let (root, warnings) = syntax::parse(query, true)?;
        Ok(Query {
            source: query.to_string(),
            root,
            warnings,
        })
    }

    /// The fields the query names, lowercase, in order, each once.
    pub fn fields(&self) -> Vec<String> {
        let mut fields = Vec::new();
        if let Some(root) = &self.root {
            visit_terms(root, &mut |term| {
                if let Some(field) = &term.field {
                    let field = field.to_ascii_lowercase();
                    if !fields.contains(&field) {
                        fields.push(field);
                    }
                }
            });
        }
        fields
    }

    /// The warnings of the fields the connection does not know (`known` says which it does),
    /// as Shopify words them, after those of the syntax.
    pub fn invalid_fields(&self, known: impl Fn(&str) -> bool) -> Vec<Warning> {
        let mut warnings = self.warnings.clone();
        for field in self.fields() {
            if field != "default" && !known(&field) {
                warnings.push(Warning::new(
                    Some(field),
                    "Invalid search field for this query.",
                ));
            }
        }
        warnings
    }

    /// The query as `extensions.search[].parsed` shows it: `{"and": [...]}`, `{"or": [...]}`,
    /// `{"not": …}`, and for a term `{"field", "match_all"}` (the form of shopify.dev's
    /// example), `match_phrase`, `match_prefix`, `exists`, or `range` with `lt`, `lte`, `gt`,
    /// `gte`. Only `match_all` is Shopify's documented shape; the others follow it.
    pub fn parsed(&self) -> Json {
        fn shown(node: &Node) -> Json {
            match node {
                Node::And(items) => json!({"and": items.iter().map(shown).collect::<Vec<_>>()}),
                Node::Or(items) => json!({"or": items.iter().map(shown).collect::<Vec<_>>()}),
                Node::Not(inner) => json!({"not": shown(inner)}),
                Node::Term(term) => {
                    let field = term.field.as_deref().unwrap_or("default");
                    let bound = match term.comparator {
                        Comparator::Eq => None,
                        Comparator::Lt => Some("lt"),
                        Comparator::Le => Some("lte"),
                        Comparator::Gt => Some("gt"),
                        Comparator::Ge => Some("gte"),
                    };
                    match (&term.value, bound) {
                        (Value::Any, _) => json!({"field": field, "exists": true}),
                        (value, Some(bound)) => {
                            json!({"field": field, "range": {bound: value.text()}})
                        }
                        (Value::Phrase(text), None) => {
                            json!({"field": field, "match_phrase": text})
                        }
                        (Value::Prefix(text), None) => {
                            json!({"field": field, "match_prefix": text})
                        }
                        (Value::Word(text), None) => json!({"field": field, "match_all": text}),
                    }
                }
            }
        }
        self.root.as_ref().map_or(json!({}), shown)
    }

    /// Whether `record` matches, at `options.now` in the shop's time zone.
    pub fn matches(&self, record: &dyn Searchable, options: &Options) -> bool {
        self.root
            .as_ref()
            .and_then(|root| evaluate(root, record, options))
            .unwrap_or(true)
    }
}

fn visit_terms(node: &Node, visit: &mut dyn FnMut(&Term)) {
    match node {
        Node::Term(term) => visit(term),
        Node::Not(inner) => visit_terms(inner, visit),
        Node::And(items) | Node::Or(items) => {
            for item in items {
                visit_terms(item, visit);
            }
        }
    }
}

/// The search filters a version documents: `schemas/<version>/search.json`.
#[derive(Deserialize)]
pub struct Documented {
    connections: HashMap<String, HashMap<String, Vec<(String, String)>>>,
}

/// The search filters of a version, parsed the first time a request of the version searches.
pub fn for_version(version: ApiVersion) -> &'static Documented {
    static PARSED: [OnceLock<Documented>; version::COUNT] =
        [const { OnceLock::new() }; version::COUNT];
    PARSED[version.index()].get_or_init(|| {
        serde_json::from_str(version.files().search).unwrap_or_else(|error| {
            panic!("schemas/{version}/search.json is the table tools/admin/access_scopes.mjs writes: {error}")
        })
    })
}

impl Documented {
    /// The filters the `query` argument of `field` of `type_name` documents, as
    /// `(name, type)`: `("created_at", "time")`, `("tag", "string")`, `("id", "id")`,
    /// `("metafields.{namespace}.{key}", "mixed")`. The types are `string`, `id`, `time`,
    /// `date`, `boolean`, `integer`, `float`, `bigdecimal` and `mixed`. `None` for a field
    /// without one.
    pub fn filters(&self, type_name: &str, field: &str) -> Option<&[(String, String)]> {
        self.connections
            .get(type_name)?
            .get(field)
            .map(Vec::as_slice)
    }

    /// Whether `filter` (`title`, `metafields.custom.on_sale`) is one `field` of `type_name`
    /// documents; a `{placeholder}` of a documented name stands for one segment.
    pub fn is_documented(&self, type_name: &str, field: &str, filter: &str) -> bool {
        let filter = filter.to_ascii_lowercase();
        self.filters(type_name, field).is_some_and(|filters| {
            filters.iter().any(|(name, _)| {
                let pattern: Vec<&str> = name.split('.').collect();
                let parts: Vec<&str> = filter.split('.').collect();
                pattern.len() == parts.len()
                    && pattern
                        .iter()
                        .zip(&parts)
                        .all(|(pattern, part)| pattern.starts_with('{') || pattern == part)
            })
        })
    }
}

/// [`Documented::filters`] of the default version: what code that does not know the version of
/// its request asks. The resolver asks the filters of its request's version.
pub fn documented_filters(type_name: &str, field: &str) -> Option<&'static [(String, String)]> {
    for_version(ApiVersion::default_version()).filters(type_name, field)
}

/// [`Documented::is_documented`] of the default version.
pub fn is_documented_filter(type_name: &str, field: &str, filter: &str) -> bool {
    for_version(ApiVersion::default_version()).is_documented(type_name, field, filter)
}

/// What a search compares dates with.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// What `now` is: the shop's clock.
    pub now: DateTime<Utc>,
    /// The shop's time zone, for dates written without one (`created_at:2024-01-15` is that day
    /// in the shop).
    pub offset: FixedOffset,
}

impl Options {
    /// At `now`, in UTC.
    pub fn utc(now: DateTime<Utc>) -> Options {
        Options {
            now,
            offset: FixedOffset::east_opt(0).expect("UTC is an offset"),
        }
    }

    /// At the shop's clock, in the shop's time zone (`Shop.timezoneOffsetMinutes`, UTC without
    /// one): how a search reads a date without a zone, and how every input that gives a day
    /// without a time (`2026-03-15`) is read, so that a day is the shop's everywhere.
    pub fn of(state: &crate::state::ShopState) -> Options {
        let offset_minutes = state
            .shop()
            .and_then(|shop| shop.get("timezoneOffsetMinutes"))
            .and_then(Json::as_i64)
            .unwrap_or(0);
        Options {
            now: state.now(),
            offset: i32::try_from(offset_minutes * 60)
                .ok()
                .and_then(FixedOffset::east_opt)
                .unwrap_or_else(|| FixedOffset::east_opt(0).expect("UTC is an offset")),
        }
    }

    /// The first instant of a day of the shop.
    pub fn day_start(&self, day: NaiveDate) -> DateTime<Utc> {
        use chrono::TimeZone;
        self.offset
            .from_local_datetime(&day.and_time(chrono::NaiveTime::MIN))
            .single()
            .map_or_else(
                || day.and_time(chrono::NaiveTime::MIN).and_utc(),
                |at| at.with_timezone(&Utc),
            )
    }

    /// The last second of a day of the shop.
    pub fn day_end(&self, day: NaiveDate) -> DateTime<Utc> {
        day.succ_opt().map_or_else(
            || self.day_start(day),
            |next| self.day_start(next) - chrono::Duration::seconds(1),
        )
    }

    /// The shop's day of an instant (`today` for its clock).
    pub fn day_of(&self, at: DateTime<Utc>) -> NaiveDate {
        at.with_timezone(&self.offset).date_naive()
    }

    /// A `DateTime` as written, or a `Date` at the start of that day of the shop.
    pub fn instant(&self, text: &str) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(text.trim())
            .map(|at| at.with_timezone(&Utc))
            .ok()
            .or_else(|| {
                NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d")
                    .ok()
                    .map(|day| self.day_start(day))
            })
    }
}

/// A value of a field of a record, typed by how a search compares it.
#[derive(Clone, Debug, PartialEq)]
pub enum SearchValue {
    /// Tokenized text (`title`, `body`): `:` finds the term, or the phrase, among its words.
    Text(String),
    /// A value matched whole (`status`, `handle`, `sku`, `vendor`, a tag, an email): `:` is
    /// equality, ignoring case; `*` a prefix.
    Keyword(String),
    /// `inventory_total`, `price`, `orders_count`.
    Number(f64),
    /// `created_at`, `updated_at`.
    Date(DateTime<Utc>),
    /// `gift_card:true`.
    Bool(bool),
    /// A numeric id: `id:1234`, `id:>=1234`, `customer_id:gid://shopify/Customer/1`.
    Id(u64),
}

/// A record a search runs on: the domains implement it for each type of record, from the
/// filters each connection documents (`title`, `tag`, `status`, `vendor`, `sku`, `email`, …).
pub trait Searchable {
    /// The values of `field` (lowercase). `None` when the type has no such field, which makes
    /// Shopify ignore the term; `Some(vec![])` when the record has no value (`field:*` is
    /// false).
    fn values(&self, field: &str) -> Option<Vec<SearchValue>>;

    /// The text a term without a field is searched in: the title, the handle, the tags, …
    fn default_text(&self) -> Vec<String>;
}

/// The result of a node: `None` when it is ignored (an unknown field).
fn evaluate(node: &Node, record: &dyn Searchable, options: &Options) -> Option<bool> {
    match node {
        Node::Term(term) => match term.field.as_deref().map(str::to_ascii_lowercase) {
            None => Some(matches_default(term, &record.default_text())),
            Some(field) if field == "default" => {
                Some(matches_default(term, &record.default_text()))
            }
            Some(field) => record
                .values(&field)
                .map(|values| matches_values(term, &values, options)),
        },
        Node::Not(inner) => evaluate(inner, record, options).map(|result| !result),
        Node::And(items) => items
            .iter()
            .filter_map(|item| evaluate(item, record, options))
            .fold(None, |all, result| Some(all.unwrap_or(true) && result)),
        Node::Or(items) => items
            .iter()
            .filter_map(|item| evaluate(item, record, options))
            .fold(None, |any, result| Some(any.unwrap_or(false) || result)),
    }
}

/// The lowercase words of a text: what is between the characters that are not letters or
/// digits.
pub(crate) fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Whether `needle` occurs in `haystack` as consecutive words, its last word a prefix when
/// `prefix`.
fn has_phrase(haystack: &[String], needle: &[String], prefix: bool) -> bool {
    if needle.is_empty() {
        return false;
    }
    haystack.windows(needle.len()).any(|window| {
        window
            .iter()
            .zip(needle)
            .enumerate()
            .all(|(at, (word, wanted))| {
                if prefix && at == needle.len() - 1 {
                    word.starts_with(wanted.as_str())
                } else {
                    word == wanted
                }
            })
    })
}

fn matches_default(term: &Term, texts: &[String]) -> bool {
    match &term.value {
        Value::Any => true,
        value => {
            let needle = words(value.text());
            let prefix = matches!(value, Value::Prefix(_));
            texts
                .iter()
                .any(|text| has_phrase(&words(text), &needle, prefix))
        }
    }
}

fn matches_values(term: &Term, values: &[SearchValue], options: &Options) -> bool {
    if term.value == Value::Any {
        return !values.is_empty();
    }
    // An unquoted list: any of its items.
    let alternatives: Vec<Value> = match &term.value {
        Value::Word(text) if term.comparator == Comparator::Eq && text.contains(',') => text
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
            .map(|item| Value::Word(item.to_string()))
            .collect(),
        value => vec![value.clone()],
    };
    values.iter().any(|value| {
        matches_one(term.comparator, &term.value, value, options)
            || (alternatives.len() > 1
                && alternatives
                    .iter()
                    .any(|item| matches_one(term.comparator, item, value, options)))
    })
}

fn matches_one(
    comparator: Comparator,
    wanted: &Value,
    value: &SearchValue,
    options: &Options,
) -> bool {
    let text = wanted.text();
    match value {
        SearchValue::Text(field) => match comparator {
            Comparator::Eq => has_phrase(
                &words(field),
                &words(text),
                matches!(wanted, Value::Prefix(_)),
            ),
            _ => compare_text(comparator, field, text),
        },
        SearchValue::Keyword(field) => match (comparator, wanted) {
            (Comparator::Eq, Value::Prefix(prefix)) => {
                field.to_lowercase().starts_with(&prefix.to_lowercase())
            }
            (Comparator::Eq, _) => field.to_lowercase() == text.to_lowercase(),
            _ => compare_text(comparator, field, text),
        },
        SearchValue::Number(number) => compare_number(comparator, *number, text),
        SearchValue::Id(id) => {
            let gid = text.rsplit('/').next().unwrap_or(text);
            let gid = gid.split('?').next().unwrap_or(gid);
            gid.parse::<u64>()
                .is_ok_and(|wanted| compare_ordered(comparator, *id, wanted))
        }
        SearchValue::Date(date) => date::period(text, options.now, options.offset)
            .is_some_and(|period| period.compare(comparator, *date)),
        SearchValue::Bool(flag) => {
            comparator == Comparator::Eq
                && match text.to_ascii_lowercase().as_str() {
                    "true" => *flag,
                    "false" => !*flag,
                    _ => false,
                }
        }
    }
}

/// `value` compared with `wanted` as `comparator` asks: `value < wanted` for `:<`.
fn compare_ordered<T: PartialOrd>(comparator: Comparator, value: T, wanted: T) -> bool {
    match comparator {
        Comparator::Eq => value == wanted,
        Comparator::Lt => value < wanted,
        Comparator::Le => value <= wanted,
        Comparator::Gt => value > wanted,
        Comparator::Ge => value >= wanted,
    }
}

fn compare_text(comparator: Comparator, value: &str, wanted: &str) -> bool {
    compare_ordered(comparator, value.to_lowercase(), wanted.to_lowercase())
}

/// A number compared with a number written in the query, or a range of two (`10..20`).
fn compare_number(comparator: Comparator, value: f64, wanted: &str) -> bool {
    if comparator == Comparator::Eq
        && let Some((low, high)) = wanted.split_once("..")
    {
        return match (low.trim().parse::<f64>(), high.trim().parse::<f64>()) {
            (Ok(low), Ok(high)) => low <= value && value <= high,
            _ => false,
        };
    }
    wanted
        .trim()
        .parse::<f64>()
        .is_ok_and(|wanted| compare_ordered(comparator, value, wanted))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_day_without_a_time_is_the_shops() {
        let zone = Options {
            now: "2026-03-16T00:30:00Z".parse().unwrap(),
            offset: FixedOffset::west_opt(4 * 3600).unwrap(),
        };
        let day = NaiveDate::from_ymd_opt(2026, 3, 15).unwrap();
        assert_eq!(
            zone.day_start(day).to_rfc3339(),
            "2026-03-15T04:00:00+00:00"
        );
        assert_eq!(zone.day_end(day).to_rfc3339(), "2026-03-16T03:59:59+00:00");
        assert_eq!(zone.day_of(zone.now), day);
        assert_eq!(zone.instant("2026-03-15"), Some(zone.day_start(day)));
        assert_eq!(
            zone.instant("2026-03-15T12:00:00Z")
                .map(|at| at.to_rfc3339()),
            Some("2026-03-15T12:00:00+00:00".to_string())
        );
    }

    fn word(text: &str) -> Value {
        Value::Word(text.to_string())
    }

    fn term(field: Option<&str>, comparator: Comparator, value: Value) -> Node {
        Node::Term(Term {
            field: field.map(str::to_string),
            comparator,
            value,
        })
    }

    fn field(name: &str, value: &str) -> Node {
        term(Some(name), Comparator::Eq, word(value))
    }

    fn bare(value: &str) -> Node {
        term(None, Comparator::Eq, word(value))
    }

    fn root(query: &str) -> Node {
        let parsed = Query::parse_strict(query).unwrap_or_else(|error| panic!("{query}: {error}"));
        parsed.root.unwrap()
    }

    #[test]
    fn parses_the_grammar_examples() {
        // shopify.dev, "Search query syntax".
        assert_eq!(
            root("John Smith"),
            Node::And(vec![bare("John"), bare("Smith")])
        );
        assert_eq!(
            root("state:enabled OR state:disabled"),
            Node::Or(vec![field("state", "enabled"), field("state", "disabled")])
        );
        assert_eq!(
            root("-first_name:Bob AND orders_count:>3 orders_count:<=4"),
            Node::And(vec![
                Node::Not(Box::new(field("first_name", "Bob"))),
                term(Some("orders_count"), Comparator::Gt, word("3")),
                term(Some("orders_count"), Comparator::Le, word("4")),
            ])
        );
        assert_eq!(root("-first_name:Bob"), root("NOT first_name:Bob"));
        assert_eq!(
            root("state:enabled NOT (first_name:Bob OR first_name:Norman)"),
            Node::And(vec![
                field("state", "enabled"),
                Node::Not(Box::new(Node::Or(vec![
                    field("first_name", "Bob"),
                    field("first_name", "Norman")
                ]))),
            ])
        );
        assert_eq!(
            root("NOT first_name:Bob NOT first_name:Norman"),
            Node::And(vec![
                Node::Not(Box::new(field("first_name", "Bob"))),
                Node::Not(Box::new(field("first_name", "Norman"))),
            ])
        );
        // OR binds tighter than AND.
        let expected = Node::And(vec![
            Node::Or(vec![bare("bob"), bare("norman")]),
            bare("Shopify"),
        ]);
        assert_eq!(root("bob OR norman AND Shopify"), expected);
        assert_eq!(root("bob OR norman Shopify"), expected);
        assert_eq!(
            root("state:disabled AND (\"sale shopper\" OR VIP)"),
            Node::And(vec![
                field("state", "disabled"),
                Node::Or(vec![
                    term(
                        None,
                        Comparator::Eq,
                        Value::Phrase("sale shopper".to_string())
                    ),
                    bare("VIP"),
                ]),
            ])
        );
        assert_eq!(
            root("first_name:\"Bob Norman\""),
            term(
                Some("first_name"),
                Comparator::Eq,
                Value::Phrase("Bob Norman".to_string())
            )
        );
        assert_eq!(
            root("norm*"),
            term(None, Comparator::Eq, Value::Prefix("norm".to_string()))
        );
        assert_eq!(
            root("published_at:*"),
            term(Some("published_at"), Comparator::Eq, Value::Any)
        );
        assert_eq!(
            root("-published_at:*"),
            Node::Not(Box::new(term(
                Some("published_at"),
                Comparator::Eq,
                Value::Any
            )))
        );
        // `first_name: Bob` searches the terms "first_name" and "Bob".
        assert_eq!(
            Query::parse("first_name: Bob").root.unwrap(),
            Node::And(vec![bare("first_name"), bare("Bob")])
        );
        // `orders_count:=16` searches the term "=16".
        assert_eq!(root("orders_count:=16"), field("orders_count", "=16"));
        // `title:Caramel Apple` is `title:Caramel` and `Apple`.
        assert_eq!(
            root("title:Caramel Apple"),
            Node::And(vec![field("title", "Caramel"), bare("Apple")])
        );
    }

    #[test]
    fn parses_values_with_colons_quotes_and_escapes() {
        assert_eq!(
            root("created_at:>'2020-10-21T23:39:20Z'"),
            term(
                Some("created_at"),
                Comparator::Gt,
                Value::Phrase("2020-10-21T23:39:20Z".to_string())
            )
        );
        assert_eq!(
            root("created_at:>=2020-01-01T12:00:00Z"),
            term(
                Some("created_at"),
                Comparator::Ge,
                word("2020-01-01T12:00:00Z")
            )
        );
        assert_eq!(
            root("id:gid://shopify/Product/1"),
            field("id", "gid://shopify/Product/1")
        );
        assert_eq!(root("title:Bob's"), field("title", "Bob's"));
        assert_eq!(root(r"title:a\:b\(c\)"), field("title", "a:b(c)"));
        assert_eq!(
            root(r#"title:"say \"hi\"""#),
            term(
                Some("title"),
                Comparator::Eq,
                Value::Phrase("say \"hi\"".to_string())
            )
        );
        assert_eq!(root(r"title:5\*"), field("title", "5*"));
        assert_eq!(root("t-shirt"), bare("t-shirt"));
        assert_eq!(
            root("metafields.custom.on_sale:true"),
            field("metafields.custom.on_sale", "true")
        );
        // `and` and `or` in lowercase are words.
        assert_eq!(
            root("rock and roll"),
            Node::And(vec![bare("rock"), bare("and"), bare("roll")])
        );
        assert_eq!(Query::parse("").root, None);
        assert_eq!(Query::parse("   ").root, None);
    }

    #[test]
    fn reads_malformed_queries_with_warnings() {
        let query = Query::parse("title:a OR (tag:b");
        assert_eq!(
            query.root.unwrap(),
            Node::Or(vec![field("title", "a"), field("tag", "b")])
        );
        assert_eq!(query.warnings.len(), 1);
        let query = Query::parse("title:a) tag:b");
        assert_eq!(
            query.root.unwrap(),
            Node::And(vec![field("title", "a"), field("tag", "b")])
        );
        assert!(!query.warnings.is_empty());
        let query = Query::parse("title:a AND");
        assert_eq!(query.root.unwrap(), field("title", "a"));
        let query = Query::parse("OR title:a -");
        assert_eq!(query.root.unwrap(), field("title", "a"));
        let query = Query::parse("title:\"unclosed");
        assert_eq!(
            query.root.unwrap(),
            term(
                Some("title"),
                Comparator::Eq,
                Value::Phrase("unclosed".to_string())
            )
        );
        for bad in [
            "title:a OR (tag:b",
            "title:a)",
            "AND title:a",
            "title:a OR",
            "title:",
            "-",
            "\"open",
        ] {
            assert!(Query::parse_strict(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_query_nested_too_deeply_is_not_read_past_the_limit() {
        // 20,000 levels overflowed the stack of the request's thread, and the server aborted.
        let deep = "(".repeat(20_000);
        let lenient = Query::parse(&deep);
        assert_eq!(lenient.root, None);
        assert_eq!(
            lenient.warnings,
            [
                Warning::new(
                    None,
                    "Parentheses nest more than 100 deep: what they hold is ignored."
                ),
                Warning::new(None, "A `(` is not closed."),
            ]
        );
        let error = Query::parse_strict(&deep).unwrap_err();
        assert_eq!(
            error.message,
            "parentheses nest more than 100 deep: what they hold is ignored"
        );
        // A deep group is skipped whole, and what follows it is read.
        let skipped = format!(
            "{}tag:a{} title:b",
            "(".repeat(syntax::MAX_DEPTH + 1),
            ")".repeat(syntax::MAX_DEPTH + 1)
        );
        let query = Query::parse(&skipped);
        assert_eq!(query.root, Some(field("title", "b")));
        assert_eq!(query.warnings.len(), 1);
        assert!(Query::parse_strict(&skipped).is_err());
        // Up to the limit, the query is read.
        let fine = format!(
            "{}tag:a{}",
            "(".repeat(syntax::MAX_DEPTH),
            ")".repeat(syntax::MAX_DEPTH)
        );
        assert_eq!(root(&fine), field("tag", "a"));
        // A chain of modifiers is no nesting: `NOT` 20,000 times, then 20,001 times.
        let even = format!("{}tag:a", "NOT ".repeat(20_000));
        assert_eq!(root(&even), field("tag", "a"));
        let odd = format!("-{}tag:a", "-".repeat(20_000));
        assert_eq!(root(&odd), Node::Not(Box::new(field("tag", "a"))));
        assert_eq!(
            root("NOT (NOT tag:a)"),
            field("tag", "a"),
            "the negations cancel through a group"
        );
        // A webhook filter is parsed strictly.
        assert!(
            Filter::parse(&format!("{}a:b{}", "(".repeat(20_000), ")".repeat(20_000))).is_err()
        );
    }

    /// A record of a test: its fields and its default text.
    struct Record {
        fields: HashMap<&'static str, Vec<SearchValue>>,
        text: Vec<String>,
    }

    impl Searchable for Record {
        fn values(&self, field: &str) -> Option<Vec<SearchValue>> {
            self.fields.get(field).cloned()
        }

        fn default_text(&self) -> Vec<String> {
            self.text.clone()
        }
    }

    fn date(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn product(title: &str, tags: &[&str], vendor: &str, inventory: f64, created: &str) -> Record {
        Record {
            fields: HashMap::from([
                ("title", vec![SearchValue::Text(title.to_string())]),
                (
                    "tag",
                    tags.iter()
                        .map(|tag| SearchValue::Keyword(tag.to_string()))
                        .collect(),
                ),
                ("vendor", vec![SearchValue::Keyword(vendor.to_string())]),
                ("status", vec![SearchValue::Keyword("ACTIVE".to_string())]),
                ("handle", vec![SearchValue::Keyword(words(title).join("-"))]),
                ("inventory_total", vec![SearchValue::Number(inventory)]),
                ("created_at", vec![SearchValue::Date(date(created))]),
                ("published_at", vec![]),
                ("gift_card", vec![SearchValue::Bool(false)]),
                ("id", vec![SearchValue::Id(1234)]),
            ]),
            text: vec![title.to_string(), vendor.to_string()]
                .into_iter()
                .chain(tags.iter().map(|tag| tag.to_string()))
                .collect(),
        }
    }

    fn options() -> Options {
        Options::utc(date("2026-10-31T12:00:00Z"))
    }

    fn finds(query: &str, record: &Record) -> bool {
        Query::parse(query).matches(record, &options())
    }

    #[test]
    fn matches_records_as_shopify_documents() {
        let apple = product(
            "Caramel Apple",
            &["sweet", "Fall sale"],
            "Orchard",
            750.0,
            "2020-10-22T10:00:00Z",
        );
        let pear = product("Pear", &[], "Orchard", 12.0, "2024-06-01T00:00:00Z");

        // Default search: case-insensitive, anywhere.
        assert!(finds("caramel", &apple));
        assert!(finds("CARAMEL apple", &apple));
        assert!(!finds("caramel pear", &apple));
        assert!(finds("orchard", &pear));
        assert!(finds("\"caramel apple\"", &apple));
        assert!(!finds("\"apple caramel\"", &apple));
        assert!(finds("cara*", &apple));
        // Field search.
        assert!(finds("title:caramel", &apple));
        assert!(finds("title:\"Caramel Apple\"", &apple));
        assert!(finds("title:car*", &apple));
        assert!(finds("title:Caramel Apple", &apple));
        assert!(!finds("title:Caramel Pear", &apple));
        assert!(finds("tag:sweet", &apple));
        assert!(finds("tag:'fall sale'", &apple));
        assert!(!finds("tag:fall", &apple), "a keyword is matched whole");
        assert!(finds("status:active", &apple));
        assert!(finds("handle:pear,caramel-apple", &apple));
        assert!(finds("vendor:Orch*", &pear));
        // Ranges and comparisons.
        assert!(finds("inventory_total:>500 inventory_total:<=1000", &apple));
        assert!(!finds("inventory_total:>500 inventory_total:<=1000", &pear));
        assert!(finds(
            "(title:Caramel Apple) OR (inventory_total:>500 inventory_total:<=1000)",
            &apple
        ));
        assert!(finds("inventory_total:12", &pear));
        assert!(finds("inventory_total:10..20", &pear));
        assert!(!finds("inventory_total:abc", &pear));
        assert!(finds("created_at:>'2020-10-21T23:39:20Z'", &apple));
        assert!(finds("created_at:>2020-10-21", &apple));
        assert!(!finds("created_at:>2020-10-22", &apple));
        assert!(finds("created_at:<now", &pear));
        assert!(finds("created_at:<=2024", &pear));
        assert!(!finds("created_at:<2024", &pear));
        assert!(finds("created_at:2024-06-01", &pear));
        assert!(finds("id:1234", &pear));
        assert!(finds("id:>=1000 id:<=2000", &pear));
        assert!(finds("id:gid://shopify/Product/1234", &pear));
        assert!(finds("gift_card:false", &pear));
        assert!(!finds("gift_card:true", &pear));
        // Existence.
        assert!(finds("title:*", &pear));
        assert!(!finds("published_at:*", &pear));
        assert!(finds("-published_at:*", &pear));
        // Boolean operators.
        assert!(finds("pear OR apple", &pear));
        assert!(finds("pear OR apple Orchard", &pear));
        assert!(!finds("pear OR apple -Orchard", &pear));
        assert!(finds("NOT tag:sweet", &pear));
        assert!(!finds("-tag:sweet", &apple));
        assert!(finds("status:active AND (tag:sweet OR title:pear)", &pear));
    }

    #[test]
    fn ignores_fields_the_type_does_not_know() {
        let pear = product("Pear", &[], "Orchard", 12.0, "2024-06-01T00:00:00Z");
        // "If you specify an invalid field, then the query is ignored and all results are
        // returned."
        assert!(finds("name:nothing", &pear));
        assert!(finds("-name:nothing", &pear));
        assert!(finds("title:pear AND name:nothing", &pear));
        assert!(!finds("title:apple AND name:nothing", &pear));
        assert!(!finds("title:apple OR name:nothing", &pear));
        // The warnings of `extensions.search`.
        let query = Query::parse("name:blue jeans");
        assert_eq!(query.fields(), ["name"]);
        let warnings = query.invalid_fields(|field| field == "title");
        assert_eq!(
            warnings.iter().map(Warning::to_json).collect::<Vec<_>>(),
            [json!({"field": "name", "message": "Invalid search field for this query."})]
        );
        assert!(
            Query::parse("default:x title:y")
                .invalid_fields(|field| field == "title")
                .is_empty()
        );
    }

    #[test]
    fn knows_the_filters_each_connection_documents() {
        let products = documented_filters("QueryRoot", "products").unwrap();
        assert!(products.contains(&("created_at".to_string(), "time".to_string())));
        assert!(products.contains(&("inventory_total".to_string(), "integer".to_string())));
        assert!(is_documented_filter("QueryRoot", "products", "tag"));
        assert!(is_documented_filter(
            "QueryRoot",
            "products",
            "metafields.custom.on_sale"
        ));
        assert!(!is_documented_filter(
            "QueryRoot",
            "products",
            "metafields.custom"
        ));
        assert!(!is_documented_filter("QueryRoot", "products", "name"));
        assert!(is_documented_filter(
            "QueryRoot",
            "orders",
            "financial_status"
        ));
        assert!(documented_filters("Customer", "orders").is_some());
        assert_eq!(documented_filters("QueryRoot", "shop"), None);
        let query = Query::parse("name:blue jeans");
        assert_eq!(
            query.invalid_fields(|field| is_documented_filter("QueryRoot", "products", field))[0]
                .field,
            Some("name".to_string())
        );
    }

    #[test]
    fn shows_the_parsed_query() {
        assert_eq!(
            Query::parse("name:blue jeans").parsed(),
            json!({"and": [
                {"field": "name", "match_all": "blue"},
                {"field": "default", "match_all": "jeans"},
            ]})
        );
        assert_eq!(
            Query::parse("-tag:\"a b\" OR created_at:>=2024 OR sku:x* OR published_at:*").parsed(),
            json!({"or": [
                {"not": {"field": "tag", "match_phrase": "a b"}},
                {"field": "created_at", "range": {"gte": "2024"}},
                {"field": "sku", "match_prefix": "x"},
                {"field": "published_at", "exists": true},
            ]})
        );
        assert_eq!(Query::parse("").parsed(), json!({}));
    }

    #[test]
    fn reads_dates_in_the_shops_time_zone() {
        let late = product("Late", &[], "V", 0.0, "2024-01-15T23:30:00Z");
        let paris = Options {
            now: date("2026-10-31T12:00:00Z"),
            offset: FixedOffset::east_opt(3_600).unwrap(),
        };
        // 23:30 UTC is the 16th in Paris.
        assert!(Query::parse("created_at:2024-01-16").matches(&late, &paris));
        assert!(!Query::parse("created_at:2024-01-15").matches(&late, &paris));
        assert!(Query::parse("created_at:2024-01-15").matches(&late, &options()));
    }
}
