//! One discount, two targets: from a number of items in the cart, a percentage off every line
//! (`cart.lines.discounts.generate.run`) and a percentage off shipping
//! (`cart.delivery-options.discounts.generate.run`). The JSON metafield `$app:config.settings` of
//! the discount, which has the classes PRODUCT and SHIPPING:
//!
//! ```json
//! {"minimumItems": 10, "percentage": 5, "shippingPercentage": 50}
//! ```
//!
//! What it shows: a Function with several targets, each with its own input query and export.

use serde::Deserialize;
use shopify_function::prelude::*;
use shopify_function::Result;

#[typegen("./schema.graphql")]
mod schema {
    #[query("./src/cart_lines_discounts_generate_run.graphql")]
    pub mod cart_lines_discounts_generate_run {}

    #[query("./src/cart_delivery_options_discounts_generate_run.graphql")]
    pub mod cart_delivery_options_discounts_generate_run {}
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Settings {
    minimum_items: i32,
    percentage: f64,
    shipping_percentage: f64,
}

fn settings(raw: Option<&String>) -> Settings {
    raw.and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default()
}

#[shopify_function]
fn cart_lines_discounts_generate_run(
    input: schema::cart_lines_discounts_generate_run::Input,
) -> Result<schema::CartLinesDiscountsGenerateRunResult> {
    let settings = settings(
        input
            .discount()
            .metafield()
            .map(|metafield| metafield.value()),
    );
    let items: i32 = input
        .cart()
        .lines()
        .iter()
        .map(|line| *line.quantity())
        .sum();
    log!("{items} items, {} needed", settings.minimum_items);
    if items < settings.minimum_items
        || settings.percentage <= 0.0
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
                    message: Some(format!("{items} items: {}% off", settings.percentage)),
                    value: schema::ProductDiscountCandidateValue::Percentage(schema::Percentage {
                        value: Decimal(settings.percentage),
                    }),
                    associated_discount_code: None,
                }],
            },
        )],
    })
}

#[shopify_function]
fn cart_delivery_options_discounts_generate_run(
    input: schema::cart_delivery_options_discounts_generate_run::Input,
) -> Result<schema::CartDeliveryOptionsDiscountsGenerateRunResult> {
    let settings = settings(
        input
            .discount()
            .metafield()
            .map(|metafield| metafield.value()),
    );
    let items: i32 = input
        .cart()
        .lines()
        .iter()
        .map(|line| *line.quantity())
        .sum();
    if items < settings.minimum_items
        || settings.shipping_percentage <= 0.0
        || !input
            .discount()
            .discount_classes()
            .contains(&schema::DiscountClass::Shipping)
    {
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
            message: Some(format!(
                "{items} items: {}% off shipping",
                settings.shipping_percentage
            )),
            value: schema::DeliveryDiscountCandidateValue::Percentage(schema::Percentage {
                value: Decimal(settings.shipping_percentage),
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
