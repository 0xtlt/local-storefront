//! A fixed amount off the order once the subtotal reaches a minimum. The JSON metafield
//! `$app:config.settings` of the discount:
//!
//! ```json
//! {"amount": "10.00", "minimumSubtotal": 100}
//! ```
//!
//! What it shows: an ORDER discount of a fixed amount (`orderDiscountsAdd` with `fixedAmount`),
//! and nothing at all below the minimum.

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
fn cart_lines_discounts_generate_run(
    input: schema::run::Input,
) -> Result<schema::CartLinesDiscountsGenerateRunResult> {
    let settings: Settings = input
        .discount()
        .metafield()
        .and_then(|metafield| serde_json::from_str(metafield.value()).ok())
        .unwrap_or_default();
    let subtotal = input.cart().cost().subtotal_amount().amount().0;
    let amount: f64 = settings.amount.parse().unwrap_or(0.0);
    if !input
        .discount()
        .discount_classes()
        .contains(&schema::DiscountClass::Order)
        || amount <= 0.0
        || subtotal < settings.minimum_subtotal
    {
        log!("subtotal {subtotal}: below {}", settings.minimum_subtotal);
        return Ok(schema::CartLinesDiscountsGenerateRunResult { operations: vec![] });
    }
    Ok(schema::CartLinesDiscountsGenerateRunResult {
        operations: vec![schema::CartOperation::OrderDiscountsAdd(
            schema::OrderDiscountsAddOperation {
                selection_strategy: schema::OrderDiscountSelectionStrategy::First,
                candidates: vec![schema::OrderDiscountCandidate {
                    targets: vec![schema::OrderDiscountCandidateTarget::OrderSubtotal(
                        schema::OrderSubtotalTarget {
                            excluded_cart_line_ids: vec![],
                        },
                    )],
                    message: Some(format!(
                        "{} off orders over {}",
                        settings.amount, settings.minimum_subtotal
                    )),
                    value: schema::OrderDiscountCandidateValue::FixedAmount(schema::FixedAmount {
                        amount: Decimal(amount),
                    }),
                    conditions: None,
                    associated_discount_code: None,
                }],
            },
        )],
    })
}
