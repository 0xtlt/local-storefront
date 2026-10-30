//! Asks for a gift note when a gift product is in the cart: the note is a cart attribute. The
//! JSON metafield `$app:config.settings` of the validation is only the variables of the input
//! query:
//!
//! ```json
//! {"tags": ["gift-box"], "attribute": "gift_note"}
//! ```
//!
//! What it shows: `cart.attribute(key:)` with a key from a variable, read under an alias. A
//! checkout UI extension (or the `attribute` operation of the checkout) can set the attribute to
//! clear the error.

use shopify_function::prelude::*;
use shopify_function::Result;

#[typegen("./schema.graphql")]
mod schema {
    #[query("./src/run.graphql")]
    pub mod run {}
}

use schema::run::input::cart::lines::Merchandise;

#[shopify_function]
fn cart_validations_generate_run(
    input: schema::run::Input,
) -> Result<schema::CartValidationsGenerateRunResult> {
    let gift = input
        .cart()
        .lines()
        .iter()
        .find_map(|line| match line.merchandise() {
            Merchandise::ProductVariant(variant) if *variant.product().has_any_tag() => {
                Some(variant.product().title().clone())
            }
            _ => None,
        });
    let note = input
        .cart()
        .note()
        .and_then(|attribute| attribute.value())
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    let mut errors = Vec::new();
    match gift {
        Some(gift) if note.is_empty() => errors.push(schema::ValidationError {
            message: format!("{gift}: write the gift note that goes in the box"),
            target: "$.cart".to_string(),
        }),
        Some(_) => log!("gift note: {note}"),
        None => {}
    }
    Ok(schema::CartValidationsGenerateRunResult {
        operations: vec![schema::Operation::ValidationAdd(
            schema::ValidationAddOperation { errors },
        )],
    })
}
