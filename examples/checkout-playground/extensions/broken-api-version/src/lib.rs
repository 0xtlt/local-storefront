//! Declares `api_version = "2025-07"`, a release lsf has no schemas for (it supports 2026-10 and
//! 2027-01). The checkout does not run it, and says so when it loads the app. (It is built
//! against the 2027-01 schema, and would put cash on delivery first.)
//!
//! What it shows: the "not supported" banner of the home page, and a Function that never runs.

use shopify_function::prelude::*;
use shopify_function::Result;

#[typegen("./schema.graphql")]
mod schema {
    #[query("./src/run.graphql")]
    pub mod run {}
}

#[shopify_function]
fn cart_payment_methods_transform_run(
    input: schema::run::Input,
) -> Result<schema::CartPaymentMethodsTransformRunResult> {
    let operations = input
        .payment_methods()
        .iter()
        .filter(|method| method.name() == "Cash on Delivery")
        .map(|method| {
            schema::Operation::PaymentMethodMove(schema::PaymentMethodMoveOperation {
                payment_method_id: method.id().clone(),
                index: 0,
            })
        })
        .collect();
    Ok(schema::CartPaymentMethodsTransformRunResult { operations })
}
