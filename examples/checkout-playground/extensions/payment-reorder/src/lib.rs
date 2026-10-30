//! Moves payment methods to the top of the list, in the order of its settings. The JSON
//! metafield `$app:config.settings` of the payment customization:
//!
//! ```json
//! {"first": ["PayPal", "Credit card"]}
//! ```
//!
//! What it shows: `paymentMethodMove`, and `placements` (the express buttons, Shop Pay and Apple
//! Pay, are not in the list it reorders).

use serde::Deserialize;
use shopify_function::prelude::*;
use shopify_function::Result;

#[typegen("./schema.graphql")]
mod schema {
    #[query("./src/run.graphql")]
    pub mod run {}
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Settings {
    first: Vec<String>,
}

#[shopify_function]
fn cart_payment_methods_transform_run(
    input: schema::run::Input,
) -> Result<schema::CartPaymentMethodsTransformRunResult> {
    let settings: Settings = input
        .payment_customization()
        .metafield()
        .and_then(|metafield| serde_json::from_str(metafield.value()).ok())
        .unwrap_or_default();
    let mut operations = Vec::new();
    for (index, name) in settings.first.iter().enumerate() {
        let method = input.payment_methods().iter().find(|method| {
            method.name() == name
                && method
                    .placements()
                    .contains(&schema::PaymentCustomizationPaymentMethodPlacement::PaymentMethod)
        });
        if let Some(method) = method {
            log!("{name} at {index}");
            operations.push(schema::Operation::PaymentMethodMove(
                schema::PaymentMethodMoveOperation {
                    payment_method_id: method.id().clone(),
                    index: index as i32,
                },
            ));
        }
    }
    Ok(schema::CartPaymentMethodsTransformRunResult { operations })
}
