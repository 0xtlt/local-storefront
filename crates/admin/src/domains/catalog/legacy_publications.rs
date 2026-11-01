//! The deprecated publish mutations of products and collections: `productPublish`,
//! `productUnpublish`, `collectionPublish`, `collectionUnpublish` ("Use `publishablePublish`
//! instead."). They publish through the same path as `publishablePublish`
//! ([`super::publish_on`]), with its events: `product_publications/create` (or `…/delete`),
//! `products/update` (`collection_publications/…`, `collections/update`), and the listing events
//! of a sales channel's publication.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - `productPublish(input: {id, productPublications: [{publicationId, publishDate}]})`
//!   "Publishes a Product to specified Publication objects"; "Products sold exclusively on
//!   subscription (`requiresSellingPlan: true`) can only be published to online stores". The
//!   payload has the `product` and the deprecated `productPublications` (a `ProductPublication`
//!   per publication: its `channel`, `isPublished`, `publishDate`).
//! - `productUnpublish(input: {id, productPublications})` "Unpublishes a product".
//! - `collectionPublish(input: {id, collectionPublications: [{publicationId}]})` and
//!   `collectionUnpublish`: the same for a collection; `CollectionPublicationInput` has no
//!   `publishDate`.
//! - The deprecated `channelId` of an input is a publication's id, as `publishablePublish` reads
//!   it; `channelHandle` names a channel by its handle (`online_store` for the Online Store).
//!
//! The reference gives no example error: the messages are those of `publishablePublish` where it
//! has the same case, lsf's otherwise (each says so). The `channel` of a `ProductPublication` on
//! a publication that is no `Channel` record (the Online Store's) is built from the publication:
//! `gid://shopify/Channel/<publication number>`, as the deprecated `channelId` and a publication
//! share their number.

use serde_json::{Map, Value, json};

use super::online_store_publication_gid;
use super::publish_on;
use super::records::{field, input, objects, text};
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{ShopState, gid_number};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("productPublish", product_publish)
        .mutation("productUnpublish", product_unpublish)
        .mutation("collectionPublish", collection_publish)
        .mutation("collectionUnpublish", collection_unpublish);
}

/// The handle the deprecated `channelHandle` names the Online Store by.
const ONLINE_STORE_HANDLE: &str = "online_store";

/// The publication of a `ProductPublicationInput` or `CollectionPublicationInput`.
fn publication_of(state: &ShopState, input: &Map<String, Value>) -> Result<String, &'static str> {
    if let Some(id) = text(input, "publicationId").or_else(|| text(input, "channelId")) {
        if state.is_of(id, "Publication") {
            return Ok(id.to_string());
        }
        return Err(if text(input, "publicationId").is_some() {
            "Publication does not exist or is not publishable"
        } else {
            "Channel does not exist or is not publishable"
        });
    }
    let Some(handle) = text(input, "channelHandle") else {
        return Err("PublicationId cannot be empty");
    };
    if handle == ONLINE_STORE_HANDLE || handle == "online-store" {
        return Ok(online_store_publication_gid());
    }
    state
        .all("Channel")
        .find(|channel| field(channel, "handle") == Some(handle))
        .and_then(|channel| field(channel, "lsfPublication"))
        .filter(|publication| state.contains(publication))
        .map(str::to_string)
        .ok_or("Channel does not exist or is not publishable")
}

/// The publications of the input's list (`productPublications`, `collectionPublications`),
/// checked: `(publication, publish date)`.
fn targets(
    state: &ShopState,
    input: &Map<String, Value>,
    list: &str,
    errors: &mut Vec<UserError>,
) -> Vec<(String, Option<String>)> {
    let mut found = Vec::new();
    for (index, item) in objects(input, list).iter().enumerate() {
        let at = index.to_string();
        match publication_of(state, item) {
            Ok(publication) => {
                found.push((publication, text(item, "publishDate").map(str::to_string)))
            }
            Err(message) => {
                // The input field that named it, `publicationId` when none did.
                let name = ["publicationId", "channelId", "channelHandle"]
                    .into_iter()
                    .find(|name| text(item, name).is_some())
                    .unwrap_or("publicationId");
                errors.push(UserError::new(&["input", list, &at, name], message));
            }
        }
    }
    found
}

/// The `channel` of a publication: its channel's record, else one built from the publication.
fn channel_of(state: &ShopState, publication: &str) -> Value {
    let record = state.get(publication);
    if let Some(channel) = record
        .and_then(|record| field(record, "lsfChannel"))
        .filter(|channel| state.contains(channel))
    {
        return json!(channel);
    }
    let number = gid_number(publication);
    let online_store = publication == online_store_publication_gid();
    json!({
        "__typename": "Channel",
        "id": format!("gid://shopify/Channel/{number}"),
        "name": record.and_then(|record| field(record, "name")).unwrap_or_default(),
        "handle": if online_store { ONLINE_STORE_HANDLE } else { "" },
        "activeRegions": [],
        "supportsFuturePublishing": false,
    })
}

/// The publish date a publication shows for a publishable: the Online Store's is the product's
/// `publishedAt`, another's the date given, else now.
fn publish_date(
    state: &ShopState,
    publishable: &str,
    publication: &str,
    date: Option<&str>,
) -> Value {
    if publication == online_store_publication_gid()
        && let Some(published) = state
            .get(publishable)
            .and_then(|record| record.get("publishedAt"))
            .filter(|date| !date.is_null())
    {
        return published.clone();
    }
    json!(
        date.map(str::to_string)
            .unwrap_or_else(|| state.timestamp())
    )
}

/// Notes a future `publishDate` on a publication that does not schedule, as `publishablePublish`.
fn note_future(mutation: &mut Mutation<'_>, name: &str, targets: &[(String, Option<String>)]) {
    let now = mutation.timestamp();
    let state = mutation.state();
    let future = targets.iter().any(|(publication, date)| {
        date.as_deref().is_some_and(|date| date > now.as_str())
            && state
                .get(publication)
                .and_then(|record| record.get("supportsFuturePublishing"))
                .and_then(Value::as_bool)
                != Some(true)
    });
    if future {
        mutation.note(format!(
            "{name}: lsf does not schedule publishing: a future `publishDate` is published at once"
        ));
    }
}

/// `productPublish` and `productUnpublish`.
fn product_change(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    published: bool,
) -> Result<Value, FieldError> {
    let name = if published {
        "productPublish"
    } else {
        "productUnpublish"
    };
    let empty = if published {
        json!({"product": null, "productPublications": null})
    } else {
        json!({"product": null})
    };
    let input = input(ctx, "input");
    let state = mutation.state();
    let product = text(&input, "id").unwrap_or_default().to_string();
    if !state.is_of(&product, "Product") {
        // lsf's wording: `publishablePublish`'s "Resource does not exist" names a product here.
        let error = UserError::new(&["input", "id"], "Product does not exist");
        return Ok(payload(empty, &[error]));
    }
    let mut errors = Vec::new();
    let publications = targets(state, &input, "productPublications", &mut errors);
    let requires_selling_plan = state
        .get(&product)
        .and_then(|record| record.get("requiresSellingPlan"))
        .and_then(Value::as_bool)
        == Some(true);
    if published && requires_selling_plan {
        for (index, (publication, _)) in publications.iter().enumerate() {
            if *publication != online_store_publication_gid() {
                // lsf's wording of the reference's rule.
                errors.push(UserError::new(
                    &["input", "productPublications", &index.to_string(), "publicationId"],
                    "Products sold exclusively on subscription can only be published to online stores",
                ));
            }
        }
    }
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    if published {
        note_future(mutation, name, &publications);
    }
    let state = mutation.state_mut();
    publish_on(state, &product, &publications, published);
    if !published {
        return Ok(payload(json!({"product": product}), &[]));
    }
    let listed: Vec<Value> = publications
        .iter()
        .map(|(publication, date)| {
            json!({
                "__typename": "ProductPublication",
                "channel": channel_of(state, publication),
                "isPublished": true,
                "product": product,
                "publishDate": publish_date(state, &product, publication, date.as_deref()),
            })
        })
        .collect();
    Ok(payload(
        json!({"product": product, "productPublications": listed}),
        &[],
    ))
}

/// `collectionPublish` and `collectionUnpublish`.
fn collection_change(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    published: bool,
) -> Result<Value, FieldError> {
    let empty = if published {
        json!({"collection": null, "collectionPublications": null})
    } else {
        json!({"collection": null})
    };
    let input = input(ctx, "input");
    let state = mutation.state();
    let collection = text(&input, "id").unwrap_or_default().to_string();
    if !state.is_of(&collection, "Collection") {
        // lsf's wording, as `productPublish`'s.
        let error = UserError::new(&["input", "id"], "Collection does not exist");
        return Ok(payload(empty, &[error]));
    }
    let mut errors = Vec::new();
    let publications = targets(state, &input, "collectionPublications", &mut errors);
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let state = mutation.state_mut();
    publish_on(state, &collection, &publications, published);
    if !published {
        return Ok(payload(json!({"collection": collection}), &[]));
    }
    let listed: Vec<Value> = publications
        .iter()
        .map(|(publication, _)| {
            json!({
                "__typename": "CollectionPublication",
                "channel": channel_of(state, publication),
                "collection": collection,
                "isPublished": true,
                "publication": publication,
                "publishDate": publish_date(state, &collection, publication, None),
            })
        })
        .collect();
    Ok(payload(
        json!({"collection": collection, "collectionPublications": listed}),
        &[],
    ))
}

fn product_publish(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    product_change(mutation, ctx, true)
}

fn product_unpublish(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    product_change(mutation, ctx, false)
}

fn collection_publish(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    collection_change(mutation, ctx, true)
}

fn collection_unpublish(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    collection_change(mutation, ctx, false)
}
