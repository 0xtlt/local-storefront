//! Merges the lines of some variants into one parent product, at a discount, once every one of
//! them is in the cart. The JSON metafield `$app:config.settings` of the cart transform (a black
//! tee and a navy beanie make the Cold-Weather Set):
//!
//! ```json
//! {"variants": ["gid://shopify/ProductVariant/3784131125857", "gid://shopify/ProductVariant/5803446973649"],
//!  "parentVariant": "gid://shopify/ProductVariant/2770579135767", "title": "Cold-Weather Set", "percentageDecrease": 10}
//! ```
//!
//! What it shows: `linesMerge` with a `percentageDecrease`. With two tees and one beanie, one set
//! is made and the other tee stays on a line of its own.

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
    variants: Vec<String>,
    parent_variant: String,
    title: Option<String>,
    percentage_decrease: Option<f64>,
}

#[shopify_function]
fn cart_transform_run(input: schema::run::Input) -> Result<schema::CartTransformRunResult> {
    let settings: Settings = input
        .cart_transform()
        .metafield()
        .and_then(|metafield| serde_json::from_str(metafield.value()).ok())
        .unwrap_or_default();
    let mut cart_lines = Vec::new();
    for wanted in &settings.variants {
        let line = input
            .cart()
            .lines()
            .iter()
            .find(|line| match line.merchandise() {
                Merchandise::ProductVariant(variant) => variant.id() == wanted,
                _ => false,
            });
        match line {
            Some(line) => cart_lines.push(line),
            None => {
                log!("{wanted} is not in the cart: no set");
                return Ok(schema::CartTransformRunResult { operations: vec![] });
            }
        }
    }
    if cart_lines.is_empty() {
        return Ok(schema::CartTransformRunResult { operations: vec![] });
    }
    // As many sets as the smallest line allows.
    let sets = cart_lines
        .iter()
        .map(|line| *line.quantity())
        .min()
        .unwrap_or(0);
    log!("{sets} set(s)");
    Ok(schema::CartTransformRunResult {
        operations: vec![schema::Operation::LinesMerge(schema::LinesMergeOperation {
            parent_variant_id: settings.parent_variant,
            title: settings.title,
            attributes: Some(vec![]),
            cart_lines: cart_lines
                .iter()
                .map(|line| schema::CartLineInput {
                    cart_line_id: line.id().clone(),
                    quantity: sets,
                })
                .collect(),
            price: settings
                .percentage_decrease
                .map(|value| schema::PriceAdjustment {
                    percentage_decrease: Some(schema::PriceAdjustmentValue {
                        value: Decimal(value),
                    }),
                }),
            image: None,
        })],
    })
}
