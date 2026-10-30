//! A percentage off the order by the country of the buyer, or else by their market. The JSON
//! metafield `$app:config.settings` of the discount:
//!
//! ```json
//! {"countries": {"DE": 15}, "markets": {"europe": 10}}
//! ```
//!
//! What it shows: `localization.country` and `localization.market` (the `markets` of the store
//! data: `europe` is FR and DE), which follow the shipping address. A buyer in France gets 10%,
//! one in Germany 15%, one in the US nothing.

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
    markets: BTreeMap<String, f64>,
    countries: BTreeMap<String, f64>,
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
    let market = input.localization().market().handle();
    let country = input.localization().country().iso_code().clone();
    let (percentage, reason) = match settings.countries.get(&country) {
        Some(percentage) => (*percentage, country.clone()),
        None => (
            settings
                .markets
                .get(market.as_str())
                .copied()
                .unwrap_or(0.0),
            format!("market {market}"),
        ),
    };
    log!("market {market}, country {country}: {percentage}%");
    if percentage <= 0.0
        || !input
            .discount()
            .discount_classes()
            .contains(&schema::DiscountClass::Order)
    {
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
                    message: Some(format!("{percentage}% off in {reason}")),
                    value: schema::OrderDiscountCandidateValue::Percentage(schema::Percentage {
                        value: Decimal(percentage),
                    }),
                    conditions: None,
                    associated_discount_code: None,
                }],
            },
        )],
    })
}
