//! Refuses to ship to some countries. The JSON metafield `$app:config.settings` of the
//! validation:
//!
//! ```json
//! {"countries": ["DE"]}
//! ```
//!
//! What it shows: an error on a field of the address
//! (`$.cart.deliveryGroups[0].deliveryAddress.countryCode`). Change the country in the form and
//! the error comes and goes.

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
    countries: Vec<String>,
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
    let mut errors = Vec::new();
    for (index, group) in input.cart().delivery_groups().iter().enumerate() {
        let country = group
            .delivery_address()
            .and_then(|address| address.country_code());
        if let Some(country) = country.filter(|country| settings.countries.contains(country)) {
            errors.push(schema::ValidationError {
                message: format!("We do not ship to {country} yet"),
                target: format!("$.cart.deliveryGroups[{index}].deliveryAddress.countryCode"),
            });
        }
    }
    Ok(schema::CartValidationsGenerateRunResult {
        operations: vec![schema::Operation::ValidationAdd(
            schema::ValidationAddOperation { errors },
        )],
    })
}
