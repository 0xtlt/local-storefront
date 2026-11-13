//! `cart` and `line_item`, computed from the session's cart lines.

use std::any::Any;
use std::borrow::Cow;
use std::sync::Arc;

use indexmap::IndexMap;
use lsf_core::cart::{ResolvedLine, resolve_line};
use lsf_core::util::short_hash;
use lsf_liquid::{Hash, Object, Value};
use serde_json::{Value as Json, json};

use super::localization::currency_value;
use super::media::{ImageDrop, image_base_url};
use super::product::{ProductDrop, VariantDrop, product_url};
use super::selling_plan::{allocation_value, line_allocation_json};
use super::{Memo, SiteRef, hash};

/// The session's cart lines that still point at existing variants.
pub fn resolve_lines(site: &SiteRef) -> Vec<ResolvedLine<'_>> {
    site.session
        .cart_lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let mut resolved = resolve_line(&site.store, line)?;
            resolved.transformed = site.cart_transforms.get(index).and_then(Option::as_ref);
            Some(resolved)
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

/// `line_item.properties` as Shopify's Liquid has it: a `for` loop and `| json` give
/// `[name, value]` pairs, `properties.name` and `properties['name']` a value, and `| size` the
/// number of properties, but `.size` is nil: a theme that tests `item.properties.size > 0`
/// never shows its properties on a store, and the docs test `item.properties == empty`.
/// https://shopify.dev/docs/storefronts/themes/architecture/templates/cart#display-line-item-properties
struct LineItemProperties(IndexMap<String, String>);

impl Object for LineItemProperties {
    fn type_name(&self) -> &str {
        "line_item_properties"
    }

    fn get(&self, key: &str) -> Option<Value> {
        match self.0.get(key) {
            Some(value) => Some(Value::from(value)),
            // `.first` is the first pair, as on a hash; anything else, `.size` included, is nil.
            None if key == "first" => None,
            None => Some(Value::Nil),
        }
    }

    fn items(&self) -> Option<Arc<Vec<Value>>> {
        Some(Arc::new(
            self.0
                .iter()
                .map(|(key, value)| Value::array(vec![Value::from(key), Value::from(value)]))
                .collect(),
        ))
    }

    fn render(&self) -> Cow<'_, str> {
        Cow::Owned(properties_value(&self.0).to_str().into_owned())
    }

    fn to_json(&self) -> Json {
        Json::Array(
            self.0
                .iter()
                .map(|(key, value)| json!([key, value]))
                .collect(),
        )
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The cart attributes a theme and the Ajax API see: all but the private ones, whose name starts
/// with `__`. "Private cart attributes are not available in the Liquid `cart.attributes` object
/// or the Ajax API." The checkout and the order still have them.
/// https://shopify.dev/docs/api/ajax/reference/cart#private-cart-attributes
pub fn public_attributes(attributes: &IndexMap<String, String>) -> IndexMap<String, String> {
    attributes
        .iter()
        .filter(|(name, _)| !name.starts_with("__"))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
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
                // A product image without alt text has the product's title, as on Shopify.
                "alt": if image.alt.is_empty() { &line.product.title } else { &image.alt },
                "height": image.height,
                "url": image_base_url(site, image),
                "width": image.width,
            }),
            // A product without an image: an object whose fields are null, as on Shopify.
            None => json!({
                "aspect_ratio": null,
                "alt": null,
                "height": null,
                "url": null,
                "width": null,
            }),
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
        "has_components": line.transformed.is_some_and(|line| !line.components.is_empty()),
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
        "attributes": public_attributes(&site.session.cart_attributes),
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
            "attributes" => properties_value(&public_attributes(&site.session.cart_attributes)),
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

/// A component of a line a cart transform expanded, as `line_item.item_components` gives it: a
/// line item of its variant, its quantity and its share of the price of the line. `None` for a
/// variant the store does not have.
fn component_value(site: &SiteRef, component: &lsf_core::cart::LineComponent) -> Option<Value> {
    let (product_index, variant_index) = site.store.variant_location(component.variant_id)?;
    let product = &site.store.products[product_index];
    let variant = &product.variants[variant_index];
    let title = if product.has_only_default_variant() {
        product.title.clone()
    } else {
        format!("{} - {}", product.title, variant.title)
    };
    let price = component.line_price / i64::from(component.quantity.max(1));
    Some(hash([
        ("id", Value::Int(variant.id as i64)),
        ("variant_id", Value::Int(variant.id as i64)),
        ("product_id", Value::Int(product.id as i64)),
        ("title", Value::from(title)),
        ("quantity", Value::Int(i64::from(component.quantity))),
        ("sku", Value::from(&variant.sku)),
        ("vendor", Value::from(&product.vendor)),
        ("price", Value::Int(price)),
        ("final_price", Value::Int(price)),
        ("line_price", Value::Int(component.line_price)),
        ("final_line_price", Value::Int(component.line_price)),
        ("product", ProductDrop::value(site, product_index)),
        (
            "variant",
            VariantDrop::value(site, product_index, variant_index),
        ),
    ]))
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
            // 0-based, unlike the 1-based `line` of `/cart/change.js`: themes post `index | plus: 1`.
            "index" => Value::Int(self.index as i64),
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
            | "tax_lines" => Value::array(Vec::new()),
            "item_components" => match line.transformed {
                Some(transformed) => Value::array(
                    transformed
                        .components
                        .iter()
                        .filter_map(|component| component_value(site, component))
                        .collect(),
                ),
                None => Value::array(Vec::new()),
            },
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
                lsf_core::urls::encode_component(&line.key())
            )),
            "image" => ImageDrop::optional(site, line.image()),
            "properties" => Value::object(LineItemProperties(line.line.properties.clone())),
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
    use lsf_core::store::load::{DataSource, LoadOptions, load};
    use lsf_core::store::{
        Adjustment, CartLine, CheckoutCharge, PriceAdjustment, SellingPlan, SellingPlanGroup, Store,
    };

    use super::*;

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
