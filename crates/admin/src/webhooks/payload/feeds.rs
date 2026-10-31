//! The payloads of product feeds: `product_feeds/create` and `/update` (the feed), and the
//! syncs, `product_feeds/full_sync`, `/incremental_sync` and `/full_sync_finish`, whose product
//! is in the shape of the GraphQL `Product` (camelCase, connections as `edges`), as the examples
//! of the webhook reference and of "Contextual product feeds" show it, not REST's.
//!
//! A sync's record is the snapshot `domains::catalog::feeds` emits: the feed, the product (its
//! GID, and a copy for a product deleted) and the sync's metadata. The examples write the
//! metadata's time `occurred_at` (milliseconds, UTC) for a full sync and `occured_at` (sic, as
//! the record) for an incremental one, and the finish's `createdAt` as Ruby's `Time#to_s`
//! (`2021-12-31 19:00:00 -0500`): lsf writes them so.

use serde_json::{Map, Value, json};

use super::{Lookup, field, items, lower, millis, object, records, resolve, text, time};

/// `product_feeds/create`, `product_feeds/update`: the feed's GID, country, language and status.
pub(super) fn feed(record: &Map<String, Value>) -> Value {
    json!({
        "id": text(record, "id"),
        "country": text(record, "country"),
        "language": text(record, "language"),
        "status": lower(field(record, "status")),
    })
}

/// The `productFeed` of a sync: the feed and the shop's GID.
fn product_feed(sync: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let feed = object(field(sync, "feed"));
    json!({
        "id": text(&feed, "id"),
        "shop_id": lookup.shop().map_or(Value::Null, |shop| text(&shop, "id")),
        "language": text(&feed, "language"),
        "country": text(&feed, "country"),
    })
}

/// The product of a sync: the record of its GID, else the snapshot's copy.
fn product_of(sync: &Map<String, Value>, lookup: &dyn Lookup) -> Option<Map<String, Value>> {
    resolve(field(sync, "product"), lookup)
        .or_else(|| field(sync, "productRecord").as_object().cloned())
}

/// `product_feeds/full_sync`: one product of a full sync.
pub(super) fn full_sync(sync: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let metadata = object(field(sync, "metadata"));
    json!({
        "metadata": {
            "action": text(&metadata, "action"),
            "type": "FULL",
            "resource": "PRODUCT",
            "fullSyncId": text(&metadata, "fullSyncId"),
            "truncatedFields": [],
            "occurred_at": millis(field(&metadata, "occurredAt")),
        },
        "productFeed": product_feed(sync, lookup),
        "product": product_of(sync, lookup).map_or(Value::Null, |product| product_payload(&product, sync, lookup)),
        "products": null,
    })
}

/// `product_feeds/incremental_sync`: a product listed, changed or unlisted.
pub(super) fn incremental_sync(sync: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let metadata = object(field(sync, "metadata"));
    json!({
        "metadata": {
            "action": text(&metadata, "action"),
            "type": "INCREMENTAL",
            "resource": "PRODUCT",
            "truncatedFields": [],
            "occured_at": time(field(&metadata, "occurredAt")),
        },
        "productFeed": product_feed(sync, lookup),
        "product": product_of(sync, lookup).map_or(Value::Null, |product| product_payload(&product, sync, lookup)),
        "products": null,
    })
}

/// `product_feeds/full_sync_finish`: the end of a full sync, with how many products it sent.
pub(super) fn full_sync_finish(sync: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let metadata = object(field(sync, "metadata"));
    json!({
        "metadata": {
            "action": text(&metadata, "action"),
            "type": "FULL",
            "resource": "PRODUCT",
            "fullSyncId": text(&metadata, "fullSyncId"),
            "truncatedFields": [],
            "occurred_at": millis(field(&metadata, "occurredAt")),
        },
        "productFeed": product_feed(sync, lookup),
        "fullSync": {
            "createdAt": ruby_time(field(&metadata, "createdAt")),
            "errorCode": null,
            "status": "completed",
            "count": field(&metadata, "count").as_u64().unwrap_or(0),
            "url": null,
        },
    })
}

/// A date as Ruby's `Time#to_s` writes it: `2021-12-31 19:00:00 -0500`.
fn ruby_time(value: &Value) -> Value {
    value
        .as_str()
        .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
        .map_or_else(
            || value.clone(),
            |at| json!(at.format("%Y-%m-%d %H:%M:%S %z").to_string()),
        )
}

/// A connection as GraphQL answers it: `{"edges": [{"node": …}]}`.
fn edges(nodes: Vec<Value>) -> Value {
    json!({"edges": nodes.into_iter().map(|node| json!({"node": node})).collect::<Vec<_>>()})
}

/// An image as the feed writes it: `id`, `url`, `height`, `width`.
fn image(value: &Value) -> Value {
    let image = object(value);
    if image.is_empty() {
        return Value::Null;
    }
    json!({
        "id": text(&image, "id"),
        "url": text(&image, "url"),
        "height": field(&image, "height"),
        "width": field(&image, "width"),
    })
}

/// The metafields of an owner whose type is `id`: the `customIds` a feed reads products by.
fn custom_ids(owner: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    Value::Array(
        records(field(owner, "metafields"), lookup)
            .iter()
            .filter(|metafield| field(metafield, "type").as_str() == Some("id"))
            .map(|metafield| {
                json!({
                    "namespace": text(metafield, "namespace"),
                    "key": text(metafield, "key"),
                    "value": text(metafield, "value"),
                    "type": "id",
                })
            })
            .collect(),
    )
}

/// The product of a feed's sync, as the GraphQL `Product` reads (the feed's own selection).
fn product_payload(
    product: &Map<String, Value>,
    sync: &Map<String, Value>,
    lookup: &dyn Lookup,
) -> Value {
    let currency = lookup
        .shop()
        .map_or(Value::Null, |shop| text(&shop, "currencyCode"));
    // Published on the feed's channel: what a sync is of, but a product unlisted.
    let action = field(&object(field(sync, "metadata")), "action").clone();
    let published = action.as_str() != Some("DELETE");
    let images: Vec<Value> = items(field(product, "images")).iter().map(image).collect();
    let media: Vec<Value> = records(field(product, "media"), lookup)
        .iter()
        .map(|medium| {
            json!({
                "id": text(medium, "id"),
                "mediaContentType": text(medium, "mediaContentType"),
                "image": {"url": field(medium, "image").get("url").cloned().unwrap_or(Value::Null)},
            })
        })
        .collect();
    let options: Vec<Value> = records(field(product, "options"), lookup)
        .iter()
        .map(|option| json!({"name": text(option, "name"), "values": field(option, "values")}))
        .collect();
    let seo = object(field(product, "seo"));
    let variants: Vec<Value> = records(field(product, "variants"), lookup)
        .iter()
        .map(|variant| {
            let item = resolve(field(variant, "inventoryItem"), lookup).unwrap_or_default();
            let weight = object(field(&object(field(&item, "measurement")), "weight"));
            let money = |name: &str| match field(variant, name) {
                Value::String(amount) => json!({"amount": amount, "currencyCode": currency}),
                Value::Object(money) => Value::Object(money.clone()),
                _ => Value::Null,
            };
            json!({
                "id": text(variant, "id"),
                "title": text(variant, "title"),
                "price": money("price"),
                "compareAtPrice": money("compareAtPrice"),
                "sku": text(variant, "sku"),
                "barcode": text(variant, "barcode"),
                "quantityAvailable": field(variant, "inventoryQuantity"),
                "availableForSale": field(variant, "availableForSale"),
                "weight": field(&weight, "value"),
                "weightUnit": field(&weight, "unit"),
                "requiresShipping": field(&item, "requiresShipping"),
                "inventoryPolicy": text(variant, "inventoryPolicy"),
                "createdAt": time(field(variant, "createdAt")),
                "updatedAt": time(field(variant, "updatedAt")),
                "image": image(field(variant, "image")),
                "selectedOptions": items(field(variant, "selectedOptions"))
                    .iter()
                    .map(|option| {
                        let option = object(option);
                        json!({"name": text(&option, "name"), "value": text(&option, "value")})
                    })
                    .collect::<Vec<_>>(),
                "customIds": custom_ids(variant, lookup),
                "deliveryOptionEstimates": null,
            })
        })
        .collect();
    json!({
        "id": text(product, "id"),
        "title": text(product, "title"),
        "description": text(product, "description"),
        "onlineStoreUrl": text(product, "onlineStoreUrl"),
        "createdAt": time(field(product, "createdAt")),
        "updatedAt": time(field(product, "updatedAt")),
        "isPublished": published,
        "isBundle": field(product, "hasVariantsThatRequiresComponents").as_bool().unwrap_or(false),
        "publishedAt": time(field(product, "publishedAt")),
        "productType": text(product, "productType"),
        "vendor": text(product, "vendor"),
        "handle": text(product, "handle"),
        "images": edges(images),
        "media": edges(media),
        "options": options,
        "seo": {"title": text(&seo, "title"), "description": text(&seo, "description")},
        "tags": field(product, "tags"),
        "category": field(product, "category"),
        "customIds": custom_ids(product, lookup),
        "variants": edges(variants),
    })
}
