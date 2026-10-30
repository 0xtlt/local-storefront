//! An amount off one delivery option, written with the legacy Shipping Discount API
//! (`purchase.shipping-discount.run`). The JSON metafield `$app:config.settings` of the
//! discount:
//!
//! ```json
//! {"option": "express", "amount": "2.00"}
//! ```
//!
//! What it shows: a `deliveryOption` target. It only counts when the buyer picks that option: one
//! shipping discount applies per order, the one that takes the most off the selected option.

use serde::Deserialize;
use shopify_function::prelude::*;
use shopify_function::Result;

#[typegen("./schema.graphql")]
mod schema {
    #[query("./src/run.graphql")]
    pub mod run {}
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Settings {
    option: String,
    amount: String,
}

#[shopify_function]
fn run(input: schema::run::Input) -> Result<schema::FunctionRunResult> {
    let settings: Settings = input
        .discount_node()
        .metafield()
        .and_then(|metafield| serde_json::from_str(metafield.value()).ok())
        .unwrap_or_default();
    let amount: f64 = settings.amount.parse().unwrap_or(0.0);
    let mut discounts = Vec::new();
    for group in input.cart().delivery_groups() {
        for option in group.delivery_options() {
            if *option.handle() == settings.option && amount > 0.0 {
                discounts.push(schema::Discount {
                    targets: vec![schema::Target::DeliveryOption(
                        schema::DeliveryOptionTarget {
                            handle: option.handle().clone(),
                        },
                    )],
                    value: schema::Value::FixedAmount(schema::FixedAmount {
                        amount: Decimal(amount),
                    }),
                    message: Some(format!(
                        "{}: {} off (legacy)",
                        option.title().map(String::as_str).unwrap_or("Delivery"),
                        settings.amount
                    )),
                });
            }
        }
    }
    Ok(schema::FunctionRunResult { discounts })
}
