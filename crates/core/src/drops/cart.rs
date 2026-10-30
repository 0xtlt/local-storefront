//! `cart` and `line_item`, computed from the session's cart lines.

use std::any::Any;

use indexmap::IndexMap;
use lsf_liquid::{Hash, Object, Value};
use serde_json::{Value as Json, json};

use super::localization::currency_value;
use super::media::{ImageDrop, image_base_url};
use super::product::{ProductDrop, VariantDrop, product_url};
use super::selling_plan::{allocation_value, line_allocation_json};
use super::{Memo, SiteRef, hash};
use crate::store::{
    Allocation, CartLine, Image, MediaKind, Product, SellingPlan, SellingPlanGroup, Store, Variant,
};
use crate::util::short_hash;

/// A cart line resolved against the store. Everything the storefront cart and the checkout say
/// of what a line costs comes from here, so that they cannot drift apart.
pub struct ResolvedLine<'a> {
    pub line: &'a CartLine,
    pub product_index: usize,
    pub variant_index: usize,
    pub product: &'a Product,
    pub variant: &'a Variant,
    /// The selling plan the line is bought with, when the product is still sold with it.
    pub selling_plan: Option<(&'a SellingPlanGroup, &'a SellingPlan)>,
}

impl ResolvedLine<'_> {
    /// What the variant costs with the line's selling plan.
    pub fn allocation(&self) -> Option<Allocation> {
        self.selling_plan
            .map(|(_, plan)| plan.allocation(self.variant))
    }

    /// The unit price before discounts: the price of the variant, or the one its selling plan
    /// sets. A selling plan changes the price itself; it is not a discount.
    pub fn original_price(&self) -> i64 {
        self.allocation()
            .map_or(self.variant.price, |allocation| allocation.price)
    }

    /// The compare-at price of a unit: the one of its selling plan when it has one (the price of
    /// the variant when the plan lowers it), the compare-at price of the variant otherwise.
    /// `None` when there is none; a caller that shows it only when it is above the price checks
    /// that itself.
    pub fn compare_at_price(&self) -> Option<i64> {
        match self.allocation() {
            Some(allocation) => allocation.compare_at_price,
            None => self.variant.compare_at_price,
        }
    }

    /// The unit price, taking quantity price breaks into account.
    pub fn unit_price(&self) -> i64 {
        if let Some(allocation) = self.allocation() {
            return allocation.price;
        }
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

    pub fn original_line_price(&self) -> i64 {
        self.original_price() * i64::from(self.line.quantity)
    }

    /// What is due at checkout for the line: less than its price when its selling plan takes
    /// a deposit.
    pub fn checkout_charge(&self) -> i64 {
        match self.allocation() {
            Some(allocation) => allocation.checkout_charge_amount * i64::from(self.line.quantity),
            None => self.line_price(),
        }
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

    /// The image of the line: the one of its variant when that is an image, the first one of
    /// its product otherwise.
    pub fn image(&self) -> Option<&'_ Image> {
        self.variant
            .media_index
            .and_then(|index| self.product.media.get(index))
            .filter(|media| matches!(media.kind, MediaKind::Image))
            .and_then(|media| media.preview.as_ref())
            .or_else(|| self.product.images().next())
    }
}

/// Resolves a cart line against the store. `None` when the variant no longer exists.
pub fn resolve_line<'a>(store: &'a Store, line: &'a CartLine) -> Option<ResolvedLine<'a>> {
    let (product_index, variant_index) = store.variant_location(line.variant_id)?;
    let product = &store.products[product_index];
    Some(ResolvedLine {
        line,
        product_index,
        variant_index,
        product,
        variant: &product.variants[variant_index],
        selling_plan: line
            .selling_plan
            .and_then(|id| store.selling_plan_of(product, id)),
    })
}

/// The key identifying a line: the variant plus a hash of the line's properties and selling
/// plan, as lines that differ by those stay separate.
pub fn line_key(line: &CartLine) -> String {
    let mut properties: Vec<String> = line
        .properties
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    if let Some(selling_plan) = line.selling_plan {
        properties.push(format!("\u{0}selling_plan={selling_plan}"));
    }
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
        .filter_map(|line| resolve_line(&site.store, line))
        .collect()
}

fn properties_value(properties: &IndexMap<String, String>) -> Value {
    let map: Hash = properties
        .iter()
        .map(|(key, value)| (key.clone(), Value::from(value)))
        .collect();
    Value::hash(map)
}

pub fn line_json(site: &SiteRef, line: &ResolvedLine<'_>) -> Json {
    let image = line.image();
    let url = format!(
        "{}?variant={}",
        product_url(site, line.product),
        line.variant.id
    );
    let default_variant = line.product.has_only_default_variant();
    let mut out = json!({
        "id": line.variant.id,
        "properties": line.line.properties,
        "quantity": line.line.quantity,
        "variant_id": line.variant.id,
        "key": line.key(),
        "title": line.title(),
        "price": line.unit_price(),
        "original_price": line.original_price(),
        "presentment_price": line.unit_price() as f64 / 100.0,
        "discounted_price": line.unit_price(),
        "line_price": line.line_price(),
        "original_line_price": line.original_line_price(),
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
    });
    // Only the lines bought with a selling plan have the key.
    if let Some((group, plan)) = line.selling_plan {
        out["selling_plan_allocation"] = line_allocation_json(group, plan, line.variant);
    }
    out
}

/// The cart as `/cart.js` returns it.
pub fn cart_json(site: &SiteRef) -> Json {
    let lines = resolve_lines(site);
    let total: i64 = lines.iter().map(ResolvedLine::line_price).sum();
    let original_total: i64 = lines.iter().map(ResolvedLine::original_line_price).sum();
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
        let original_total: i64 = lines.iter().map(ResolvedLine::original_line_price).sum();
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
            "total_price" | "items_subtotal_price" => Value::Int(total),
            "checkout_charge_amount" => {
                Value::Int(lines.iter().map(ResolvedLine::checkout_charge).sum())
            }
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
            "original_price" => Value::Int(line.original_price()),
            "line_price" | "final_line_price" => Value::Int(line.line_price()),
            "original_line_price" => Value::Int(line.original_line_price()),
            "total_discount" | "line_level_total_discount" => {
                Value::Int((line.original_price() - line.unit_price()) * quantity)
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
            "image" => ImageDrop::optional(site, line.image()),
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
            "unit_price" => match line.allocation() {
                Some(allocation) => allocation.unit_price.map_or(Value::Nil, Value::Int),
                None => line.variant.unit_price.map_or(Value::Nil, Value::Int),
            },
            "selling_plan_allocation" => match line.selling_plan {
                Some((group, plan)) => allocation_value(site, group, plan, line.variant),
                None => Value::Nil,
            },
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::load::{DataSource, LoadOptions, load};
    use crate::store::{Adjustment, CheckoutCharge, PriceAdjustment};

    /// The demo store, with the first variant at 32.00 with a compare-at price of 40.00, volume
    /// prices from 3 units, and a selling plan (5.00 off, half paid at checkout) on its product.
    /// Returns the store and the id of the variant.
    fn store_with_plan() -> (Store, u64) {
        let (mut store, _) = load(&DataSource::Demo, &LoadOptions::default());
        let group = store.selling_plan_groups.len();
        store.selling_plan_groups.push(SellingPlanGroup {
            id: "subscribe".to_string(),
            name: "Subscribe".to_string(),
            app_id: None,
            options: vec!["Delivery".to_string()],
            plans: vec![SellingPlan {
                id: 42,
                name: "Monthly".to_string(),
                description: None,
                options: vec!["Monthly".to_string()],
                price_adjustments: vec![PriceAdjustment {
                    adjustment: Adjustment::FixedAmount(500),
                    order_count: None,
                }],
                recurring_deliveries: true,
                checkout_charge: CheckoutCharge::Percentage(50),
            }],
        });
        let product = &mut store.products[0];
        product.selling_plan_groups = vec![group];
        let variant = &mut product.variants[0];
        variant.price = 3200;
        variant.compare_at_price = Some(4000);
        variant.quantity_price_breaks = vec![(3, 2500), (10, 2000)];
        let id = variant.id;
        (store, id)
    }

    fn line(variant_id: u64, quantity: u32, selling_plan: Option<u64>) -> CartLine {
        CartLine {
            variant_id,
            quantity,
            properties: Default::default(),
            selling_plan,
        }
    }

    #[test]
    fn a_line_costs_the_price_of_its_variant_and_the_breaks_of_its_quantity() {
        let (store, id) = store_with_plan();
        let prices = |quantity| {
            let cart_line = line(id, quantity, None);
            let resolved = resolve_line(&store, &cart_line).expect("the variant exists");
            (
                resolved.unit_price(),
                resolved.original_price(),
                resolved.line_price(),
                resolved.compare_at_price(),
            )
        };
        assert_eq!(prices(2), (3200, 3200, 6400, Some(4000)));
        assert_eq!(prices(3), (2500, 3200, 7500, Some(4000)));
        assert_eq!(prices(12), (2000, 3200, 24000, Some(4000)));
    }

    #[test]
    fn a_selling_plan_sets_the_price_and_no_break_stacks_with_it() {
        let (store, id) = store_with_plan();
        let cart_line = line(id, 12, Some(42));
        let resolved = resolve_line(&store, &cart_line).expect("the variant exists");
        assert_eq!(resolved.unit_price(), 2700, "32.00 less 5.00, not 20.00");
        assert_eq!(resolved.original_price(), 2700);
        assert_eq!(resolved.line_price(), 32400);
        assert_eq!(resolved.checkout_charge(), 12 * 1350);
        assert_eq!(
            resolved.compare_at_price(),
            Some(3200),
            "what the plan lowered"
        );
        let (group, plan) = resolved.selling_plan.expect("the plan");
        assert_eq!((group.name.as_str(), plan.id), ("Subscribe", 42));
    }

    #[test]
    fn a_plan_the_product_is_not_sold_with_is_ignored() {
        let (store, _) = store_with_plan();
        let other = store.products[1].variants[0].id;
        let cart_line = line(other, 1, Some(42));
        let resolved = resolve_line(&store, &cart_line).expect("the variant exists");
        assert!(resolved.selling_plan.is_none());
        assert_eq!(resolved.unit_price(), store.products[1].variants[0].price);
    }

    #[test]
    fn a_line_of_a_variant_that_is_gone_does_not_resolve() {
        let (store, _) = store_with_plan();
        assert!(resolve_line(&store, &line(1, 1, None)).is_none());
    }
}
