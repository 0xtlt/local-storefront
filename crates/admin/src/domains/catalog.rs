//! The catalog: products and their variants, options and media, inventory and locations,
//! collections, the Online Store publication, selling plans and gift cards.
//!
//! The records and their ids: the "Records" section of `crates/admin/docs/design.md`.

use lsf_core::store::{
    Adjustment, CheckoutCharge, Collection, GiftCard, InventoryPolicy, Location, Media, MediaKind,
    Product, SellingPlan, SellingPlanGroup, SortOrder, Variant, VideoHost,
};
use lsf_core::util::stable_id;
use serde_json::{Value, json};

mod collections;
mod gift_card_extras;
mod gift_cards;
mod inventory;
mod locations;
mod media;
#[cfg(test)]
mod mutation_tests;
mod options;
mod product_set;
mod products;
mod publications;
mod queries;
mod records;
mod selling_plans;
mod variants;

pub use collections::apply_rules;
pub use inventory::with_idempotency;
pub use media::done_job;
pub use products::{delete_product, tags_of};
pub use publications::publish_on;
pub use records::{
    after_inventory_change, check_metafields, is_active_location, level_at, new_level, quantity,
    set_metafields, stock_of,
};

use crate::state::seed::Seed;
use crate::state::value::{self, date, date_time, money, optional, text};
use crate::state::{Record, ShopState, gid};

/// The names of the quantities an inventory level has, in the order Shopify documents them.
pub const QUANTITY_NAMES: [&str; 8] = [
    "available",
    "committed",
    "damaged",
    "incoming",
    "on_hand",
    "quality_control",
    "reserved",
    "safety_stock",
];

/// The name of the location lsf adds when the store data has none: every Shopify shop has at
/// least one.
pub const DEFAULT_LOCATION_NAME: &str = "Shop location";

pub fn product_gid(product: &Product) -> String {
    gid("Product", product.id)
}

pub fn variant_gid(variant: &Variant) -> String {
    gid("ProductVariant", variant.id)
}

/// The id of the inventory item of a variant, which lsf-core does not give.
pub fn inventory_item_id(variant_id: u64) -> u64 {
    stable_id("inventory_item", &variant_id.to_string())
}

pub fn inventory_item_gid(variant_id: u64) -> String {
    gid("InventoryItem", inventory_item_id(variant_id))
}

/// The GID of the inventory level of a variant at a location, with the `inventory_item_id`
/// query Shopify writes: `gid://shopify/InventoryLevel/<id>?inventory_item_id=<item id>`.
pub fn inventory_level_gid(variant_id: u64, location_id: u64) -> String {
    format!(
        "{}?inventory_item_id={}",
        gid(
            "InventoryLevel",
            stable_id("inventory_level", &format!("{variant_id}/{location_id}"))
        ),
        inventory_item_id(variant_id)
    )
}

pub fn option_gid(product_id: u64, option: &str) -> String {
    gid(
        "ProductOption",
        stable_id("product_option", &format!("{product_id}/{option}")),
    )
}

pub fn option_value_gid(product_id: u64, option: &str, value: &str) -> String {
    gid(
        "ProductOptionValue",
        stable_id(
            "product_option_value",
            &format!("{product_id}/{option}/{value}"),
        ),
    )
}

/// The GID of a piece of product media: its type depends on its kind, its id is lsf-core's.
pub fn media_gid(media: &Media) -> String {
    gid(media_type(media), media.id)
}

fn media_type(media: &Media) -> &'static str {
    match media.kind {
        MediaKind::Image => "MediaImage",
        MediaKind::Video { .. } => "Video",
        MediaKind::ExternalVideo { .. } => "ExternalVideo",
        MediaKind::Model { .. } => "Model3d",
    }
}

pub fn location_gid(location: &Location) -> String {
    gid("Location", location.id)
}

/// The id of the location lsf adds when the data has none: the one lsf-core would give a
/// location of that name.
pub fn default_location_id() -> u64 {
    stable_id("location", &DEFAULT_LOCATION_NAME.to_lowercase())
}

/// The publication of the Online Store, on which every product and collection of the store data
/// is published.
pub fn online_store_publication_gid() -> String {
    gid("Publication", stable_id("publication", "online-store"))
}

/// The GID of a selling plan group: lsf-core's id is a hexadecimal string (as in Liquid), the
/// Admin API's a number, made from the name.
pub fn selling_plan_group_gid(group: &SellingPlanGroup) -> String {
    gid(
        "SellingPlanGroup",
        stable_id("selling_plan_group", &group.name.to_lowercase()),
    )
}

pub fn selling_plan_gid(plan: &SellingPlan) -> String {
    gid("SellingPlan", plan.id)
}

pub fn collection_gid(collection: &Collection) -> String {
    gid("Collection", collection.id)
}

pub fn gift_card_gid(card: &GiftCard) -> String {
    gid("GiftCard", card.id)
}

/// Whether a collection is the `all` one lsf-core adds when the data does not define it: on
/// Shopify, `/collections/all` is not a collection of the Admin API.
pub fn is_implicit_all(store: &lsf_core::Store, collection: &Collection) -> bool {
    collection.handle == "all"
        && collection.id == stable_id("collection", "all")
        && collection.title == "Products"
        && collection.description.is_empty()
        && collection.metafields.is_empty()
        && collection.image.is_none()
        && collection.sort_order == SortOrder::TitleAscending
        && collection.products.len() == store.products.len()
}

/// The locations of the shop: those of the store data, or one made of the shop's address.
pub fn locations(store: &lsf_core::Store) -> Vec<Location> {
    if !store.locations.is_empty() {
        return store.locations.clone();
    }
    vec![Location {
        id: default_location_id(),
        name: DEFAULT_LOCATION_NAME.to_string(),
        address: store.shop.address.clone(),
        pick_up_enabled: false,
        pick_up_time: String::new(),
        physical_storefront: false,
        latitude: None,
        longitude: None,
        metafields: Default::default(),
    }]
}

pub fn seed(seed: &mut Seed) {
    let store = seed.store;
    let locations = locations(store);
    for location in &locations {
        seed_location(seed, location);
    }
    let selling_plan_groups: Vec<String> = store
        .selling_plan_groups
        .iter()
        .map(selling_plan_group_gid)
        .collect();
    // The products not on the storefront too: the Admin API lists them, with their status.
    for product in store.products.iter().chain(&store.unlisted_products) {
        seed_product(seed, product, &locations, &selling_plan_groups);
    }
    let mut collections = Vec::new();
    for collection in &store.collections {
        if is_implicit_all(store, collection) {
            continue;
        }
        collections.push(seed_collection(seed, collection));
    }
    for group in &store.selling_plan_groups {
        seed_selling_plan_group(seed, group);
    }
    for card in &store.gift_cards {
        seed_gift_card(seed, card);
    }
    let published: Vec<String> = store
        .products
        .iter()
        .chain(&store.unlisted_products)
        .filter(|product| product.published)
        .map(product_gid)
        .collect();
    seed.insert(json!({
        "__typename": "Publication",
        "id": online_store_publication_gid(),
        "name": "Online Store",
        "products": published,
        "collections": collections,
    }));
    let products: Vec<String> = store
        .products
        .iter()
        .chain(&store.unlisted_products)
        .map(product_gid)
        .collect();
    for product in &products {
        refresh_product(&mut seed.state, product);
    }
}

fn seed_location(seed: &mut Seed, location: &Location) {
    let id = location_gid(location);
    let created = Seed::epoch_string();
    let metafields = seed.metafields(&id, "LOCATION", &created, &location.metafields);
    let address = &location.address;
    let mut record = value::object(json!({
        "__typename": "Location",
        "id": id,
        "name": location.name,
        "address": {
            "address1": text(&address.address1),
            "address2": text(&address.address2),
            "city": text(&address.city),
            "province": text(&address.province),
            "provinceCode": text(&address.province_code),
            "country": text(&address.country),
            "countryCode": text(&address.country_code.to_ascii_uppercase()),
            "zip": text(&address.zip),
            "phone": text(&address.phone),
            "latitude": location.latitude,
            "longitude": location.longitude,
            "formatted": value::formatted(address),
        },
        "isActive": true,
        "fulfillsOnlineOrders": true,
        "isFulfillmentService": false,
        "hasActiveInventory": true,
        "shipsInventory": true,
        "createdAt": created,
        "updatedAt": created,
        "inventoryLevels": [],
        "metafields": metafields,
    }));
    if location.pick_up_enabled
        && let Some(time) = pickup_time(&location.pick_up_time)
    {
        record.insert(
            "localPickupSettingsV2".to_string(),
            json!({"pickupTime": time, "instructions": ""}),
        );
    }
    seed.insert_record(record);
}

/// The `DeliveryLocalPickupTime` of the text lsf-core keeps ("Usually ready in 24 hours", the
/// way Shopify words each value).
pub fn pickup_time(text: &str) -> Option<&'static str> {
    let text = text.to_lowercase();
    [
        ("1 hour", "ONE_HOUR"),
        ("2 hours", "TWO_HOURS"),
        ("4 hours", "FOUR_HOURS"),
        ("24 hours", "TWENTY_FOUR_HOURS"),
        ("2-4 days", "TWO_TO_FOUR_DAYS"),
        ("2 to 4 days", "TWO_TO_FOUR_DAYS"),
        ("5 or more days", "FIVE_OR_MORE_DAYS"),
        ("5+ days", "FIVE_OR_MORE_DAYS"),
    ]
    .iter()
    .find(|(words, _)| {
        // A number that starts the words: "4 hours" is not in "24 hours".
        text.match_indices(words).any(|(at, _)| {
            !text[..at]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_digit())
        })
    })
    .map(|(_, time)| *time)
}

fn seed_product(
    seed: &mut Seed,
    product: &Product,
    locations: &[Location],
    selling_plan_groups: &[String],
) {
    let store = seed.store;
    let id = product_gid(product);
    let created = date_time(product.created_at);
    let updated = date_time(product.updated_at);

    let mut options = Vec::new();
    for option in &product.options {
        let option_id = option_gid(product.id, &option.name);
        let mut values = Vec::new();
        for (index, name) in option.values.iter().enumerate() {
            let value_id = option_value_gid(product.id, &option.name, name);
            let swatch = option
                .swatches
                .get(index)
                .and_then(Option::as_ref)
                .map(|swatch| {
                    json!({
                        "color": swatch.color,
                        "image": swatch.image.as_ref().map(|image| seed.file(&image.src)),
                    })
                });
            let has_variants = product.variants.iter().any(|variant| {
                variant.options.get(option.position.saturating_sub(1)) == Some(name)
            });
            seed.insert(json!({
                "__typename": "ProductOptionValue",
                "id": value_id,
                "name": name,
                "hasVariants": has_variants,
                "swatch": swatch,
            }));
            values.push(value_id);
        }
        seed.insert(json!({
            "__typename": "ProductOption",
            "id": option_id,
            "name": option.name,
            "position": option.position,
            "values": option.values,
            "optionValues": values,
            "createdAt": created,
        }));
        options.push(option_id);
    }

    let mut media = Vec::new();
    for item in &product.media {
        media.push(seed_media(seed, item, &created, &updated));
    }

    let mut variants = Vec::new();
    for (position, variant) in product.variants.iter().enumerate() {
        variants.push(seed_variant(
            seed,
            product,
            variant,
            position,
            locations,
            selling_plan_groups,
        ));
    }

    let images: Vec<Value> = product
        .media
        .iter()
        .filter(|media| matches!(media.kind, MediaKind::Image))
        .filter_map(|media| media.preview.as_ref())
        .map(|image| seed.image(image, "ImageSource"))
        .collect();
    let collections: Vec<String> = product
        .collections
        .iter()
        .map(|index| &store.collections[*index])
        .filter(|collection| !is_implicit_all(store, collection))
        .map(collection_gid)
        .collect();
    let groups: Vec<&String> = product
        .selling_plan_groups
        .iter()
        .filter_map(|index| selling_plan_groups.get(*index))
        .collect();
    let metafields = seed.metafields(&id, "PRODUCT", &created, &product.metafields);
    seed.insert(json!({
        "__typename": "Product",
        "id": id,
        "title": product.title,
        "handle": product.handle,
        "descriptionHtml": product.description,
        "bodyHtml": product.description,
        "description": value::plain_text(&product.description),
        "vendor": product.vendor,
        "productType": product.product_type,
        "tags": product.tags,
        "status": product.status.admin_name(),
        "createdAt": created,
        "updatedAt": updated,
        // When the product was published on the Online Store: none when it is not.
        "publishedAt": product.published.then(|| date_time(product.published_at)),
        "templateSuffix": optional(product.template_suffix.as_deref()),
        "giftCardTemplateSuffix": null,
        "isGiftCard": product.gift_card,
        "requiresSellingPlan": product.requires_selling_plan,
        "hasVariantsThatRequiresComponents": false,
        "options": options,
        "variants": variants,
        "media": media,
        "images": images,
        "seo": {"title": null, "description": null},
        // Shopify gives no URL to a product that is not on the Online Store.
        "onlineStoreUrl": product
            .listed()
            .then(|| seed.url(&format!("/products/{}", product.handle))),
        "onlineStorePreviewUrl": seed.url(&format!("/products/{}", product.handle)),
        "collections": collections,
        "sellingPlanGroups": groups,
        "metafields": metafields,
    }));
}

fn seed_media(seed: &mut Seed, media: &Media, created: &str, updated: &str) -> String {
    let id = media_gid(media);
    let preview = media
        .preview
        .as_ref()
        .map(|image| json!({"image": seed.image(image, "ImageSource"), "status": "READY"}));
    let mut record = value::object(json!({
        "__typename": media_type(media),
        "id": id,
        "alt": text(&media.alt),
        "status": "READY",
        "fileStatus": "READY",
        "preview": preview,
        "createdAt": created,
        "updatedAt": updated,
        "fileErrors": [],
        "mediaErrors": [],
        "mediaWarnings": [],
    }));
    let source = |source: &lsf_core::store::MediaSource| {
        json!({
            "url": seed.file_url(&source.url),
            "mimeType": source.mime_type,
            "format": source.format,
            "width": source.width,
            "height": source.height,
        })
    };
    let fields = match &media.kind {
        MediaKind::Image => json!({
            "mediaContentType": "IMAGE",
            "image": media.preview.as_ref().map(|image| seed.image(image, "ImageSource")),
            "mimeType": media.preview.as_ref().map(|image| mime_type(&image.src)),
            "originalSource": media.preview.as_ref().map(|image| json!({"url": seed.file_url(&image.src)})),
        }),
        MediaKind::Video { sources, duration } => json!({
            "mediaContentType": "VIDEO",
            "duration": duration,
            "filename": sources.first().map_or("", |source| file_name(&source.url)),
            "sources": sources.iter().map(source).collect::<Vec<_>>(),
            "originalSource": sources.first().map(source),
        }),
        MediaKind::ExternalVideo { host, external_id } => {
            let (host, embed, origin) = match host {
                VideoHost::Youtube => (
                    "YOUTUBE",
                    format!("https://www.youtube.com/embed/{external_id}"),
                    format!("https://www.youtube.com/watch?v={external_id}"),
                ),
                VideoHost::Vimeo => (
                    "VIMEO",
                    format!("https://player.vimeo.com/video/{external_id}"),
                    format!("https://vimeo.com/{external_id}"),
                ),
            };
            json!({
                "mediaContentType": "EXTERNAL_VIDEO",
                "host": host,
                "embedUrl": embed,
                "originUrl": origin,
            })
        }
        MediaKind::Model { sources } => {
            let model_source = |source: &lsf_core::store::MediaSource| {
                json!({
                    "url": seed.file_url(&source.url),
                    "mimeType": source.mime_type,
                    "format": source.format,
                })
            };
            json!({
                "mediaContentType": "MODEL_3D",
                "filename": sources.first().map_or("", |source| file_name(&source.url)),
                "sources": sources.iter().map(model_source).collect::<Vec<_>>(),
                "originalSource": sources.first().map(model_source),
            })
        }
    };
    record.extend(value::object(fields));
    seed.insert_record(record)
}

/// The file name at the end of a path or URL.
fn file_name(path: &str) -> &str {
    let path = path.split(['?', '#']).next().unwrap_or(path);
    path.rsplit('/').next().unwrap_or(path)
}

/// The MIME type of a file, from its extension.
pub fn mime_type(path: &str) -> String {
    let extension = file_name(path)
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "glb" => "model/gltf-binary",
        "usdz" => "model/vnd.usdz+zip",
        "pdf" => "application/pdf",
        "json" => "application/json",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// The kind of file a path of `files/` is, by its extension: the GID type its record has and
/// the kind lsf-core's `stable_id` takes for it (the ids Liquid shows for a `file_reference`).
pub fn file_kind(path: &str) -> (&'static str, &'static str) {
    let extension = file_name(path)
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "avif" | "svg" | "bmp" => ("MediaImage", "image"),
        "mp4" | "mov" | "webm" | "m4v" => ("Video", "video"),
        _ => ("GenericFile", "file"),
    }
}

/// The GID of a file of `files/`: `MediaImage`, `Video` or `GenericFile`, by its extension.
pub fn file_gid(path: &str) -> String {
    let (type_name, kind) = file_kind(path);
    gid(type_name, stable_id(kind, path))
}

fn seed_variant(
    seed: &mut Seed,
    product: &Product,
    variant: &Variant,
    position: usize,
    locations: &[Location],
    selling_plan_groups: &[String],
) -> String {
    let store = seed.store;
    let id = variant_gid(variant);
    let created = date_time(product.created_at);
    let updated = date_time(product.updated_at);
    let item = inventory_item_gid(variant.id);

    // The locations that stock the variant, and whether it is in stock at each: what the data
    // says, or every location (the one lsf adds when the data has none).
    let stocked: Vec<(&Location, bool)> = if store.locations.is_empty() {
        locations
            .iter()
            .map(|location| (location, variant.available))
            .collect()
    } else {
        variant
            .store_availabilities
            .iter()
            .filter_map(|(index, in_stock)| Some((locations.get(*index)?, *in_stock)))
            .collect()
    };
    let quantities = split_quantity(
        variant.inventory_quantity,
        &stocked
            .iter()
            .map(|(_, in_stock)| *in_stock)
            .collect::<Vec<_>>(),
    );
    let mut levels = Vec::new();
    for ((location, _), quantity) in stocked.iter().zip(quantities) {
        let level = inventory_level_gid(variant.id, location.id);
        let quantities: Vec<Value> = QUANTITY_NAMES
            .iter()
            .map(|name| {
                let amount = match *name {
                    "available" | "on_hand" => quantity,
                    _ => 0,
                };
                json!({
                    "id": gid(
                        "InventoryQuantity",
                        stable_id("inventory_quantity", &format!("{level}/{name}")),
                    ),
                    "name": name,
                    "quantity": amount,
                    "updatedAt": updated,
                })
            })
            .collect();
        seed.insert(json!({
            "__typename": "InventoryLevel",
            "id": level,
            "item": item,
            "location": location_gid(location),
            "quantities": quantities,
            "isActive": true,
            "canDeactivate": true,
            "createdAt": created,
            "updatedAt": updated,
        }));
        if let Some(Value::Array(location_levels)) = seed
            .state
            .get_mut(&location_gid(location))
            .and_then(|record| record.get_mut("inventoryLevels"))
        {
            location_levels.push(Value::String(level.clone()));
        }
        levels.push(level);
    }

    let weight = json!({
        "unit": weight_unit(&variant.weight_unit),
        "value": lsf_core::drops::product::weight_in_unit(variant.weight, &variant.weight_unit),
    });
    seed.insert(json!({
        "__typename": "InventoryItem",
        "id": item,
        "sku": text(&variant.sku),
        "tracked": variant.inventory_tracked,
        "requiresShipping": variant.requires_shipping,
        "measurement": {
            "id": gid("InventoryItemMeasurement", inventory_item_id(variant.id)),
            "weight": weight,
        },
        "inventoryLevels": levels,
        "variant": id,
        "variants": [id],
        "createdAt": created,
        "updatedAt": updated,
    }));

    let selected_options: Vec<Value> = product
        .options
        .iter()
        .zip(&variant.options)
        .map(|(option, value)| {
            json!({
                "name": option.name,
                "value": value,
                "optionValue": option_value_gid(product.id, &option.name, value),
            })
        })
        .collect();
    let media = variant
        .media_index
        .and_then(|index| product.media.get(index));
    let groups: Vec<&String> = product
        .selling_plan_groups
        .iter()
        .filter_map(|index| selling_plan_groups.get(*index))
        .collect();
    let metafields = seed.metafields(&id, "PRODUCTVARIANT", &created, &variant.metafields);
    seed.insert(json!({
        "__typename": "ProductVariant",
        "id": id,
        "title": variant.title,
        "displayName": format!("{} - {}", product.title, variant.title),
        "sku": text(&variant.sku),
        "barcode": text(&variant.barcode),
        "barcodes": if variant.barcode.is_empty() {
            json!([])
        } else {
            json!([{"value": variant.barcode}])
        },
        "price": money(variant.price),
        "compareAtPrice": variant.compare_at_price.map(money),
        "position": position + 1,
        "selectedOptions": selected_options,
        "inventoryItem": item,
        "inventoryPolicy": match variant.inventory_policy {
            InventoryPolicy::Deny => "DENY",
            InventoryPolicy::Continue => "CONTINUE",
        },
        "inventoryQuantity": variant.inventory_quantity,
        "availableForSale": variant.available,
        "taxable": variant.taxable,
        "requiresComponents": false,
        "product": product_gid(product),
        "media": media.map(media_gid).into_iter().collect::<Vec<_>>(),
        "image": media
            .filter(|media| matches!(media.kind, MediaKind::Image))
            .and_then(|media| media.preview.as_ref())
            .map(|image| seed.image(image, "ImageSource")),
        "unitPrice": variant.unit_price.map(|cents| seed.money_v2(cents)),
        "unitPriceMeasurement": variant.unit_price_measurement.as_ref().map(|measurement| {
            json!({
                "measuredType": measured_type(&measurement.measured_type),
                "quantityValue": measurement.quantity_value,
                "quantityUnit": measured_unit(&measurement.quantity_unit),
                "referenceValue": measurement.reference_value.round() as i64,
                "referenceUnit": measured_unit(&measurement.reference_unit),
            })
        }),
        "showUnitPrice": variant.unit_price.is_some(),
        "sellingPlanGroups": groups,
        "createdAt": created,
        "updatedAt": updated,
        "metafields": metafields,
    }));
    id
}

/// How the stock of a variant is spread over the locations that stock it: evenly over those
/// where it is in stock (the first ones take the remainder), none where it is not. A stock
/// below zero, or one no location has in stock, is at the first location.
pub fn split_quantity(total: i64, in_stock: &[bool]) -> Vec<i64> {
    let mut quantities = vec![0; in_stock.len()];
    let holders: Vec<usize> = (0..in_stock.len()).filter(|i| in_stock[*i]).collect();
    if quantities.is_empty() {
        return quantities;
    }
    if total <= 0 || holders.is_empty() {
        quantities[holders.first().copied().unwrap_or(0)] = total;
        return quantities;
    }
    let count = holders.len() as i64;
    for (rank, index) in holders.iter().enumerate() {
        quantities[*index] = total / count + i64::from((rank as i64) < total % count);
    }
    quantities
}

/// The `WeightUnit` of the unit lsf-core keeps (`g`, `kg`, `oz`, `lb`).
pub fn weight_unit(unit: &str) -> &'static str {
    match unit {
        "g" => "GRAMS",
        "oz" => "OUNCES",
        "lb" => "POUNDS",
        _ => "KILOGRAMS",
    }
}

fn measured_type(kind: &str) -> &'static str {
    match kind.to_ascii_lowercase().as_str() {
        "weight" => "WEIGHT",
        "volume" => "VOLUME",
        "length" => "LENGTH",
        "area" => "AREA",
        "count" => "COUNT",
        _ => "UNKNOWN",
    }
}

fn measured_unit(unit: &str) -> &'static str {
    match unit.to_ascii_lowercase().as_str() {
        "mg" => "MG",
        "g" => "G",
        "kg" => "KG",
        "oz" => "OZ",
        "lb" => "LB",
        "ml" => "ML",
        "cl" => "CL",
        "l" => "L",
        "m3" => "M3",
        "floz" | "fl oz" => "FLOZ",
        "pt" => "PT",
        "qt" => "QT",
        "gal" => "GAL",
        "mm" => "MM",
        "cm" => "CM",
        "m" => "M",
        "in" => "IN",
        "ft" => "FT",
        "yd" => "YD",
        "m2" => "M2",
        "ft2" => "FT2",
        "item" => "ITEM",
        _ => "UNKNOWN",
    }
}

fn seed_collection(seed: &mut Seed, collection: &Collection) -> String {
    let store = seed.store;
    let id = collection_gid(collection);
    let created = date_time(collection.published_at);
    let metafields = seed.metafields(&id, "COLLECTION", &created, &collection.metafields);
    let products: Vec<String> = collection
        .products
        .iter()
        .map(|index| product_gid(&store.products[*index]))
        .collect();
    seed.insert(json!({
        "__typename": "Collection",
        "id": id,
        "title": collection.title,
        "handle": collection.handle,
        "descriptionHtml": collection.description,
        "description": value::plain_text(&collection.description),
        "image": collection.image.as_ref().map(|image| seed.image(image, "CollectionImage")),
        "products": products,
        "sortOrder": match collection.sort_order {
            SortOrder::Manual => "MANUAL",
            SortOrder::BestSelling => "BEST_SELLING",
            SortOrder::TitleAscending => "ALPHA_ASC",
            SortOrder::TitleDescending => "ALPHA_DESC",
            SortOrder::PriceAscending => "PRICE_ASC",
            SortOrder::PriceDescending => "PRICE_DESC",
            SortOrder::CreatedAscending => "CREATED",
            SortOrder::CreatedDescending => "CREATED_DESC",
        },
        "ruleSet": null,
        "templateSuffix": optional(collection.template_suffix.as_deref()),
        "seo": {"title": null, "description": null},
        "createdAt": created,
        "updatedAt": date_time(collection.updated_at),
        "metafields": metafields,
    }))
}

fn seed_selling_plan_group(seed: &mut Seed, group: &SellingPlanGroup) {
    let store = seed.store;
    let id = selling_plan_group_gid(group);
    let created = Seed::epoch_string();
    let index = store
        .selling_plan_groups
        .iter()
        .position(|candidate| candidate.id == group.id);
    let products: Vec<String> = store
        .products
        .iter()
        .filter(|product| index.is_some_and(|index| product.selling_plan_groups.contains(&index)))
        .map(product_gid)
        .collect();
    let mut plans = Vec::new();
    for (position, plan) in group.plans.iter().enumerate() {
        let plan_id = selling_plan_gid(plan);
        seed.insert(selling_plan(seed, plan, position, &created));
        plans.push(plan_id);
    }
    seed.insert(json!({
        "__typename": "SellingPlanGroup",
        "id": id,
        "name": group.name,
        "appId": group.app_id,
        "merchantCode": lsf_core::util::handleize(&group.name),
        "options": group.options,
        "createdAt": created,
        "products": products,
        "productVariants": [],
        "sellingPlans": plans,
    }));
}

/// A selling plan. The data says how the plan prices, charges and whether it recurs; how often
/// it bills and delivers is read from its options and name (`Every 3 months`), a month when
/// they do not say.
fn selling_plan(seed: &Seed, plan: &SellingPlan, position: usize, created: &str) -> Value {
    let adjustment_value = |adjustment: Adjustment| match adjustment {
        Adjustment::Percentage(percent) => (
            "PERCENTAGE",
            json!({"__typename": "SellingPlanPricingPolicyPercentageValue", "percentage": percent as f64}),
        ),
        Adjustment::FixedAmount(cents) => (
            "FIXED_AMOUNT",
            with_typename(seed.money_v2(cents), "MoneyV2"),
        ),
        Adjustment::Price(cents) => ("PRICE", with_typename(seed.money_v2(cents), "MoneyV2")),
    };
    let mut pricing_policies = Vec::new();
    for (index, adjustment) in plan.price_adjustments.iter().enumerate() {
        let (kind, value) = adjustment_value(adjustment.adjustment);
        let mut policy = json!({
            "__typename": "SellingPlanFixedPricingPolicy",
            "adjustmentType": kind,
            "adjustmentValue": value,
            "createdAt": created,
        });
        if index > 0 {
            policy["__typename"] = json!("SellingPlanRecurringPricingPolicy");
            policy["afterCycle"] = json!(
                plan.price_adjustments[index - 1]
                    .order_count
                    .map_or(0, i64::from)
            );
        }
        pricing_policies.push(policy);
    }
    let described = std::iter::once(plan.name.as_str())
        .chain(plan.options.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ");
    let (interval, count) = interval_of(&described).unwrap_or(("MONTH", 1));
    let (billing, delivery, category) = if plan.recurring_deliveries {
        (
            json!({
                "__typename": "SellingPlanRecurringBillingPolicy",
                "interval": interval,
                "intervalCount": count,
                "anchors": [],
                "createdAt": created,
            }),
            json!({
                "__typename": "SellingPlanRecurringDeliveryPolicy",
                "interval": interval,
                "intervalCount": count,
                "anchors": [],
                "intent": "FULFILLMENT_BEGIN",
                "preAnchorBehavior": "ASAP",
                "createdAt": created,
            }),
            "SUBSCRIPTION",
        )
    } else {
        let (charge, full) = match plan.checkout_charge {
            CheckoutCharge::Percentage(percent) => (
                json!({
                    "type": "PERCENTAGE",
                    "value": {"__typename": "SellingPlanCheckoutChargePercentageValue", "percentage": percent as f64},
                }),
                percent >= 100,
            ),
            CheckoutCharge::Price(cents) => (
                json!({"type": "PRICE", "value": with_typename(seed.money_v2(cents), "MoneyV2")}),
                false,
            ),
        };
        (
            json!({
                "__typename": "SellingPlanFixedBillingPolicy",
                "checkoutCharge": charge,
                "remainingBalanceChargeTrigger": if full { "NO_REMAINING_BALANCE" } else { "ON_FULFILLMENT" },
            }),
            json!({
                "__typename": "SellingPlanFixedDeliveryPolicy",
                "anchors": [],
                "fulfillmentTrigger": "ASAP",
                "intent": "FULFILLMENT_BEGIN",
                "preAnchorBehavior": "ASAP",
            }),
            if full { "OTHER" } else { "PRE_ORDER" },
        )
    };
    json!({
        "__typename": "SellingPlan",
        "id": selling_plan_gid(plan),
        "name": plan.name,
        "description": plan.description,
        "options": plan.options,
        "position": position + 1,
        "category": category,
        "billingPolicy": billing,
        "deliveryPolicy": delivery,
        "pricingPolicies": pricing_policies,
        "createdAt": created,
        "metafields": [],
    })
}

fn with_typename(mut value: Value, type_name: &str) -> Value {
    if let Value::Object(map) = &mut value {
        map.insert("__typename".to_string(), json!(type_name));
    }
    value
}

/// `every 3 months`, `every week`, `monthly`: the `SellingPlanInterval` and its count.
fn interval_of(text: &str) -> Option<(&'static str, u32)> {
    let text = text.to_lowercase();
    let words: Vec<&str> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    let unit = |word: &str| match word.trim_end_matches('s') {
        "day" => Some("DAY"),
        "week" => Some("WEEK"),
        "month" => Some("MONTH"),
        "year" => Some("YEAR"),
        _ => None,
    };
    for (index, word) in words.iter().enumerate() {
        if *word == "every" {
            let next = words.get(index + 1)?;
            if let Some(unit) = unit(next) {
                return Some((unit, 1));
            }
            let count = match *next {
                "other" => Some(2),
                number => number.parse::<u32>().ok().filter(|count| *count > 0),
            };
            if let (Some(count), Some(unit)) = (count, words.get(index + 2).and_then(|w| unit(w))) {
                return Some((unit, count));
            }
        }
        let adverb = match *word {
            "daily" => Some("DAY"),
            "weekly" => Some("WEEK"),
            "monthly" => Some("MONTH"),
            "yearly" | "annually" => Some("YEAR"),
            _ => None,
        };
        if let Some(unit) = adverb {
            return Some((unit, 1));
        }
    }
    None
}

fn seed_gift_card(seed: &mut Seed, card: &GiftCard) {
    let store = seed.store;
    let now = seed.state.now();
    let id = gift_card_gid(card);
    let created = Seed::epoch_string();
    let currency = card.currency.to_ascii_uppercase();
    let expired = card.expires_on.is_some_and(|expires| expires < now);
    let last = card
        .code
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    let recipient = card.recipient.as_ref().and_then(|recipient| {
        let customer = store.customer_by_email(&recipient.email)?;
        Some(json!({
            "recipient": gid("Customer", customer.id),
            "preferredName": text(&recipient.nickname),
            "message": text(&card.message),
            "sendNotificationAt": card.send_on.map(date_time),
        }))
    });
    seed.insert(json!({
        "__typename": "GiftCard",
        "id": id,
        "balance": value::money_v2(card.balance, &currency),
        "initialValue": value::money_v2(card.initial_value, &currency),
        "enabled": card.enabled,
        // The day the data gives, in the shop's time zone: lsf-core keeps its start.
        "expiresOn": card
            .expires_on
            .map(|expires| date(expires.with_timezone(&store.shop.timezone).naive_local().and_utc())),
        "isRedeemable": card.enabled && !expired && card.balance > 0,
        "lastCharacters": last,
        "maskedCode": format!("•••• •••• •••• {last}"),
        "customer": card.customer.map(|customer| gid("Customer", customer)),
        "recipientAttributes": recipient,
        "templateSuffix": optional(card.template_suffix.as_deref()),
        "createdAt": created,
        "updatedAt": created,
    }));
}

/// Recomputes what a product derives from its variants, media and inventory: its price ranges,
/// `totalInventory`, `tracksInventory`, `hasOutOfStockVariants`, `hasOnlyDefaultVariant`,
/// `featuredMedia` and `featuredImage`, and the `inventoryQuantity` of each variant (the
/// `available` quantity of its levels). The seed runs it on every product; a mutation that
/// changes a variant, a level or the media of a product runs it after.
pub fn refresh_product(state: &mut ShopState, product: &str) {
    let Some(record) = state.get(product) else {
        return;
    };
    let variants = gids(record.get("variants"));
    let media = gids(record.get("media"));
    let options = gids(record.get("options"));
    let currency = state
        .shop()
        .and_then(|shop| shop.get("currencyCode"))
        .and_then(Value::as_str)
        .unwrap_or("USD")
        .to_string();

    let mut prices = Vec::new();
    let mut compare_at = Vec::new();
    let mut total = 0;
    let mut tracked = false;
    let mut out_of_stock = false;
    let mut default_only = variants.len() == 1;
    let mut quantities = Vec::new();
    for variant in &variants {
        let Some(record) = state.get(variant) else {
            continue;
        };
        if let Some(price) = cents_of(record.get("price")) {
            prices.push(price);
        }
        if let Some(price) = cents_of(record.get("compareAtPrice")) {
            compare_at.push(price);
        }
        default_only &= record.get("title").and_then(Value::as_str) == Some("Default Title");
        let item = record
            .get("inventoryItem")
            .and_then(Value::as_str)
            .and_then(|item| state.get(item));
        let item_tracked = item
            .and_then(|item| item.get("tracked"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let available: i64 = item
            .map(|item| gids(item.get("inventoryLevels")))
            .unwrap_or_default()
            .iter()
            .filter_map(|level| state.get(level))
            .map(available_of)
            .sum();
        tracked |= item_tracked;
        out_of_stock |= item_tracked && available <= 0;
        total += available;
        quantities.push((variant.clone(), available));
    }
    let range = |values: &[i64], min: &str, max: &str| {
        json!({
            min: value::money_v2(values.iter().copied().min().unwrap_or(0), &currency),
            max: value::money_v2(values.iter().copied().max().unwrap_or(0), &currency),
        })
    };
    let price_range = range(&prices, "minVariantPrice", "maxVariantPrice");
    let compare_at_range = (!compare_at.is_empty()).then(|| {
        range(
            &compare_at,
            "minVariantCompareAtPrice",
            "maxVariantCompareAtPrice",
        )
    });
    let featured = media.first().cloned();
    let featured_image = media
        .iter()
        .filter_map(|media| state.get(media))
        .find_map(|media| media.get("image").filter(|image| !image.is_null()).cloned());
    let default_only = default_only
        && options.len() == 1
        && state
            .get(&options[0])
            .and_then(|option| option.get("name"))
            .and_then(Value::as_str)
            == Some("Title");

    for (variant, available) in quantities {
        if let Some(record) = state.get_mut(&variant) {
            record.insert("inventoryQuantity".to_string(), json!(available));
        }
    }
    if let Some(record) = state.get_mut(product) {
        record.insert("priceRangeV2".to_string(), price_range.clone());
        record.insert("priceRange".to_string(), price_range);
        record.insert(
            "compareAtPriceRange".to_string(),
            compare_at_range.unwrap_or(Value::Null),
        );
        record.insert("totalInventory".to_string(), json!(total));
        record.insert("tracksInventory".to_string(), json!(tracked));
        record.insert("hasOutOfStockVariants".to_string(), json!(out_of_stock));
        record.insert("hasOnlyDefaultVariant".to_string(), json!(default_only));
        record.insert("featuredMedia".to_string(), json!(featured));
        record.insert(
            "featuredImage".to_string(),
            featured_image.unwrap_or(Value::Null),
        );
    }
}

/// What a mutation runs after it changed a product's variants, options, media or fields: the
/// variants' positions, titles and display names, whether each option value has variants, what
/// [`refresh_product`] derives, and the product's membership of the smart collections.
pub fn product_changed(state: &mut ShopState, product: &str) {
    let Some(record) = state.get(product) else {
        return;
    };
    let title = record
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let variants = gids(record.get("variants"));
    let options = gids(record.get("options"));
    let mut used: Vec<String> = Vec::new();
    for (index, variant) in variants.iter().enumerate() {
        let Some(record) = state.get(variant) else {
            continue;
        };
        let selected: Vec<(String, Option<String>)> = record
            .get("selectedOptions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|option| {
                (
                    option
                        .get("value")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    option
                        .get("optionValue")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                )
            })
            .collect();
        used.extend(selected.iter().filter_map(|(_, value)| value.clone()));
        let variant_title = selected
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(" / ");
        let display = format!("{title} - {variant_title}");
        let position = index as i64 + 1;
        let unchanged = record.get("position").and_then(Value::as_i64) == Some(position)
            && record.get("title").and_then(Value::as_str) == Some(variant_title.as_str())
            && record.get("displayName").and_then(Value::as_str) == Some(display.as_str());
        if !unchanged && let Some(record) = state.get_mut(variant) {
            record.insert("position".into(), json!(position));
            record.insert("title".into(), json!(variant_title));
            record.insert("displayName".into(), json!(display));
        }
    }
    for option in &options {
        for value in state
            .get(option)
            .map(|record| gids(record.get("optionValues")))
            .unwrap_or_default()
        {
            let has = used.contains(&value);
            let known = state
                .get(&value)
                .and_then(|record| record.get("hasVariants"))
                .and_then(Value::as_bool);
            if known != Some(has)
                && let Some(record) = state.get_mut(&value)
            {
                record.insert("hasVariants".into(), json!(has));
            }
        }
    }
    refresh_product(state, product);
    publications::refresh_online_store_url(state, product);
    collections::apply_rules(state, product);
}

/// The `available` quantity of an inventory level record.
pub fn available_of(level: &Record) -> i64 {
    level
        .get("quantities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|quantity| quantity.get("name").and_then(Value::as_str) == Some("available"))
        .and_then(|quantity| quantity.get("quantity"))
        .and_then(Value::as_i64)
        .unwrap_or(0)
}

/// The GIDs of a list field of a record.
pub fn gids(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// The cents of a `Money` or `Decimal` string.
pub fn cents_of(value: Option<&Value>) -> Option<i64> {
    let text = value?.as_str()?;
    lsf_core::store::model::Money::Decimal(text.to_string()).cents()
}

/// The custom resolvers and the mutations of the catalog: see "Writing a resolver or a mutation"
/// in `crates/admin/docs/design.md`.
pub fn register(registry: &mut crate::resolve::Registry) {
    products::register(registry);
    product_set::register(registry);
    variants::register(registry);
    options::register(registry);
    media::register(registry);
    collections::register(registry);
    inventory::register(registry);
    locations::register(registry);
    selling_plans::register(registry);
    publications::register(registry);
    gift_cards::register(registry);
    gift_card_extras::register(registry);
    queries::register(registry);
    registry.field("InventoryLevel", "quantities", level_quantities);
}

/// `InventoryLevel.quantities(names:)`: the quantities of the names asked, in their order (the
/// level keeps all eight).
fn level_quantities(
    r: &crate::resolve::ShopResolver<'_>,
    ctx: &crate::graphql::FieldContext<'_>,
    object: &crate::resolve::Obj,
) -> Result<crate::graphql::Output<crate::resolve::Obj>, crate::graphql::FieldError> {
    let quantities = r.items_of(object, "quantities");
    let names: Vec<&str> = ctx
        .arg("names")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let asked: Vec<Value> = names
        .iter()
        .filter_map(|name| {
            quantities
                .iter()
                .find(|quantity| quantity.get("name").and_then(Value::as_str) == Some(name))
                .cloned()
        })
        .collect();
    r.convert(ctx, &Value::Array(asked), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spreads_stock_over_locations() {
        assert_eq!(split_quantity(10, &[true, true, true]), vec![4, 3, 3]);
        assert_eq!(split_quantity(10, &[false, true]), vec![0, 10]);
        assert_eq!(split_quantity(5, &[false, false]), vec![5, 0]);
        assert_eq!(split_quantity(-2, &[true, true]), vec![-2, 0]);
        assert_eq!(split_quantity(3, &[]), Vec::<i64>::new());
    }

    #[test]
    fn reads_intervals() {
        assert_eq!(
            interval_of("Deliver every month, 10% off"),
            Some(("MONTH", 1))
        );
        assert_eq!(interval_of("Every 3 months"), Some(("MONTH", 3)));
        assert_eq!(interval_of("every other week"), Some(("WEEK", 2)));
        assert_eq!(interval_of("Weekly box"), Some(("WEEK", 1)));
        assert_eq!(interval_of("Pre-order"), None);
    }

    #[test]
    fn knows_files() {
        assert_eq!(file_kind("a/b.JPG"), ("MediaImage", "image"));
        assert_eq!(file_kind("clip.mov"), ("Video", "video"));
        assert_eq!(file_kind("manual.pdf"), ("GenericFile", "file"));
        assert_eq!(mime_type("manual.pdf"), "application/pdf");
        assert_eq!(
            pickup_time("Usually ready in 24 hours"),
            Some("TWENTY_FOUR_HOURS")
        );
        assert_eq!(pickup_time("Usually ready in 2 hours"), Some("TWO_HOURS"));
        assert_eq!(pickup_time("Usually ready in 4 hours"), Some("FOUR_HOURS"));
    }
}
