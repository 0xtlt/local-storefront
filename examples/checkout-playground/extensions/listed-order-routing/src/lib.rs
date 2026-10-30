//! An order routing location rule (`cart.fulfillment-groups.location-rankings.generate.run`): it
//! would rank the locations an order ships from. The local checkout lists it and does not run it.
//! lsf has no schema for this API, so it is written without the typegen and ranks nothing.
//!
//! What it shows: a Function of an API the local checkout lists and does not run.

use std::collections::BTreeMap;

use shopify_function::prelude::*;
use shopify_function::scalars::JsonValue;
use shopify_function::Result;

#[shopify_function]
fn cart_fulfillment_groups_location_rankings_generate_run(_input: JsonValue) -> Result<JsonValue> {
    Ok(JsonValue::Object(BTreeMap::from([(
        "operations".to_string(),
        JsonValue::Array(vec![]),
    )])))
}
