//! The payloads of the catalog: products and their variants, collections, inventory items and
//! levels, locations.

use serde_json::{Map, Value, json};

use super::{
    Lookup, boolean, field, first, id, integer, item_weight, items, lower, lower_or, millis,
    object, price, records, ref_gid, ref_id, resolve, strings, tags, text, text_or, time,
    weight_unit,
};

/// `products/create`, `products/update`: a `Product`.
pub(super) fn product(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let product_id = id(record);
    let variants = records(field(record, "variants"), lookup);
    let options = records(field(record, "options"), lookup);
    let images = images(record, &product_id, &variants, lookup);
    let media = media(record, &product_id, &variants, lookup);
    let image = images.first().cloned().unwrap_or(Value::Null);
    // Most recently updated first, as Shopify lists them.
    let mut variant_gids: Vec<(Value, Value)> = variants
        .iter()
        .map(|variant| (text(variant, "id"), text(variant, "updatedAt")))
        .collect();
    let instant = |at: &Value| at.as_str().and_then(crate::state::value::parse_date_time);
    variant_gids.sort_by_key(|(_, updated_at)| std::cmp::Reverse(instant(updated_at)));
    let requires_components = field(record, "hasVariantsThatRequiresComponents")
        .as_bool()
        .unwrap_or_else(|| {
            variants
                .iter()
                .any(|variant| field(variant, "requiresComponents").as_bool() == Some(true))
        });
    json!({
        "admin_graphql_api_id": text(record, "id"),
        "body_html": first(record, &["descriptionHtml", "bodyHtml"]),
        "created_at": time(field(record, "createdAt")),
        "handle": text(record, "handle"),
        "id": product_id,
        "product_type": text_or(record, "productType", ""),
        "published_at": time(field(record, "publishedAt")),
        "template_suffix": text(record, "templateSuffix"),
        "title": text(record, "title"),
        "updated_at": time(field(record, "updatedAt")),
        "vendor": text(record, "vendor"),
        "status": lower_or(field(record, "status"), "active"),
        "published_scope": "web",
        "tags": tags(field(record, "tags")),
        "variants": variants
            .iter()
            .enumerate()
            .map(|(index, variant)| self::variant(variant, &product_id, index))
            .collect::<Vec<_>>(),
        "options": options
            .iter()
            .enumerate()
            .map(|(index, option)| self::option(option, &product_id, index))
            .collect::<Vec<_>>(),
        "images": images,
        "image": image,
        "media": media,
        "variant_gids": variant_gids
            .into_iter()
            .map(|(gid, updated_at)| json!({
                "admin_graphql_api_id": gid,
                "updated_at": millis(&updated_at),
            }))
            .collect::<Vec<_>>(),
        "has_variants_that_requires_components": requires_components,
        "category": category(field(record, "category"), lookup),
    })
}

/// A variant of a product payload.
fn variant(variant: &Map<String, Value>, product_id: &Value, index: usize) -> Value {
    let quantity = integer(variant, "inventoryQuantity", Value::from(0));
    // `barcodes` is a connection of `ProductVariantBarcode`s; REST lists their values.
    let barcodes: Vec<Value> = match field(variant, "barcodes") {
        Value::Null => match text(variant, "barcode") {
            Value::Null => Vec::new(),
            barcode => vec![barcode],
        },
        list => items(list)
            .into_iter()
            .map(|barcode| match barcode {
                Value::Object(barcode) => text(&barcode, "value"),
                other => other,
            })
            .collect(),
    };
    let image = first(variant, &["image", "featuredMedia"]);
    json!({
        "admin_graphql_api_id": text(variant, "id"),
        "barcode": text(variant, "barcode"),
        "compare_at_price": price(field(variant, "compareAtPrice")),
        "created_at": time(field(variant, "createdAt")),
        "id": id(variant),
        "inventory_policy": inventory_policy(variant),
        "position": integer(variant, "position", Value::from(index + 1)),
        "price": price(field(variant, "price")),
        "product_id": product_id,
        "sku": text(variant, "sku"),
        "taxable": boolean(variant, "taxable", true),
        "title": text(variant, "title"),
        "updated_at": time(field(variant, "updatedAt")),
        "option1": selected_option(variant, 0),
        "option2": selected_option(variant, 1),
        "option3": selected_option(variant, 2),
        "image_id": ref_id(image),
        "inventory_item_id": ref_id(field(variant, "inventoryItem")),
        "inventory_quantity": quantity,
        "old_inventory_quantity": quantity,
        "barcodes": barcodes,
    })
}

/// The value of a variant's selected option at `position`: REST's `option1` to `option3`.
fn selected_option(variant: &Map<String, Value>, position: usize) -> Value {
    field(variant, "selectedOptions")
        .get(position)
        .and_then(|option| option.get("value"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// A variant's `inventory_policy`: `deny` unless it says otherwise.
pub(super) fn inventory_policy(variant: &Map<String, Value>) -> Value {
    lower_or(field(variant, "inventoryPolicy"), "deny")
}

/// An option of a product payload.
fn option(option: &Map<String, Value>, product_id: &Value, index: usize) -> Value {
    json!({
        "id": id(option),
        "product_id": product_id,
        "name": text(option, "name"),
        "position": integer(option, "position", Value::from(index + 1)),
        "values": option_values(option),
    })
}

/// The values of a product option: its `values`, else the names of its `optionValues`.
pub(super) fn option_values(option: &Map<String, Value>) -> Value {
    match field(option, "values") {
        Value::Array(values) => Value::Array(values.clone()),
        _ => Value::Array(
            field(option, "optionValues")
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|value| value.get("name").cloned())
                .collect(),
        ),
    }
}

/// The images of a product, REST's `images`: its `MediaImage`s (or its deprecated `images`).
pub(super) fn images(
    record: &Map<String, Value>,
    product_id: &Value,
    variants: &[Map<String, Value>],
    lookup: &dyn Lookup,
) -> Vec<Value> {
    let media = image_media(record, lookup);
    media
        .iter()
        .enumerate()
        .map(|(index, media)| {
            let image = object(first(media, &["image", "preview"]));
            let image = match image.get("image") {
                // A `MediaPreviewImage` holds its `Image` in `image`.
                Some(Value::Object(inner)) => inner.clone(),
                _ => image,
            };
            json!({
                "id": id(media),
                "alt": first(media, &["alt", "altText"]),
                "position": index + 1,
                "product_id": product_id,
                "created_at": time(field(media, "createdAt")),
                "updated_at": time(field(media, "updatedAt")),
                "admin_graphql_api_id": text(media, "id"),
                "width": field(&image, "width"),
                "height": field(&image, "height"),
                "src": first(&image, &["url", "src", "originalSrc"]),
                "variant_ids": variant_ids_of(media, variants),
            })
        })
        .collect()
}

/// The media of a product, REST's `media`.
fn media(
    record: &Map<String, Value>,
    product_id: &Value,
    variants: &[Map<String, Value>],
    lookup: &dyn Lookup,
) -> Vec<Value> {
    records(field(record, "media"), lookup)
        .iter()
        .enumerate()
        .map(|(index, media)| {
            let preview = object(field(media, "preview"));
            let image = match first(&preview, &["image"]) {
                Value::Object(image) => image.clone(),
                _ => object(field(media, "image")),
            };
            json!({
                "id": id(media),
                "product_id": product_id,
                "position": index + 1,
                "created_at": time(field(media, "createdAt")),
                "updated_at": time(field(media, "updatedAt")),
                "alt": first(media, &["alt", "altText"]),
                "status": text_or(media, "status", "READY"),
                "media_content_type": text_or(media, "mediaContentType", "IMAGE"),
                "preview_image": json!({
                    "width": field(&image, "width"),
                    "height": field(&image, "height"),
                    "src": first(&image, &["url", "src"]),
                    "status": text_or(&preview, "status", "READY"),
                }),
                "variant_ids": variant_ids_of(media, variants),
                "admin_graphql_api_id": text(media, "id"),
            })
        })
        .collect()
}

/// The `MediaImage`s of a product, or the `Image`s of its deprecated `images`.
fn image_media(record: &Map<String, Value>, lookup: &dyn Lookup) -> Vec<Map<String, Value>> {
    let media: Vec<Map<String, Value>> = records(field(record, "media"), lookup)
        .into_iter()
        .filter(|media| {
            field(media, "mediaContentType")
                .as_str()
                .is_none_or(|kind| kind == "IMAGE")
                && field(media, "__typename")
                    .as_str()
                    .is_none_or(|kind| kind == "MediaImage")
        })
        .collect();
    if !media.is_empty() {
        return media;
    }
    records(field(record, "images"), lookup)
        .into_iter()
        .map(|image| {
            let mut media = image.clone();
            media.insert("image".to_string(), Value::Object(image));
            media
        })
        .collect()
}

/// The variants whose image is this media: their `image` (or `featuredMedia`), or one of their
/// `media`.
fn variant_ids_of(media: &Map<String, Value>, variants: &[Map<String, Value>]) -> Vec<Value> {
    let gid = text(media, "id");
    if gid.is_null() {
        return Vec::new();
    }
    variants
        .iter()
        .filter(|variant| {
            ref_gid(first(variant, &["image", "featuredMedia"])) == gid
                || items(field(variant, "media"))
                    .iter()
                    .any(|own| ref_gid(own) == gid)
        })
        .map(id)
        .collect()
}

/// The category of a product, REST's `category`.
fn category(reference: &Value, lookup: &dyn Lookup) -> Value {
    let Some(category) = resolve(reference, lookup) else {
        return Value::Null;
    };
    json!({
        "admin_graphql_api_id": text(&category, "id"),
        "name": text(&category, "name"),
        "full_name": text(&category, "fullName"),
    })
}

/// `collections/create`, `collections/update`: a `Collection`.
pub(super) fn collection(record: &Map<String, Value>) -> Value {
    json!({
        "id": id(record),
        "handle": text(record, "handle"),
        "title": text(record, "title"),
        "updated_at": time(field(record, "updatedAt")),
        "body_html": first(record, &["descriptionHtml", "bodyHtml"]),
        "published_at": time(field(record, "publishedAt")),
        // `BEST_SELLING` is `best-selling`.
        "sort_order": match field(record, "sortOrder") {
            Value::String(order) => json!(order.to_ascii_lowercase().replace('_', "-")),
            _ => Value::Null,
        },
        "template_suffix": text(record, "templateSuffix"),
        "published_scope": "web",
        "admin_graphql_api_id": text(record, "id"),
    })
}

/// `collections/delete`.
pub(super) fn collection_deleted(record: &Map<String, Value>) -> Value {
    json!({
        "id": id(record),
        "published_scope": "web",
        "admin_graphql_api_id": text(record, "id"),
    })
}

/// `inventory_items/create`, `inventory_items/update`: an `InventoryItem`.
pub(super) fn inventory_item(record: &Map<String, Value>) -> Value {
    let weight = item_weight(record);
    json!({
        "id": id(record),
        "sku": text(record, "sku"),
        "created_at": time(field(record, "createdAt")),
        "updated_at": time(field(record, "updatedAt")),
        "requires_shipping": boolean(record, "requiresShipping", true),
        "cost": price(field(record, "unitCost").get("amount").unwrap_or(&Value::Null)),
        "country_code_of_origin": text(record, "countryCodeOfOrigin"),
        "province_code_of_origin": text(record, "provinceCodeOfOrigin"),
        "harmonized_system_code": text(record, "harmonizedSystemCode"),
        "tracked": boolean(record, "tracked", true),
        "country_harmonized_system_codes": harmonized_codes(record),
        "weight_value": weight.get("value").and_then(Value::as_f64).unwrap_or(0.0),
        "weight_unit": weight_unit(weight.get("unit").unwrap_or(&Value::Null)),
        "admin_graphql_api_id": text(record, "id"),
    })
}

/// `inventory_items/delete`.
pub(super) fn inventory_item_deleted(record: &Map<String, Value>) -> Value {
    json!({
        "id": id(record),
        "country_code_of_origin": text(record, "countryCodeOfOrigin"),
        "province_code_of_origin": text(record, "provinceCodeOfOrigin"),
        "harmonized_system_code": text(record, "harmonizedSystemCode"),
        "country_harmonized_system_codes": harmonized_codes(record),
        "admin_graphql_api_id": text(record, "id"),
    })
}

fn harmonized_codes(record: &Map<String, Value>) -> Value {
    Value::Array(
        items(field(record, "countryHarmonizedSystemCodes"))
            .iter()
            .map(|code| {
                let code = object(code);
                json!({
                    "harmonized_system_code": text(&code, "harmonizedSystemCode"),
                    "country_code": text(&code, "countryCode"),
                })
            })
            .collect(),
    )
}

/// `inventory_levels/update`, `inventory_levels/connect`: an `InventoryLevel`, whose
/// `available` is its quantity named `available`.
pub(super) fn inventory_level(record: &Map<String, Value>) -> Value {
    let item = ref_id(field(record, "item"));
    let location = ref_id(field(record, "location"));
    let available = field(record, "quantities")
        .as_array()
        .into_iter()
        .flatten()
        .find(|quantity| quantity.get("name").and_then(Value::as_str) == Some("available"))
        .and_then(|quantity| quantity.get("quantity").cloned())
        .unwrap_or_else(|| integer(record, "available", Value::Null));
    let gid = match text(record, "id") {
        Value::Null => json!(format!(
            "gid://shopify/InventoryLevel/{location}?inventory_item_id={item}"
        )),
        gid => gid,
    };
    json!({
        "inventory_item_id": item,
        "location_id": location,
        "available": available,
        "updated_at": time(field(record, "updatedAt")),
        "admin_graphql_api_id": gid,
    })
}

/// `inventory_levels/disconnect`.
pub(super) fn inventory_level_disconnected(record: &Map<String, Value>) -> Value {
    json!({
        "inventory_item_id": ref_id(field(record, "item")),
        "location_id": ref_id(field(record, "location")),
    })
}

/// `locations/create`, `locations/update`: a `Location`, whose address is its `address`.
pub(super) fn location(record: &Map<String, Value>) -> Value {
    let address = match field(record, "address") {
        Value::Object(address) => address.clone(),
        _ => record.clone(),
    };
    let country_code = first(&address, &["countryCode", "countryCodeV2"]).clone();
    json!({
        "id": id(record),
        "name": text(record, "name"),
        "address1": text(&address, "address1"),
        "address2": text(&address, "address2"),
        "city": text(&address, "city"),
        "zip": text(&address, "zip"),
        "province": text(&address, "province"),
        "country": country_code,
        "phone": text(&address, "phone"),
        "created_at": time(field(record, "createdAt")),
        "updated_at": time(field(record, "updatedAt")),
        "country_code": country_code,
        "country_name": text(&address, "country"),
        "province_code": text(&address, "provinceCode"),
        "legacy": boolean(record, "isFulfillmentService", false),
        "active": boolean(record, "isActive", true),
        "admin_graphql_api_id": text(record, "id"),
    })
}

/// `variants/in_stock`, `variants/out_of_stock`: a `ProductVariant`, in the keys of the
/// reference's example (a REST variant without its inventory item and weight).
pub(super) fn variant_stock(record: &Map<String, Value>) -> Value {
    let quantity = integer(record, "inventoryQuantity", Value::from(0));
    json!({
        "id": id(record),
        "product_id": ref_id(field(record, "product")),
        "title": text(record, "title"),
        "price": price(field(record, "price")),
        "position": integer(record, "position", Value::from(1)),
        "inventory_policy": inventory_policy(record),
        "compare_at_price": price(field(record, "compareAtPrice")),
        "option1": selected_option(record, 0),
        "option2": selected_option(record, 1),
        "option3": selected_option(record, 2),
        "created_at": time(field(record, "createdAt")),
        "updated_at": time(field(record, "updatedAt")),
        "taxable": boolean(record, "taxable", true),
        "barcode": text(record, "barcode"),
        "sku": text(record, "sku"),
        "inventory_quantity": quantity,
        "old_inventory_quantity": quantity,
        "admin_graphql_api_id": text(record, "id"),
        "image_id": ref_id(first(record, &["image", "featuredMedia"])),
    })
}

/// `product_publications/create`, `…/update`, and the `collection_publications/…` ones: a
/// `ResourcePublication` (its `publication` and `publishable`), whose REST id is `null` in the
/// reference's examples.
pub(super) fn resource_publication(record: &Map<String, Value>, resource: &str) -> Value {
    let mut payload = json!({
        "id": null,
        "publication_id": ref_id(field(record, "publication")),
        "published_at": time(field(record, "publishDate")),
        "published": boolean(record, "isPublished", true),
        "created_at": null,
        "updated_at": null,
    });
    if let Value::Object(map) = &mut payload {
        map.insert(
            format!("{resource}_id"),
            ref_id(field(record, "publishable")),
        );
    }
    payload
}

/// `selling_plan_groups/create`, `selling_plan_groups/update`: a `SellingPlanGroup` with its
/// plans. The example lists `products` and `product_variants` empty: lsf writes each as its id
/// and GID *(lsf's guess of their shape)*, and a plan's policies with REST's lowercase enums.
pub(super) fn selling_plan_group(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let plans = records(field(record, "sellingPlans"), lookup);
    let refs = |list: &str| -> Vec<Value> {
        items(field(record, list))
            .iter()
            .map(|item| json!({"id": ref_id(item), "admin_graphql_api_id": ref_gid(item)}))
            .collect()
    };
    json!({
        "admin_graphql_api_id": text(record, "id"),
        "id": id(record),
        "name": text(record, "name"),
        "merchant_code": text(record, "merchantCode"),
        // The app that made the group: GraphQL's `SellingPlanGroup` does not name it.
        "admin_graphql_api_app": text(record, "app"),
        "app_id": text(record, "appId"),
        "description": text(record, "description"),
        "options": strings(field(record, "options")),
        "position": field(record, "position").clone(),
        "summary": text(record, "summary"),
        "selling_plans": plans.iter().map(selling_plan).collect::<Vec<_>>(),
        "product_variants": refs("productVariants"),
        "products": refs("products"),
    })
}

/// A plan of a `selling_plan_groups/…` payload.
fn selling_plan(plan: &Map<String, Value>) -> Value {
    let billing = object(field(plan, "billingPolicy"));
    let delivery = object(field(plan, "deliveryPolicy"));
    let billing_policy = if billing.contains_key("interval") {
        json!({
            "interval": lower(field(&billing, "interval")),
            "interval_count": field(&billing, "intervalCount").clone(),
            "min_cycles": field(&billing, "minCycles").clone(),
            "max_cycles": field(&billing, "maxCycles").clone(),
        })
    } else {
        json!({
            "checkout_charge": field(&billing, "checkoutCharge").clone(),
            "remaining_balance_charge_trigger": lower(field(&billing, "remainingBalanceChargeTrigger")),
        })
    };
    let delivery_policy = if delivery.contains_key("interval") {
        json!({
            "interval": lower(field(&delivery, "interval")),
            "interval_count": field(&delivery, "intervalCount").clone(),
            "anchors": items(field(&delivery, "anchors")),
            "cutoff": field(&delivery, "cutoff").clone(),
            "pre_anchor_behavior": lower(field(&delivery, "preAnchorBehavior")),
        })
    } else {
        json!({
            "anchors": items(field(&delivery, "anchors")),
            "cutoff": field(&delivery, "cutoff").clone(),
            "fulfillment_trigger": lower(field(&delivery, "fulfillmentTrigger")),
            "pre_anchor_behavior": lower(field(&delivery, "preAnchorBehavior")),
        })
    };
    let pricing: Vec<Value> = items(field(plan, "pricingPolicies"))
        .iter()
        .map(|policy| {
            let policy = object(policy);
            let value = object(field(&policy, "adjustmentValue"));
            json!({
                "adjustment_type": lower(field(&policy, "adjustmentType")),
                "adjustment_value": match field(&value, "percentage") {
                    Value::Null => price(field(&value, "amount")),
                    percentage => percentage.clone(),
                },
                "after_cycle": field(&policy, "afterCycle").clone(),
            })
        })
        .collect();
    json!({
        "name": text(plan, "name"),
        "options": strings(field(plan, "options")),
        "position": field(plan, "position").clone(),
        "description": text(plan, "description"),
        "billing_policy": billing_policy,
        "delivery_policy": delivery_policy,
        "pricing_policies": pricing,
    })
}

/// `selling_plan_groups/delete`.
pub(super) fn selling_plan_group_deleted(record: &Map<String, Value>) -> Value {
    json!({
        "admin_graphql_api_id": text(record, "id"),
        "id": id(record),
    })
}
