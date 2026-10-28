//! Expands a bundle product into its components. The product holds them in its JSON metafield
//! `custom.bundle_components` (the Weekend Bundle of the store data is a white tee and a tote):
//!
//! ```json
//! [{"variant": "gid://shopify/ProductVariant/8182177638701", "quantity": 1},
//!  {"variant": "gid://shopify/ProductVariant/4381608573867", "quantity": 1}]
//! ```
//!
//! What it shows: `lineExpand`. The line keeps the bundle's price, shared between the
//! components, and lists them under it.

use serde::Deserialize;
use shopify_function::prelude::*;
use shopify_function::Result;

#[typegen("./schema.graphql")]
mod schema {
    #[query("./src/run.graphql")]
    pub mod run {}
}

use schema::run::input::cart::lines::Merchandise;

#[derive(Deserialize)]
struct Component {
    variant: String,
    quantity: i32,
}

#[shopify_function]
fn cart_transform_run(input: schema::run::Input) -> Result<schema::CartTransformRunResult> {
    let mut operations = Vec::new();
    for line in input.cart().lines() {
        let Merchandise::ProductVariant(variant) = line.merchandise() else {
            continue;
        };
        let Some(raw) = variant.product().components() else {
            continue;
        };
        let components: Vec<Component> = serde_json::from_str(raw.value())?;
        log!(
            "{}: {} components",
            variant.product().title(),
            components.len()
        );
        operations.push(schema::Operation::LineExpand(schema::LineExpandOperation {
            cart_line_id: line.id().clone(),
            expanded_cart_items: components
                .into_iter()
                .map(|component| schema::ExpandedItem {
                    merchandise_id: component.variant,
                    quantity: component.quantity,
                    price: None,
                    attributes: Some(vec![]),
                })
                .collect(),
            price: None,
            title: Some(format!("{} (bundle)", variant.product().title())),
            image: None,
        }));
    }
    Ok(schema::CartTransformRunResult { operations })
}
