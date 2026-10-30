//! An amount off shipping. Two code discounts of the store data use it, `SHIP1` (1.00 off) and
//! `SHIP3` (3.00 off), with the JSON metafield `$app:config.settings`:
//!
//! ```json
//! {"amount": "3.00"}
//! ```
//!
//! What it shows: only one shipping discount applies per order, the best for the buyer. Enter
//! both codes: 3.00 off applies, and 1.00 off is listed under "Discounts not applied" in the
//! Functions dialog (`skippedDiscounts`).

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
    amount: String,
}

#[shopify_function]
fn cart_delivery_options_discounts_generate_run(
    input: schema::run::Input,
) -> Result<schema::CartDeliveryOptionsDiscountsGenerateRunResult> {
    let settings: Settings = input
        .discount()
        .metafield()
        .and_then(|metafield| serde_json::from_str(metafield.value()).ok())
        .unwrap_or_default();
    let amount: f64 = settings.amount.parse().unwrap_or(0.0);
    if amount <= 0.0
        || !input
            .discount()
            .discount_classes()
            .contains(&schema::DiscountClass::Shipping)
    {
        return Ok(schema::CartDeliveryOptionsDiscountsGenerateRunResult { operations: vec![] });
    }
    let code = input.triggering_discount_code().cloned();
    let candidates = input
        .cart()
        .delivery_groups()
        .iter()
        .map(|group| schema::DeliveryDiscountCandidate {
            targets: vec![schema::DeliveryDiscountCandidateTarget::DeliveryGroup(
                schema::DeliveryGroupTarget {
                    id: group.id().clone(),
                },
            )],
            message: Some(format!("{} off shipping", settings.amount)),
            value: schema::DeliveryDiscountCandidateValue::FixedAmount(schema::FixedAmount {
                amount: Decimal(amount),
            }),
            associated_discount_code: code
                .clone()
                .map(|code| schema::AssociatedDiscountCode { code }),
        })
        .collect();
    Ok(schema::CartDeliveryOptionsDiscountsGenerateRunResult {
        operations: vec![schema::DeliveryOperation::DeliveryDiscountsAdd(
            schema::DeliveryDiscountsAddOperation {
                selection_strategy: schema::DeliveryDiscountSelectionStrategy::All,
                candidates,
            },
        )],
    })
}
