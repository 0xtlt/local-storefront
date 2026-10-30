//! Refuses the emails of a blocked domain, with the legacy validation target
//! (`purchase.validation.run`), whose result is `{errors: [{localizedMessage, target}]}`. The
//! JSON metafield `$app:config.settings` of the validation:
//!
//! ```json
//! {"blockedDomain": "blocked.example"}
//! ```
//!
//! What it shows: a legacy target that still works in both API versions, flagged on the home
//! page with the target that replaces it (`cart.validations.generate.run`).

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
    blocked_domain: String,
}

#[shopify_function]
fn run(input: schema::run::Input) -> Result<schema::FunctionRunResult> {
    let settings: Settings = input
        .validation()
        .metafield()
        .and_then(|metafield| serde_json::from_str(metafield.value()).ok())
        .unwrap_or_default();
    let email = input
        .cart()
        .buyer_identity()
        .and_then(|buyer| buyer.email());
    let mut errors = Vec::new();
    if let Some(email) = email {
        if !settings.blocked_domain.is_empty()
            && email.ends_with(&format!("@{}", settings.blocked_domain))
        {
            errors.push(schema::FunctionError {
                localized_message: format!("Addresses at {} cannot order", settings.blocked_domain),
                target: "$.cart.buyerIdentity.email".to_string(),
            });
        }
    }
    Ok(schema::FunctionRunResult { errors })
}
