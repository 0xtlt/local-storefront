//! Catalogs: `catalogCreate`, `catalogUpdate`, `catalogDelete`, `catalogContextUpdate`; the
//! `catalogs(type:)`, `catalogsCount` and `catalogOperations` queries, and what reads the
//! catalogs of a context (`CompanyLocation.catalogs`, `catalogsCount`, `inCatalog`, `market`,
//! `MarketCatalog.markets(status:, type:)`).
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - A catalog has a context, markets (`MarketCatalog`) or company locations
//!   (`CompanyLocationCatalog`), "exactly one context type"
//!   (`MUST_PROVIDE_EXACTLY_ONE_CONTEXT_TYPE`); a price list and a publication, both optional
//!   ("When a publication isn't associated with a catalog, product availability is determined
//!   by the sales channel"); a title and a status (`ACTIVE`, `DRAFT`, `ARCHIVED`).
//! - `catalogCreate`'s examples answer `gid://shopify/MarketCatalog/<n>`; `marketCreate`'s takes
//!   `gid://shopify/Catalog/<n>` for the same catalog: the catalogs share one id counter, and
//!   `Catalog/<n>` names any of them.
//! - A price list and a publication belong to one catalog: assigning one sets its `catalog`.
//! - `catalogDelete(deleteDependentResources: true)` deletes the price list and the publication
//!   with the catalog; without it, they stay without a catalog.
//! - The catalogs of apps (the Online Store's, a channel's) are the platform's: they cannot be
//!   created, changed or deleted here (`CANNOT_CREATE_APP_CATALOG`, `CANNOT_MODIFY_APP_CATALOG`,
//!   `CANNOT_DELETE_APP_CATALOG`).
//!
//! Shopify has no webhook topic for catalogs: their changes are `catalogs/create`,
//! `catalogs/update` and `catalogs/delete` events, which no subscription can name. The messages
//! are lsf's where shopify.dev publishes none (each says so).

use serde_json::{Map, Value, json};

use super::{
    CATALOG_TYPES, catalog_key, catalog_type, coded, is, market_currency, records_input, strings,
    text,
};
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::sort::SortValue;
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::search::SearchValue;
use crate::state::{ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("catalogCreate", create)
        .mutation("catalogUpdate", update)
        .mutation("catalogDelete", delete)
        .mutation("catalogContextUpdate", context_update)
        .query("catalogs", catalogs)
        .query("catalogsCount", catalogs_count)
        .query("catalogOperations", |_r, _ctx, _object| {
            Ok(Output::List(Vec::new()))
        })
        .field("MarketCatalog", "markets", catalog_markets)
        .field("MarketCatalog", "marketsCount", catalog_markets_count)
        .field("CompanyLocation", "catalogs", location_catalogs)
        .field("CompanyLocation", "catalogsCount", location_catalogs_count)
        .field("CompanyLocation", "inCatalog", location_in_catalog)
        .field("CompanyLocation", "market", location_market)
        .sort_key("Catalog", "TYPE", |r, item| {
            SortValue::of(
                r.item_map(item)
                    .and_then(|catalog| catalog.get("__typename")),
            )
        })
        .search_field("Catalog", "market_id", |_r, _item, catalog| {
            Some(
                strings(catalog.get("markets"))
                    .iter()
                    .filter_map(|market| crate::state::parse_gid(market))
                    .map(|(_, id)| SearchValue::Keyword(id.to_string()))
                    .collect(),
            )
        })
        .search_field("Catalog", "company_id", |r, _item, catalog| {
            Some(
                strings(catalog.get("companyLocations"))
                    .iter()
                    .filter_map(|location| r.state().get(location)?.get("company")?.as_str())
                    .filter_map(SearchValue::id)
                    .collect(),
            )
        });
}

/// The catalogs of a `CatalogType` (every one without), in creation order.
fn of_type(state: &ShopState, wanted: Option<&str>) -> Vec<Value> {
    state
        .iter()
        .filter(|(key, _)| {
            state.type_of(key).is_some_and(|type_name| {
                CATALOG_TYPES.contains(&type_name)
                    && wanted.is_none_or(|wanted| catalog_type(type_name) == wanted)
            })
        })
        .map(|(key, _)| json!(key))
        .collect()
}

fn catalogs(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    r.connection(ctx, of_type(r.state(), ctx.arg_str("type")))
}

fn catalogs_count(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let items = r.search(ctx, "Catalog", of_type(r.state(), ctx.arg_str("type")));
    Ok(r.count(ctx, items.len()))
}

/// The markets of a market catalog, with `status:` and `type:` applied.
fn markets_of(r: &ShopResolver<'_>, ctx: &FieldContext<'_>, object: &Obj) -> Vec<Value> {
    let state = r.state();
    let status = ctx.arg_str("status");
    let kind = ctx.arg_str("type");
    r.items_of(object, "markets")
        .into_iter()
        .filter(|market| {
            market
                .as_str()
                .and_then(|market| state.get(market))
                .is_some_and(|record| {
                    status.is_none_or(|status| record.get("status") == Some(&json!(status)))
                        && kind.is_none_or(|kind| record.get("type") == Some(&json!(kind)))
                })
        })
        .collect()
}

fn catalog_markets(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    r.connection(ctx, markets_of(r, ctx, object))
}

fn catalog_markets_count(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let items = r.search(ctx, "Market", markets_of(r, ctx, object));
    Ok(r.count(ctx, items.len()))
}

/// The catalogs of a company location: those whose context has it.
pub fn of_location(state: &ShopState, location: &str) -> Vec<Value> {
    state
        .keys("CompanyLocationCatalog")
        .filter(|catalog| {
            state.get(catalog).is_some_and(|record| {
                strings(record.get("companyLocations"))
                    .iter()
                    .any(|known| known == location)
            })
        })
        .map(|catalog| json!(catalog))
        .collect()
}

fn location_catalogs(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let location = object.key().unwrap_or_default();
    r.connection(ctx, of_location(r.state(), location))
}

fn location_catalogs_count(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let location = object.key().unwrap_or_default();
    Ok(r.count(ctx, of_location(r.state(), location).len()))
}

fn location_in_catalog(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let location = object.key().unwrap_or_default();
    let catalog = ctx
        .arg_str("catalogId")
        .and_then(|id| catalog_key(state, id));
    let inside = catalog.is_some_and(|catalog| {
        of_location(state, location)
            .iter()
            .any(|known| known.as_str() == Some(catalog.as_str()))
    });
    Ok(Output::value(inside))
}

/// The market of a company location: the active company location market that has it (or all
/// of them), else the region market of its shipping address's country.
pub fn market_of_location(state: &ShopState, location: &str) -> Option<String> {
    let company = state.keys("Market").find(|market| {
        state.get(market).is_some_and(|record| {
            super::is_active(record)
                && record
                    .get("conditions")
                    .and_then(|conditions| conditions.get("companyLocationsCondition"))
                    .is_some_and(|condition| {
                        condition.get("applicationLevel") == Some(&json!("ALL"))
                            || strings(condition.get("companyLocations"))
                                .iter()
                                .any(|known| known == location)
                    })
        })
    });
    if let Some(market) = company {
        return Some(market.to_string());
    }
    let country = state
        .get(location)
        .and_then(|record| record.get("shippingAddress"))
        .and_then(|address| {
            address
                .get("countryCode")
                .or_else(|| address.get("countryCodeV2"))
        })
        .and_then(Value::as_str)?;
    super::market_of_country(state, country)
}

fn location_market(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let location = object.key().unwrap_or_default();
    match market_of_location(r.state(), location) {
        Some(market) => Ok(Output::Object(Obj::record(&market))),
        None => r.generic(ctx, object),
    }
}

// --- Mutations ---

fn not_found(field: &[&str]) -> UserError {
    // The code's description in the schema.
    coded(field, "Catalog not found.", "CATALOG_NOT_FOUND")
}

/// The catalog of an argument.
fn catalog_arg(state: &ShopState, ctx: &FieldContext<'_>, name: &str) -> Option<String> {
    catalog_key(state, ctx.arg_str(name)?)
}

/// The context of a `CatalogContextInput`, checked: `(type, markets, company locations)`.
fn context_of(
    state: &ShopState,
    context: &Map<String, Value>,
    at: &[&str],
    errors: &mut Vec<UserError>,
) -> (Option<&'static str>, Vec<String>, Vec<String>) {
    let markets = context
        .get("marketIds")
        .filter(|value| !value.is_null())
        .map(|value| strings(Some(value)));
    let locations = context
        .get("companyLocationIds")
        .filter(|value| !value.is_null())
        .map(|value| strings(Some(value)));
    let kind = match (&markets, &locations) {
        (Some(_), None) => Some("MarketCatalog"),
        (None, Some(_)) => Some("CompanyLocationCatalog"),
        _ => {
            // lsf's wording.
            errors.push(coded(
                at,
                "Must provide exactly one context type",
                "MUST_PROVIDE_EXACTLY_ONE_CONTEXT_TYPE",
            ));
            None
        }
    };
    let markets = records_input(
        state,
        markets.as_deref().unwrap_or_default(),
        "Market",
        &[at, &["marketIds"]].concat(),
        "Market not found.",
        "MARKET_NOT_FOUND",
        errors,
    );
    let locations = records_input(
        state,
        locations.as_deref().unwrap_or_default(),
        "CompanyLocation",
        &[at, &["companyLocationIds"]].concat(),
        "Company location not found.",
        "COMPANY_LOCATION_NOT_FOUND",
        errors,
    );
    (kind, markets, locations)
}

/// Checks a price list and a publication for a catalog (`catalog`: the one being updated).
fn check_links(
    state: &ShopState,
    input: &Map<String, Value>,
    catalog: Option<&str>,
    markets: &[String],
    errors: &mut Vec<UserError>,
) -> (Option<String>, Option<String>) {
    let mut list = None;
    if let Some(id) = text(input, "priceListId") {
        if !is(state, id, "PriceList") {
            // The code's description in the schema.
            errors.push(coded(
                &["input", "priceListId"],
                "Price list not found.",
                "PRICE_LIST_NOT_FOUND",
            ));
        } else if state
            .get(id)
            .and_then(|record| record.get("catalog"))
            .and_then(Value::as_str)
            .is_some_and(|other| Some(other) != catalog && state.contains(other))
        {
            // lsf's wording.
            errors.push(coded(
                &["input", "priceListId"],
                "Price list is already assigned to another catalog",
                "TAKEN",
            ));
        } else {
            let currency = state
                .get(id)
                .and_then(|record| record.get("currency"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            if markets
                .iter()
                .any(|market| market_currency(state, market) != currency)
            {
                // The code's description in the schema.
                errors.push(coded(
                    &["input", "priceListId"],
                    "The market and price list currency must match.",
                    "MARKET_AND_PRICE_LIST_CURRENCY_MISMATCH",
                ));
            }
            list = Some(id.to_string());
        }
    }
    let mut publication = None;
    if let Some(id) = text(input, "publicationId") {
        if !is(state, id, "Publication") {
            // The code's description in the schema.
            errors.push(coded(
                &["input", "publicationId"],
                "Publication not found.",
                "PUBLICATION_NOT_FOUND",
            ));
        } else if state
            .get(id)
            .and_then(|record| record.get("catalog"))
            .and_then(Value::as_str)
            .is_some_and(|other| Some(other) != catalog && state.contains(other))
        {
            // lsf's wording.
            errors.push(coded(
                &["input", "publicationId"],
                "Publication is already assigned to another catalog",
                "TAKEN",
            ));
        } else {
            publication = Some(id.to_string());
        }
    }
    (list, publication)
}

/// Whether a title is another catalog's.
fn title_taken(state: &ShopState, title: &str, catalog: Option<&str>) -> bool {
    of_type(state, None)
        .iter()
        .filter_map(Value::as_str)
        .any(|other| {
            Some(other) != catalog
                && state
                    .get(other)
                    .and_then(|record| record.get("title"))
                    .and_then(Value::as_str)
                    .is_some_and(|known| known.eq_ignore_ascii_case(title))
        })
}

/// Points a price list or a publication at a catalog (or at none).
fn set_owner(state: &mut ShopState, record: Option<&str>, catalog: Option<&str>) {
    if let Some(record) = record.and_then(|record| state.get_mut(record)) {
        record.insert("catalog".into(), json!(catalog));
    }
}

/// `catalogCreate`.
fn create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("input");
    let state = mutation.state();
    let mut errors = Vec::new();
    let title = text(&input, "title").unwrap_or_default().trim().to_string();
    if title.is_empty() {
        errors.push(coded(&["input", "title"], "Title can't be blank", "BLANK"));
    } else if title_taken(state, &title, None) {
        errors.push(coded(
            &["input", "title"],
            "Title has already been taken",
            "TAKEN",
        ));
    }
    let context = input
        .get("context")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let (kind, markets, locations) =
        context_of(state, &context, &["input", "context"], &mut errors);
    let (list, publication) = check_links(state, &input, None, &markets, &mut errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"catalog": null}), &errors));
    }
    let kind = kind.unwrap_or("MarketCatalog");
    let status = text(&input, "status").unwrap_or("ACTIVE");
    let state = mutation.state_mut();
    let catalog = state.new_gid(kind);
    let mut record = value::object(json!({
        "__typename": kind,
        "id": catalog,
        "title": title,
        "status": status,
        "priceList": list,
        "publication": publication,
        "operations": [],
    }));
    if kind == "MarketCatalog" {
        record.insert("markets".into(), json!(markets));
    } else {
        record.insert("companyLocations".into(), json!(locations));
    }
    state.insert(record);
    set_owner(state, list.as_deref(), Some(&catalog));
    set_owner(state, publication.as_deref(), Some(&catalog));
    state.emit("catalogs/create", &catalog);
    Ok(payload(json!({"catalog": catalog}), &[]))
}

/// `catalogUpdate`.
fn update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("input");
    let state = mutation.state();
    let Some(catalog) = catalog_arg(state, ctx, "id") else {
        return Ok(payload(json!({"catalog": null}), &[not_found(&["id"])]));
    };
    let type_name = state.type_of(&catalog).unwrap_or_default().to_string();
    if type_name == "AppCatalog" {
        // The code's description in the schema.
        let error = coded(
            &["id"],
            "Cannot modify app catalogs.",
            "CANNOT_MODIFY_APP_CATALOG",
        );
        return Ok(payload(json!({"catalog": null}), &[error]));
    }
    let mut errors = Vec::new();
    let title = text(&input, "title").map(|title| title.trim().to_string());
    if let Some(title) = &title {
        if title.is_empty() {
            errors.push(coded(&["input", "title"], "Title can't be blank", "BLANK"));
        } else if title_taken(state, title, Some(&catalog)) {
            errors.push(coded(
                &["input", "title"],
                "Title has already been taken",
                "TAKEN",
            ));
        }
    }
    let mut context = None;
    if let Some(Value::Object(given)) = input.get("context") {
        let (kind, markets, locations) =
            context_of(state, given, &["input", "context"], &mut errors);
        if kind.is_some_and(|kind| kind != type_name) {
            // lsf's wording.
            errors.push(coded(
                &["input", "context"],
                "The catalog's context cannot change type",
                "CONTEXT_DRIVER_MISMATCH",
            ));
        }
        context = Some((markets, locations));
    }
    let markets = match &context {
        Some((markets, _)) if type_name == "MarketCatalog" => markets.clone(),
        _ => strings(state.get(&catalog).and_then(|record| record.get("markets"))),
    };
    let (list, publication) = check_links(state, &input, Some(&catalog), &markets, &mut errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"catalog": null}), &errors));
    }
    let previous = state.get(&catalog).cloned().unwrap_or_default();
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&catalog) {
        if let Some(title) = title {
            record.insert("title".into(), json!(title));
        }
        if let Some(status) = text(&input, "status") {
            record.insert("status".into(), json!(status));
        }
        if let Some((markets, locations)) = context {
            if type_name == "MarketCatalog" {
                record.insert("markets".into(), json!(markets));
            } else {
                record.insert("companyLocations".into(), json!(locations));
            }
        }
        if list.is_some() {
            record.insert("priceList".into(), json!(list));
        }
        if publication.is_some() {
            record.insert("publication".into(), json!(publication));
        }
    }
    if let Some(list) = &list {
        let old = previous.get("priceList").and_then(Value::as_str);
        if old != Some(list.as_str()) {
            set_owner(state, old, None);
        }
        set_owner(state, Some(list), Some(&catalog));
    }
    if let Some(publication) = &publication {
        let old = previous.get("publication").and_then(Value::as_str);
        if old != Some(publication.as_str()) {
            set_owner(state, old, None);
        }
        set_owner(state, Some(publication), Some(&catalog));
    }
    state.emit("catalogs/update", &catalog);
    Ok(payload(json!({"catalog": catalog}), &[]))
}

/// `catalogDelete`.
fn delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let state = mutation.state();
    let Some(catalog) = catalog_arg(state, ctx, "id") else {
        return Ok(payload(json!({"deletedId": null}), &[not_found(&["id"])]));
    };
    if is(state, &catalog, "AppCatalog") {
        // The code's description in the schema.
        let error = coded(
            &["id"],
            "Cannot delete app catalogs.",
            "CANNOT_DELETE_APP_CATALOG",
        );
        return Ok(payload(json!({"deletedId": null}), &[error]));
    }
    let record = state.get(&catalog).cloned().unwrap_or_default();
    let dependents = ctx.arg_bool("deleteDependentResources") == Some(true);
    let list = record
        .get("priceList")
        .and_then(Value::as_str)
        .map(str::to_string);
    let publication = record
        .get("publication")
        .and_then(Value::as_str)
        .map(str::to_string);
    if dependents {
        if let Some(list) = &list {
            super::price_lists::delete_price_list(mutation.state_mut(), list);
        }
        if let Some(publication) = &publication {
            super::publications::delete_publication(mutation.state_mut(), publication);
        }
    }
    let state = mutation.state_mut();
    set_owner(state, list.as_deref(), None);
    set_owner(state, publication.as_deref(), None);
    state.delete(&catalog, "catalogs/delete");
    Ok(payload(json!({"deletedId": catalog}), &[]))
}

/// `catalogContextUpdate`.
fn context_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let Some(catalog) = catalog_arg(state, ctx, "catalogId") else {
        return Ok(payload(
            json!({"catalog": null}),
            &[not_found(&["catalogId"])],
        ));
    };
    let type_name = state.type_of(&catalog).unwrap_or_default().to_string();
    if type_name == "AppCatalog" {
        let error = coded(
            &["catalogId"],
            "Cannot modify app catalogs.",
            "CANNOT_MODIFY_APP_CATALOG",
        );
        return Ok(payload(json!({"catalog": null}), &[error]));
    }
    let add = ctx.arg_object("contextsToAdd").cloned();
    let remove = ctx.arg_object("contextsToRemove").cloned();
    if add.is_none() && remove.is_none() {
        // The code's description in the schema.
        let error = coded(
            &[],
            "Must provide contexts to add or remove.",
            "REQUIRES_CONTEXTS_TO_ADD_OR_REMOVE",
        );
        return Ok(payload(json!({"catalog": null}), &[error]));
    }
    let mut errors = Vec::new();
    let mut changes = Vec::new();
    for (name, given) in [("contextsToAdd", &add), ("contextsToRemove", &remove)] {
        let Some(given) = given else {
            continue;
        };
        let (kind, markets, locations) = context_of(state, given, &[name], &mut errors);
        if kind.is_some_and(|kind| kind != type_name) {
            // The code's description in the schema.
            errors.push(coded(
                &[name],
                "The catalog context type is invalid.",
                "INVALID_CATALOG_CONTEXT_TYPE",
            ));
        }
        changes.push((name, markets, locations));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"catalog": null}), &errors));
    }
    let (list, field) = if type_name == "MarketCatalog" {
        ("markets", 1)
    } else {
        ("companyLocations", 2)
    };
    let state = mutation.state_mut();
    if let Some(Value::Array(items)) = state
        .get_mut(&catalog)
        .and_then(|record| record.get_mut(list))
    {
        for (name, markets, locations) in changes {
            let ids = if field == 1 { markets } else { locations };
            for id in ids {
                items.retain(|known| known.as_str() != Some(id.as_str()));
                if name == "contextsToAdd" {
                    items.push(json!(id));
                }
            }
        }
    }
    state.emit("catalogs/update", &catalog);
    Ok(payload(json!({"catalog": catalog}), &[]))
}
