//! The catalog's queries that need more than reading records: the sort keys and the search
//! filters of `products`, `productVariants`, `collections`, `giftCards`, `sellingPlanGroups`
//! (the filters `schemas/2026-10/search.json` documents for them), `productTags`,
//! `productTypes`, `productVendors`, and the fields that take an argument
//! (`Product.inCollection`, `InventoryItem.inventoryLevel`, `Location.inventoryLevel`).
//!
//! What the generic rules read already (`title`, `product_type`, `vendor`, `status`, `tag`,
//! `handle`, the dates, `collection_id`, `variant_id`, …) is not repeated here. The filters lsf
//! cannot read (`category_id`, `bundles`, `delivery_profile_id`, …) are ignored with a note in
//! `extensions.lsf.notes`, as the resolver does for any.
//!
//! Sort keys: `INVENTORY_TOTAL` is `totalInventory`; a variant's `FULL_TITLE` is its product's
//! title then its own, `NAME` its display name, `INVENTORY_MANAGEMENT` whether it is tracked,
//! `INVENTORY_LEVELS_AVAILABLE` its stock; the products of a collection sort by `PRICE` (the
//! lowest variant price) and `CREATED`. `BEST_SELLING`, `POPULAR` and `RELEVANCE` have no data
//! behind them: they keep the order of the ids.

use serde_json::{Value, json};

use super::records::{default_location, field};
use super::{cents_of, gids};
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::sort::SortValue;
use crate::resolve::{Obj, Registry, ShopResolver};
use crate::search::SearchValue;
use crate::state::{Record, parse_gid};

pub fn register(registry: &mut Registry) {
    registry
        // Products.
        .sort_key("Product", "INVENTORY_TOTAL", |r, item| {
            SortValue::of(
                r.item_map(item)
                    .and_then(|product| product.get("totalInventory")),
            )
        })
        .sort_key("Product", "PRICE", |r, item| {
            SortValue::of(
                r.item_map(item)
                    .and_then(|product| product.get("priceRangeV2"))
                    .and_then(|range| range.get("minVariantPrice")),
            )
        })
        .sort_key("Product", "CREATED", |r, item| {
            SortValue::of(
                r.item_map(item)
                    .and_then(|product| product.get("createdAt")),
            )
        })
        .search_field("Product", "sku", |r, _item, product| {
            Some(variant_strings(r, product, |variant| {
                text_of(variant, "sku")
            }))
        })
        .search_field("Product", "barcode", |r, _item, product| {
            Some(variant_strings(r, product, |variant| {
                text_of(variant, "barcode")
            }))
        })
        .search_field("Product", "variant_title", |r, _item, product| {
            Some(
                variants_of(r, product)
                    .iter()
                    .filter_map(|variant| field(variant, "title"))
                    .map(|title| SearchValue::Text(title.to_string()))
                    .collect(),
            )
        })
        .search_field("Product", "price", |r, _item, product| {
            Some(
                variants_of(r, product)
                    .iter()
                    .filter_map(|variant| cents_of(variant.get("price")))
                    .map(|cents| SearchValue::Number(cents as f64 / 100.0))
                    .collect(),
            )
        })
        .search_field("Product", "inventory_total", |_r, _item, product| {
            Some(number(product.get("totalInventory")))
        })
        .search_field("Product", "is_price_reduced", |r, _item, product| {
            let reduced = variants_of(r, product).iter().any(|variant| {
                match (
                    cents_of(variant.get("compareAtPrice")),
                    cents_of(variant.get("price")),
                ) {
                    (Some(compare_at), Some(price)) => compare_at > price,
                    _ => false,
                }
            });
            Some(vec![SearchValue::Bool(reduced)])
        })
        .search_field("Product", "out_of_stock_somewhere", |r, _item, product| {
            let out = variants_of(r, product).iter().any(|variant| {
                let Some(item) =
                    field(variant, "inventoryItem").and_then(|item| r.state().get(item))
                else {
                    return false;
                };
                item.get("tracked").and_then(Value::as_bool) == Some(true)
                    && gids(item.get("inventoryLevels"))
                        .iter()
                        .filter_map(|level| r.state().get(level))
                        .any(|level| super::available_of(level) <= 0)
            });
            Some(vec![SearchValue::Bool(out)])
        })
        .search_field("Product", "publication_ids", |r, item, _product| {
            let key = item.as_str().unwrap_or_default();
            Some(
                r.state()
                    .keys("Publication")
                    .filter(|publication| {
                        r.state().get(publication).is_some_and(|record| {
                            gids(record.get("products"))
                                .iter()
                                .any(|known| known == key)
                        })
                    })
                    .filter_map(parse_gid)
                    .map(|(_, id)| SearchValue::Id(id))
                    .collect(),
            )
        })
        .search_field("Product", "published_status", |_r, _item, product| {
            Some(published_status(product))
        })
        .default_text("Product", |r, product| {
            let mut texts: Vec<String> = ["title", "handle", "vendor", "productType"]
                .iter()
                .filter_map(|name| field(product, name))
                .map(str::to_string)
                .collect();
            texts.extend(strings(product.get("tags")));
            for variant in variants_of(r, product) {
                texts.extend(
                    ["sku", "barcode"]
                        .iter()
                        .filter_map(|name| field(variant, name))
                        .map(str::to_string),
                );
            }
            texts
        })
        // Variants.
        .sort_key("ProductVariant", "FULL_TITLE", |r, item| {
            let variant = r.item_map(item);
            let product = variant
                .and_then(|variant| field(variant, "product"))
                .and_then(|product| r.state().get(product))
                .and_then(|product| field(product, "title"))
                .unwrap_or_default();
            let title = variant
                .and_then(|variant| field(variant, "title"))
                .unwrap_or_default();
            SortValue::Text(format!("{product} {title}").to_lowercase())
        })
        .sort_key("ProductVariant", "NAME", |r, item| {
            SortValue::of(
                r.item_map(item)
                    .and_then(|variant| variant.get("displayName")),
            )
        })
        .sort_key("ProductVariant", "INVENTORY_MANAGEMENT", |r, item| {
            let tracked = r
                .item_map(item)
                .and_then(|variant| field(variant, "inventoryItem"))
                .and_then(|item| r.state().get(item))
                .and_then(|item| item.get("tracked").cloned());
            SortValue::of(tracked.as_ref())
        })
        .sort_key("ProductVariant", "INVENTORY_LEVELS_AVAILABLE", |r, item| {
            SortValue::of(
                r.item_map(item)
                    .and_then(|variant| variant.get("inventoryQuantity")),
            )
        })
        .search_field("ProductVariant", "product_ids", |_r, _item, variant| {
            Some(ids(field(variant, "product")))
        })
        .search_field("ProductVariant", "collection", |r, _item, variant| {
            Some(
                product_of(r, variant)
                    .map(|product| gids(product.get("collections")))
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|collection| parse_gid(collection))
                    .map(|(_, id)| SearchValue::Id(id))
                    .collect(),
            )
        })
        .search_field("ProductVariant", "location_id", |r, _item, variant| {
            let item = field(variant, "inventoryItem").and_then(|item| r.state().get(item));
            Some(
                item.map(|item| gids(item.get("inventoryLevels")))
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|level| r.state().get(level))
                    .filter_map(|level| field(level, "location"))
                    .filter_map(parse_gid)
                    .map(|(_, id)| SearchValue::Id(id))
                    .collect(),
            )
        })
        .search_field("ProductVariant", "managed", |r, _item, variant| {
            let tracked = field(variant, "inventoryItem")
                .and_then(|item| r.state().get(item))
                .and_then(|item| item.get("tracked"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Some(vec![SearchValue::Bool(tracked)])
        })
        .search_field("ProductVariant", "option1", |_r, _item, variant| {
            Some(option(variant, 0))
        })
        .search_field("ProductVariant", "option2", |_r, _item, variant| {
            Some(option(variant, 1))
        })
        .search_field("ProductVariant", "option3", |_r, _item, variant| {
            Some(option(variant, 2))
        })
        .search_field("ProductVariant", "gift_card", |r, _item, variant| {
            Some(
                product_of(r, variant)
                    .and_then(|product| product.get("isGiftCard"))
                    .and_then(Value::as_bool)
                    .map(SearchValue::Bool)
                    .into_iter()
                    .collect(),
            )
        })
        .search_field("ProductVariant", "product_status", |r, _item, variant| {
            Some(product_keywords(r, variant, "status"))
        })
        .search_field("ProductVariant", "product_type", |r, _item, variant| {
            Some(product_keywords(r, variant, "productType"))
        })
        .search_field("ProductVariant", "vendor", |r, _item, variant| {
            Some(product_keywords(r, variant, "vendor"))
        })
        .search_field("ProductVariant", "tag", |r, _item, variant| {
            Some(
                product_of(r, variant)
                    .map(|product| strings(product.get("tags")))
                    .unwrap_or_default()
                    .into_iter()
                    .map(SearchValue::Keyword)
                    .collect(),
            )
        })
        .search_field("ProductVariant", "published_status", |r, _item, variant| {
            Some(
                product_of(r, variant)
                    .map(published_status)
                    .unwrap_or_default(),
            )
        })
        .default_text("ProductVariant", |r, variant| {
            let mut texts: Vec<String> = ["title", "displayName", "sku", "barcode"]
                .iter()
                .filter_map(|name| field(variant, name))
                .map(str::to_string)
                .collect();
            if let Some(title) = product_of(r, variant).and_then(|product| field(product, "title"))
            {
                texts.push(title.to_string());
            }
            texts
        })
        // Collections.
        .search_field("Collection", "published_status", |r, item, _collection| {
            let key = item.as_str().unwrap_or_default();
            let published = r
                .state()
                .get(&super::online_store_publication_gid())
                .is_some_and(|record| {
                    gids(record.get("collections"))
                        .iter()
                        .any(|known| known == key)
                });
            Some(status_keywords(published))
        })
        // Gift cards.
        .search_field("GiftCard", "balance_status", |_r, _item, card| {
            let balance =
                cents_of(card.get("balance").and_then(|money| money.get("amount"))).unwrap_or(0);
            let initial = cents_of(
                card.get("initialValue")
                    .and_then(|money| money.get("amount")),
            )
            .unwrap_or(0);
            let status = if balance <= 0 {
                vec!["empty"]
            } else if balance >= initial {
                vec!["full", "full_or_partial"]
            } else {
                vec!["partial", "full_or_partial"]
            };
            Some(
                status
                    .into_iter()
                    .map(|word| SearchValue::Keyword(word.to_string()))
                    .collect(),
            )
        })
        .search_field("GiftCard", "status", |r, _item, card| {
            let today = super::gift_cards::today(r.state());
            let enabled = card
                .get("enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let expired = field(card, "expiresOn").is_some_and(|expires| expires < today.as_str());
            let mut words = vec![if enabled { "enabled" } else { "disabled" }];
            if expired {
                words.push("expired");
            } else if field(card, "expiresOn").is_some() {
                words.push("expiring");
            }
            Some(
                words
                    .into_iter()
                    .map(|word| SearchValue::Keyword(word.to_string()))
                    .collect(),
            )
        })
        .search_field("GiftCard", "expires_on", |_r, _item, card| {
            Some(
                field(card, "expiresOn")
                    .and_then(|date| chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
                    .and_then(|date| date.and_hms_opt(0, 0, 0))
                    .map(|date| SearchValue::Date(date.and_utc()))
                    .into_iter()
                    .collect(),
            )
        })
        .search_field("GiftCard", "source", |_r, _item, card| {
            let source = if card.get("order").is_some_and(|order| !order.is_null()) {
                "purchased"
            } else {
                "api_client"
            };
            Some(vec![SearchValue::Keyword(source.to_string())])
        })
        .sort_key("GiftCard", "CODE", |r, item| {
            SortValue::of(r.item_map(item).and_then(|card| card.get("lastCharacters")))
        })
        .sort_key("GiftCard", "DISABLED_AT", |r, item| {
            SortValue::of(r.item_map(item).and_then(|card| card.get("deactivatedAt")))
        })
        .default_text("GiftCard", |_r, card| {
            field(card, "lastCharacters")
                .map(str::to_string)
                .into_iter()
                .collect()
        })
        // Selling plan groups.
        .search_field("SellingPlanGroup", "app_id", |_r, _item, group| {
            Some(
                field(group, "appId")
                    .map(|app| SearchValue::Keyword(app.to_string()))
                    .into_iter()
                    .collect(),
            )
        })
        // Fields that take an argument.
        .query("productTags", |r, ctx, _object| {
            distinct(r, ctx, |product| strings(product.get("tags")))
        })
        .query("productTypes", |r, ctx, _object| {
            distinct(r, ctx, |product| {
                field(product, "productType")
                    .map(str::to_string)
                    .into_iter()
                    .collect()
            })
        })
        .query("productVendors", |r, ctx, _object| {
            distinct(r, ctx, |product| {
                field(product, "vendor")
                    .map(str::to_string)
                    .into_iter()
                    .collect()
            })
        })
        .field("Product", "inCollection", in_collection)
        .field("InventoryItem", "inventoryLevel", item_level)
        .field("Location", "inventoryLevel", location_level)
        // The primary location: `location` without an id ("If no ID is provided, the primary
        // location of the Shop is returned", the 2026-10 schema), where a new variant is stocked.
        .query("location", |r, ctx, object| match ctx.arg_str("id") {
            Some(_) => r.generic(ctx, object),
            None => Ok(Output::object_or_null(
                default_location(r.state()).map(|location| Obj::record(&location)),
            )),
        })
        .field("Location", "isPrimary", |r, _ctx, object| {
            let primary = default_location(r.state());
            Ok(Output::value(
                object.key().is_some() && object.key() == primary.as_deref(),
            ))
        });
}

/// The variants of a product record.
fn variants_of<'r>(r: &'r ShopResolver<'_>, product: &Record) -> Vec<&'r Record> {
    gids(product.get("variants"))
        .iter()
        .filter_map(|variant| r.state().get(variant))
        .collect()
}

/// A string of each variant of a product, as keywords.
fn variant_strings(
    r: &ShopResolver<'_>,
    product: &Record,
    read: impl Fn(&Record) -> Option<String>,
) -> Vec<SearchValue> {
    variants_of(r, product)
        .into_iter()
        .filter_map(read)
        .map(SearchValue::Keyword)
        .collect()
}

fn text_of(record: &Record, name: &str) -> Option<String> {
    field(record, name).map(str::to_string)
}

/// The strings of a list field.
fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn number(value: Option<&Value>) -> Vec<SearchValue> {
    value
        .and_then(Value::as_f64)
        .map(SearchValue::Number)
        .into_iter()
        .collect()
}

/// The number of a GID, as an id.
fn ids(gid: Option<&str>) -> Vec<SearchValue> {
    gid.and_then(parse_gid)
        .map(|(_, id)| SearchValue::Id(id))
        .into_iter()
        .collect()
}

/// The product of a variant record.
fn product_of<'r>(r: &'r ShopResolver<'_>, variant: &Record) -> Option<&'r Record> {
    field(variant, "product").and_then(|product| r.state().get(product))
}

/// A field of a variant's product, as a keyword.
fn product_keywords(r: &ShopResolver<'_>, variant: &Record, name: &str) -> Vec<SearchValue> {
    product_of(r, variant)
        .and_then(|product| field(product, name))
        .map(|value| SearchValue::Keyword(value.to_string()))
        .into_iter()
        .collect()
}

/// The value of an option of a variant.
fn option(variant: &Record, index: usize) -> Vec<SearchValue> {
    variant
        .get("selectedOptions")
        .and_then(Value::as_array)
        .and_then(|options| options.get(index))
        .and_then(|option| option.get("value"))
        .and_then(Value::as_str)
        .map(|value| SearchValue::Keyword(value.to_string()))
        .into_iter()
        .collect()
}

/// `published_status` of a product: `published` (or `visible`) when it is on the Online Store,
/// `unpublished` (or `hidden`) otherwise; `any` always.
fn published_status(product: &Record) -> Vec<SearchValue> {
    let published = product
        .get("publishedAt")
        .is_some_and(|date| !date.is_null());
    status_keywords(published)
}

fn status_keywords(published: bool) -> Vec<SearchValue> {
    let words: &[&str] = if published {
        &["published", "visible", "any", "online_store:visible"]
    } else {
        &["unpublished", "hidden", "any", "online_store:hidden"]
    };
    words
        .iter()
        .map(|word| SearchValue::Keyword(word.to_string()))
        .collect()
}

/// `productTags`, `productTypes`, `productVendors`: the distinct values of the products, sorted.
fn distinct(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    read: impl Fn(&Record) -> Vec<String>,
) -> Result<Output<Obj>, FieldError> {
    let mut values: Vec<String> = Vec::new();
    for product in r.state().all("Product") {
        for value in read(product) {
            if !value.is_empty()
                && !values
                    .iter()
                    .any(|known| known.eq_ignore_ascii_case(&value))
            {
                values.push(value);
            }
        }
    }
    values.sort_by_key(|value| value.to_lowercase());
    r.connection(ctx, values.into_iter().map(Value::String).collect())
}

/// `Product.inCollection(id:)`.
fn in_collection(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default();
    let product = object.key().unwrap_or_default();
    let listed = r.state().get(id).is_some_and(|collection| {
        gids(collection.get("products"))
            .iter()
            .any(|known| known == product)
    });
    Ok(Output::value(listed))
}

/// The level of an item at a location, active only unless `includeInactive`.
fn level(
    r: &ShopResolver<'_>,
    item: &str,
    location: &str,
    include_inactive: bool,
) -> Option<String> {
    let level = super::records::level_at(r.state(), item, location)?;
    let active = r
        .state()
        .get(&level)
        .and_then(|record| record.get("isActive"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    (active || include_inactive).then_some(level)
}

/// `InventoryItem.inventoryLevel(locationId:)`.
fn item_level(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let item = object.key().unwrap_or_default();
    let location = ctx.arg_str("locationId").unwrap_or_default();
    let include = ctx.arg_bool("includeInactive").unwrap_or(false);
    let found = level(r, item, location, include);
    r.convert(ctx, &json!(found), true)
}

/// `Location.inventoryLevel(inventoryItemId:)`.
fn location_level(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let location = object.key().unwrap_or_default();
    let item = ctx.arg_str("inventoryItemId").unwrap_or_default();
    let include = ctx.arg_bool("includeInactive").unwrap_or(false);
    let found = level(r, item, location, include);
    r.convert(ctx, &json!(found), true)
}
