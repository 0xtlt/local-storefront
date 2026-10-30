//! Updates, expands and merges lines as their properties ask, in one answer:
//!
//! - `_engraving: "text"`: the title says the text and the unit price grows by `engravingPrice`;
//! - `_gift_wrap: "yes"`: the unit price grows by `giftWrapPrice`;
//! - `_bundle_of: "<variant id>:<quantity>,..."`: the line expands into those variants;
//! - `_set: "<name>"`: the lines with the same set merge into the parent variant of that set.
//!
//! The JSON metafield `$app:config.settings` of the cart transform:
//!
//! ```json
//! {"engravingPrice": "6.00", "giftWrapPrice": "3.00", "sets": {"cold": "gid://shopify/ProductVariant/2770579135767"}}
//! ```
//!
//! What it shows: line properties (`attribute(key:)` of a line, under aliases), and several
//! kinds of operations at once.

use std::collections::BTreeMap;

use serde::Deserialize;
use shopify_function::prelude::*;
use shopify_function::Result;

#[typegen("./schema.graphql")]
mod schema {
    #[query("./src/run.graphql")]
    pub mod run {}
}

use schema::run::input::cart::lines::Merchandise;

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Settings {
    engraving_price: String,
    gift_wrap_price: String,
    sets: BTreeMap<String, String>,
}

#[shopify_function]
fn cart_transform_run(input: schema::run::Input) -> Result<schema::CartTransformRunResult> {
    let settings: Settings = input
        .cart_transform()
        .metafield()
        .and_then(|metafield| serde_json::from_str(metafield.value()).ok())
        .unwrap_or_default();
    let engraving_price: f64 = settings.engraving_price.parse().unwrap_or(0.0);
    let gift_wrap_price: f64 = settings.gift_wrap_price.parse().unwrap_or(0.0);
    let mut operations = Vec::new();
    let mut sets: BTreeMap<&str, Vec<&schema::run::input::cart::Lines>> = BTreeMap::new();

    for line in input.cart().lines() {
        let title = match line.merchandise() {
            Merchandise::ProductVariant(variant) => variant.product().title().clone(),
            _ => "Item".to_string(),
        };
        if let Some(set) = line.set().and_then(|attribute| attribute.value()) {
            sets.entry(set.as_str()).or_default().push(line);
            continue;
        }
        if let Some(items) = line.bundle_of().and_then(|attribute| attribute.value()) {
            let expanded: Vec<schema::ExpandedItem> = items
                .split(',')
                .filter_map(|item| {
                    let (id, quantity) = item.trim().split_once(':')?;
                    Some(schema::ExpandedItem {
                        merchandise_id: format!("gid://shopify/ProductVariant/{id}"),
                        quantity: quantity.parse().ok()?,
                        price: None,
                        attributes: Some(vec![]),
                    })
                })
                .collect();
            log!("{title}: expands into {} items", expanded.len());
            operations.push(schema::Operation::LineExpand(schema::LineExpandOperation {
                cart_line_id: line.id().clone(),
                expanded_cart_items: expanded,
                price: None,
                title: None,
                image: None,
            }));
            continue;
        }
        let engraving = line.engraving().and_then(|attribute| attribute.value());
        let gift_wrap = line
            .gift_wrap()
            .and_then(|attribute| attribute.value())
            .is_some_and(|value| value == "yes");
        if engraving.is_none() && !gift_wrap {
            continue;
        }
        let mut price = line.cost().amount_per_quantity().amount().0;
        let mut new_title = title.clone();
        if let Some(text) = engraving {
            price += engraving_price;
            new_title = format!("{new_title}, engraved \"{text}\"");
        }
        if gift_wrap {
            price += gift_wrap_price;
            new_title = format!("{new_title}, gift wrapped");
        }
        operations.push(schema::Operation::LineUpdate(schema::LineUpdateOperation {
            cart_line_id: line.id().clone(),
            title: Some(new_title),
            image: None,
            price: Some(schema::LineUpdateOperationPriceAdjustment {
                adjustment: schema::LineUpdateOperationPriceAdjustmentValue::FixedPricePerUnit(
                    schema::LineUpdateOperationFixedPricePerUnitAdjustment {
                        amount: Decimal(price),
                    },
                ),
            }),
        }));
    }

    for (name, lines) in sets {
        let Some(parent) = settings.sets.get(name) else {
            log!("set {name}: no parent variant in the settings");
            continue;
        };
        log!("set {name}: {} lines", lines.len());
        operations.push(schema::Operation::LinesMerge(schema::LinesMergeOperation {
            parent_variant_id: parent.clone(),
            title: None,
            attributes: Some(vec![]),
            cart_lines: lines
                .iter()
                .map(|line| schema::CartLineInput {
                    cart_line_id: line.id().clone(),
                    quantity: *line.quantity(),
                })
                .collect(),
            price: None,
            image: None,
        }));
    }
    Ok(schema::CartTransformRunResult { operations })
}
