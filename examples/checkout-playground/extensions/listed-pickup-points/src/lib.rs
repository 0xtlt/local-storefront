//! A pickup point delivery option generator, with network access
//! (`cart.pickup-point-delivery-option-generator.fetch` and `.run`): it would ask a carrier for
//! the pickup points near the address and offer them. The local checkout lists both targets and
//! runs neither. lsf has no schema for this API, so it is written without the typegen, asks for
//! no request and offers no point.
//!
//! What it shows: `.fetch` targets that are not the three the checkout runs.

use std::collections::BTreeMap;

use shopify_function::prelude::*;
use shopify_function::scalars::JsonValue;
use shopify_function::Result;

#[shopify_function]
fn cart_pickup_point_delivery_option_generator_fetch(_input: JsonValue) -> Result<JsonValue> {
    Ok(JsonValue::Object(BTreeMap::from([(
        "request".to_string(),
        JsonValue::Null,
    )])))
}

#[shopify_function]
fn cart_pickup_point_delivery_option_generator_run(_input: JsonValue) -> Result<JsonValue> {
    Ok(JsonValue::Object(BTreeMap::from([(
        "operations".to_string(),
        JsonValue::Array(vec![]),
    )])))
}
