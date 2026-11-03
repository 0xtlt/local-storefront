//! A discount Function, configured by the JSON metafield `$app:config.settings` of its discount:
//!
//! ```json
//! {"minimumQuantity": 3, "percentage": 10, "orderPercentage": 5, "freeShippingAbove": 50}
//! ```
//!
//! - PRODUCT class: `percentage` off every line with at least `minimumQuantity` items.
//! - ORDER class: `orderPercentage` off the order.
//! - SHIPPING class: free shipping above `freeShippingAbove` of subtotal.
//!
//! When the buyer entered a code that triggers the discount, the discounts are linked to it.

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
struct Config {
    minimum_quantity: i32,
    percentage: f64,
    order_percentage: f64,
    free_shipping_above: Option<f64>,
}

fn config(raw: Option<&str>) -> Config {
    raw.and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default()
}

#[shopify_function]
fn cart_lines_discounts_generate_run(
    input: schema::cart_lines_discounts_generate_run::Input,
) -> Result<schema::CartLinesDiscountsGenerateRunResult> {
    let config = config(input.discount().metafield().map(|m| m.value().as_str()));
    let classes = input.discount().discount_classes();
    let code = input.triggering_discount_code().cloned();
    log!("volume discount: config {{ minimum quantity {} }}, code {:?}", config.minimum_quantity, code);
    let associated = code.map(|code| schema::AssociatedDiscountCode { code });

    let mut operations = Vec::new();

    if classes.contains(&schema::DiscountClass::Product) && config.percentage > 0.0 {
        let targets: Vec<_> = input
            .cart()
            .lines()
            .iter()
            .filter(|line| *line.quantity() >= config.minimum_quantity)
            .map(|line| {
                schema::ProductDiscountCandidateTarget::CartLine(schema::CartLineTarget {
                    id: line.id().clone(),
                    quantity: None,
                })
            })
            .collect();
        if !targets.is_empty() {
            operations.push(schema::CartOperation::ProductDiscountsAdd(
                schema::ProductDiscountsAddOperation {
                    selection_strategy: schema::ProductDiscountSelectionStrategy::First,
                    candidates: vec![schema::ProductDiscountCandidate {
                        targets,
                        prerequisites: None,
                        message: Some(format!("{}% off {}+ items", config.percentage, config.minimum_quantity)),
                        value: schema::ProductDiscountCandidateValue::Percentage(schema::Percentage {
                            value: Decimal(config.percentage),
                        }),
                        associated_discount_code: associated.clone(),
                    }],
                },
            ));
        }
    }

    if classes.contains(&schema::DiscountClass::Order) && config.order_percentage > 0.0 {
        operations.push(schema::CartOperation::OrderDiscountsAdd(
            schema::OrderDiscountsAddOperation {
                selection_strategy: schema::OrderDiscountSelectionStrategy::First,
                candidates: vec![schema::OrderDiscountCandidate {
                    targets: vec![schema::OrderDiscountCandidateTarget::OrderSubtotal(
                        schema::OrderSubtotalTarget {
                            excluded_cart_line_ids: vec![],
                        },
                    )],
                    message: Some(format!("{}% off the order", config.order_percentage)),
                    value: schema::OrderDiscountCandidateValue::Percentage(schema::Percentage {
                        value: Decimal(config.order_percentage),
                    }),
                    conditions: None,
                    associated_discount_code: associated,
                }],
            },
        ));
    }

    Ok(schema::CartLinesDiscountsGenerateRunResult { operations })
}

#[shopify_function]
fn cart_delivery_options_discounts_generate_run(
    input: schema::cart_delivery_options_discounts_generate_run::Input,
) -> Result<schema::CartDeliveryOptionsDiscountsGenerateRunResult> {
    let config = config(input.discount().metafield().map(|m| m.value().as_str()));
    let mut operations = Vec::new();
    let subtotal = input.cart().cost().subtotal_amount().amount().0;

    if input.discount().discount_classes().contains(&schema::DiscountClass::Shipping)
        && config.free_shipping_above.is_some_and(|threshold| subtotal >= threshold)
    {
        let associated = input
            .triggering_discount_code()
            .cloned()
            .map(|code| schema::AssociatedDiscountCode { code });
        operations.push(schema::DeliveryOperation::DeliveryDiscountsAdd(
            schema::DeliveryDiscountsAddOperation {
                selection_strategy: schema::DeliveryDiscountSelectionStrategy::All,
                candidates: input
                    .cart()
                    .delivery_groups()
                    .iter()
                    .map(|group| schema::DeliveryDiscountCandidate {
                        targets: vec![schema::DeliveryDiscountCandidateTarget::DeliveryGroup(
                            schema::DeliveryGroupTarget { id: group.id().clone() },
                        )],
                        message: Some("Free shipping".to_string()),
                        value: schema::DeliveryDiscountCandidateValue::Percentage(schema::Percentage {
                            value: Decimal(100.0),
                        }),
                        associated_discount_code: associated.clone(),
                    })
                    .collect(),
            },
        ));
    }

    Ok(schema::CartDeliveryOptionsDiscountsGenerateRunResult { operations })
}
