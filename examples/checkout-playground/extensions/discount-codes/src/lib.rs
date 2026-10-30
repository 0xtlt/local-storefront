//! Code discounts: several discounts of the store data use this Function, each with its code
//! and its settings (the JSON metafield `$app:config.settings`):
//!
//! ```json
//! {"percentage": 10}
//! {"amount": "5.00"}
//! {"percentage": 20, "minimumSubtotal": 200}
//! ```
//!
//! It runs once per discount whose code the buyer entered, and links what it takes off to that
//! code (`associatedDiscountCode`). Below `minimumSubtotal` it answers nothing. What it shows:
//! `triggeringDiscountCode`, `enteredDiscountCodes`, codes that apply and codes that do not (a
//! code no discount has is refused by the checkout).

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
    percentage: Option<f64>,
    amount: Option<String>,
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
    let entered: Vec<&str> = input
        .entered_discount_codes()
        .iter()
        .map(|code| code.code().as_str())
        .collect();
    let Some(code) = input.triggering_discount_code().cloned() else {
        log!("no code triggers this discount (entered: {entered:?})");
        return Ok(schema::CartLinesDiscountsGenerateRunResult { operations: vec![] });
    };
    let subtotal = input.cart().cost().subtotal_amount().amount().0;
    log!("code {code} (entered: {entered:?}), subtotal {subtotal}");
    if subtotal < settings.minimum_subtotal
        || !input
            .discount()
            .discount_classes()
            .contains(&schema::DiscountClass::Order)
    {
        log!("{code}: needs a subtotal of {}", settings.minimum_subtotal);
        return Ok(schema::CartLinesDiscountsGenerateRunResult { operations: vec![] });
    }
    let (value, message) = match (&settings.percentage, &settings.amount) {
        (Some(percentage), _) => (
            schema::OrderDiscountCandidateValue::Percentage(schema::Percentage {
                value: Decimal(*percentage),
            }),
            format!("{code}: {percentage}% off"),
        ),
        (None, Some(amount)) => (
            schema::OrderDiscountCandidateValue::FixedAmount(schema::FixedAmount {
                amount: Decimal(amount.parse().unwrap_or(0.0)),
            }),
            format!("{code}: {amount} off"),
        ),
        (None, None) => {
            return Ok(schema::CartLinesDiscountsGenerateRunResult { operations: vec![] })
        }
    };
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
                    message: Some(message),
                    value,
                    conditions: None,
                    associated_discount_code: Some(schema::AssociatedDiscountCode { code }),
                }],
            },
        )],
    })
}
