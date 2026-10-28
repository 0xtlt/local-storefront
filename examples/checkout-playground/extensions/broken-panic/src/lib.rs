//! Panics, on purpose. Built with `panic = "abort"`, as Shopify's templates are, the panic is a
//! Wasm trap: the run fails, its output is lost, and the checkout cannot be completed.
//!
//! What it shows: a failed run. The Functions badge turns red, the Functions dialog opens on the
//! run with the error and the panic message in the logs, and `lsf function run` exits with 1.

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
    let lines = input.cart().lines();
    log!("about to read line {} of {}", lines.len() + 1, lines.len());
    let missing = &lines[lines.len()];
    log!("never logged: {}", missing.quantity());
    Ok(schema::CartValidationsGenerateRunResult { operations: vec![] })
}
