//! A percentage off every line for a B2B buyer, by the external id of the company location they
//! buy for. The JSON metafield `$app:config.settings` of the discount:
//!
//! ```json
//! {"locations": {"NW-SEA": 12, "NW-PDX": 8}}
//! ```
//!
//! What it shows: `buyerIdentity.purchasingCompany` (company and location). Switch the location
//! of `alex.morgan@example.com` in the checkout and the percentage follows.

use std::collections::BTreeMap;

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
    locations: BTreeMap<String, f64>,
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
    let Some(company) = input
        .cart()
        .buyer_identity()
        .and_then(|buyer| buyer.purchasing_company())
    else {
        log!("not a B2B buyer");
        return Ok(schema::CartLinesDiscountsGenerateRunResult { operations: vec![] });
    };
    let location = company.location();
    let percentage = location
        .external_id()
        .and_then(|id| settings.locations.get(id))
        .copied()
        .unwrap_or(0.0);
    log!(
        "{} at {}: {percentage}%",
        company.company().name(),
        location.name()
    );
    if percentage <= 0.0
        || !input
            .discount()
            .discount_classes()
            .contains(&schema::DiscountClass::Product)
    {
        return Ok(schema::CartLinesDiscountsGenerateRunResult { operations: vec![] });
    }
    let targets = input
        .cart()
        .lines()
        .iter()
        .map(|line| {
            schema::ProductDiscountCandidateTarget::CartLine(schema::CartLineTarget {
                id: line.id().clone(),
                quantity: None,
            })
        })
        .collect();
    Ok(schema::CartLinesDiscountsGenerateRunResult {
        operations: vec![schema::CartOperation::ProductDiscountsAdd(
            schema::ProductDiscountsAddOperation {
                selection_strategy: schema::ProductDiscountSelectionStrategy::First,
                candidates: vec![schema::ProductDiscountCandidate {
                    targets,
                    prerequisites: None,
                    message: Some(format!("{}: {percentage}% contract price", location.name())),
                    value: schema::ProductDiscountCandidateValue::Percentage(schema::Percentage {
                        value: Decimal(percentage),
                    }),
                    associated_discount_code: None,
                }],
            },
        )],
    })
}
