//! Declares a target that no Function API has, `cart.validations.generate.later`, next to one
//! that runs. The checkout runs the known one and says the other is not run.
//!
//! What it shows: the warning on the home page and in the `diagnostics` (a typo in a target name
//! would look the same).

use shopify_function::prelude::*;
use shopify_function::Result;

#[typegen("./schema.graphql")]
mod schema {
    #[query("./src/cart_validations_generate_run.graphql")]
    pub mod cart_validations_generate_run {}
}

#[shopify_function]
fn cart_validations_generate_run(
    input: schema::cart_validations_generate_run::Input,
) -> Result<schema::CartValidationsGenerateRunResult> {
    log!(
        "the known target ran, for {} lines",
        input.cart().lines().len()
    );
    Ok(schema::CartValidationsGenerateRunResult {
        operations: vec![schema::Operation::ValidationAdd(
            schema::ValidationAddOperation { errors: vec![] },
        )],
    })
}
