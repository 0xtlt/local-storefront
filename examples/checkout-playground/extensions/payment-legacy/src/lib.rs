//! The legacy payment customization target, `purchase.payment-customization.run`. Its result
//! only takes `paymentTermsSet` now: this Function sets net payment terms for B2B buyers. The JSON
//! metafield `$app:config.settings` of the payment customization:
//!
//! ```json
//! {"dueInDays": 30}
//! ```
//!
//! What it shows: a legacy target flagged on the home page, and an operation the local checkout
//! accepts and has no place for (payment terms are not simulated).

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
    due_in_days: i32,
}

#[shopify_function]
fn run(input: schema::run::Input) -> Result<schema::FunctionRunResult> {
    let settings: Settings = input
        .payment_customization()
        .metafield()
        .and_then(|metafield| serde_json::from_str(metafield.value()).ok())
        .unwrap_or_default();
    let company = input
        .cart()
        .buyer_identity()
        .and_then(|buyer| buyer.purchasing_company());
    let (Some(company), true) = (company, settings.due_in_days > 0) else {
        return Ok(schema::FunctionRunResult { operations: vec![] });
    };
    log!(
        "{}: net {} days",
        company.company().name(),
        settings.due_in_days
    );
    Ok(schema::FunctionRunResult {
        operations: vec![schema::DeprecatedOperation::PaymentTermsSet(
            schema::PaymentTermsSetOperation {
                payment_terms: Some(schema::PaymentTerms::Net(schema::NetPaymentTerms {
                    deposit: None,
                    due_in_days: settings.due_in_days,
                    issued_at: None,
                })),
            },
        )],
    })
}
