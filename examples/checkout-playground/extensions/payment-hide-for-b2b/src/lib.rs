//! Payment methods by kind of buyer: some are hidden for B2B buyers (who pay by invoice), and the
//! invoice is hidden for everyone else. The JSON metafield `$app:config.settings` of the payment
//! customization:
//!
//! ```json
//! {"hideForB2b": ["PayPal", "Cash on Delivery"], "onlyForB2b": ["Invoice (Net 30)"]}
//! ```
//!
//! What it shows: `buyerIdentity.purchasingCompany`. Log in as `alex.morgan@example.com` and
//! the list changes.

use serde::Deserialize;
use shopify_function::prelude::*;
use shopify_function::Result;

#[typegen("./schema.graphql")]
mod schema {
    #[query("./src/run.graphql")]
    pub mod run {}
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Settings {
    hide_for_b2b: Vec<String>,
    only_for_b2b: Vec<String>,
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
    let company = input
        .cart()
        .buyer_identity()
        .and_then(|buyer| buyer.purchasing_company());
    let hidden = match company {
        Some(company) => {
            log!("B2B buyer of {}", company.company().name());
            &settings.hide_for_b2b
        }
        None => &settings.only_for_b2b,
    };
    let operations = input
        .payment_methods()
        .iter()
        .filter(|method| hidden.contains(method.name()))
        .map(|method| {
            schema::Operation::PaymentMethodHide(schema::PaymentMethodHideOperation {
                payment_method_id: method.id().clone(),
                placements: None,
            })
        })
        .collect();
    Ok(schema::CartPaymentMethodsTransformRunResult { operations })
}
