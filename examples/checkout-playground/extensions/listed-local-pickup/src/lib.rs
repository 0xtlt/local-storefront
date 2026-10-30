//! A local pickup delivery option generator
//! (`cart.local-pickup-delivery-option-generator.run`): it would offer pickup at the store's
//! locations. The local checkout lists it and does not run it (a rate of kind `pick_up` in the
//! store data stands for store pickup). lsf has no schema for this API, so it is written without
//! the typegen and offers nothing.
//!
//! What it shows: a Function of an API the local checkout lists and does not run.

use std::collections::BTreeMap;

use shopify_function::prelude::*;
use shopify_function::scalars::JsonValue;
use shopify_function::Result;

#[shopify_function]
fn cart_local_pickup_delivery_option_generator_run(_input: JsonValue) -> Result<JsonValue> {
    Ok(JsonValue::Object(BTreeMap::from([(
        "operations".to_string(),
        JsonValue::Array(vec![]),
    )])))
}
