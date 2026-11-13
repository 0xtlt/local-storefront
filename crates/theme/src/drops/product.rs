//! `product`, `variant`, `product_option` and `product_option_value`, with the swatches of
//! option values and the store availabilities of variants.

use std::any::Any;
use std::borrow::Cow;

use lsf_core::color::Color;
use lsf_core::store::{InventoryPolicy, Location, MediaKind, Product, Swatch, Variant};
use lsf_core::util::stable_id;
use lsf_liquid::{Object, Value};
use serde_json::{Value as Json, json};

use super::color::ColorDrop;
use super::media::{ImageDrop, MediaDrop, image_base_url, image_media_json};
use super::metafield::MetafieldsDrop;
use super::selling_plan::{
    allocation_json, allocation_value, allocations_value, group_json, group_value, plan_value,
    plans, selected_plan,
};
use super::shop::AddressDrop;
use super::{Memo, SiteRef, hash, strings, time_value};
use crate::render::cost;

pub struct ProductDrop {
    pub site: SiteRef,
    pub index: usize,
    memo: Memo,
}

impl ProductDrop {
    pub fn value(site: &SiteRef, index: usize) -> Value {
        cost::loaded(cost::PRODUCT, index);
        Value::object(ProductDrop {
            site: site.clone(),
            index,
            memo: Memo::default(),
        })
    }

    pub fn product(&self) -> &Product {
        &self.site.store.products[self.index]
    }

    /// The variant named by `?variant=`, when it belongs to this product.
    pub fn selected_variant(&self) -> Option<usize> {
        selected_variant(&self.site, self.index)
    }

    pub fn selected_or_first_available(&self) -> usize {
        selected_or_first_available(&self.site, self.index)
    }

    pub fn url(&self) -> String {
        product_url(&self.site, self.product())
    }
}

pub fn product_url(site: &SiteRef, product: &Product) -> String {
    site.request
        .localized(&format!("/products/{}", product.handle))
}

/// The id of a product option value, as themes send it back in `?option_values=`.
pub fn option_value_id(product: &Product, position: usize, value: &str) -> u64 {
    stable_id(
        "option_value",
        &format!("{}/{position}/{value}", product.handle),
    )
}

/// The variant the URL selects: `?variant=<id>`, `/variants/<id>`, or
/// `?option_values=<id>,<id>` naming one value per option.
pub fn selected_variant(site: &SiteRef, product: usize) -> Option<usize> {
    let data = &site.store.products[product];
    let named = site.request.param("variant").or_else(|| {
        site.request
            .path
            .trim_matches('/')
            .strip_prefix("variants/")
    });
    if let Some(id) = named.and_then(|id| id.parse::<u64>().ok()) {
        return data.variants.iter().position(|variant| variant.id == id);
    }
    let wanted: Vec<u64> = site
        .request
        .param("option_values")?
        .split(',')
        .filter_map(|id| id.trim().parse().ok())
        .collect();
    if wanted.is_empty() {
        return None;
    }
    data.variants.iter().position(|variant| {
        variant
            .options
            .iter()
            .enumerate()
            .all(|(position, value)| wanted.contains(&option_value_id(data, position, value)))
    })
}

pub fn selected_or_first_available(site: &SiteRef, product: usize) -> usize {
    selected_variant(site, product)
        .or_else(|| {
            site.store.products[product]
                .variants
                .iter()
                .position(|variant| variant.available)
        })
        .unwrap_or(0)
}

/// Whether store availabilities are defined for a variant: for the selected one and for the
/// first available one. Shopify leaves `variant.store_availabilities` empty for the others.
fn has_store_availabilities(site: &SiteRef, product: usize, variant: usize) -> bool {
    let first_available = site.store.products[product]
        .variants
        .iter()
        .position(|variant| variant.available)
        .unwrap_or(0);
    variant == first_available || selected_variant(site, product) == Some(variant)
}

/// The `swatch` object, or nil for a value that has none.
pub fn swatch_value(site: &SiteRef, swatch: Option<&Swatch>) -> Value {
    let Some(swatch) = swatch else {
        return Value::Nil;
    };
    hash([
        (
            "color",
            swatch
                .color
                .as_deref()
                .and_then(Color::parse)
                .map_or(Value::Nil, ColorDrop::value),
        ),
        ("image", ImageDrop::optional(site, swatch.image.as_ref())),
    ])
}

/// The `location` object of a store availability.
pub fn location_value(site: &SiteRef, location: &Location) -> Value {
    hash([
        ("id", Value::Int(location.id as i64)),
        ("name", Value::from(&location.name)),
        (
            "address",
            AddressDrop::value(site, &location.address, false),
        ),
        (
            "latitude",
            location.latitude.map_or(Value::Nil, Value::Float),
        ),
        (
            "longitude",
            location.longitude.map_or(Value::Nil, Value::Float),
        ),
        (
            "metafields",
            MetafieldsDrop::value(site, &location.metafields),
        ),
    ])
}

/// The products as drops, for arrays of products.
pub fn product_list(site: &SiteRef, indexes: &[usize]) -> Value {
    Value::array(
        indexes
            .iter()
            .map(|&index| ProductDrop::value(site, index))
            .collect(),
    )
}

impl Object for ProductDrop {
    fn type_name(&self) -> &str {
        "product"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let site = &self.site;
        let product = self.product();
        Some(match key {
            "id" => Value::Int(product.id as i64),
            "title" => Value::from(&product.title),
            "handle" => Value::from(&product.handle),
            "object_type" => Value::str("product"),
            "description" | "content" => Value::from(&product.description),
            "vendor" => Value::from(&product.vendor),
            "type" => Value::from(&product.product_type),
            "tags" => self.memo.get("tags", || strings(&product.tags)),
            "url" => Value::from(self.url()),
            "template_suffix" => product
                .template_suffix
                .as_ref()
                .map_or_else(Value::empty_string, Value::from),
            "available" => Value::Bool(product.available()),
            "price" | "price_min" => Value::Int(product.price_min()),
            "price_max" => Value::Int(product.price_max()),
            "price_varies" => Value::Bool(product.price_min() != product.price_max()),
            "compare_at_price" => {
                // The lowest compare-at price, or nil when no variant has one.
                match product
                    .variants
                    .iter()
                    .filter_map(|variant| variant.compare_at_price)
                    .min()
                {
                    Some(price) => Value::Int(price),
                    None => Value::Nil,
                }
            }
            "compare_at_price_min" => Value::Int(product.compare_at_price_min()),
            "compare_at_price_max" => Value::Int(product.compare_at_price_max()),
            "compare_at_price_varies" => {
                Value::Bool(product.compare_at_price_min() != product.compare_at_price_max())
            }
            "variants" => self.memo.get("variants", || {
                Value::array(
                    (0..product.variants.len().min(MAX_VARIANTS))
                        .map(|variant| VariantDrop::value(site, self.index, variant))
                        .collect(),
                )
            }),
            "variants_count" => Value::from(product.variants.len()),
            "first_available_variant" => match product
                .variants
                .iter()
                .position(|variant| variant.available)
            {
                Some(variant) => VariantDrop::value(site, self.index, variant),
                None => Value::Nil,
            },
            "selected_variant" => match self.selected_variant() {
                Some(variant) => VariantDrop::value(site, self.index, variant),
                None => Value::Nil,
            },
            "selected_or_first_available_variant" => {
                self.memo.get("selected_or_first_available_variant", || {
                    VariantDrop::value(site, self.index, self.selected_or_first_available())
                })
            }
            "has_only_default_variant" => Value::Bool(product.has_only_default_variant()),
            "options" => Value::array(
                product
                    .options
                    .iter()
                    .map(|option| Value::from(&option.name))
                    .collect(),
            ),
            "options_with_values" => self.memo.get("options_with_values", || {
                Value::array(
                    (0..product.options.len())
                        .map(|position| {
                            Value::object(ProductOptionDrop::new(site, self.index, position))
                        })
                        .collect(),
                )
            }),
            "options_by_name" => self.memo.get("options_by_name", || {
                Value::object(OptionsByName(
                    product
                        .options
                        .iter()
                        .enumerate()
                        .map(|(position, option)| {
                            (
                                option.name.clone(),
                                Value::object(ProductOptionDrop::new(site, self.index, position)),
                            )
                        })
                        .collect(),
                ))
            }),
            "media" => self.memo.get("media", || {
                Value::array(
                    product
                        .media
                        .iter()
                        .map(|media| MediaDrop::value(site, media, Some(self.index)))
                        .collect(),
                )
            }),
            "images" => self.memo.get("images", || {
                Value::array(
                    product
                        .media
                        .iter()
                        .filter(|media| matches!(media.kind, MediaKind::Image))
                        .map(|media| MediaDrop::value(site, media, Some(self.index)))
                        .collect(),
                )
            }),
            "featured_image" => match product
                .media
                .iter()
                .find(|media| matches!(media.kind, MediaKind::Image))
            {
                Some(media) => MediaDrop::value(site, media, Some(self.index)),
                None => Value::Nil,
            },
            "featured_media" => match product.media.first() {
                Some(media) => MediaDrop::value(site, media, Some(self.index)),
                None => Value::Nil,
            },
            "collections" => self.memo.get("collections", || {
                Value::array(
                    product
                        .collections
                        .iter()
                        .map(|&collection| {
                            super::collection::CollectionDrop::value(site, collection)
                        })
                        .collect(),
                )
            }),
            "metafields" => self.memo.get("metafields", || {
                MetafieldsDrop::value(site, &product.metafields)
            }),
            "published_at" => time_value(site, product.published_at),
            "created_at" => time_value(site, product.created_at),
            "updated_at" => time_value(site, product.updated_at),
            "gift_card?" | "gift_card" => Value::Bool(product.gift_card),
            "quantity_price_breaks_configured?" => Value::Bool(
                product
                    .variants
                    .iter()
                    .any(|variant| !variant.quantity_price_breaks.is_empty()),
            ),
            "requires_selling_plan" => Value::Bool(product.requires_selling_plan),
            "selling_plan_groups" => self.memo.get("selling_plan_groups", || {
                Value::array(
                    product
                        .selling_plan_groups
                        .iter()
                        .map(|group| group_value(site, &site.store.selling_plan_groups[*group]))
                        .collect(),
                )
            }),
            "selected_selling_plan" => match selected_plan(site, product) {
                Some((group, plan)) => plan_value(site, group, plan),
                None => Value::Nil,
            },
            // Both a variant and a selling plan have to be selected.
            "selected_selling_plan_allocation" => {
                match (self.selected_variant(), selected_plan(site, product)) {
                    (Some(variant), Some((group, plan))) => {
                        allocation_value(site, group, plan, &product.variants[variant])
                    }
                    _ => Value::Nil,
                }
            }
            // The selected allocation, or else the first one of the first available variant
            // (of the first variant when none is available).
            "selected_or_first_available_selling_plan_allocation" => {
                match (self.selected_variant(), selected_plan(site, product)) {
                    (Some(selected), Some((group, plan))) => {
                        allocation_value(site, group, plan, &product.variants[selected])
                    }
                    _ => match plans(&site.store, product).next() {
                        Some((group, plan)) => {
                            let variant = product
                                .first_available_variant()
                                .unwrap_or(&product.variants[0]);
                            allocation_value(site, group, plan, variant)
                        }
                        None => Value::Nil,
                    },
                }
            }
            "category" => Value::Nil,
            _ => return None,
        })
    }

    fn to_json(&self) -> Json {
        let site = &self.site;
        let product = self.product();
        let images: Vec<Json> = product
            .images()
            .map(|image| json!(image_base_url(site, image)))
            .collect();
        let media: Vec<Json> = product
            .media
            .iter()
            .map(|media| match (&media.kind, &media.preview) {
                (MediaKind::Image, Some(image)) => image_media_json(site, media, image),
                _ => MediaDrop::value(site, media, Some(self.index)).to_json(),
            })
            .collect();
        json!({
            "id": product.id,
            "title": product.title,
            "handle": product.handle,
            "description": product.description,
            "published_at": iso(site, product.published_at),
            "created_at": iso(site, product.created_at),
            "vendor": product.vendor,
            "type": product.product_type,
            "tags": product.tags,
            "price": product.price_min(),
            "price_min": product.price_min(),
            "price_max": product.price_max(),
            "available": product.available(),
            "price_varies": product.price_min() != product.price_max(),
            "compare_at_price": product.variants.iter().filter_map(|variant| variant.compare_at_price).min(),
            "compare_at_price_min": product.compare_at_price_min(),
            "compare_at_price_max": product.compare_at_price_max(),
            "compare_at_price_varies": product.compare_at_price_min() != product.compare_at_price_max(),
            "variants": (0..product.variants.len().min(MAX_VARIANTS)).map(|variant| variant_json(site, self.index, variant)).collect::<Vec<_>>(),
            "images": images,
            "featured_image": images.first().cloned().unwrap_or(Json::Null),
            "options": product.options.iter().map(|option| option.name.clone()).collect::<Vec<_>>(),
            "media": media,
            "requires_selling_plan": product.requires_selling_plan,
            "selling_plan_groups": product
                .selling_plan_groups
                .iter()
                .map(|group| group_json(&site.store.selling_plan_groups[*group]))
                .collect::<Vec<_>>(),
            "content": product.description,
        })
    }

    fn identity(&self) -> Option<String> {
        Some(format!("product:{}", self.product().id))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The most variants `product.variants` and the product JSON of the Ajax API give: the first 250,
/// of the up to 2,048 a product can have. `product.variants_count` stays the total.
/// https://shopify.dev/docs/storefronts/themes/product-merchandising/variants/support-high-variant-products
/// https://shopify.dev/docs/api/ajax ("Product JSON responses are limited to a maximum of 250
/// variants")
pub const MAX_VARIANTS: usize = 250;

/// ISO 8601 in the shop's time zone, as Shopify's JSON has it.
pub fn iso(site: &SiteRef, time: chrono::DateTime<chrono::Utc>) -> String {
    time.with_timezone(&site.store.shop.timezone)
        .format("%Y-%m-%dT%H:%M:%S%:z")
        .to_string()
}

/// An image of a product as the product JSON has it (a variant's `featured_image`, the `images`
/// of `/products/<handle>.json`): the media at `media_index`, when it is an image.
fn product_image_json(site: &SiteRef, product: &Product, media_index: usize) -> Option<Json> {
    let media = product.media.get(media_index)?;
    let image = media.preview.as_ref()?;
    Some(json!({
        "id": image.id,
        "product_id": product.id,
        "position": media.position,
        "created_at": iso(site, product.created_at),
        "updated_at": iso(site, product.updated_at),
        "alt": if image.alt.is_empty() { Json::Null } else { json!(image.alt) },
        "width": image.width,
        "height": image.height,
        "src": image_base_url(site, image),
        "variant_ids": product
            .variants
            .iter()
            .filter(|variant| variant.media_index == Some(media_index))
            .map(|variant| variant.id)
            .collect::<Vec<_>>(),
    }))
}

/// The product as `/products/<handle>.json` gives it, a format other than the Ajax API's of
/// `.js`: the fields of the product resource of the REST Admin API that a storefront shows.
/// `body_html` and `product_type` for `description` and `type`, `tags` as one string, prices as
/// decimal strings, `options` and `images` as objects with ids, and `image`, the first image.
/// https://shopify.dev/docs/api/admin-rest/latest/resources/product
pub fn storefront_json(site: &SiteRef, index: usize) -> Json {
    let product = &site.store.products[index];
    let decimal = |cents: i64| format!("{}.{:02}", cents / 100, cents % 100);
    let currency = site.currency();
    let images: Vec<Json> = product
        .media
        .iter()
        .enumerate()
        .filter(|(_, media)| matches!(media.kind, MediaKind::Image))
        .filter_map(|(media_index, _)| product_image_json(site, product, media_index))
        .collect();
    let variants: Vec<Json> = product
        .variants
        .iter()
        .take(MAX_VARIANTS)
        .enumerate()
        .map(|(position, variant)| {
            let option = |position: usize| variant.options.get(position).map_or(Json::Null, |value| json!(value));
            json!({
                "id": variant.id,
                "product_id": product.id,
                "title": variant.title,
                "price": decimal(variant.price),
                "sku": variant.sku,
                "position": position + 1,
                "compare_at_price": variant.compare_at_price.map(decimal),
                "fulfillment_service": "manual",
                "inventory_management": if variant.inventory_tracked { json!("shopify") } else { Json::Null },
                "option1": option(0),
                "option2": option(1),
                "option3": option(2),
                "created_at": iso(site, product.created_at),
                "updated_at": iso(site, product.updated_at),
                "taxable": variant.taxable,
                "barcode": if variant.barcode.is_empty() { Json::Null } else { json!(variant.barcode) },
                "grams": variant.weight,
                "image_id": variant
                    .media_index
                    .and_then(|index| product.media.get(index))
                    .and_then(|media| media.preview.as_ref())
                    .map(|image| image.id),
                "weight": lsf_core::store::weight_in_unit(variant.weight, &variant.weight_unit),
                "weight_unit": variant.weight_unit,
                "requires_shipping": variant.requires_shipping,
                "quantity_rule": {
                    "min": variant.quantity_rule.min,
                    "max": variant.quantity_rule.max,
                    "increment": variant.quantity_rule.increment,
                },
                "price_currency": currency,
                "compare_at_price_currency": currency,
                "quantity_price_breaks": variant
                    .quantity_price_breaks
                    .iter()
                    .map(|(minimum, price)| json!({"minimum_quantity": minimum, "price": decimal(*price)}))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    let image = images.first().cloned().unwrap_or(Json::Null);
    json!({
        "id": product.id,
        "title": product.title,
        "body_html": product.description,
        "vendor": product.vendor,
        "product_type": product.product_type,
        "created_at": iso(site, product.created_at),
        "handle": product.handle,
        "updated_at": iso(site, product.updated_at),
        "published_at": iso(site, product.published_at),
        "template_suffix": product.template_suffix,
        "published_scope": "web",
        "tags": product.tags.join(", "),
        "variants": variants,
        "options": product
            .options
            .iter()
            .map(|option| json!({
                "id": stable_id("product_option", &format!("{}/{}", product.id, option.name)),
                "product_id": product.id,
                "name": option.name,
                "position": option.position,
                "values": option.values,
            }))
            .collect::<Vec<_>>(),
        "images": images,
        "image": image,
    })
}

pub fn variant_json(site: &SiteRef, product_index: usize, variant_index: usize) -> Json {
    let product = &site.store.products[product_index];
    let variant = &product.variants[variant_index];
    let option = |position: usize| {
        variant
            .options
            .get(position)
            .cloned()
            .map_or(Json::Null, Json::String)
    };
    let default_title = product.has_only_default_variant();
    let image = variant
        .media_index
        .and_then(|index| product.media.get(index));
    let featured_image = variant
        .media_index
        .and_then(|index| product_image_json(site, product, index))
        .unwrap_or(Json::Null);
    let mut out = json!({
        "id": variant.id,
        "title": variant.title,
        "option1": option(0),
        "option2": option(1),
        "option3": option(2),
        "sku": variant.sku,
        "requires_shipping": variant.requires_shipping,
        "taxable": variant.taxable,
        "featured_image": featured_image,
        "available": variant.available,
        "name": if default_title { product.title.clone() } else { format!("{} - {}", product.title, variant.title) },
        "public_title": if default_title { Json::Null } else { json!(variant.title) },
        "options": variant.options,
        "price": variant.price,
        "weight": variant.weight,
        "compare_at_price": variant.compare_at_price,
        "inventory_management": if variant.inventory_tracked { json!("shopify") } else { Json::Null },
        "barcode": if variant.barcode.is_empty() { Json::Null } else { json!(variant.barcode) },
        "requires_selling_plan": product.requires_selling_plan,
        "selling_plan_allocations": plans(&site.store, product)
            .map(|(group, plan)| allocation_json(group, plan, variant))
            .collect::<Vec<_>>(),
        "quantity_rule": {
            "min": variant.quantity_rule.min,
            "max": variant.quantity_rule.max,
            "increment": variant.quantity_rule.increment,
        },
    });
    if let Some((media, image)) =
        image.and_then(|media| media.preview.as_ref().map(|image| (media, image)))
    {
        out["featured_media"] = json!({
            "alt": if image.alt.is_empty() { Json::Null } else { json!(image.alt) },
            "id": media.id,
            "position": media.position,
            "preview_image": {
                "aspect_ratio": image.aspect_ratio(),
                "height": image.height,
                "width": image.width,
                "src": image_base_url(site, image),
            },
        });
    }
    out
}

/// A weight in grams, in `unit` (`g`, `kg`, `lb`, `oz`; anything else is `kg`), as Shopify
/// gives it in `variant.weight_in_unit` and `weight_with_unit`: grams are a whole number, the
/// other units a decimal number rounded to the precision Shopify keeps for each (3 places for
/// kilograms, 2 for pounds, 1 for ounces), which prints `10.0`, never `10`.
pub fn weight_value(grams: f64, unit: &str) -> Value {
    let rounded = |amount: f64, places: i32| {
        let scale = 10f64.powi(places);
        Value::Float((amount * scale).round() / scale)
    };
    match unit {
        "g" => Value::Int(grams.round() as i64),
        "lb" => rounded(grams / 453.592_37, 2),
        "oz" => rounded(grams / 28.349_523_125, 1),
        _ => rounded(grams / 1000.0, 3),
    }
}

pub struct VariantDrop {
    pub site: SiteRef,
    pub product: usize,
    pub index: usize,
    memo: Memo,
}

impl VariantDrop {
    pub fn value(site: &SiteRef, product: usize, index: usize) -> Value {
        cost::loaded(cost::VARIANT, (product, index));
        Value::object(VariantDrop {
            site: site.clone(),
            product,
            index,
            memo: Memo::default(),
        })
    }

    pub fn variant(&self) -> &Variant {
        &self.site.store.products[self.product].variants[self.index]
    }

    pub fn url(&self) -> String {
        format!(
            "{}?variant={}",
            product_url(&self.site, &self.site.store.products[self.product]),
            self.variant().id
        )
    }
}

impl Object for VariantDrop {
    fn type_name(&self) -> &str {
        "variant"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let site = &self.site;
        let product = &site.store.products[self.product];
        let variant = self.variant();
        let media = variant
            .media_index
            .and_then(|index| product.media.get(index));
        Some(match key {
            "id" => Value::Int(variant.id as i64),
            "title" => Value::from(&variant.title),
            "price" => Value::Int(variant.price),
            "compare_at_price" => variant.compare_at_price.map_or(Value::Nil, Value::Int),
            "sku" => Value::from(&variant.sku),
            "barcode" => Value::from(&variant.barcode),
            "available" => Value::Bool(variant.available),
            "selected" => Value::Bool(selected_variant(site, self.product) == Some(self.index)),
            "matched" => Value::Bool(true),
            "options" => strings(&variant.options),
            "option1" => variant.options.first().map_or(Value::Nil, Value::from),
            "option2" => variant.options.get(1).map_or(Value::Nil, Value::from),
            "option3" => variant.options.get(2).map_or(Value::Nil, Value::from),
            "options_with_values" => Value::array(
                product
                    .options
                    .iter()
                    .zip(&variant.options)
                    .map(|(option, value)| {
                        hash([
                            ("name", Value::from(&option.name)),
                            ("value", Value::from(value)),
                        ])
                    })
                    .collect(),
            ),
            "url" => Value::from(self.url()),
            "product" => self
                .memo
                .get("product", || ProductDrop::value(site, self.product)),
            "weight" => Value::Int(variant.weight as i64),
            "weight_unit" => Value::from(&variant.weight_unit),
            "weight_in_unit" => weight_value(variant.weight as f64, &variant.weight_unit),
            "inventory_quantity" => Value::Int(variant.inventory_quantity),
            "inventory_management" => {
                if variant.inventory_tracked {
                    Value::str("shopify")
                } else {
                    Value::Nil
                }
            }
            "inventory_policy" => Value::str(match variant.inventory_policy {
                InventoryPolicy::Deny => "deny",
                InventoryPolicy::Continue => "continue",
            }),
            "requires_shipping" => Value::Bool(variant.requires_shipping),
            "taxable" => Value::Bool(variant.taxable),
            "image" | "featured_image" => {
                match media.filter(|media| matches!(media.kind, MediaKind::Image)) {
                    Some(media) => MediaDrop::value(site, media, Some(self.product)),
                    None => Value::Nil,
                }
            }
            "featured_media" => match media {
                Some(media) => MediaDrop::value(site, media, Some(self.product)),
                None => Value::Nil,
            },
            "unit_price" => variant.unit_price.map_or(Value::Nil, Value::Int),
            "unit_price_measurement" => match &variant.unit_price_measurement {
                Some(measurement) => hash([
                    ("measured_type", Value::from(&measurement.measured_type)),
                    ("quantity_value", number(measurement.quantity_value)),
                    ("quantity_unit", Value::from(&measurement.quantity_unit)),
                    ("reference_value", number(measurement.reference_value)),
                    ("reference_unit", Value::from(&measurement.reference_unit)),
                ]),
                None => Value::Nil,
            },
            "quantity_rule" => hash([
                ("min", Value::from(variant.quantity_rule.min)),
                (
                    "max",
                    variant.quantity_rule.max.map_or(Value::Nil, Value::from),
                ),
                ("increment", Value::from(variant.quantity_rule.increment)),
            ]),
            "quantity_price_breaks" => Value::array(
                variant
                    .quantity_price_breaks
                    .iter()
                    .map(|(minimum, price)| {
                        hash([
                            ("minimum_quantity", Value::from(*minimum)),
                            ("price", Value::Int(*price)),
                        ])
                    })
                    .collect(),
            ),
            "quantity_price_breaks_configured?" => {
                Value::Bool(!variant.quantity_price_breaks.is_empty())
            }
            "metafields" => self.memo.get("metafields", || {
                MetafieldsDrop::value(site, &variant.metafields)
            }),
            "incoming" => Value::Bool(false),
            "next_incoming_date" => Value::Nil,
            "requires_selling_plan" => Value::Bool(product.requires_selling_plan),
            "selected_selling_plan_allocation" => match selected_plan(site, product) {
                Some((group, plan)) => allocation_value(site, group, plan, variant),
                None => Value::Nil,
            },
            "selling_plan_allocations" => self.memo.get("selling_plan_allocations", || {
                allocations_value(site, product, variant)
            }),
            "store_availabilities" => self.memo.get("store_availabilities", || {
                if !has_store_availabilities(site, self.product, self.index) {
                    return Value::array(Vec::new());
                }
                Value::array(
                    variant
                        .store_availabilities
                        .iter()
                        .map(|(location, available)| {
                            let location = &site.store.locations[*location];
                            hash([
                                ("available", Value::Bool(*available)),
                                ("location", location_value(site, location)),
                                (
                                    "physical_storefront",
                                    Value::Bool(location.physical_storefront),
                                ),
                                ("pick_up_enabled", Value::Bool(location.pick_up_enabled)),
                                ("pick_up_time", Value::from(&location.pick_up_time)),
                            ])
                        })
                        .collect(),
                )
            }),
            _ => return None,
        })
    }

    fn to_json(&self) -> Json {
        variant_json(&self.site, self.product, self.index)
    }

    fn identity(&self) -> Option<String> {
        Some(format!("variant:{}", self.variant().id))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A whole number stays an integer, as measurement values do in Shopify.
fn number(value: f64) -> Value {
    if value.fract() == 0.0 {
        Value::Int(value as i64)
    } else {
        Value::Float(value)
    }
}

/// `product.options_by_name`: the options by their name, which a lookup matches in any case, as
/// on Shopify ("When accessing a specific option, the name is case-insensitive"). Otherwise a
/// hash: a loop gives `[name, option]` pairs.
struct OptionsByName(lsf_liquid::Hash);

impl Object for OptionsByName {
    fn type_name(&self) -> &str {
        "options_by_name"
    }

    fn get(&self, key: &str) -> Option<Value> {
        self.0.get(key).cloned().or_else(|| {
            let key = key.to_lowercase();
            self.0
                .iter()
                .find(|(name, _)| name.to_lowercase() == key)
                .map(|(_, option)| option.clone())
        })
    }

    fn items(&self) -> Option<std::sync::Arc<Vec<Value>>> {
        Some(std::sync::Arc::new(
            self.0
                .iter()
                .map(|(name, option)| Value::array(vec![Value::str(name), option.clone()]))
                .collect(),
        ))
    }

    fn size(&self) -> Option<usize> {
        Some(self.0.len())
    }

    fn render(&self) -> Cow<'_, str> {
        Cow::Owned(Value::hash(self.0.clone()).inspect())
    }

    fn to_json(&self) -> Json {
        Value::hash(self.0.clone()).to_json()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct ProductOptionDrop {
    site: SiteRef,
    product: usize,
    /// 0-based index of the option.
    position: usize,
    memo: Memo,
}

impl ProductOptionDrop {
    pub fn new(site: &SiteRef, product: usize, position: usize) -> Self {
        ProductOptionDrop {
            site: site.clone(),
            product,
            position,
            memo: Memo::default(),
        }
    }
}

impl Object for ProductOptionDrop {
    fn type_name(&self) -> &str {
        "product_option"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let product = &self.site.store.products[self.product];
        let option = &product.options[self.position];
        let selected = &product.variants[selected_or_first_available(&self.site, self.product)];
        Some(match key {
            "name" => Value::from(&option.name),
            "position" => Value::from(option.position),
            "selected_value" => selected
                .options
                .get(self.position)
                .map_or(Value::Nil, Value::from),
            "values" => self.memo.get("values", || {
                Value::array(
                    option
                        .values
                        .iter()
                        .map(|value| {
                            Value::object(ProductOptionValueDrop {
                                site: self.site.clone(),
                                product: self.product,
                                position: self.position,
                                name: value.clone(),
                            })
                        })
                        .collect(),
                )
            }),
            _ => return None,
        })
    }

    fn render(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.site.store.products[self.product].options[self.position].name)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct ProductOptionValueDrop {
    site: SiteRef,
    product: usize,
    position: usize,
    name: String,
}

impl ProductOptionValueDrop {
    /// The variant choosing this value leads to: the one that keeps the other selected options
    /// when it exists, otherwise the first variant with this value.
    fn variant(&self) -> Option<usize> {
        let product = &self.site.store.products[self.product];
        let selected = &product.variants[selected_or_first_available(&self.site, self.product)];
        let has_value = |variant: &Variant| variant.options.get(self.position) == Some(&self.name);
        product
            .variants
            .iter()
            .position(|variant| {
                has_value(variant)
                    && variant.options.iter().enumerate().all(|(position, value)| {
                        position == self.position || selected.options.get(position) == Some(value)
                    })
            })
            .or_else(|| product.variants.iter().position(has_value))
    }
}

impl Object for ProductOptionValueDrop {
    fn type_name(&self) -> &str {
        "product_option_value"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let product = &self.site.store.products[self.product];
        let selected = &product.variants[selected_or_first_available(&self.site, self.product)];
        Some(match key {
            "name" => Value::from(&self.name),
            "id" => Value::Int(option_value_id(product, self.position, &self.name) as i64),
            "selected" => Value::Bool(selected.options.get(self.position) == Some(&self.name)),
            "available" => {
                // Available when a purchasable variant has this value together with the
                // values already selected for the options before it.
                let available = product.variants.iter().any(|variant| {
                    variant.available
                        && variant.options.get(self.position) == Some(&self.name)
                        && (0..self.position).all(|position| {
                            variant.options.get(position) == selected.options.get(position)
                        })
                });
                Value::Bool(available)
            }
            "variant" => match self.variant() {
                Some(variant) => VariantDrop::value(&self.site, self.product, variant),
                None => Value::Nil,
            },
            "swatch" => swatch_value(
                &self.site,
                product.options[self.position].swatch(&self.name),
            ),
            "product_url" => Value::Nil,
            _ => return None,
        })
    }

    /// An option value behaves as its name: older themes loop over `option.values` as strings.
    fn to_value(&self) -> Option<Value> {
        Some(Value::from(&self.name))
    }

    fn to_json(&self) -> Json {
        json!(self.name)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_are_rounded_as_shopify_rounds_them() {
        // #128: what a store prints, and kilograms, pounds and ounces at the precision the
        // Admin API gives them (3, 2 and 1 places).
        let printed = |grams: f64, unit: &str| weight_value(grams, unit).to_str().into_owned();
        assert_eq!(printed(4536.0, "lb"), "10.0");
        assert_eq!(printed(4536.0, "kg"), "4.536");
        assert_eq!(printed(1000.0, "kg"), "1.0");
        assert_eq!(printed(1.0, "kg"), "0.001");
        assert_eq!(printed(1234.0, "kg"), "1.234");
        assert_eq!(printed(-1000.0, "kg"), "-1.0");
        assert_eq!(printed(28.0, "oz"), "1.0");
        assert_eq!(printed(6.7, "oz"), "0.2");
        assert_eq!(printed(200.0, "lb"), "0.44");
        assert_eq!(printed(2.5, "g"), "3");
        assert_eq!(printed(500.0, "g"), "500");
        assert!(matches!(weight_value(500.0, "g"), Value::Int(500)));
    }
}
