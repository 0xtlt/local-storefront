//! Hides cash on delivery once the subtotal reaches an amount. The JSON metafield
//! `$app:config.settings` of the payment customization:
//!
//! ```json
//! {"method": "Cash on Delivery", "from": 100}
//! ```
//!
//! What it shows: `paymentMethodHide`, decided on the cost of the cart.

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
    method: String,
    from: f64,
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
    let subtotal = input.cart().cost().subtotal_amount().amount().0;
    if subtotal < settings.from {
        log!("subtotal {subtotal}: {} stays", settings.method);
        return Ok(schema::CartPaymentMethodsTransformRunResult { operations: vec![] });
    }
    let operations = input
        .payment_methods()
        .iter()
        .filter(|method| *method.name() == settings.method)
        .map(|method| {
            log!("subtotal {subtotal}: hiding {}", method.name());
            schema::Operation::PaymentMethodHide(schema::PaymentMethodHideOperation {
                payment_method_id: method.id().clone(),
                placements: None,
            })
        })
        .collect();
    Ok(schema::CartPaymentMethodsTransformRunResult { operations })
}
