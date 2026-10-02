//! `cart` and `line_item`, computed from the session's cart lines.

use std::any::Any;

use indexmap::IndexMap;
use lsf_liquid::{Hash, Object, Value};
use serde_json::{Value as Json, json};

use super::localization::currency_value;
use super::media::{ImageDrop, image_base_url};
use super::product::{ProductDrop, VariantDrop, product_url};
use super::{Memo, SiteRef, hash};
use crate::store::{CartLine, MediaKind, Product, Variant};
use crate::util::short_hash;

/// A cart line resolved against the store.
pub struct ResolvedLine<'a> {
    pub line: &'a CartLine,
    pub product_index: usize,
    pub variant_index: usize,
    pub product: &'a Product,
    pub variant: &'a Variant,
}

impl ResolvedLine<'_> {
    /// The unit price, taking quantity price breaks into account.
    pub fn unit_price(&self) -> i64 {
        self.variant
            .quantity_price_breaks
            .iter()
            .filter(|(minimum, _)| self.line.quantity >= *minimum)
            .map(|(_, price)| *price)
            .min()
            .unwrap_or(self.variant.price)
    }

    pub fn line_price(&self) -> i64 {
        self.unit_price() * i64::from(self.line.quantity)
    }

    pub fn title(&self) -> String {
        if self.product.has_only_default_variant() {
            self.product.title.clone()
        } else {
            format!("{} - {}", self.product.title, self.variant.title)
        }
    }

    pub fn key(&self) -> String {
        line_key(self.line)
    }
}

/// The key identifying a line: the variant plus a hash of the line's properties, as lines with
/// different properties stay separate.
pub fn line_key(line: &CartLine) -> String {
    let properties: Vec<String> = line
        .properties
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    format!(
        "{}:{}",
        line.variant_id,
        &short_hash(&properties.join("\n"))[..16]
    )
}

/// The session's cart lines that still point at existing variants.
pub fn resolve_lines(site: &SiteRef) -> Vec<ResolvedLine<'_>> {
    site.session
        .cart_lines
        .iter()
        .filter_map(|line| {
            let (product_index, variant_index) = site.store.variant_location(line.variant_id)?;
            let product = &site.store.products[product_index];
            Some(ResolvedLine {
                line,
                product_index,
                variant_index,
                product,
                variant: &product.variants[variant_index],
            })
        })
        .collect()
}

fn properties_value(properties: &IndexMap<String, String>) -> Value {
    let map: Hash = properties
        .iter()
        .map(|(key, value)| (key.clone(), Value::from(value)))
        .collect();
    Value::hash(map)
}

fn line_image<'a>(line: &'a ResolvedLine<'_>) -> Option<&'a crate::store::Image> {
    line.variant
        .media_index
        .and_then(|index| line.product.media.get(index))
        .filter(|media| matches!(media.kind, MediaKind::Image))
        .and_then(|media| media.preview.as_ref())
        .or_else(|| line.product.images().next())
}

pub fn line_json(site: &SiteRef, line: &ResolvedLine<'_>) -> Json {
    let image = line_image(line);
    let url = format!(
        "{}?variant={}",
        product_url(site, line.product),
        line.variant.id
    );
    let default_variant = line.product.has_only_default_variant();
    json!({
        "id": line.variant.id,
        "properties": line.line.properties,
        "quantity": line.line.quantity,
        "variant_id": line.variant.id,
        "key": line.key(),
        "title": line.title(),
        "price": line.unit_price(),
        "original_price": line.variant.price,
        "presentment_price": line.unit_price() as f64 / 100.0,
        "discounted_price": line.unit_price(),
        "line_price": line.line_price(),
        "original_line_price": line.variant.price * i64::from(line.line.quantity),
        "total_discount": 0,
        "discounts": [],
        "sku": line.variant.sku,
        "grams": line.variant.weight,
        "vendor": line.product.vendor,
        "taxable": line.variant.taxable,
        "product_id": line.product.id,
        "product_has_only_default_variant": default_variant,
        "gift_card": line.product.gift_card,
        "final_price": line.unit_price(),
        "final_line_price": line.line_price(),
        "url": url,
        "featured_image": match image {
            Some(image) => json!({
                "aspect_ratio": image.aspect_ratio(),
                "alt": image.alt,
                "height": image.height,
                "url": image_base_url(site, image),
                "width": image.width,
            }),
            None => Json::Null,
        },
        "image": image.map(|image| image_base_url(site, image)),
        "handle": line.product.handle,
        "requires_shipping": line.variant.requires_shipping,
        "product_type": line.product.product_type,
        "product_title": line.product.title,
        "product_description": line.product.description,
        "variant_title": if default_variant { Json::Null } else { json!(line.variant.title) },
        "variant_options": line.variant.options,
        "options_with_values": line.product.options.iter().zip(&line.variant.options).map(|(option, value)| json!({"name": option.name, "value": value})).collect::<Vec<_>>(),
        "line_level_discount_allocations": [],
        "line_level_total_discount": 0,
        "quantity_rule": {
            "min": line.variant.quantity_rule.min,
            "max": line.variant.quantity_rule.max,
            "increment": line.variant.quantity_rule.increment,
        },
        "has_components": false,
    })
}

/// The cart as `/cart.js` returns it.
pub fn cart_json(site: &SiteRef) -> Json {
    let lines = resolve_lines(site);
    let total: i64 = lines.iter().map(ResolvedLine::line_price).sum();
    let original_total: i64 = lines
        .iter()
        .map(|line| line.variant.price * i64::from(line.line.quantity))
        .sum();
    json!({
        "token": cart_token(site),
        "note": if site.session.cart_note.is_empty() { Json::Null } else { json!(site.session.cart_note) },
        "attributes": site.session.cart_attributes,
        "original_total_price": original_total,
        "total_price": total,
        "total_discount": original_total - total,
        "total_weight": lines.iter().map(|line| line.variant.weight as f64 * f64::from(line.line.quantity)).sum::<f64>(),
        "item_count": lines.iter().map(|line| u64::from(line.line.quantity)).sum::<u64>(),
        "items": lines.iter().map(|line| line_json(site, line)).collect::<Vec<_>>(),
        "requires_shipping": lines.iter().any(|line| line.variant.requires_shipping),
        "currency": site.currency(),
        "items_subtotal_price": total,
        "cart_level_discount_applications": [],
        "discount_codes": [],
    })
}

pub fn cart_token(site: &SiteRef) -> String {
    if site.session.cart_token.is_empty() {
        format!("local-{}", &short_hash(&site.request.host)[..12])
    } else {
        site.session.cart_token.clone()
    }
}

pub struct CartDrop {
    pub site: SiteRef,
    memo: Memo,
}

impl CartDrop {
    pub fn value(site: &SiteRef) -> Value {
        Value::object(CartDrop {
            site: site.clone(),
            memo: Memo::default(),
        })
    }
}

impl Object for CartDrop {
    fn type_name(&self) -> &str {
        "cart"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let site = &self.site;
        let lines = resolve_lines(site);
        let total: i64 = lines.iter().map(ResolvedLine::line_price).sum();
        let original_total: i64 = lines
            .iter()
            .map(|line| line.variant.price * i64::from(line.line.quantity))
            .sum();
        Some(match key {
            "items" => self.memo.get("items", || {
                Value::array(
                    (0..lines.len())
                        .map(|index| {
                            Value::object(LineItemDrop {
                                site: site.clone(),
                                index,
                                memo: Memo::default(),
                            })
                        })
                        .collect(),
                )
            }),
            "item_count" => {
                Value::Int(lines.iter().map(|line| i64::from(line.line.quantity)).sum())
            }
            "empty?" => Value::Bool(lines.is_empty()),
            "total_price" | "items_subtotal_price" | "checkout_charge_amount" => Value::Int(total),
            "original_total_price" => Value::Int(original_total),
            "total_discount" => Value::Int(original_total - total),
            "total_weight" => Value::Int(
                lines
                    .iter()
                    .map(|line| line.variant.weight as i64 * i64::from(line.line.quantity))
                    .sum(),
            ),
            "note" => {
                if site.session.cart_note.is_empty() {
                    Value::Nil
                } else {
                    Value::from(&site.session.cart_note)
                }
            }
            "attributes" => properties_value(&site.session.cart_attributes),
            "currency" => currency_value(site.currency()),
            "requires_shipping" => {
                Value::Bool(lines.iter().any(|line| line.variant.requires_shipping))
            }
            "taxes_included" => Value::Bool(site.store.shop.taxes_included),
            "duties_included" => Value::Bool(false),
            "discount_applications" | "cart_level_discount_applications" | "discounts" => {
                Value::array(Vec::new())
            }
            _ => return None,
        })
    }

    fn to_json(&self) -> Json {
        cart_json(&self.site)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub struct LineItemDrop {
    site: SiteRef,
    /// Position of the line among the resolved cart lines.
    index: usize,
    memo: Memo,
}

impl LineItemDrop {
    /// `(product index, variant index, variant id)` of the line, for the cart filters.
    pub fn location(&self) -> Option<(usize, usize, u64)> {
        let lines = resolve_lines(&self.site);
        let line = lines.get(self.index)?;
        Some((line.product_index, line.variant_index, line.variant.id))
    }
}

impl Object for LineItemDrop {
    fn type_name(&self) -> &str {
        "line_item"
    }

    fn get(&self, key: &str) -> Option<Value> {
        let site = &self.site;
        let lines = resolve_lines(site);
        let line = lines.get(self.index)?;
        let quantity = i64::from(line.line.quantity);
        Some(match key {
            "id" | "variant_id" => Value::Int(line.variant.id as i64),
            "key" => Value::from(line.key()),
            "quantity" => Value::Int(quantity),
            "title" => Value::from(line.title()),
            "price" | "final_price" => Value::Int(line.unit_price()),
            "original_price" => Value::Int(line.variant.price),
            "line_price" | "final_line_price" => Value::Int(line.line_price()),
            "original_line_price" => Value::Int(line.variant.price * quantity),
            "total_discount" | "line_level_total_discount" => {
                Value::Int((line.variant.price - line.unit_price()) * quantity)
            }
            "discount_allocations"
            | "line_level_discount_allocations"
            | "discounts"
            | "tax_lines"
            | "item_components" => Value::array(Vec::new()),
            "product_id" => Value::Int(line.product.id as i64),
            "product" => self
                .memo
                .get("product", || ProductDrop::value(site, line.product_index)),
            "variant" => self.memo.get("variant", || {
                VariantDrop::value(site, line.product_index, line.variant_index)
            }),
            "sku" => Value::from(&line.variant.sku),
            "vendor" => Value::from(&line.product.vendor),
            "taxable" => Value::Bool(line.variant.taxable),
            "gift_card" => Value::Bool(line.product.gift_card),
            "requires_shipping" => Value::Bool(line.variant.requires_shipping),
            "grams" => Value::Int(line.variant.weight as i64),
            "url" => Value::from(format!(
                "{}?variant={}",
                product_url(site, line.product),
                line.variant.id
            )),
            "url_to_remove" => Value::from(format!(
                "{}?id={}&quantity=0",
                site.request.localized("/cart/change"),
                crate::urls::encode_component(&line.key())
            )),
            "image" => ImageDrop::optional(site, line_image(line)),
            "properties" => properties_value(&line.line.properties),
            "options_with_values" => Value::array(
                line.product
                    .options
                    .iter()
                    .zip(&line.variant.options)
                    .map(|(option, value)| {
                        hash([
                            ("name", Value::from(&option.name)),
                            ("value", Value::from(value)),
                        ])
                    })
                    .collect(),
            ),
            "unit_price" => line.variant.unit_price.map_or(Value::Nil, Value::Int),
            "unit_price_measurement" => {
                VariantDrop::value(site, line.product_index, line.variant_index)
                    .get("unit_price_measurement")
            }
            "instructions" => hash([
                ("can_remove", Value::Bool(true)),
                ("can_update_quantity", Value::Bool(true)),
            ]),
            "successfully_fulfilled_quantity" => Value::Int(0),
            "message"
            | "error_message"
            | "selling_plan_allocation"
            | "parent_relationship"
            | "fulfillment"
            | "fulfillment_service" => Value::Nil,
            _ => return None,
        })
    }

    fn to_json(&self) -> Json {
        let lines = resolve_lines(&self.site);
        lines
            .get(self.index)
            .map_or(Json::Null, |line| line_json(&self.site, line))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}
