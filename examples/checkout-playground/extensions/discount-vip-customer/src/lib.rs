//! A percentage off the order for a logged-in customer with one of the tags. The JSON metafield
//! `$app:config.settings` of the discount, whose `tags` are also the variables of the input query:
//!
//! ```json
//! {"tags": ["vip"], "percentage": 20}
//! ```
//!
//! What it shows: `buyerIdentity.customer.hasAnyTag`. Log in as `jane.doe@example.com` (tag
//! `vip`) and the discount appears; log out and it goes.

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
    percentage: f64,
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
    let vip = input
        .cart()
        .buyer_identity()
        .and_then(|buyer| buyer.customer())
        .filter(|customer| *customer.has_any_tag());
    let Some(customer) = vip else {
        log!("not a VIP customer");
        return Ok(schema::CartLinesDiscountsGenerateRunResult { operations: vec![] });
    };
    if !input
        .discount()
        .discount_classes()
        .contains(&schema::DiscountClass::Order)
    {
        return Ok(schema::CartLinesDiscountsGenerateRunResult { operations: vec![] });
    }
    log!("VIP: {}", customer.display_name());
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
                    message: Some(format!("VIP: {}% off", settings.percentage)),
                    value: schema::OrderDiscountCandidateValue::Percentage(schema::Percentage {
                        value: Decimal(settings.percentage),
                    }),
                    conditions: None,
                    associated_discount_code: None,
                }],
            },
        )],
    })
}
