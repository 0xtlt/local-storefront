//! A member price: a logged-in customer with one of the tags pays the price of the product
//! metafield `custom.member_price`, and the title of the line says so. The JSON metafield
//! `$app:config.settings` of the cart transform is the variables of the input query:
//!
//! ```json
//! {"tags": ["vip"]}
//! ```
//!
//! What it shows: `lineUpdate` with a title and a fixed price per unit. Log in as
//! `jane.doe@example.com` (tag `vip`): the tee costs 28.00 instead of 32.00.

use shopify_function::prelude::*;
use shopify_function::Result;

#[typegen("./schema.graphql")]
mod schema {
    #[query("./src/run.graphql")]
    pub mod run {}
}

use schema::run::input::cart::lines::Merchandise;

#[shopify_function]
fn cart_transform_run(input: schema::run::Input) -> Result<schema::CartTransformRunResult> {
    let member = input
        .cart()
        .buyer_identity()
        .and_then(|buyer| buyer.customer())
        .is_some_and(|customer| *customer.has_any_tag());
    if !member {
        log!("not a member: list prices");
        return Ok(schema::CartTransformRunResult { operations: vec![] });
    }
    let mut operations = Vec::new();
    for line in input.cart().lines() {
        let Merchandise::ProductVariant(variant) = line.merchandise() else {
            continue;
        };
        let product = variant.product();
        let Some(price) = product
            .member_price()
            .and_then(|metafield| metafield.value().parse::<f64>().ok())
        else {
            continue;
        };
        operations.push(schema::Operation::LineUpdate(schema::LineUpdateOperation {
            cart_line_id: line.id().clone(),
            title: Some(format!("{} (member price)", product.title())),
            image: None,
            price: Some(schema::LineUpdateOperationPriceAdjustment {
                adjustment: schema::LineUpdateOperationPriceAdjustmentValue::FixedPricePerUnit(
                    schema::LineUpdateOperationFixedPricePerUnitAdjustment {
                        amount: Decimal(price),
                    },
                ),
            }),
        }));
    }
    Ok(schema::CartTransformRunResult { operations })
}
