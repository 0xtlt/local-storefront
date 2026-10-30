//! Answers an output that does not fit the schema: `deliveryOptionHide` wants a
//! `deliveryOptionHandle`, and this Function writes `handle`. It is written without the typegen
//! (input and output are plain JSON), which is the only way to get it wrong: the generated types
//! would not let it compile.
//!
//! What it shows: the checkout checks every output against the result type of the target in the
//! schema of the Function's API version, and refuses the run with the field that is wrong.

use std::collections::BTreeMap;

use shopify_function::prelude::*;
use shopify_function::scalars::JsonValue;
use shopify_function::Result;

fn object(entries: Vec<(&str, JsonValue)>) -> JsonValue {
    JsonValue::Object(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect::<BTreeMap<_, _>>(),
    )
}

#[shopify_function]
fn cart_delivery_options_transform_run(_input: JsonValue) -> Result<JsonValue> {
    let hide = object(vec![("handle", JsonValue::String("express".to_string()))]);
    Ok(object(vec![(
        "operations",
        JsonValue::Array(vec![object(vec![("deliveryOptionHide", hide)])]),
    )]))
}
