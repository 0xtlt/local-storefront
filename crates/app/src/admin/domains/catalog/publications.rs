//! Publications: `publishablePublish`, `publishableUnpublish`, and the publication fields of
//! products, collections, and product variants (`publishedOnPublication`, `resourcePublications`,
//! `resourcePublicationsV2`, `resourcePublicationsCount`, `unpublishedPublications`, …) and of
//! publications (`hasCollection`, `includedProducts`, `productPublicationsV3`, …).
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - `publishablePublish` publishes a product, collection, or product variant on publications
//!   (`publicationId`, or the deprecated `channelId`), at `publishDate` (else now);
//!   `publishableUnpublish` takes it off. "Resource does not exist" (field `["id"]`),
//!   "Publication does not exist or is not publishable" (field `["input", "0", "publicationId"]`)
//!   and "PublicationId cannot be empty" are the reference's examples.
//! - A `ProductVariant` inherits its parent product's publication state by default and can be
//!   published or unpublished independently per publication.
//! - On the Online Store, a product's `publishedAt` is when it was published, `null` when it is
//!   not; its `onlineStoreUrl` is set while it is published and `ACTIVE`. The store sync reads
//!   them: a product is on the storefront when it is `ACTIVE` and published.
//! - A `publishDate` in the future publishes at once: lsf does not schedule.
//! - Events: `product_publications/create` or `collection_publications/create` (and
//!   `…/delete` when unpublished), with `products/update` (or `collections/update`), whose
//!   `published_at` changed. Changing a variant's publication state touches the variant and its
//!   parent product and emits `products/update`. `product_listings/*` and `collection_listings/*`
//!   are sent to the sales channel app whose own publication changes: a channel's
//!   (`channelCreate`), whose listings the markets family emits (`markets::scheduled`,
//!   `markets::listed`).
//!
//! A publication event names no record (a product on a publication is not one): its GID is the
//! product's with `?publication_id=<id>`, and its snapshot holds the publication and the
//! product, which the payload is built from.

use serde_json::{Value, json};

use super::product_changed;
use super::records::{self, field, origin, text};
use crate::admin::graphql::{FieldContext, FieldError, Output};
use crate::admin::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::admin::state::{Record, ShopState, gid_number, value};

/// Variants explicitly published on a publication when their parent product is not.
const PUBLISHED_VARIANTS: &str = "lsfPublishedVariants";

/// Variants explicitly unpublished from a publication when their parent product is published.
const UNPUBLISHED_VARIANTS: &str = "lsfUnpublishedVariants";

pub fn register(registry: &mut Registry) {
    registry
        .mutation("publishablePublish", publish)
        .mutation("publishableUnpublish", unpublish);
    for type_name in ["Product", "Collection", "ProductVariant"] {
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

/// Whether `id` names a `Publishable` record (`Product`, `Collection`, or `ProductVariant`).
pub fn is_publishable(state: &ShopState, id: &str) -> bool {
    matches!(
        state.type_of(id),
        Some("Product" | "Collection" | "ProductVariant")
    )
}

/// The parent product of a `ProductVariant`.
fn parent_product<'s>(state: &'s ShopState, variant: &str) -> Option<&'s str> {
    (state.type_of(variant) == Some("ProductVariant"))
        .then(|| {
            state
                .get(variant)
                .and_then(|record| field(record, "product"))
        })
        .flatten()
}

/// The list of a publication that holds publishables of a type.
fn list_of(type_name: &str) -> &'static str {
    if type_name == "Collection" {
        "collections"
    } else {
        "products"
    }
}

/// Whether a publication lists a product, collection, or product variant.
///
/// A `ProductVariant` inherits its parent product's publication state unless it has been
/// explicitly published or unpublished on that publication.
pub fn is_published(state: &ShopState, publication: &str, publishable: &str) -> bool {
    match state.type_of(publishable) {
        Some("Collection") => records::lists(state, publication, "collections", publishable),
        Some("Product") => records::lists(state, publication, "products", publishable),
        Some("ProductVariant") => {
            if records::lists(state, publication, PUBLISHED_VARIANTS, publishable) {
                return true;
            }
            if records::lists(state, publication, UNPUBLISHED_VARIANTS, publishable) {
                return false;
            }
            parent_product(state, publishable)
                .is_some_and(|product| records::lists(state, publication, "products", product))
        }
        _ => false,
    }
}

/// Adds or removes a variant GID from an internal `lsf…` override list on a publication.
fn set_variant_override(
    state: &mut ShopState,
    publication: &str,
    field: &str,
    variant: &str,
    member: bool,
) {
    let Some(record) = state.get_mut(publication) else {
        return;
    };
    if member {
        let entry = record.entry(field).or_insert_with(|| json!([]));
        if let Value::Array(items) = entry
            && !items.iter().any(|known| known.as_str() == Some(variant))
        {
            items.push(json!(variant));
        }
    } else if let Some(Value::Array(items)) = record.get_mut(field) {
        items.retain(|known| known.as_str() != Some(variant));
    }
}

/// Removes variant overrides for `variants` from a single publication.
fn clear_variant_overrides(state: &mut ShopState, publication: &str, variants: &[String]) {
    if variants.is_empty() {
        return;
    }
    let Some(record) = state.get_mut(publication) else {
        return;
    };
    for field in [PUBLISHED_VARIANTS, UNPUBLISHED_VARIANTS] {
        if let Some(Value::Array(items)) = record.get_mut(field) {
            items.retain(|known| {
                !known
                    .as_str()
                    .is_some_and(|id| variants.iter().any(|variant| variant == id))
            });
        }
    }
}

/// Removes variant publication overrides for deleted variants from every publication.
pub fn removing_variants(state: &mut ShopState, variants: &[String]) {
    if variants.is_empty() {
        return;
    }
    let publications: Vec<String> = state.keys("Publication").map(str::to_string).collect();
    for publication in publications {
        clear_variant_overrides(state, &publication, variants);
    }
}

/// A `ResourcePublication` of a publishable on a publication, inline.
fn resource_publication(state: &ShopState, publishable: &str, publication: &str) -> Value {
    let record = state.get(publishable);
    let date = record
        .and_then(|record| record.get("publishedAt").filter(|date| !date.is_null()))
        .or_else(|| {
            parent_product(state, publishable)
                .and_then(|product| state.get(product))
                .and_then(|product| product.get("publishedAt").filter(|date| !date.is_null()))
        })
        .or_else(|| record.and_then(|record| record.get("createdAt")))
        .cloned()
        .unwrap_or(Value::Null);
    json!({
        "publication": publication,
        "channel": null,
        "publishable": publishable,
        "publishDate": date,
        "isPublished": is_published(state, publication, publishable),
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
    Ok(Output::value(is_published(
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
    let catalog_type = ctx.arg_str("catalogType");
    state
        .keys("Publication")
        .filter(|publication| {
            catalog_type.is_none_or(|wanted| {
                crate::admin::domains::markets::catalog_type_of(state, publication) == wanted
            })
        })
        .filter(|publication| !only_published || is_published(state, publication, publishable))
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
        .filter(|publication| !is_published(state, publication, publishable))
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
            Some(id) if !state.is_of(id, "Publication") => {
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

/// The publishable of `id`: a product, collection, or product variant.
fn publishable_arg(state: &ShopState, ctx: &FieldContext<'_>) -> Option<String> {
    let id = ctx.arg_str("id")?;
    is_publishable(state, id).then(|| id.to_string())
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
    let id = gid_number(publication);
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
    let Some(type_name) = state.type_of(publishable).map(str::to_string) else {
        return false;
    };
    if type_name == "ProductVariant" {
        if is_published(state, publication, publishable) == published {
            return false;
        }
        let parent_published = parent_product(state, publishable)
            .is_some_and(|product| records::lists(state, publication, "products", product));
        set_variant_override(
            state,
            publication,
            PUBLISHED_VARIANTS,
            publishable,
            published && !parent_published,
        );
        set_variant_override(
            state,
            publication,
            UNPUBLISHED_VARIANTS,
            publishable,
            !published && parent_published,
        );
        return true;
    }
    if !records::set_listed(
        state,
        publication,
        list_of(&type_name),
        publishable,
        published,
    ) {
        return false;
    }
    if type_name == "Product" {
        let variants = state
            .get(publishable)
            .map(|record| super::gids(record.get("variants")))
            .unwrap_or_default();
        clear_variant_overrides(state, publication, &variants);
        let online_store = publication == super::online_store_publication_gid();
        if online_store {
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
    // A sales channel's publication that supports future publishing schedules it.
    if published
        && publications.iter().any(|(publication, date)| {
            date.as_deref()
                .is_some_and(|date| date > mutation.timestamp().as_str())
                && state
                    .get(publication)
                    .and_then(|record| record.get("supportsFuturePublishing"))
                    .and_then(Value::as_bool)
                    != Some(true)
        })
    {
        mutation.note("publishablePublish: lsf does not schedule publishing: a future `publishDate` is published at once");
    }
    publish_on(mutation.state_mut(), &publishable, &publications, published);
    Ok(payload(json!({"publishable": publishable}), &[]))
}

/// Publishes a product, collection, or product variant on publications
/// (`(publication, publish date)`), or unpublishes it, with the events of `publishablePublish`:
/// what the markets family's publications and channels share. A listing on a sales channel's
/// publication is the markets family's (`markets::scheduled`, `markets::listed`). The
/// publications it changed.
pub fn publish_on(
    state: &mut ShopState,
    publishable: &str,
    publications: &[(String, Option<String>)],
    published: bool,
) -> Vec<String> {
    let mut changed = Vec::new();
    for (publication, date) in publications {
        if crate::admin::domains::markets::scheduled(
            state,
            publishable,
            publication,
            date.as_deref(),
            published,
        ) {
            continue;
        }
        if set_published(state, publishable, publication, date.as_deref(), published) {
            changed.push(publication.clone());
        }
    }
    if !changed.is_empty() {
        let action = if published { "create" } else { "delete" };
        match state.type_of(publishable) {
            Some("Product") => {
                for publication in &changed {
                    let snapshot = publication_snapshot(state, publishable, publication, published);
                    state.emit_with(
                        &format!("product_publications/{action}"),
                        &event_gid(publishable, publication),
                        snapshot,
                    );
                    crate::admin::domains::markets::listed(
                        state,
                        publishable,
                        publication,
                        published,
                    );
                }
                state.touch(publishable);
                product_changed(state, publishable);
                state.emit("products/update", publishable);
            }
            Some("Collection") => {
                for publication in &changed {
                    let snapshot = publication_snapshot(state, publishable, publication, published);
                    state.emit_with(
                        &format!("collection_publications/{action}"),
                        &event_gid(publishable, publication),
                        snapshot,
                    );
                    crate::admin::domains::markets::listed(
                        state,
                        publishable,
                        publication,
                        published,
                    );
                }
                state.touch(publishable);
                state.emit("collections/update", publishable);
            }
            Some("ProductVariant") => {
                state.touch(publishable);
                if let Some(product) = parent_product(state, publishable).map(str::to_string) {
                    state.touch(&product);
                    product_changed(state, &product);
                    state.emit("products/update", &product);
                }
            }
            _ => {}
        }
    }
    changed
}

fn publish(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    change(mutation, ctx, true)
}

fn unpublish(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    change(mutation, ctx, false)
}
