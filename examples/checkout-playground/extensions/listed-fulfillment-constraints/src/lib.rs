//! A fulfillment constraints Function (`cart.fulfillment-constraints.generate.run`): it would
//! tell Shopify which lines must ship together, or from which locations. The local checkout lists
//! it and does not run it (one delivery group holds every line). lsf has no schema for this API,
//! so it is written without the typegen and answers no constraint.
//!
//! What it shows: a Function of an API the local checkout lists and does not run.

use std::collections::BTreeMap;

use shopify_function::prelude::*;
use shopify_function::scalars::JsonValue;
use shopify_function::Result;

#[shopify_function]
fn cart_fulfillment_constraints_generate_run(_input: JsonValue) -> Result<JsonValue> {
    Ok(JsonValue::Object(BTreeMap::from([(
        "operations".to_string(),
        JsonValue::Array(vec![]),
    )])))
}
