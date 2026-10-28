//! A fixed amount off the order above a subtotal, written with the legacy Order Discount API
//! (`purchase.order-discount.run`). The JSON metafield `$app:config.settings` of the discount:
//!
//! ```json
//! {"amount": "5.00", "minimumSubtotal": 150}
//! ```
//!
//! What it shows: the legacy `orderSubtotal` target, with `excludedVariantIds`.

use serde::Deserialize;
use shopify_function::prelude::*;
use shopify_function::Result;

#[typegen("./schema.graphql")]
mod schema {
    #[query("./src/run.graphql")]
    pub mod run {}
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Settings {
    amount: String,
    minimum_subtotal: f64,
}

#[shopify_function]
fn run(input: schema::run::Input) -> Result<schema::FunctionRunResult> {
    let settings: Settings = input
        .discount_node()
        .metafield()
        .and_then(|metafield| serde_json::from_str(metafield.value()).ok())
        .unwrap_or_default();
    let subtotal = input.cart().cost().subtotal_amount().amount().0;
    let amount: f64 = settings.amount.parse().unwrap_or(0.0);
    log!("subtotal {subtotal}, minimum {}", settings.minimum_subtotal);
    let discounts = if subtotal < settings.minimum_subtotal || amount <= 0.0 {
        vec![]
    } else {
        vec![schema::Discount {
            targets: vec![schema::Target::OrderSubtotal(schema::OrderSubtotalTarget {
                excluded_variant_ids: vec![],
            })],
            value: schema::Value::FixedAmount(schema::FixedAmount {
                amount: Decimal(amount),
            }),
            conditions: None,
            message: Some(format!("{} off (legacy)", settings.amount)),
        }]
    };
    Ok(schema::FunctionRunResult {
        discounts,
        discount_application_strategy: schema::DiscountApplicationStrategy::First,
    })
}
