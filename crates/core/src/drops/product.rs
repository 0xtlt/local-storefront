//! `product`, `variant`, `product_option` and `product_option_value`.

use std::any::Any;
use std::borrow::Cow;

use serde_json::{Value as Json, json};
use slt_liquid::{Object, Value};

use super::media::{MediaDrop, image_base_url, image_media_json};
use super::metafield::MetafieldsDrop;
use super::{Memo, SiteRef, hash, strings, time_value};
use crate::store::{InventoryPolicy, MediaKind, Product, Variant};
use crate::util::stable_id;

pub struct ProductDrop {
    pub site: SiteRef,
    pub index: usize,
    memo: Memo,
}

impl ProductDrop {
    pub fn value(site: &SiteRef, index: usize) -> Value {
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

/// The variant the URL selects: `?variant=<id>`, or `?option_values=<id>,<id>` naming one
/// value per option.
pub fn selected_variant(site: &SiteRef, product: usize) -> Option<usize> {
    let data = &site.store.products[product];
    if let Some(id) = site
        .request
        .param("variant")
        .and_then(|id| id.parse::<u64>().ok())
    {
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
                    (0..product.variants.len())
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
                Value::hash(
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
                )
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
            "requires_selling_plan" => Value::Bool(false),
            "selling_plan_groups" => Value::array(Vec::new()),
            "selected_selling_plan"
            | "selected_selling_plan_allocation"
            | "selected_or_first_available_selling_plan_allocation"
            | "category" => Value::Nil,
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
            "variants": (0..product.variants.len()).map(|variant| variant_json(site, self.index, variant)).collect::<Vec<_>>(),
            "images": images,
            "featured_image": images.first().cloned().unwrap_or(Json::Null),
            "options": product.options.iter().map(|option| option.name.clone()).collect::<Vec<_>>(),
            "media": media,
            "requires_selling_plan": false,
            "selling_plan_groups": [],
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

/// ISO 8601 in the shop's time zone, as Shopify's JSON has it.
pub fn iso(site: &SiteRef, time: chrono::DateTime<chrono::Utc>) -> String {
    time.with_timezone(&site.store.shop.timezone)
        .format("%Y-%m-%dT%H:%M:%S%:z")
        .to_string()
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
    let featured_image =
        match image.and_then(|media| media.preview.as_ref().map(|image| (media, image))) {
            Some((media, image)) => json!({
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
                    .filter(|other| other.media_index == variant.media_index)
                    .map(|other| other.id)
                    .collect::<Vec<_>>(),
            }),
            None => Json::Null,
        };
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
        "requires_selling_plan": false,
        "selling_plan_allocations": [],
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

pub struct VariantDrop {
    pub site: SiteRef,
    pub product: usize,
    pub index: usize,
    memo: Memo,
}

impl VariantDrop {
    pub fn value(site: &SiteRef, product: usize, index: usize) -> Value {
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

/// The weight of a variant expressed in its display unit.
pub fn weight_in_unit(grams: u64, unit: &str) -> f64 {
    let grams = grams as f64;
    match unit {
        "g" => grams,
        "oz" => grams / 28.349_523_125,
        "lb" => grams / 453.592_37,
        _ => grams / 1000.0,
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
            "weight_in_unit" => Value::Float(weight_in_unit(variant.weight, &variant.weight_unit)),
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
            "incoming" | "requires_selling_plan" => Value::Bool(false),
            "next_incoming_date" | "selected_selling_plan_allocation" => Value::Nil,
            "selling_plan_allocations" | "store_availabilities" => Value::array(Vec::new()),
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
            "swatch" | "product_url" => Value::Nil,
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
