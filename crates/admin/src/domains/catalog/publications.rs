//! Publications: `publishablePublish`, `publishableUnpublish`, and the publication fields of
//! products and collections (`publishedOnPublication`, `resourcePublications`,
//! `resourcePublicationsCount`, `unpublishedPublications`, …) and of publications
//! (`hasCollection`, `includedProducts`, `productPublicationsV3`, …).
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - `publishablePublish` publishes a product or a collection on publications (`publicationId`,
//!   or the deprecated `channelId`), at `publishDate` (else now); `publishableUnpublish` takes
//!   it off. "Resource does not exist" (field `["id"]`), "Publication does not exist or is not
//!   publishable" (field `["input", "0", "publicationId"]`) and "PublicationId cannot be empty"
//!   are the reference's examples.
//! - On the Online Store, a product's `publishedAt` is when it was published, `null` when it is
//!   not; its `onlineStoreUrl` is set while it is published and `ACTIVE`. The store sync reads
//!   them: a product is on the storefront when it is `ACTIVE` and published.
//! - A `publishDate` in the future publishes at once: lsf does not schedule.
//! - Events: `product_publications/create` or `collection_publications/create` (and
//!   `…/delete` when unpublished), with `products/update` (or `collections/update`), whose
//!   `published_at` changed. `product_listings/*` and `collection_listings/*` are sent to the
//!   sales channel apps whose own publication changes: the app lsf serves is not one.
//!
//! A publication event names no record (a product on a publication is not one): its GID is the
//! product's with `?publication_id=<id>`, and its snapshot holds the publication and the
//! product, which the payload is built from.

use serde_json::{Value, json};

use super::records::{field, is, origin, text};
use super::{gids, product_changed};
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::{Record, ShopState, parse_gid, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("publishablePublish", publish)
        .mutation("publishableUnpublish", unpublish);
    for type_name in ["Product", "Collection"] {
        registry
            .field(
                type_name,
                "publishedOnPublication",
                published_on_publication,
            )
            .field(type_name, "resourcePublications", resource_publications)
            .field(type_name, "resourcePublicationsV2", resource_publications)
            .field(
                type_name,
                "resourcePublicationsCount",
                resource_publications_count,
            )
            .field(type_name, "publicationCount", publication_count)
            .field(
                type_name,
                "availablePublicationsCount",
                available_publications_count,
            )
            .field(
                type_name,
                "unpublishedPublications",
                unpublished_publications,
            );
    }
    registry
        .field("Publication", "hasCollection", has_collection)
        .field("Publication", "includedProducts", included_products)
        .field(
            "Publication",
            "includedProductsCount",
            included_products_count,
        )
        .field("Publication", "productPublicationsV3", product_publications)
        .field(
            "Publication",
            "collectionPublicationsV3",
            collection_publications,
        );
}

/// The list of a publication that holds publishables of a type.
fn list_of(type_name: &str) -> &'static str {
    if type_name == "Collection" {
        "collections"
    } else {
        "products"
    }
}

/// Whether a publication lists a product or a collection.
fn lists(state: &ShopState, publication: &str, publishable: &str) -> bool {
    let list = list_of(state.type_of(publishable).unwrap_or_default());
    state.get(publication).is_some_and(|record| {
        gids(record.get(list))
            .iter()
            .any(|known| known == publishable)
    })
}

/// A `ResourcePublication` of a publishable on a publication, inline.
fn resource_publication(state: &ShopState, publishable: &str, publication: &str) -> Value {
    let record = state.get(publishable);
    let date = record
        .and_then(|record| record.get("publishedAt").filter(|date| !date.is_null()))
        .or_else(|| record.and_then(|record| record.get("createdAt")))
        .cloned()
        .unwrap_or(Value::Null);
    json!({
        "publication": publication,
        "channel": null,
        "publishable": publishable,
        "publishDate": date,
        "isPublished": lists(state, publication, publishable),
    })
}

fn publishable_of(object: &Obj) -> &str {
    object.key().unwrap_or_default()
}

/// `publishedOnPublication(publicationId:)`.
fn published_on_publication(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let publication = ctx.arg_str("publicationId").unwrap_or_default();
    Ok(Output::value(lists(
        r.state(),
        publication,
        publishable_of(object),
    )))
}

/// The publications a publishable is on (with `onlyPublished: false`, every publication).
fn resource_items(r: &ShopResolver<'_>, ctx: &FieldContext<'_>, object: &Obj) -> Vec<Value> {
    let state = r.state();
    let publishable = publishable_of(object);
    let only_published = ctx.arg_bool("onlyPublished").unwrap_or(true);
    state
        .keys("Publication")
        .filter(|publication| !only_published || lists(state, publication, publishable))
        .map(|publication| resource_publication(state, publishable, publication))
        .collect()
}

/// `resourcePublications(onlyPublished:)` and `resourcePublicationsV2`.
fn resource_publications(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    r.connection(ctx, resource_items(r, ctx, object))
}

/// `resourcePublicationsCount(onlyPublished:)`.
fn resource_publications_count(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    Ok(r.count(ctx, resource_items(r, ctx, object).len()))
}

/// `publicationCount(onlyPublished:)` (deprecated).
fn publication_count(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    Ok(Output::value(resource_items(r, ctx, object).len()))
}

/// `availablePublicationsCount`: the publications the publishable can be published on.
fn available_publications_count(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    Ok(r.count(ctx, r.state().count("Publication")))
}

/// `unpublishedPublications`.
fn unpublished_publications(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let publishable = publishable_of(object);
    let items: Vec<Value> = state
        .keys("Publication")
        .filter(|publication| !lists(state, publication, publishable))
        .map(|publication| json!(publication))
        .collect();
    r.connection(ctx, items)
}

/// `Publication.hasCollection(id:)`.
fn has_collection(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default();
    let has = r
        .items_of(object, "collections")
        .iter()
        .any(|item| item.as_str() == Some(id));
    Ok(Output::value(has))
}

/// `Publication.includedProducts`: its products (lsf has no catalog that would include more).
fn included_products(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    r.connection(ctx, r.items_of(object, "products"))
}

/// `Publication.includedProductsCount`.
fn included_products_count(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let items = r.search(ctx, "Product", r.items_of(object, "products"));
    Ok(r.count(ctx, items.len()))
}

/// The `ResourcePublication`s of a publication's list.
fn publications_of_list(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
    list: &str,
) -> Result<Output<Obj>, FieldError> {
    let publication = object.key().unwrap_or_default();
    let items: Vec<Value> = r
        .items_of(object, list)
        .iter()
        .filter_map(Value::as_str)
        .map(|publishable| resource_publication(r.state(), publishable, publication))
        .collect();
    r.connection(ctx, items)
}

/// `Publication.productPublicationsV3`.
fn product_publications(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    publications_of_list(r, ctx, object, "products")
}

/// `Publication.collectionPublicationsV3`.
fn collection_publications(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    publications_of_list(r, ctx, object, "collections")
}

/// The publications of a `[PublicationInput!]!`, checked: `(publication, publish date)`.
fn targets(
    state: &ShopState,
    ctx: &FieldContext<'_>,
    errors: &mut Vec<UserError>,
) -> Vec<(String, Option<String>)> {
    let mut found = Vec::new();
    for (index, input) in super::records::arg_objects(ctx, "input").iter().enumerate() {
        let at = index.to_string();
        let channel = text(input, "channelId");
        let id = text(input, "publicationId").or(channel);
        match id {
            None => errors.push(UserError::new(
                &["input", &at, "publicationId"],
                "PublicationId cannot be empty",
            )),
            Some(id) if !is(state, id, "Publication") => {
                let message = if channel.is_some() && text(input, "publicationId").is_none() {
                    "Channel does not exist or is not publishable"
                } else {
                    "Publication does not exist or is not publishable"
                };
                errors.push(UserError::new(&["input", &at, "publicationId"], message));
            }
            Some(id) => found.push((
                id.to_string(),
                text(input, "publishDate").map(str::to_string),
            )),
        }
    }
    found
}

/// The publishable of `id`: a product or a collection.
fn publishable_arg(state: &ShopState, ctx: &FieldContext<'_>) -> Option<String> {
    let id = ctx.arg_str("id")?;
    matches!(state.type_of(id), Some("Product" | "Collection")).then(|| id.to_string())
}

/// The record a publication event's payload is built from.
fn publication_snapshot(
    state: &ShopState,
    publishable: &str,
    publication: &str,
    published: bool,
) -> Record {
    let date = state
        .get(publishable)
        .and_then(|record| {
            record
                .get("publishedAt")
                .filter(|date| !date.is_null())
                .cloned()
        })
        .unwrap_or_else(|| json!(state.timestamp()));
    value::object(json!({
        "__typename": "ResourcePublication",
        "publication": publication,
        "publishable": publishable,
        "publishDate": date,
        "isPublished": published,
    }))
}

/// The GID of a publication event: the publishable's, with the publication's id.
fn event_gid(publishable: &str, publication: &str) -> String {
    let id = parse_gid(publication).map_or(0, |(_, id)| id);
    format!("{publishable}?publication_id={id}")
}

/// Keeps a product's Online Store fields right: `onlineStoreUrl` while it is published and
/// `ACTIVE`.
pub fn refresh_online_store_url(state: &mut ShopState, product: &str) {
    let origin = origin(state);
    let Some(record) = state.get(product) else {
        return;
    };
    let published = record
        .get("publishedAt")
        .is_some_and(|date| !date.is_null());
    let active = field(record, "status") == Some("ACTIVE");
    let handle = field(record, "handle").unwrap_or_default();
    let url = (published && active).then(|| format!("{origin}/products/{handle}"));
    if record.get("onlineStoreUrl").and_then(Value::as_str) != url.as_deref()
        && let Some(record) = state.get_mut(product)
    {
        record.insert("onlineStoreUrl".into(), json!(url));
    }
}

/// Publishes or unpublishes a publishable on a publication. Whether it changed.
fn set_published(
    state: &mut ShopState,
    publishable: &str,
    publication: &str,
    date: Option<&str>,
    published: bool,
) -> bool {
    if lists(state, publication, publishable) == published {
        return false;
    }
    let type_name = state.type_of(publishable).unwrap_or_default().to_string();
    let list = list_of(&type_name);
    if let Some(Value::Array(items)) = state
        .get_mut(publication)
        .and_then(|record| record.get_mut(list))
    {
        if published {
            items.push(json!(publishable));
        } else {
            items.retain(|known| known.as_str() != Some(publishable));
        }
    }
    let online_store = publication == super::online_store_publication_gid();
    if type_name == "Product" && online_store {
        let now = state.timestamp();
        let date = if published {
            json!(date.map(str::to_string).unwrap_or(now))
        } else {
            Value::Null
        };
        if let Some(record) = state.get_mut(publishable) {
            record.insert("publishedAt".into(), date);
        }
        refresh_online_store_url(state, publishable);
    }
    true
}

/// `publishablePublish` and `publishableUnpublish`.
fn change(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    published: bool,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let Some(publishable) = publishable_arg(state, ctx) else {
        let error = UserError::new(&["id"], "Resource does not exist");
        return Ok(payload(json!({"publishable": null}), &[error]));
    };
    let mut errors = Vec::new();
    let publications = targets(state, ctx, &mut errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"publishable": null}), &errors));
    }
    let product = state.type_of(&publishable) == Some("Product");
    if published
        && publications.iter().any(|(_, date)| {
            date.as_deref()
                .is_some_and(|date| date > mutation.timestamp().as_str())
        })
    {
        mutation.note("publishablePublish: lsf does not schedule publishing: a future `publishDate` is published at once");
    }
    let state = mutation.state_mut();
    let mut changed = Vec::new();
    for (publication, date) in &publications {
        if set_published(state, &publishable, publication, date.as_deref(), published) {
            changed.push(publication.clone());
        }
    }
    if !changed.is_empty() {
        let (kind, update) = if product {
            ("product_publications", "products/update")
        } else {
            ("collection_publications", "collections/update")
        };
        let action = if published { "create" } else { "delete" };
        for publication in &changed {
            let snapshot = publication_snapshot(state, &publishable, publication, published);
            state.emit_with(
                &format!("{kind}/{action}"),
                &event_gid(&publishable, publication),
                snapshot,
            );
        }
        state.touch(&publishable);
        if product {
            product_changed(state, &publishable);
        }
        state.emit(update, &publishable);
    }
    Ok(payload(json!({"publishable": publishable}), &[]))
}

fn publish(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    change(mutation, ctx, true)
}

fn unpublish(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    change(mutation, ctx, false)
}
