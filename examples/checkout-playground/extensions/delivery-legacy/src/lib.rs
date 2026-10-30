//! The legacy delivery customization target, `purchase.delivery-customization.run`. Its result,
//! `FunctionRunResult`, takes no operation any more (`DeprecatedOperation` has no field): a
//! Function on it can only answer an empty list. This one does, and logs the options it saw.
//!
//! What it shows: the home page flags the target as legacy and names the one that replaces it
//! (`cart.delivery-options.transform.run`).

use shopify_function::prelude::*;
use shopify_function::Result;

#[typegen("./schema.graphql")]
mod schema {
    #[query("./src/run.graphql")]
    pub mod run {}
}

#[shopify_function]
fn run(input: schema::run::Input) -> Result<schema::FunctionRunResult> {
    for group in input.cart().delivery_groups() {
        for option in group.delivery_options() {
            log!(
                "saw {} ({:?}); the legacy target cannot change it",
                option.handle(),
                option.title()
            );
        }
    }
    Ok(schema::FunctionRunResult { operations: vec![] })
}
