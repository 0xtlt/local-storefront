//! A cart transform Function that does what its configuration says. The JSON metafield
//! `$app:config.settings` of the cart transform lists operations; cart lines are named by their
//! position in the cart (1 is the first line), variants by their id:
//!
//! ```json
//! {"operations": [
//!   {"type": "update", "line": 1, "title": "Member price", "price": "7.50"},
//!   {"type": "expand", "line": 2, "title": "Duo", "percentageDecrease": "10",
//!    "items": [{"variant": 123, "quantity": 2}, {"variant": 456, "quantity": 1, "price": "1.00"}]},
//!   {"type": "merge", "lines": [3, 4], "parentVariant": 789, "title": "Set", "percentageDecrease": "5"}
//! ]}
//! ```

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
struct Config {
    operations: Vec<Operation>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Operation {
    #[serde(rename_all = "camelCase")]
    Update {
        line: usize,
        title: Option<String>,
        price: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Expand {
        line: usize,
        title: Option<String>,
        percentage_decrease: Option<String>,
        items: Vec<Item>,
    },
    #[serde(rename_all = "camelCase")]
    Merge {
        lines: Vec<usize>,
        parent_variant: u64,
        title: Option<String>,
        percentage_decrease: Option<String>,
    },
}

#[derive(Deserialize)]
struct Item {
    variant: u64,
    quantity: i32,
    price: Option<String>,
}

fn decimal(text: &str) -> Decimal {
    Decimal(text.parse().unwrap_or(0.0))
}

#[shopify_function]
fn run(input: schema::run::Input) -> Result<schema::CartTransformRunResult> {
    let config: Config = input
        .cart_transform()
        .metafield()
        .and_then(|metafield| serde_json::from_str(metafield.value()).ok())
        .unwrap_or_default();
    let lines = input.cart().lines();
    let line_id = |position: usize| -> Result<String> {
        lines
            .get(position.wrapping_sub(1))
            .map(|line| line.id().clone())
            .ok_or_else(|| format!("the cart has no line {position}").into())
    };

    let mut operations = Vec::new();
    for operation in &config.operations {
        match operation {
            Operation::Update { line, title, price } => {
                operations.push(schema::Operation::LineUpdate(schema::LineUpdateOperation {
                    cart_line_id: line_id(*line)?,
                    image: None,
                    price: price.as_deref().map(|price| schema::LineUpdateOperationPriceAdjustment {
                        adjustment: schema::LineUpdateOperationPriceAdjustmentValue::FixedPricePerUnit(
                            schema::LineUpdateOperationFixedPricePerUnitAdjustment { amount: decimal(price) },
                        ),
                    }),
                    title: title.clone(),
                }));
            }
            Operation::Expand { line, title, percentage_decrease, items } => {
                operations.push(schema::Operation::LineExpand(schema::LineExpandOperation {
                    cart_line_id: line_id(*line)?,
                    expanded_cart_items: items
                        .iter()
                        .map(|item| schema::ExpandedItem {
                            attributes: Some(vec![]),
                            merchandise_id: format!("gid://shopify/ProductVariant/{}", item.variant),
                            price: item.price.as_deref().map(|price| schema::ExpandedItemPriceAdjustment {
                                adjustment: schema::ExpandedItemPriceAdjustmentValue::FixedPricePerUnit(
                                    schema::ExpandedItemFixedPricePerUnitAdjustment { amount: decimal(price) },
                                ),
                            }),
                            quantity: item.quantity,
                        })
                        .collect(),
                    image: None,
                    price: percentage_decrease.as_deref().map(|value| schema::PriceAdjustment {
                        percentage_decrease: Some(schema::PriceAdjustmentValue { value: decimal(value) }),
                    }),
                    title: title.clone(),
                }));
            }
            Operation::Merge { lines, parent_variant, title, percentage_decrease } => {
                let mut cart_lines = Vec::new();
                for position in lines {
                    cart_lines.push(schema::CartLineInput {
                        cart_line_id: line_id(*position)?,
                        quantity: *input.cart().lines()[*position - 1].quantity(),
                    });
                }
                operations.push(schema::Operation::LinesMerge(schema::LinesMergeOperation {
                    attributes: Some(vec![]),
                    cart_lines,
                    image: None,
                    parent_variant_id: format!("gid://shopify/ProductVariant/{parent_variant}"),
                    price: percentage_decrease.as_deref().map(|value| schema::PriceAdjustment {
                        percentage_decrease: Some(schema::PriceAdjustmentValue { value: decimal(value) }),
                    }),
                    title: title.clone(),
                }));
            }
        }
    }
    Ok(schema::CartTransformRunResult { operations })
}
