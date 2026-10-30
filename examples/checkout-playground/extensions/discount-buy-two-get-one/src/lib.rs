//! Buy two, get one free: for every `buy + free` units of a product with one of the tags,
//! `free` units cost nothing. The JSON metafield `$app:config.settings` of the discount, whose
//! `tags` are also the variables of the input query:
//!
//! ```json
//! {"tags": ["socks"], "buy": 2, "free": 1}
//! ```
//!
//! What it shows: a 100% discount on part of a line (`CartLineTarget.quantity`), one candidate
//! per line, all applied (`selectionStrategy: ALL`).

use serde::Deserialize;
use shopify_function::prelude::*;
use shopify_function::Result;

#[typegen("./schema.graphql")]
mod schema {
    #[query("./src/run.graphql")]
    pub mod run {}
}

use schema::run::input::cart::lines::Merchandise;

#[derive(Deserialize)]
#[serde(default)]
struct Settings {
    buy: i32,
    free: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { buy: 2, free: 1 }
    }
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
    let group = settings.buy + settings.free;
    if !input
        .discount()
        .discount_classes()
        .contains(&schema::DiscountClass::Product)
        || group <= 0
    {
        return Ok(schema::CartLinesDiscountsGenerateRunResult { operations: vec![] });
    }

    let mut candidates = Vec::new();
    for line in input.cart().lines() {
        let Merchandise::ProductVariant(variant) = line.merchandise() else {
            continue;
        };
        if !*variant.product().has_any_tag() {
            continue;
        }
        let free = line.quantity() / group * settings.free;
        log!(
            "{}: {} units, {free} free",
            variant.product().title(),
            line.quantity()
        );
        if free == 0 {
            continue;
        }
        candidates.push(schema::ProductDiscountCandidate {
            targets: vec![schema::ProductDiscountCandidateTarget::CartLine(
                schema::CartLineTarget {
                    id: line.id().clone(),
                    quantity: Some(free),
                },
            )],
            prerequisites: None,
            message: Some(format!("Buy {}, get {} free", settings.buy, settings.free)),
            value: schema::ProductDiscountCandidateValue::Percentage(schema::Percentage {
                value: Decimal(100.0),
            }),
            associated_discount_code: None,
        });
    }
    if candidates.is_empty() {
        return Ok(schema::CartLinesDiscountsGenerateRunResult { operations: vec![] });
    }
    Ok(schema::CartLinesDiscountsGenerateRunResult {
        operations: vec![schema::CartOperation::ProductDiscountsAdd(
            schema::ProductDiscountsAddOperation {
                selection_strategy: schema::ProductDiscountSelectionStrategy::All,
                candidates,
            },
        )],
    })
}
