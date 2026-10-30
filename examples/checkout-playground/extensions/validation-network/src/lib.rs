//! A stock check that asks a server: network access. The `.fetch` target asks for
//! `GET <url>?skus=A,B`, and the `.run` target refuses the lines the server says are out of
//! stock. The JSON metafield `$app:config.settings` of the validation:
//!
//! ```json
//! {"url": "http://localhost:8787/stock"}
//! ```
//!
//! The mock server of the playground (`node tools/fake-api.mjs`) answers
//! `{"unavailable": ["MUG-SLT"]}`. Without it the response is a `502`: the Function logs it and
//! lets the order through, rather than blocking every order while the service is down.

use serde::Deserialize;
use shopify_function::prelude::*;
use shopify_function::scalars::JsonValue;
use shopify_function::Result;

#[typegen("./schema.graphql")]
mod schema {
    #[query("./src/cart_validations_generate_fetch.graphql")]
    pub mod cart_validations_generate_fetch {}

    #[query("./src/cart_validations_generate_run.graphql")]
    pub mod cart_validations_generate_run {}
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Settings {
    url: String,
}

#[shopify_function]
fn cart_validations_generate_fetch(
    input: schema::cart_validations_generate_fetch::Input,
) -> Result<schema::CartValidationsGenerateFetchResult> {
    use schema::cart_validations_generate_fetch::input::cart::lines::Merchandise;
    let settings: Settings = input
        .validation()
        .metafield()
        .and_then(|metafield| serde_json::from_str(metafield.value()).ok())
        .unwrap_or_default();
    if settings.url.is_empty() {
        return Ok(schema::CartValidationsGenerateFetchResult { request: None });
    }
    let skus: Vec<&str> = input
        .cart()
        .lines()
        .iter()
        .filter_map(|line| match line.merchandise() {
            Merchandise::ProductVariant(variant) => variant.sku().map(String::as_str),
            _ => None,
        })
        .collect();
    Ok(schema::CartValidationsGenerateFetchResult {
        request: Some(schema::HttpRequest {
            method: schema::HttpRequestMethod::Get,
            url: format!("{}?skus={}", settings.url, skus.join(",")),
            headers: vec![schema::HttpRequestHeader {
                name: "accept".to_string(),
                value: "application/json".to_string(),
            }],
            body: None,
            json_body: None,
            policy: schema::HttpRequestPolicy {
                read_timeout_ms: 1000,
            },
        }),
    })
}

#[shopify_function]
fn cart_validations_generate_run(
    input: schema::cart_validations_generate_run::Input,
) -> Result<schema::CartValidationsGenerateRunResult> {
    use schema::cart_validations_generate_run::input::cart::lines::Merchandise;
    let mut unavailable: Vec<String> = Vec::new();
    match input.fetch_result() {
        Some(response) if *response.status() == 200 => {
            if let Some(JsonValue::Object(body)) = response.json_body() {
                if let Some(JsonValue::Array(skus)) = body.get("unavailable") {
                    unavailable = skus
                        .iter()
                        .filter_map(|sku| match sku {
                            JsonValue::String(sku) => Some(sku.clone()),
                            _ => None,
                        })
                        .collect();
                }
            }
            log!("out of stock: {unavailable:?}");
        }
        Some(response) => log!(
            "the stock service answered {}: nothing checked",
            response.status()
        ),
        None => log!("no request was sent"),
    }
    let mut errors = Vec::new();
    for (index, line) in input.cart().lines().iter().enumerate() {
        let Merchandise::ProductVariant(variant) = line.merchandise() else {
            continue;
        };
        if variant.sku().is_some_and(|sku| unavailable.contains(sku)) {
            errors.push(schema::ValidationError {
                message: format!("{} is out of stock", variant.product().title()),
                target: format!("$.cart.lines[{index}]"),
            });
        }
    }
    Ok(schema::CartValidationsGenerateRunResult {
        operations: vec![schema::Operation::ValidationAdd(
            schema::ValidationAddOperation { errors },
        )],
    })
}
