//! The payloads of markets and what sells in them: markets and the backup region, locales,
//! domains, channels, and the product and collection listings of a sales channel.

use serde_json::{Map, Value, json};

use super::catalog::{images, inventory_policy, option_values};
use super::{
    Lookup, boolean, field, first, grams, id, integer, item_weight, lower, lower_or, numeric,
    object, price, records, ref_id, resolve, tags, text, text_or, time, weight_unit,
};

/// `markets/create`, `markets/update`: a `Market`'s id, name, type and status.
pub(super) fn market(record: &Map<String, Value>) -> Value {
    json!({
        "id": id(record),
        "name": text(record, "name"),
        "type": lower(field(record, "type")),
        "status": lower_or(field(record, "status"), "active"),
    })
}

/// `markets_backup_region/update`: the region, its GID written `MarketRegion` as the example
/// writes it.
pub(super) fn backup_region(record: &Map<String, Value>) -> Value {
    let gid = match numeric(field(record, "id")) {
        Value::Null => Value::Null,
        number => json!(format!("gid://shopify/MarketRegion/{number}")),
    };
    json!({
        "admin_graphql_api_id": gid,
        "code": text(record, "code"),
    })
}

/// `locales/create`, `locales/update`, `locales/destroy`: a `ShopLocale`.
pub(super) fn locale(record: &Map<String, Value>) -> Value {
    json!({
        "locale": text(record, "locale"),
        "published": boolean(record, "published", false),
    })
}

/// `domains/create`, `domains/update`, `domains/destroy`: a `Domain` and its localization.
pub(super) fn domain(record: &Map<String, Value>) -> Value {
    let localization = object(field(record, "localization"));
    json!({
        "id": id(record),
        "host": text(record, "host"),
        "ssl_enabled": boolean(record, "sslEnabled", true),
        "localization": {
            "country": text(&localization, "country"),
            "default_locale": text(&localization, "defaultLocale"),
            "alternate_locales": match field(&localization, "alternateLocales") {
                Value::Array(locales) => Value::Array(locales.clone()),
                _ => json!([]),
            },
        },
    })
}

/// `channels/delete`: the channel's id, as a string (the example's).
pub(super) fn channel_deleted(record: &Map<String, Value>) -> Value {
    json!({"id": match id(record) {
        Value::Number(number) => json!(number.to_string()),
        other => other,
    }})
}

/// The listing of a listing event's record (`lsfListing`): when it was published, when it is
/// to be.
fn listing(record: &Map<String, Value>) -> Map<String, Value> {
    object(field(record, crate::admin::domains::markets::LISTING))
}

/// When a listing was published: the listing's date, else the record's.
fn published_at(listing: &Map<String, Value>, record: &Map<String, Value>) -> Value {
    match field(listing, "publishedAt") {
        Value::Null => time(field(record, "publishedAt")),
        at => at.clone(),
    }
}

/// The key a product listing payload holds the product under.
fn listing_key(scheduled: bool) -> &'static str {
    if scheduled {
        "scheduled_product_listing"
    } else {
        "product_listing"
    }
}

/// `collection_listings/add`, `collection_listings/update`: a `Collection` as a channel lists
/// it.
pub(super) fn collection_listing(record: &Map<String, Value>) -> Value {
    let listing = listing(record);
    let image = match field(record, "image") {
        Value::Object(image) => json!({
            "created_at": time(field(record, "createdAt")),
            "alt": first(image, &["altText", "alt"]),
            "width": field(image, "width"),
            "height": field(image, "height"),
            "src": first(image, &["url", "src", "originalSrc"]),
        }),
        _ => Value::Null,
    };
    json!({
        "collection_listing": {
            "collection_id": id(record),
            "updated_at": time(field(record, "updatedAt")),
            "body_html": first(record, &["descriptionHtml", "bodyHtml"]),
            "default_product_image": null,
            "handle": text(record, "handle"),
            "image": image,
            "title": text(record, "title"),
            "sort_order": lower(field(record, "sortOrder")),
            "published_at": published_at(&listing, record),
        }
    })
}

/// `collection_listings/remove`: the collection's id.
pub(super) fn collection_listing_removed(record: &Map<String, Value>) -> Value {
    json!({"collection_listing": {"collection_id": id(record)}})
}

/// `product_listings/add`, `product_listings/update`, and the scheduled ones
/// (`scheduled`: no `published_at`, a `publish_at` after the variants): a `Product` as a
/// channel lists it.
pub(super) fn product_listing(
    record: &Map<String, Value>,
    lookup: &dyn Lookup,
    scheduled: bool,
) -> Value {
    let listing = listing(record);
    let product_id = id(record);
    let variants = records(field(record, "variants"), lookup);
    let options = records(field(record, "options"), lookup);
    let shop = lookup.shop().unwrap_or_default();
    let format = shop
        .get("currencyFormats")
        .and_then(|formats| formats.get("moneyFormat"))
        .and_then(Value::as_str)
        .unwrap_or("${{amount}}")
        .to_string();
    let available = variants
        .iter()
        .any(|variant| field(variant, "availableForSale").as_bool() != Some(false));
    let images = images(record, &product_id, &variants, lookup);
    let mut body = Map::new();
    body.insert("product_id".into(), product_id.clone());
    body.insert("created_at".into(), time(field(record, "createdAt")));
    body.insert("updated_at".into(), time(field(record, "updatedAt")));
    body.insert(
        "body_html".into(),
        first(record, &["descriptionHtml", "bodyHtml"]).clone(),
    );
    body.insert("handle".into(), text(record, "handle"));
    body.insert("product_type".into(), text_or(record, "productType", ""));
    body.insert("title".into(), text(record, "title"));
    body.insert("vendor".into(), text(record, "vendor"));
    body.insert("available".into(), json!(available));
    body.insert("tags".into(), tags(field(record, "tags")));
    if !scheduled {
        body.insert("published_at".into(), published_at(&listing, record));
    }
    body.insert(
        "variants".into(),
        Value::Array(
            variants
                .iter()
                .enumerate()
                .map(|(index, variant)| listing_variant(variant, &options, index, &format, lookup))
                .collect(),
        ),
    );
    if scheduled {
        body.insert("publish_at".into(), time(field(&listing, "publishAt")));
    }
    body.insert("images".into(), Value::Array(images));
    body.insert(
        "options".into(),
        Value::Array(
            options
                .iter()
                .enumerate()
                .map(|(index, option)| {
                    json!({
                        "id": id(option),
                        "name": text(option, "name"),
                        "product_id": product_id,
                        "position": integer(option, "position", Value::from(index + 1)),
                        "values": option_values(option),
                    })
                })
                .collect(),
        ),
    );
    let mut payload = Map::new();
    payload.insert(listing_key(scheduled).into(), Value::Object(body));
    Value::Object(payload)
}

/// A variant of a product listing.
fn listing_variant(
    variant: &Map<String, Value>,
    options: &[Map<String, Value>],
    index: usize,
    format: &str,
    lookup: &dyn Lookup,
) -> Value {
    let item = resolve(field(variant, "inventoryItem"), lookup).unwrap_or_default();
    let weight = item_weight(&item);
    let unit = weight.get("unit").cloned().unwrap_or(Value::Null);
    let price_cents = crate::admin::domains::catalog::cents_of(variant.get("price")).unwrap_or(0);
    let option_values: Vec<Value> = field(variant, "selectedOptions")
        .as_array()
        .into_iter()
        .flatten()
        .map(|selected| {
            let name = selected.get("name").cloned().unwrap_or(Value::Null);
            let option = options
                .iter()
                .find(|option| option.get("name") == Some(&name))
                .map(id)
                .unwrap_or(Value::Null);
            json!({
                "option_id": option,
                "name": name,
                "value": selected.get("value").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    let tracked = item.get("tracked").and_then(Value::as_bool) == Some(true);
    json!({
        "id": id(variant),
        "title": text(variant, "title"),
        "option_values": option_values,
        "price": price(field(variant, "price")),
        "formatted_price": lsf_core::money::format_money(price_cents, format),
        "compare_at_price": price(field(variant, "compareAtPrice")),
        "grams": grams(&weight),
        "requires_shipping": boolean(&item, "requiresShipping", true),
        "sku": text(variant, "sku"),
        "barcode": text(variant, "barcode"),
        "taxable": boolean(variant, "taxable", true),
        "position": integer(variant, "position", Value::from(index + 1)),
        "available": boolean(variant, "availableForSale", true),
        "inventory_policy": inventory_policy(variant),
        "inventory_quantity": integer(variant, "inventoryQuantity", Value::from(0)),
        "inventory_management": if tracked { json!("shopify") } else { Value::Null },
        "fulfillment_service": "manual",
        "weight": weight.get("value").and_then(Value::as_f64).unwrap_or(0.0),
        "weight_unit": if unit.is_null() { json!("lb") } else { weight_unit(&unit) },
        "image_id": ref_id(first(variant, &["image", "featuredMedia"])),
        "created_at": time(field(variant, "createdAt")),
        "updated_at": time(field(variant, "updatedAt")),
    })
}

/// `product_listings/remove`, `scheduled_product_listings/remove`: the product's id.
pub(super) fn product_listing_removed(record: &Map<String, Value>, scheduled: bool) -> Value {
    let mut payload = Map::new();
    payload.insert(
        listing_key(scheduled).into(),
        json!({"product_id": id(record)}),
    );
    Value::Object(payload)
}
