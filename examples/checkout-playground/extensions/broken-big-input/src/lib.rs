//! Asks for a lot: the buyer and their tags and metafields, the company, localization, the shop's
//! metafields, the cost, the delivery options, and for each line its cost, selling plan,
//! properties, variant and product with metafields. It reads its input as plain JSON (no typegen)
//! and logs how big it is, and never refuses anything.
//!
//! What it shows: a big input in the Functions dialog, with what the store data and the scenario
//! put in it. Not broken: an edge case for the dialog and for `lsf app function run`.

use shopify_function::prelude::*;
use shopify_function::scalars::JsonValue;
use shopify_function::Result;

/// The number of values in a JSON document.
fn count(value: &JsonValue) -> usize {
    match value {
        JsonValue::Object(object) => 1 + object.values().map(count).sum::<usize>(),
        JsonValue::Array(array) => 1 + array.iter().map(count).sum::<usize>(),
        _ => 1,
    }
}

#[shopify_function]
fn cart_validations_generate_run(input: JsonValue) -> Result<JsonValue> {
    log!("the input has {} values", count(&input));
    if let JsonValue::Object(root) = &input {
        for (key, value) in root {
            log!("{key}: {} values", count(value));
        }
    }
    let errors = JsonValue::Object(
        [("errors".to_string(), JsonValue::Array(vec![]))]
            .into_iter()
            .collect(),
    );
    let operation = JsonValue::Object(
        [("validationAdd".to_string(), errors)]
            .into_iter()
            .collect(),
    );
    Ok(JsonValue::Object(
        [("operations".to_string(), JsonValue::Array(vec![operation]))]
            .into_iter()
            .collect(),
    ))
}
