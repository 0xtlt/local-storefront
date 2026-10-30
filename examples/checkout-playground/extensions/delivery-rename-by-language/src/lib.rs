//! Renames the delivery options in the language of the buyer. The JSON metafield
//! `$app:config.settings` of the delivery customization maps a language to new titles, by the
//! handle of the option:
//!
//! ```json
//! {"fr": {"standard": "Standard (3 à 5 jours ouvrés)", "express": "Express (1 à 2 jours ouvrés)"}}
//! ```
//!
//! What it shows: `localization.language` and `deliveryOptionRename`. Open the checkout under
//! `/fr/` and the options are in French.

use std::collections::BTreeMap;

use shopify_function::prelude::*;
use shopify_function::Result;

#[typegen("./schema.graphql")]
mod schema {
    #[query("./src/run.graphql")]
    pub mod run {}
}

type Settings = BTreeMap<String, BTreeMap<String, String>>;

#[shopify_function]
fn cart_delivery_options_transform_run(
    input: schema::run::Input,
) -> Result<schema::CartDeliveryOptionsTransformRunResult> {
    let settings: Settings = input
        .delivery_customization()
        .metafield()
        .and_then(|metafield| serde_json::from_str(metafield.value()).ok())
        .unwrap_or_default();
    let language = input.localization().language().iso_code().to_lowercase();
    let Some(titles) = settings.get(&language) else {
        log!("no titles in {language}");
        return Ok(schema::CartDeliveryOptionsTransformRunResult { operations: vec![] });
    };
    let mut operations = Vec::new();
    for group in input.cart().delivery_groups() {
        for option in group.delivery_options() {
            if let Some(title) = titles.get(option.handle()) {
                operations.push(schema::Operation::DeliveryOptionRename(
                    schema::DeliveryOptionRenameOperation {
                        delivery_option_handle: option.handle().clone(),
                        title: title.clone(),
                    },
                ));
            }
        }
    }
    Ok(schema::CartDeliveryOptionsTransformRunResult { operations })
}
