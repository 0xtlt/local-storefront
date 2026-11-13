//! Answers 2,000 operations: it renames the first delivery option again and again, so the last
//! title would win ("Standard, renamed 2000 times"). The output is about 200 kB.
//!
//! What it shows: an output over Shopify's limit of 40 kB. The run fails and its output is
//! ignored, as on Shopify; the Functions dialog and `lsf app function run` still show it.

use shopify_function::prelude::*;
use shopify_function::Result;

#[typegen("./schema.graphql")]
mod schema {
    #[query("./src/run.graphql")]
    pub mod run {}
}

const RENAMES: usize = 2000;

#[shopify_function]
fn cart_delivery_options_transform_run(
    input: schema::run::Input,
) -> Result<schema::CartDeliveryOptionsTransformRunResult> {
    let Some(option) = input
        .cart()
        .delivery_groups()
        .iter()
        .flat_map(|group| group.delivery_options())
        .next()
    else {
        return Ok(schema::CartDeliveryOptionsTransformRunResult { operations: vec![] });
    };
    let operations = (1..=RENAMES)
        .map(|round| {
            schema::Operation::DeliveryOptionRename(schema::DeliveryOptionRenameOperation {
                delivery_option_handle: option.handle().clone(),
                title: format!("Standard, renamed {round} times"),
            })
        })
        .collect();
    Ok(schema::CartDeliveryOptionsTransformRunResult { operations })
}
