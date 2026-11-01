//! Saved searches: `savedSearchCreate`, `savedSearchUpdate`, `savedSearchDelete`, the
//! `…SavedSearches` queries of each resource, the `savedSearchId` the connections take, and
//! `segmentMigrations`.
//!
//! As shopify.dev's reference of 2026-10 shows them:
//!
//! - A `SavedSearch` has a `name`, a `query` (Shopify's search syntax), a `resourceType`, and
//!   what the query says split in two: `searchTerms` (the terms without a field) and `filters`
//!   (`{key, value}` of each `field:value`). The `productSavedSearches` example: `query: "foo"`
//!   is `searchTerms: "foo"` and no filters, `status:"DRAFT"` is the filter `{status, DRAFT}` and
//!   `searchTerms: ""`.
//! - Validation: a blank name ("Name can't be blank"), a name another saved search of the
//!   resource has ("Name has already been taken"), a query that is not the search syntax (the
//!   parser's message): Rails' wording, lsf's choice. An id that names nothing: "Saved search
//!   does not exist" (lsf's words).
//! - `customer_groups/create`, `…/update`, `…/delete` ("Occurs whenever a customer saved search
//!   is created", the topics' descriptions) for the saved searches of customers.
//! - A connection given `savedSearchId` lists what the saved search's query keeps, and its own
//!   `query` on top (`resolve::search`).
//! - `segmentMigrations`: Shopify moved customer saved searches to segments; a saved search lsf
//!   holds was never migrated, so its migration has no segment (`segmentId: null`).
//!
//! Saved searches are not store data: they do not sync.

use serde_json::{Value, json};

use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::search::{Node, Query};
use crate::state::ShopState;

/// The root fields that list the saved searches of a resource, and its `SearchResultType`.
const ROOT_FIELDS: &[(&str, &str)] = &[
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
        .mutation("savedSearchCreate", saved_search_create)
        .mutation("savedSearchUpdate", saved_search_update)
        .mutation("savedSearchDelete", saved_search_delete)
        .query("segmentMigrations", segment_migrations)
        .default_text("SavedSearch", |_r, search| {
            search
                .get("name")
                .and_then(Value::as_str)
                .map(|name| vec![name.to_string()])
                .unwrap_or_default()
        });
    for (field, _) in ROOT_FIELDS {
        registry.query(field, saved_searches);
    }
}

/// The saved searches a root field lists.
fn saved_searches(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let wanted = ROOT_FIELDS
        .iter()
        .find(|(field, _)| *field == ctx.field_name())
        .map(|(_, kind)| *kind)
        .unwrap_or_default();
    let items = r
        .state()
        .all("SavedSearch")
        .filter(|search| search.get("resourceType").and_then(Value::as_str) == Some(wanted))
        .filter_map(|search| search.get("id").cloned())
        .collect();
    r.connection(ctx, items)
}

/// The terms of a query, as `searchTerms` and `filters` split them.
fn split(query: &str) -> Result<(String, Vec<Value>), String> {
    let parsed = Query::parse_strict(query).map_err(|error| error.to_string())?;
    let mut terms = Vec::new();
    let mut filters = Vec::new();
    fn walk(node: &Node, terms: &mut Vec<String>, filters: &mut Vec<Value>) {
        match node {
            Node::Term(term) => match &term.field {
                Some(field) => filters.push(json!({"key": field, "value": term.value.text()})),
                None => terms.push(term.value.text().to_string()),
            },
            Node::And(parts) | Node::Or(parts) => {
                for part in parts {
                    walk(part, terms, filters);
                }
            }
            Node::Not(inner) => walk(inner, terms, filters),
        }
    }
    if let Some(root) = &parsed.root {
        walk(root, &mut terms, &mut filters);
    }
    Ok((terms.join(" "), filters))
}

/// The resource type of the saved search `id`.
fn kind_of(state: &ShopState, id: &str) -> Option<String> {
    state
        .get_of(id, "SavedSearch")
        .and_then(|search| search.get("resourceType").and_then(Value::as_str))
        .map(str::to_string)
}

/// The errors of a saved search's name: blank, or another's of the same resource type
/// (`except` is the one renamed).
fn check_name(
    state: &ShopState,
    name: &str,
    kind: &str,
    except: Option<&str>,
    errors: &mut Vec<UserError>,
) {
    let taken = || {
        state.all("SavedSearch").any(|search| {
            search.get("name").and_then(Value::as_str) == Some(name)
                && search.get("resourceType").and_then(Value::as_str) == Some(kind)
                && search.get("id").and_then(Value::as_str) != except
        })
    };
    if name.is_empty() {
        errors.push(UserError::new(&["input", "name"], "Name can't be blank"));
    } else if taken() {
        errors.push(UserError::new(
            &["input", "name"],
            "Name has already been taken",
        ));
    }
}

/// [`split`], its error in `errors`.
fn split_query(query: &str, errors: &mut Vec<UserError>) -> Option<(String, Vec<Value>)> {
    split(query)
        .map_err(|problem| errors.push(UserError::new(&["input", "query"], problem)))
        .ok()
}

/// The topic of a change of a saved search: the customer groups' for customers, none (a topic
/// no subscription can name) for the others.
fn topic(kind: &str, action: &str) -> String {
    if kind == "CUSTOMER" {
        format!("customer_groups/{action}")
    } else {
        format!("saved_searches/{action}")
    }
}

fn saved_search_create(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("input");
    let text = |field: &str| {
        input
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let (name, query, kind) = (
        text("name").trim().to_string(),
        text("query"),
        text("resourceType"),
    );
    let mut errors = Vec::new();
    check_name(m.state(), &name, &kind, None, &mut errors);
    let split = split_query(&query, &mut errors);
    let (Some((terms, filters)), true) = (split, errors.is_empty()) else {
        return Ok(payload(json!({"savedSearch": null}), &errors));
    };
    let now = m.timestamp();
    let state = m.state_mut();
    let id = state.new_gid("SavedSearch");
    state.insert_as(
        id.clone(),
        crate::state::value::object(json!({
            "__typename": "SavedSearch",
            "id": id,
            "name": name,
            "query": query,
            "resourceType": kind,
            "searchTerms": terms,
            "filters": filters,
            // The dates of `customer_groups/*`, which GraphQL's `SavedSearch` does not have.
            "lsfCreatedAt": now,
            "lsfUpdatedAt": now,
        })),
    );
    state.emit(&topic(&kind, "create"), &id);
    Ok(payload(json!({"savedSearch": id}), &[]))
}

fn saved_search_update(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("input");
    let id = input
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let Some(kind) = kind_of(m.state(), &id) else {
        let error = UserError::new(&["input", "id"], "Saved search does not exist");
        return Ok(payload(json!({"savedSearch": null}), &[error]));
    };
    let name = input
        .get("name")
        .and_then(Value::as_str)
        .map(|name| name.trim().to_string());
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut errors = Vec::new();
    if let Some(name) = &name {
        check_name(m.state(), name, &kind, Some(&id), &mut errors);
    }
    let split = query
        .as_deref()
        .and_then(|query| split_query(query, &mut errors));
    if !errors.is_empty() {
        return Ok(payload(json!({"savedSearch": null}), &errors));
    }
    let now = m.timestamp();
    let state = m.state_mut();
    if let Some(record) = state.get_mut(&id) {
        if let Some(name) = name {
            record.insert("name".into(), json!(name));
        }
        if let (Some(query), Some((terms, filters))) = (query, split) {
            record.insert("query".into(), json!(query));
            record.insert("searchTerms".into(), json!(terms));
            record.insert("filters".into(), json!(filters));
        }
        record.insert("lsfUpdatedAt".into(), json!(now));
    }
    state.emit(&topic(&kind, "update"), &id);
    Ok(payload(json!({"savedSearch": id}), &[]))
}

fn saved_search_delete(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("input");
    let id = input
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let Some(kind) = kind_of(m.state(), &id) else {
        let error = UserError::new(&["input", "id"], "Saved search does not exist");
        return Ok(payload(json!({"deletedSavedSearchId": null}), &[error]));
    };
    m.state_mut().delete(&id, &topic(&kind, "delete"));
    Ok(payload(json!({"deletedSavedSearchId": id}), &[]))
}

/// The query of the saved search a connection's `savedSearchId` names, if it names one.
pub fn query_of(state: &ShopState, id: &str) -> Option<String> {
    state
        .get_of(id, "SavedSearch")
        .and_then(|search| search.get("query").and_then(Value::as_str))
        .map(str::to_string)
}

/// The query of the saved search `id` names, when it is one of `resource_type` (its
/// `SearchResultType`: `URL_REDIRECT`, `DRAFT_ORDER`, …): what a bulk mutation's
/// `savedSearchId` runs.
pub fn query_for(state: &ShopState, id: &str, resource_type: &str) -> Option<String> {
    state
        .get_of(id, "SavedSearch")
        .filter(|search| search.get("resourceType").and_then(Value::as_str) == Some(resource_type))
        .map(|search| {
            search
                .get("query")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
}

/// `segmentMigrations(savedSearchId:)`: one per customer saved search, none migrated.
fn segment_migrations(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let wanted = ctx.arg_str("savedSearchId");
    let items = r
        .state()
        .all("SavedSearch")
        .filter(|search| search.get("resourceType").and_then(Value::as_str) == Some("CUSTOMER"))
        .filter_map(|search| search.get("id").and_then(Value::as_str))
        .filter(|id| wanted.is_none_or(|wanted| wanted == *id))
        .filter_map(|id| {
            let (_, number) = crate::state::parse_gid(id)?;
            Some(json!({
                "id": format!("gid://shopify/SegmentMigration/{number}"),
                "savedSearchId": id,
                "segmentId": null,
            }))
        })
        .collect();
    r.connection(ctx, items)
}
