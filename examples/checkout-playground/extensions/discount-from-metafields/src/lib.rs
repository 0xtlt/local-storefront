//! A discount set by the store's metafields rather than by its own: the shop metafield
//! `custom.weekend_sale_settings` gives the percentage and the label, and the product metafield
//! `custom.weekend_sale` (a boolean) says which products are on sale.
//!
//! ```json
//! "shop": {"metafields": {"custom": {"weekend_sale_settings": {"type": "json", "value": {"percentage": 10, "label": "Weekend sale"}}}}}
//! ```
//!
//! What it shows: `shop.metafield` (read as `jsonValue`) and `product.metafield`. Change the
//! percentage in `store.json` and the checkout follows.

use shopify_function::prelude::*;
use shopify_function::scalars::JsonValue;
use shopify_function::Result;

#[typegen("./schema.graphql")]
mod schema {
    #[query("./src/run.graphql")]
    pub mod run {}
}

use schema::run::input::cart::lines::Merchandise;

#[shopify_function]
fn cart_lines_discounts_generate_run(
    input: schema::run::Input,
) -> Result<schema::CartLinesDiscountsGenerateRunResult> {
    let nothing = Ok(schema::CartLinesDiscountsGenerateRunResult { operations: vec![] });
    let Some(JsonValue::Object(sale)) = input
        .shop()
        .metafield()
        .map(|metafield| metafield.json_value().clone())
    else {
        log!("no shop metafield custom.weekend_sale_settings");
        return nothing;
    };
    let percentage = match sale.get("percentage") {
        Some(JsonValue::Number(percentage)) => *percentage,
        _ => 0.0,
    };
    let label = match sale.get("label") {
        Some(JsonValue::String(label)) => label.clone(),
        _ => "Sale".to_string(),
    };
    if percentage <= 0.0
        || !input
            .discount()
            .discount_classes()
            .contains(&schema::DiscountClass::Product)
    {
        return nothing;
    }

    let mut targets = Vec::new();
    for line in input.cart().lines() {
        let Merchandise::ProductVariant(variant) = line.merchandise() else {
            continue;
        };
        let on_sale = variant
            .product()
            .metafield()
            .is_some_and(|metafield| metafield.value() == "true");
        log!(
            "{}: {}",
            variant.product().title(),
            if on_sale { "on sale" } else { "not on sale" }
        );
        if on_sale {
            targets.push(schema::ProductDiscountCandidateTarget::CartLine(
                schema::CartLineTarget {
                    id: line.id().clone(),
                    quantity: None,
                },
            ));
        }
    }
    if targets.is_empty() {
        return nothing;
    }
    Ok(schema::CartLinesDiscountsGenerateRunResult {
        operations: vec![schema::CartOperation::ProductDiscountsAdd(
            schema::ProductDiscountsAddOperation {
                selection_strategy: schema::ProductDiscountSelectionStrategy::First,
                candidates: vec![schema::ProductDiscountCandidate {
                    targets,
                    prerequisites: None,
                    message: Some(format!("{label}: {percentage}% off")),
                    value: schema::ProductDiscountCandidateValue::Percentage(schema::Percentage {
                        value: Decimal(percentage),
                    }),
                    associated_discount_code: None,
                }],
            },
        )],
    })
}
