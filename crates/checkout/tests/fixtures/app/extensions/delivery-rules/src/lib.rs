//! A delivery customization Function, configured by the JSON metafield `$app:config.settings`
//! of its customization:
//!
//! ```json
//! {"hide": ["express"], "rename": {"standard": "Standard (3-5 days)"}, "first": "pickup"}
//! ```
//!
//! It hides the options in `hide`, renames those in `rename` (handle → title) and moves the
//! option `first` to the top of the list.

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
#[serde(default)]
struct Config {
    hide: Vec<String>,
    rename: BTreeMap<String, String>,
    first: Option<String>,
}

#[shopify_function]
fn run(input: schema::run::Input) -> Result<schema::FunctionRunResult> {
    let config: Config = input
        .delivery_customization()
        .metafield()
        .and_then(|metafield| serde_json::from_str(metafield.value()).ok())
        .unwrap_or_default();

    let mut operations = Vec::new();
    for group in input.cart().delivery_groups() {
        for option in group.delivery_options() {
            let handle = option.handle();
            if config.hide.contains(handle) {
                log!("hiding {handle}");
                operations.push(schema::Operation::Hide(schema::HideOperation {
                    delivery_option_handle: handle.clone(),
                }));
            } else if let Some(title) = config.rename.get(handle) {
                operations.push(schema::Operation::Rename(schema::RenameOperation {
                    delivery_option_handle: handle.clone(),
                    title: title.clone(),
                }));
            }
        }
        if let Some(first) = &config.first {
            if group.delivery_options().iter().any(|option| option.handle() == first) {
                operations.push(schema::Operation::Move(schema::MoveOperation {
                    delivery_option_handle: first.clone(),
                    index: 0,
                }));
            }
        }
    }
    Ok(schema::FunctionRunResult { operations })
}
