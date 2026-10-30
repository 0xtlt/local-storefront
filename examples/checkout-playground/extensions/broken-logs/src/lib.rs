//! Succeeds, and writes logs on the way (the `log!` macro, stderr of the Wasm module): one line
//! per cart line, and a summary.
//!
//! What it shows: the logs of a run that worked. They are in the Functions dialog and in
//! `lsf app function run`, as `shopify app function run` prints them; nothing is wrong.

use shopify_function::prelude::*;
use shopify_function::Result;

#[typegen("./schema.graphql")]
mod schema {
    #[query("./src/run.graphql")]
    pub mod run {}
}

use schema::run::input::cart::lines::Merchandise;

#[shopify_function]
fn cart_validations_generate_run(
    input: schema::run::Input,
) -> Result<schema::CartValidationsGenerateRunResult> {
    log!("step: {:?}", input.buyer_journey().step());
    for (index, line) in input.cart().lines().iter().enumerate() {
        let sku = match line.merchandise() {
            Merchandise::ProductVariant(variant) => variant.sku().cloned().unwrap_or_default(),
            _ => "custom product".to_string(),
        };
        log!("line {index}: {} x {sku}", line.quantity());
    }
    let total = input.cart().cost().total_amount();
    log!("total: {} {}", total.amount().0, total.currency_code());
    log!("all good: no error");
    Ok(schema::CartValidationsGenerateRunResult {
        operations: vec![schema::Operation::ValidationAdd(
            schema::ValidationAddOperation { errors: vec![] },
        )],
    })
}
