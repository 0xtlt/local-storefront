//! A payment customization Function, configured by the JSON metafield `$app:config.settings`
//! of its customization:
//!
//! ```json
//! {"hideName": "Cash on Delivery", "hideAbove": 100, "rename": {"Credit card": "Card (secure)"}}
//! ```
//!
//! It hides the payment method named `hideName` once the subtotal reaches `hideAbove`, renames
//! the methods in `rename` (name → new name) and moves the first renamed one to the top.

use std::collections::BTreeMap;

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
struct Config {
    hide_name: Option<String>,
    hide_above: Option<f64>,
    rename: BTreeMap<String, String>,
}

#[shopify_function]
fn run(input: schema::run::Input) -> Result<schema::FunctionRunResult> {
    let config: Config = input
        .payment_customization()
        .metafield()
        .and_then(|metafield| serde_json::from_str(metafield.value()).ok())
        .unwrap_or_default();
    let subtotal = input.cart().cost().subtotal_amount().amount().0;

    let mut operations = Vec::new();
    for method in input.payment_methods() {
        let name = method.name();
        if config.hide_name.as_ref() == Some(name)
            && config.hide_above.is_some_and(|above| subtotal >= above)
        {
            log!("hiding {name}: subtotal {subtotal}");
            operations.push(schema::Operation::Hide(schema::HideOperation {
                payment_method_id: method.id().clone(),
                placements: None,
            }));
        } else if let Some(new_name) = config.rename.get(name) {
            operations.push(schema::Operation::Rename(schema::RenameOperation {
                payment_method_id: method.id().clone(),
                name: new_name.clone(),
            }));
            operations.push(schema::Operation::Move(schema::MoveOperation {
                payment_method_id: method.id().clone(),
                index: 0,
            }));
        }
    }
    Ok(schema::FunctionRunResult { operations })
}
