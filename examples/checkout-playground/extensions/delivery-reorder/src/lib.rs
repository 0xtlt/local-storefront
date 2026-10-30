//! Moves an option to the top of the list for addresses in some countries: store pickup first
//! for buyers in Canada, where the shop is. The JSON metafield `$app:config.settings` of the
//! delivery customization:
//!
//! ```json
//! {"first": "pickup", "countries": ["CA"]}
//! ```
//!
//! What it shows: `deliveryOptionMove`, and `deliveryMethodType` (`PICK_UP` for a rate of kind
//! `pick_up` in the store data).

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
    first: String,
    countries: Vec<String>,
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
        let country = group
            .delivery_address()
            .and_then(|address| address.country_code())
            .cloned();
        if !country
            .as_ref()
            .is_some_and(|country| settings.countries.contains(country))
        {
            log!("{country:?}: the order stays");
            continue;
        }
        for option in group.delivery_options() {
            if *option.handle() == settings.first {
                log!(
                    "{} ({:?}) goes first",
                    option.handle(),
                    option.delivery_method_type()
                );
                operations.push(schema::Operation::DeliveryOptionMove(
                    schema::DeliveryOptionMoveOperation {
                        delivery_option_handle: option.handle().clone(),
                        index: 0,
                    },
                ));
            }
        }
    }
    Ok(schema::CartDeliveryOptionsTransformRunResult { operations })
}
