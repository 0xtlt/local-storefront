//! Renames payment methods. The JSON metafield `$app:config.settings` of the payment
//! customization maps a name to the new one:
//!
//! ```json
//! {"Bank Deposit": "Bank transfer (2 to 3 business days)", "Cash on Delivery": "Cash on delivery (pay the courier)"}
//! ```
//!
//! What it shows: `paymentMethodRename`. The payment customizations that run after it see the new
//! names, so the others of the playground match names it leaves alone (or run before it).

use std::collections::BTreeMap;

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
    let names: BTreeMap<String, String> = input
        .payment_customization()
        .metafield()
        .and_then(|metafield| serde_json::from_str(metafield.value()).ok())
        .unwrap_or_default();
    let operations = input
        .payment_methods()
        .iter()
        .filter_map(|method| {
            let name = names.get(method.name())?;
            Some(schema::Operation::PaymentMethodRename(
                schema::PaymentMethodRenameOperation {
                    payment_method_id: method.id().clone(),
                    name: name.clone(),
                },
            ))
        })
        .collect();
    Ok(schema::CartPaymentMethodsTransformRunResult { operations })
}
