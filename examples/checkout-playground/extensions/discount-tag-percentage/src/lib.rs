//! A percentage off the lines whose product has one of some tags, or is in one of some
//! collections. The JSON metafield `$app:config.settings` of the discount holds the settings, and
//! is also the variables of the input query (`[extensions.input.variables]`): `hasAnyTag` and
//! `inAnyCollection` get the lists from it.
//!
//! ```json
//! {"percentage": 15, "tags": ["sale"], "collections": ["gid://shopify/Collection/6759449335775"]}
//! ```
//!
//! What it shows: input query variables, `hasAnyTag`, `inAnyCollection`, a union
//! (`merchandise`) read with `__typename`.

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
fn cart_lines_discounts_generate_run(
    input: schema::run::Input,
) -> Result<schema::CartLinesDiscountsGenerateRunResult> {
    let settings: Settings = input
        .discount()
        .metafield()
        .and_then(|metafield| serde_json::from_str(metafield.value()).ok())
        .unwrap_or_default();
    if !input
        .discount()
        .discount_classes()
        .contains(&schema::DiscountClass::Product)
        || settings.percentage <= 0.0
    {
        return Ok(schema::CartLinesDiscountsGenerateRunResult { operations: vec![] });
    }

    let mut targets = Vec::new();
    for line in input.cart().lines() {
        let Merchandise::ProductVariant(variant) = line.merchandise() else {
            continue;
        };
        let product = variant.product();
        if *product.has_any_tag() || *product.in_any_collection() {
            log!("{}: tagged or in a collection", product.title());
            targets.push(schema::ProductDiscountCandidateTarget::CartLine(
                schema::CartLineTarget {
                    id: line.id().clone(),
                    quantity: None,
                },
            ));
        }
    }
    if targets.is_empty() {
        return Ok(schema::CartLinesDiscountsGenerateRunResult { operations: vec![] });
    }
    Ok(schema::CartLinesDiscountsGenerateRunResult {
        operations: vec![schema::CartOperation::ProductDiscountsAdd(
            schema::ProductDiscountsAddOperation {
                selection_strategy: schema::ProductDiscountSelectionStrategy::First,
                candidates: vec![schema::ProductDiscountCandidate {
                    targets,
                    prerequisites: None,
                    message: Some(format!("{}% off", settings.percentage)),
                    value: schema::ProductDiscountCandidateValue::Percentage(schema::Percentage {
                        value: Decimal(settings.percentage),
                    }),
                    associated_discount_code: None,
                }],
            },
        )],
    })
}
