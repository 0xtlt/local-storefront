//! Refuses an order below a minimum subtotal. The JSON metafield `$app:config.settings` of the
//! validation:
//!
//! ```json
//! {"minimum": 20}
//! ```
//!
//! What it shows: a validation error on the whole cart (`$.cart`), shown at the top of the page,
//! that blocks the order until the cart grows.

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
    minimum: f64,
}

#[shopify_function]
fn cart_validations_generate_run(
    input: schema::run::Input,
) -> Result<schema::CartValidationsGenerateRunResult> {
    let settings: Settings = input
        .validation()
        .metafield()
        .and_then(|metafield| serde_json::from_str(metafield.value()).ok())
        .unwrap_or_default();
    let subtotal = input.cart().cost().subtotal_amount();
    let mut errors = Vec::new();
    if subtotal.amount().0 < settings.minimum {
        errors.push(schema::ValidationError {
            message: format!(
                "The minimum order is {:.2} {}: add {:.2} more",
                settings.minimum,
                subtotal.currency_code(),
                settings.minimum - subtotal.amount().0
            ),
            target: "$.cart".to_string(),
        });
    }
    Ok(schema::CartValidationsGenerateRunResult {
        operations: vec![schema::Operation::ValidationAdd(
            schema::ValidationAddOperation { errors },
        )],
    })
}
