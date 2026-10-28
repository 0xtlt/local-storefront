//! Free shipping once the subtotal reaches a threshold. The JSON metafield
//! `$app:config.settings` of the discount:
//!
//! ```json
//! {"threshold": 75}
//! ```
//!
//! What it shows: the `cart.delivery-options.discounts.generate.run` target, a 100% discount on
//! the delivery group, which the shipping discounts of other Functions compete with.

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
    threshold: f64,
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
    let subtotal = input.cart().cost().subtotal_amount().amount().0;
    if !input
        .discount()
        .discount_classes()
        .contains(&schema::DiscountClass::Shipping)
        || subtotal < settings.threshold
    {
        log!(
            "subtotal {subtotal}: {} more for free shipping",
            settings.threshold - subtotal
        );
        return Ok(schema::CartDeliveryOptionsDiscountsGenerateRunResult { operations: vec![] });
    }
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
            message: Some(format!("Free shipping over {}", settings.threshold)),
            value: schema::DeliveryDiscountCandidateValue::Percentage(schema::Percentage {
                value: Decimal(100.0),
            }),
            associated_discount_code: None,
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
