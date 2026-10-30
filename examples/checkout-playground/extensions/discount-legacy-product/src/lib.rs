//! A percentage off the products with one of the tags, written with the legacy Product Discount
//! API (`purchase.product-discount.run`): the discount is `discountNode`, and the answer is
//! `{discounts, discountApplicationStrategy}` rather than operations. The JSON metafield
//! `$app:config.settings` of the discount, whose `tags` are also the variables of the input query:
//!
//! ```json
//! {"tags": ["wool"], "percentage": 10}
//! ```
//!
//! What it shows: a legacy target still runs (the home page flags it, with the target that
//! replaces it).

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
#[serde(default)]
struct Settings {
    percentage: f64,
}

#[shopify_function]
fn run(input: schema::run::Input) -> Result<schema::FunctionRunResult> {
    let settings: Settings = input
        .discount_node()
        .metafield()
        .and_then(|metafield| serde_json::from_str(metafield.value()).ok())
        .unwrap_or_default();
    let mut targets = Vec::new();
    for line in input.cart().lines() {
        let Merchandise::ProductVariant(variant) = line.merchandise() else {
            continue;
        };
        if *variant.product().has_any_tag() {
            log!("{}: tagged", variant.product().title());
            targets.push(schema::Target::CartLine(schema::CartLineTarget {
                id: line.id().clone(),
                quantity: None,
            }));
        }
    }
    let discounts = if targets.is_empty() || settings.percentage <= 0.0 {
        vec![]
    } else {
        vec![schema::Discount {
            targets,
            value: schema::Value::Percentage(schema::Percentage {
                value: Decimal(settings.percentage),
            }),
            message: Some(format!("{}% off (legacy)", settings.percentage)),
        }]
    };
    Ok(schema::FunctionRunResult {
        discounts,
        discount_application_strategy: schema::DiscountApplicationStrategy::First,
    })
}
