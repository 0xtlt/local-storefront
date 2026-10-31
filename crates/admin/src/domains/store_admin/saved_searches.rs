//! Saved searches: `savedSearchCreate`, `savedSearchUpdate`, `savedSearchDelete`; the
//! `…SavedSearches` queries (`productSavedSearches`, `urlRedirectSavedSearches`, …), each the
//! searches of its resource type.
//!
//! A `SavedSearch` keeps its `name`, `query` and `resourceType`; `searchTerms` and `filters`
//! are the query split as shopify.dev describes them: the free terms, and the `field:value`
//! terms (`{key, value}`). A name is unique within a resource type. Shopify publishes no
//! message for either check: the wording is Rails' (`Name can't be blank`,
//! `Name has already been taken`). The deprecated `CUSTOMER` searches (segments replace them)
//! are kept like the others.

use serde_json::{Map, Value, json};

use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::{Record, ShopState, value};

/// The query fields of saved searches, and the resource type each lists.
pub const QUERIES: &[(&str, &str)] = &[
    ("automaticDiscountSavedSearches", "PRICE_RULE"),
    ("codeDiscountSavedSearches", "PRICE_RULE"),
    ("collectionSavedSearches", "COLLECTION"),
    ("customerSavedSearches", "CUSTOMER"),
    ("discountRedeemCodeSavedSearches", "DISCOUNT_REDEEM_CODE"),
    ("draftOrderSavedSearches", "DRAFT_ORDER"),
    ("fileSavedSearches", "FILE"),
    ("orderSavedSearches", "ORDER"),
    ("productSavedSearches", "PRODUCT"),
    ("urlRedirectSavedSearches", "URL_REDIRECT"),
];

pub fn register(registry: &mut Registry) {
    registry
        .mutation("savedSearchCreate", create)
        .mutation("savedSearchUpdate", update)
        .mutation("savedSearchDelete", delete);
    for (field, _) in QUERIES {
        registry.query(field, saved_searches);
    }
}

fn text<'r>(record: &'r Record, field: &str) -> &'r str {
    record
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// The words of a query, quoted phrases kept whole.
fn words(query: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for character in query.chars() {
        match character {
            '"' => {
                quoted = !quoted;
                current.push(character);
            }
            ' ' | '\t' | '\n' if !quoted => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(character),
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// `searchTerms` and `filters` of a query: the free terms, and the `field:value` ones.
pub fn split(query: &str) -> (String, Vec<Value>) {
    let mut terms = Vec::new();
    let mut filters = Vec::new();
    for word in words(query) {
        match word.split_once(':') {
            Some((key, value))
                if !key.is_empty()
                    && !key.starts_with('"')
                    && key
                        .trim_start_matches('-')
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') =>
            {
                filters.push(json!({"key": key, "value": value.trim_matches('"')}));
            }
            _ => terms.push(word),
        }
    }
    (terms.join(" "), filters)
}

/// The saved search of `id`.
pub fn saved_search(state: &ShopState, id: &str) -> Option<Record> {
    (state.type_of(id) == Some("SavedSearch"))
        .then(|| state.get(id).cloned())
        .flatten()
}

fn taken(state: &ShopState, name: &str, resource_type: &str, except: Option<&str>) -> bool {
    state.keys("SavedSearch").any(|key| {
        Some(key) != except
            && state.get(key).is_some_and(|search| {
                text(search, "resourceType") == resource_type
                    && text(search, "name").eq_ignore_ascii_case(name)
            })
    })
}

fn input(ctx: &FieldContext<'_>) -> Map<String, Value> {
    ctx.arg("input")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn create(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = input(ctx);
    let get = |name: &str| {
        input
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let (name, query, resource_type) = (get("name"), get("query"), get("resourceType"));
    let mut errors = Vec::new();
    if name.is_empty() {
        errors.push(UserError::new(&["input", "name"], "Name can't be blank"));
    } else if name.chars().count() > 40 {
        errors.push(UserError::new(
            &["input", "name"],
            "Name is too long (maximum is 40 characters)",
        ));
    } else if taken(m.state(), &name, &resource_type, None) {
        errors.push(UserError::new(
            &["input", "name"],
            "Name has already been taken",
        ));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"savedSearch": null}), &errors));
    }
    let (terms, filters) = split(&query);
    let state = m.state_mut();
    let id = state.insert(value::object(json!({
        "__typename": "SavedSearch",
        "name": name,
        "query": query,
        "resourceType": resource_type,
        "searchTerms": terms,
        "filters": filters,
    })));
    state.emit("saved_searches/create", &id);
    Ok(payload(json!({"savedSearch": id}), &[]))
}

fn update(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = input(ctx);
    let id = input
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let Some(record) = saved_search(m.state(), &id) else {
        let error = UserError::new(&["input", "id"], "Saved Search does not exist");
        return Ok(payload(json!({"savedSearch": null}), &[error]));
    };
    let name = input
        .get("name")
        .and_then(Value::as_str)
        .map(|name| name.trim().to_string())
        .unwrap_or_else(|| text(&record, "name").to_string());
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .map(|query| query.trim().to_string())
        .unwrap_or_else(|| text(&record, "query").to_string());
    let mut errors = Vec::new();
    if name.is_empty() {
        errors.push(UserError::new(&["input", "name"], "Name can't be blank"));
    } else if taken(m.state(), &name, text(&record, "resourceType"), Some(&id)) {
        errors.push(UserError::new(
            &["input", "name"],
            "Name has already been taken",
        ));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"savedSearch": null}), &errors));
    }
    let (terms, filters) = split(&query);
    let state = m.state_mut();
    if let Some(search) = state.get_mut(&id) {
        search.insert("name".into(), json!(name));
        search.insert("query".into(), json!(query));
        search.insert("searchTerms".into(), json!(terms));
        search.insert("filters".into(), json!(filters));
    }
    state.emit("saved_searches/update", &id);
    Ok(payload(json!({"savedSearch": id}), &[]))
}

fn delete(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = input(ctx)
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if saved_search(m.state(), &id).is_none() {
        let error = UserError::new(&["input", "id"], "Saved Search does not exist");
        return Ok(payload(json!({"deletedSavedSearchId": null}), &[error]));
    }
    m.state_mut().delete(&id, "saved_searches/delete");
    Ok(payload(json!({"deletedSavedSearchId": id}), &[]))
}

/// A `…SavedSearches` query: the searches of its resource type.
fn saved_searches(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let resource_type = QUERIES
        .iter()
        .find(|(field, _)| *field == ctx.field_name())
        .map_or("", |(_, kind)| *kind);
    let items: Vec<Value> = r
        .records_of("SavedSearch")
        .into_iter()
        .filter(|item| {
            r.item_map(item)
                .is_some_and(|search| text(search, "resourceType") == resource_type)
        })
        .collect();
    r.connection(ctx, items)
}
