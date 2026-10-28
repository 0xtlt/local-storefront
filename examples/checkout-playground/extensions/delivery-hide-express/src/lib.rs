//! Hides express delivery for addresses in some countries, and for PO boxes (a courier cannot
//! deliver there). The JSON metafield `$app:config.settings` of the delivery customization:
//!
//! ```json
//! {"option": "express", "countries": ["FR", "DE"], "poBox": true}
//! ```
//!
//! What it shows: `deliveryOptionHide`, and a Function that reads the shipping address
//! (`deliveryGroups.deliveryAddress`): type "PO Box 12" as the address and express goes.

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
    option: String,
    countries: Vec<String>,
    po_box: bool,
}

fn is_po_box(address1: &str) -> bool {
    let compact: String = address1
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect();
    compact.starts_with("pobox") || compact.contains("postofficebox")
}

#[shopify_function]
fn cart_delivery_options_transform_run(
    input: schema::run::Input,
) -> Result<schema::CartDeliveryOptionsTransformRunResult> {
    let settings: Settings = input
        .delivery_customization()
        .metafield()
        .and_then(|metafield| serde_json::from_str(metafield.value()).ok())
        .unwrap_or_default();
    let mut operations = Vec::new();
    for group in input.cart().delivery_groups() {
        let Some(address) = group.delivery_address() else {
            log!("no address yet");
            continue;
        };
        let country = address.country_code().cloned().unwrap_or_default();
        let po_box = settings.po_box && address.address_1().is_some_and(|line| is_po_box(line));
        let reason = if settings.countries.contains(&country) {
            format!("no express delivery to {country}")
        } else if po_box {
            "no express delivery to a PO box".to_string()
        } else {
            continue;
        };
        for option in group.delivery_options() {
            if *option.handle() == settings.option {
                log!("hiding {}: {reason}", option.handle());
                operations.push(schema::Operation::DeliveryOptionHide(
                    schema::DeliveryOptionHideOperation {
                        delivery_option_handle: option.handle().clone(),
                    },
                ));
            }
        }
    }
    Ok(schema::CartDeliveryOptionsTransformRunResult { operations })
}
