//! A cart and checkout validation Function, configured by the JSON metafield
//! `$app:config.settings` of its validation:
//!
//! ```json
//! {"maxQuantity": 3, "blockedEmailDomain": "blocked.test", "onlyAtCompletion": true}
//! ```
//!
//! It blocks the checkout when a line has more than `maxQuantity` items (5 without
//! configuration), or when the buyer's email belongs to `blockedEmailDomain`. With
//! `onlyAtCompletion` it checks nothing before the order is placed
//! (`buyerJourney.step` is not `CHECKOUT_COMPLETION`).

use serde::Deserialize;
use shopify_function::prelude::*;
use shopify_function::Result;

#[typegen("./schema.graphql")]
mod schema {
    #[query("./src/run.graphql")]
    pub mod run {}
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Config {
    max_quantity: i32,
    blocked_email_domain: Option<String>,
    only_at_completion: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            max_quantity: 5,
            blocked_email_domain: None,
            only_at_completion: false,
        }
    }
}

#[shopify_function]
fn run(input: schema::run::Input) -> Result<schema::FunctionRunResult> {
    let config: Config = input
        .validation()
        .metafield()
        .and_then(|metafield| serde_json::from_str(metafield.value()).ok())
        .unwrap_or_default();

    let mut errors = Vec::new();
    let completing = matches!(
        input.buyer_journey().step(),
        Some(schema::BuyerJourneyStep::CheckoutCompletion)
    );
    if config.only_at_completion && !completing {
        return Ok(schema::FunctionRunResult { errors });
    }
    if input.cart().lines().iter().any(|line| *line.quantity() > config.max_quantity) {
        errors.push(schema::FunctionError {
            localized_message: format!("Not possible to order more than {} of each", config.max_quantity),
            target: "$.cart".to_string(),
        });
    }
    let email = input.cart().buyer_identity().and_then(|buyer| buyer.email());
    if let (Some(domain), Some(email)) = (&config.blocked_email_domain, email) {
        if email.ends_with(&format!("@{domain}")) {
            errors.push(schema::FunctionError {
                localized_message: "This email address cannot be used".to_string(),
                target: "$.cart.buyerIdentity.email".to_string(),
            });
        }
    }
    Ok(schema::FunctionRunResult { errors })
}
