//! Publications: `publicationCreate`, `publicationUpdate`, `publicationDelete`, and the
//! `publications(catalogType:)`, `publicationsCount` and `Shop.publicationCount` queries.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - A publication "controls which products and collections customers can access through a
//!   catalog": `publicationCreate` makes one for a catalog (`catalogId`), empty or with every
//!   product (`defaultState: ALL_PRODUCTS`), which adds the products created later when
//!   `autoPublish` is on.
//! - `publicationUpdate` adds and removes products and collections, "a maximum of 50 items per
//!   operation" (`PUBLICATION_UPDATE_LIMIT_EXCEEDED`), and sets `autoPublish`.
//! - The publications of app catalogs (the Online Store's, a channel's) are the platform's: they
//!   are changed by publishing (`publishablePublish`), not here
//!   (`CANNOT_MODIFY_APP_CATALOG_PUBLICATION`).
//! - Publishing and unpublishing go through the catalog family's path (`catalog::publish_on`):
//!   `product_publications/create` and `/delete` (`collection_publications/…`), then
//!   `products/update` (`collections/update`). A publication created or deleted is the
//!   `publications/create` or `publications/delete` event, which no subscription can name.
//!
//! The messages are lsf's where shopify.dev publishes none (each says so).

use serde_json::{Value, json};

use super::{catalog_type, is, strings, text};
use crate::domains::catalog;
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, coded, payload};
use crate::state::{ShopState, value};

/// The most publishables `publicationUpdate` takes at once.
const UPDATE_LIMIT: usize = 50;

pub fn register(registry: &mut Registry) {
    registry
        .mutation("publicationCreate", create)
        .mutation("publicationUpdate", update)
        .mutation("publicationDelete", delete)
        .query("publications", publications)
        .query("publicationsCount", publications_count)
        .field("Shop", "publicationCount", |r, _ctx, _object| {
            Ok(Output::value(r.state().count("Publication")))
        });
}

/// The `CatalogType` of a publication's catalog (`NONE` without one).
fn catalog_type_of(state: &ShopState, publication: &str) -> &'static str {
    let catalog = state
        .get(publication)
        .and_then(|record| record.get("catalog"))
        .and_then(Value::as_str);
    catalog
        .and_then(|catalog| state.type_of(catalog))
        .map_or("NONE", catalog_type)
}

fn of_type(state: &ShopState, wanted: Option<&str>) -> Vec<Value> {
    state
        .keys("Publication")
        .filter(|publication| {
            wanted.is_none_or(|wanted| catalog_type_of(state, publication) == wanted)
        })
        .map(|publication| json!(publication))
        .collect()
}

fn publications(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    r.connection(ctx, of_type(r.state(), ctx.arg_str("catalogType")))
}

fn publications_count(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    Ok(r.count(ctx, of_type(r.state(), ctx.arg_str("catalogType")).len()))
}

fn not_found(field: &str) -> UserError {
    // The code's description in the schema.
    coded(&[field], "Publication not found.", "PUBLICATION_NOT_FOUND")
}

/// Whether a publication is an app catalog's (the Online Store's, a channel's).
fn is_app_publication(state: &ShopState, publication: &str) -> bool {
    catalog_type_of(state, publication) == "APP"
}

/// `publicationCreate`.
fn create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("input");
    let state = mutation.state();
    let mut errors = Vec::new();
    let catalog = match text(&input, "catalogId") {
        None => None,
        Some(id) => match super::catalog_key(state, id) {
            None => {
                // The code's description in the schema.
                errors.push(coded(
                    &["input", "catalogId"],
                    "Catalog not found.",
                    "CATALOG_NOT_FOUND",
                ));
                None
            }
            Some(catalog) if is(state, &catalog, "AppCatalog") => {
                errors.push(coded(
                    &["input", "catalogId"],
                    "Cannot modify app catalogs.",
                    "CANNOT_MODIFY_APP_CATALOG",
                ));
                None
            }
            Some(catalog)
                if state
                    .get(&catalog)
                    .and_then(|record| record.get("publication"))
                    .and_then(Value::as_str)
                    .is_some_and(|known| state.contains(known)) =>
            {
                // lsf's wording.
                errors.push(coded(
                    &["input", "catalogId"],
                    "The catalog already has a publication",
                    "TAKEN",
                ));
                None
            }
            Some(catalog) => Some(catalog),
        },
    };
    if !errors.is_empty() {
        return Ok(payload(json!({"publication": null}), &errors));
    }
    let name = catalog
        .as_deref()
        .and_then(|catalog| state.get(catalog)?.get("title")?.as_str())
        .unwrap_or("Publication")
        .to_string();
    let all = text(&input, "defaultState") == Some("ALL_PRODUCTS");
    let products: Vec<String> = if all {
        state.keys("Product").map(str::to_string).collect()
    } else {
        Vec::new()
    };
    let state = mutation.state_mut();
    let publication = state.new_gid("Publication");
    state.insert(value::object(json!({
        "__typename": "Publication",
        "id": publication,
        "name": name,
        "autoPublish": input.get("autoPublish").and_then(Value::as_bool).unwrap_or(false),
        "catalog": catalog,
        "supportsFuturePublishing": false,
        "operation": null,
        "channels": [],
        "products": [],
        "collections": [],
    })));
    if let Some(catalog) = &catalog
        && let Some(record) = state.get_mut(catalog)
    {
        record.insert("publication".into(), json!(publication));
    }
    state.emit("publications/create", &publication);
    let targets = [(publication.clone(), None)];
    for product in products {
        catalog::publish_on(state, &product, &targets, true);
    }
    Ok(payload(json!({"publication": publication}), &[]))
}

/// The publishables of a list of an input: products and collections, or the errors at `at`.
fn publishables(
    state: &ShopState,
    ids: &[String],
    at: &str,
    errors: &mut Vec<UserError>,
) -> Vec<String> {
    let mut found = Vec::new();
    for (index, id) in ids.iter().enumerate() {
        let index = index.to_string();
        match state.type_of(id) {
            Some("Product" | "Collection") => found.push(id.clone()),
            Some(_) => errors.push(coded(
                &["input", at, &index],
                // The code's description in the schema.
                "Publishable type not supported.",
                "UNSUPPORTED_PUBLISHABLE_TYPE",
            )),
            None => errors.push(coded(
                &["input", at, &index],
                "Publishable ID not found.",
                "INVALID_PUBLISHABLE_ID",
            )),
        }
    }
    found
}

/// `publicationUpdate`.
fn update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = ctx.arg_object_or_default("input");
    let state = mutation.state();
    let Some(publication) = ctx
        .arg_str("id")
        .filter(|id| is(state, id, "Publication"))
        .map(str::to_string)
    else {
        return Ok(payload(json!({"publication": null}), &[not_found("id")]));
    };
    if is_app_publication(state, &publication) {
        // The code's description in the schema.
        let error = coded(
            &["id"],
            "Cannot modify publications of app catalogs.",
            "CANNOT_MODIFY_APP_CATALOG_PUBLICATION",
        );
        return Ok(payload(json!({"publication": null}), &[error]));
    }
    let to_add = strings(input.get("publishablesToAdd"));
    let to_remove = strings(input.get("publishablesToRemove"));
    let mut errors = Vec::new();
    if to_add.len() + to_remove.len() > UPDATE_LIMIT {
        // lsf's wording of the limit the reference states.
        errors.push(coded(
            &["input"],
            format!("A publication can be updated with {UPDATE_LIMIT} publishables at most"),
            "PUBLICATION_UPDATE_LIMIT_EXCEEDED",
        ));
    }
    let to_add = publishables(state, &to_add, "publishablesToAdd", &mut errors);
    let to_remove = publishables(state, &to_remove, "publishablesToRemove", &mut errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"publication": null}), &errors));
    }
    let state = mutation.state_mut();
    if let Some(auto) = input.get("autoPublish").and_then(Value::as_bool)
        && let Some(record) = state.get_mut(&publication)
    {
        record.insert("autoPublish".into(), json!(auto));
    }
    let targets = [(publication.clone(), None)];
    for publishable in &to_add {
        catalog::publish_on(state, publishable, &targets, true);
    }
    for publishable in &to_remove {
        catalog::publish_on(state, publishable, &targets, false);
    }
    state.emit("publications/update", &publication);
    Ok(payload(json!({"publication": publication}), &[]))
}

/// Removes a publication and its catalog's link to it.
pub fn delete_publication(state: &mut ShopState, publication: &str) {
    let Some(record) = state.get(publication).cloned() else {
        return;
    };
    if let Some(catalog) = record.get("catalog").and_then(Value::as_str)
        && let Some(catalog) = state.get_mut(catalog)
        && catalog.get("publication") == Some(&json!(publication))
    {
        catalog.insert("publication".into(), Value::Null);
    }
    state.delete(publication, "publications/delete");
}

/// `publicationDelete`.
fn delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let state = mutation.state();
    let Some(publication) = ctx
        .arg_str("id")
        .filter(|id| is(state, id, "Publication"))
        .map(str::to_string)
    else {
        return Ok(payload(json!({"deletedId": null}), &[not_found("id")]));
    };
    if is_app_publication(state, &publication) {
        let error = coded(
            &["id"],
            "Cannot modify publications of app catalogs.",
            "CANNOT_MODIFY_APP_CATALOG_PUBLICATION",
        );
        return Ok(payload(json!({"deletedId": null}), &[error]));
    }
    delete_publication(mutation.state_mut(), &publication);
    Ok(payload(json!({"deletedId": publication}), &[]))
}

/// The publications that publish the products created: those with `autoPublish`.
pub fn auto_publishing(state: &ShopState) -> Vec<String> {
    state
        .keys("Publication")
        .filter(|publication| {
            state
                .get(publication)
                .and_then(|record| record.get("autoPublish"))
                .and_then(Value::as_bool)
                == Some(true)
        })
        .map(str::to_string)
        .collect()
}
