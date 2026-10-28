//! Makes the gift box free once the cart has three other items, with the legacy cart transform
//! target (`purchase.cart-transform.run`), whose operations are `expand`, `merge` and `update`
//! rather than `lineExpand`, `linesMerge` and `lineUpdate`. The JSON metafield
//! `$app:config.settings` of the cart transform:
//!
//! ```json
//! {"freeWith": 3}
//! ```
//!
//! What it shows: a legacy target flagged on the home page, and an `update` to a price of 0.

use shopify_function::prelude::*;
use shopify_function::Result;

#[typegen("./schema.graphql")]
mod schema {
    #[query("./src/run.graphql")]
    pub mod run {}
}

use schema::run::input::cart::lines::Merchandise;

const GIFT_BOX: &str = "GIFT-BOX";

#[shopify_function]
fn run(input: schema::run::Input) -> Result<schema::FunctionRunResult> {
    let free_with = input
        .cart_transform()
        .metafield()
        .and_then(|metafield| {
            let value = metafield.value();
            let digits: String = value.chars().filter(|c| c.is_ascii_digit()).collect();
            digits.parse::<i32>().ok()
        })
        .unwrap_or(3);
    let is_gift_box = |line: &schema::run::input::cart::Lines| matches!(line.merchandise(), Merchandise::ProductVariant(variant) if variant.sku().map(String::as_str) == Some(GIFT_BOX));
    let others: i32 = input
        .cart()
        .lines()
        .iter()
        .filter(|line| !is_gift_box(line))
        .map(|line| *line.quantity())
        .sum();
    log!("{others} other items, {free_with} needed");
    let mut operations = Vec::new();
    if others >= free_with {
        for line in input.cart().lines().iter().filter(|line| is_gift_box(line)) {
            operations.push(schema::CartOperation::Update(schema::UpdateOperation {
                cart_line_id: line.id().clone(),
                title: Some("Gift Box (free)".to_string()),
                image: None,
                price: Some(schema::UpdateOperationPriceAdjustment {
                    adjustment: schema::UpdateOperationPriceAdjustmentValue::FixedPricePerUnit(
                        schema::UpdateOperationFixedPricePerUnitAdjustment {
                            amount: Decimal(0.0),
                        },
                    ),
                }),
            }));
        }
    }
    Ok(schema::FunctionRunResult { operations })
}
