//! Refuses a shipping address without a street number, but only when the order is placed: while
//! the buyer types (`CHECKOUT_INTERACTION`) it says nothing.
//!
//! What it shows: `buyerJourney.step`. The page shows no error until "Pay now"; then the order
//! is refused (`lsf app function run --step interaction` and `--step completion` show both sides).

use shopify_function::prelude::*;
use shopify_function::Result;

#[typegen("./schema.graphql")]
mod schema {
    #[query("./src/run.graphql")]
    pub mod run {}
}

#[shopify_function]
fn cart_validations_generate_run(
    input: schema::run::Input,
) -> Result<schema::CartValidationsGenerateRunResult> {
    let step = input.buyer_journey().step();
    let mut errors = Vec::new();
    if matches!(step, Some(schema::BuyerJourneyStep::CheckoutCompletion)) {
        for (index, group) in input.cart().delivery_groups().iter().enumerate() {
            let address1 = group
                .delivery_address()
                .and_then(|address| address.address_1());
            if let Some(address1) =
                address1.filter(|line| !line.chars().any(|c| c.is_ascii_digit()))
            {
                errors.push(schema::ValidationError {
                    message: format!("\"{address1}\" has no street number"),
                    target: format!("$.cart.deliveryGroups[{index}].deliveryAddress.address1"),
                });
            }
        }
    } else {
        log!("step {step:?}: checked when the order is placed");
    }
    Ok(schema::CartValidationsGenerateRunResult {
        operations: vec![schema::Operation::ValidationAdd(
            schema::ValidationAddOperation { errors },
        )],
    })
}
