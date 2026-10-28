//! Refuses more than a number of units of each product with one of the tags (whatever the
//! variants). The JSON metafield `$app:config.settings` of the validation, whose `tags` are also
//! the variables of the input query:
//!
//! ```json
//! {"tags": ["limited"], "maximum": 2}
//! ```
//!
//! What it shows: an error on a line (`$.cart.lines[0].quantity`), shown next to that line.

use std::collections::BTreeMap;

use serde::Deserialize;
use shopify_function::prelude::*;
use shopify_function::Result;

#[typegen("./schema.graphql")]
mod schema {
    #[query("./src/run.graphql")]
    pub mod run {}
}

use schema::run::input::cart::lines::Merchandise;

#[derive(Default, Deserialize)]
#[serde(default)]
struct Settings {
    maximum: i32,
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
    // Units of each limited product, and the first line it is on.
    let mut products: BTreeMap<&str, (i32, usize, &str)> = BTreeMap::new();
    for (index, line) in input.cart().lines().iter().enumerate() {
        let Merchandise::ProductVariant(variant) = line.merchandise() else {
            continue;
        };
        let product = variant.product();
        if *product.has_any_tag() {
            let entry = products.entry(product.id().as_str()).or_insert((
                0,
                index,
                product.title().as_str(),
            ));
            entry.0 += line.quantity();
        }
    }
    let errors = products
        .values()
        .filter(|(units, _, _)| *units > settings.maximum)
        .map(|(units, index, title)| schema::ValidationError {
            message: format!(
                "{title}: {} per order at most ({units} in the cart)",
                settings.maximum
            ),
            target: format!("$.cart.lines[{index}].quantity"),
        })
        .collect();
    Ok(schema::CartValidationsGenerateRunResult {
        operations: vec![schema::Operation::ValidationAdd(
            schema::ValidationAddOperation { errors },
        )],
    })
}
