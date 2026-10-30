//! Works too long: a loop of a million rounds, about 26 million Wasm instructions, past the 11
//! million Shopify lets a Function run. The answer it then gives does not count.
//!
//! What it shows: the instruction count of every run (the Functions dialog and `lsf function
//! run` print it), and the error of a run past the limit.

use std::hint::black_box;

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
    let mut checksum: u64 = input.payment_methods().len() as u64;
    for round in 0..1_000_000u64 {
        checksum = black_box(checksum.wrapping_mul(31).wrapping_add(round));
    }
    log!("checksum {checksum}");
    Ok(schema::CartPaymentMethodsTransformRunResult { operations: vec![] })
}
