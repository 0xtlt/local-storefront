//! Asks for a request that breaks Shopify's limits: `policy.readTimeoutMs` must be between 100
//! and 2000, and the settings ask for 5000. The JSON metafield `$app:config.settings` of the
//! validation:
//!
//! ```json
//! {"url": "http://localhost:8787/stock", "readTimeoutMs": 5000}
//! ```
//!
//! What it shows: the `.fetch` target fails with the limit it breaks, no request is sent, and its
//! `.run` target does not run.

use serde::Deserialize;
use shopify_function::prelude::*;
use shopify_function::Result;

#[typegen("./schema.graphql")]
mod schema {
    #[query("./src/cart_validations_generate_fetch.graphql")]
    pub mod cart_validations_generate_fetch {}

    #[query("./src/cart_validations_generate_run.graphql")]
    pub mod cart_validations_generate_run {}
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Settings {
    url: String,
    read_timeout_ms: i32,
}

#[shopify_function]
fn cart_validations_generate_fetch(
    input: schema::cart_validations_generate_fetch::Input,
) -> Result<schema::CartValidationsGenerateFetchResult> {
    let settings: Settings = input
        .validation()
        .metafield()
        .and_then(|metafield| serde_json::from_str(metafield.value()).ok())
        .unwrap_or_default();
    log!("asking for a timeout of {} ms", settings.read_timeout_ms);
    Ok(schema::CartValidationsGenerateFetchResult {
        request: Some(schema::HttpRequest {
            method: schema::HttpRequestMethod::Get,
            url: settings.url,
            headers: vec![],
            body: None,
            json_body: None,
            policy: schema::HttpRequestPolicy {
                read_timeout_ms: settings.read_timeout_ms,
            },
        }),
    })
}

#[shopify_function]
fn cart_validations_generate_run(
    input: schema::cart_validations_generate_run::Input,
) -> Result<schema::CartValidationsGenerateRunResult> {
    log!(
        "never runs: status {:?}",
        input.fetch_result().map(|response| *response.status())
    );
    Ok(schema::CartValidationsGenerateRunResult { operations: vec![] })
}
