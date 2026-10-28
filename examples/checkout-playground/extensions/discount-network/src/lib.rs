//! A discount whose percentage a server decides: network access. The `.fetch` target asks for an
//! HTTP request (POST the subtotal to the `url` of the settings), the checkout sends it, and the
//! `.run` target reads the response as `fetchResult`. The JSON metafield `$app:config.settings`
//! of the discount:
//!
//! ```json
//! {"url": "http://localhost:8787/discount"}
//! ```
//!
//! The mock server of the playground (`node tools/fake-api.mjs`) answers
//! `{"percentage": 12, "message": "Partner price"}`. Without it the response is a `502` and the
//! discount does not apply: the request, the response and the logs show why.

use std::collections::BTreeMap;

use serde::Deserialize;
use shopify_function::prelude::*;
use shopify_function::scalars::JsonValue;
use shopify_function::Result;

#[typegen("./schema.graphql")]
mod schema {
    #[query("./src/cart_lines_discounts_generate_fetch.graphql")]
    pub mod cart_lines_discounts_generate_fetch {}

    #[query("./src/cart_lines_discounts_generate_run.graphql")]
    pub mod cart_lines_discounts_generate_run {}
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Settings {
    url: String,
}

#[shopify_function]
fn cart_lines_discounts_generate_fetch(
    input: schema::cart_lines_discounts_generate_fetch::Input,
) -> Result<schema::CartLinesDiscountsGenerateFetchResult> {
    let settings: Settings = input
        .discount()
        .metafield()
        .and_then(|metafield| serde_json::from_str(metafield.value()).ok())
        .unwrap_or_default();
    if settings.url.is_empty() {
        return Ok(schema::CartLinesDiscountsGenerateFetchResult { request: None });
    }
    let cost = input.cart().cost().subtotal_amount();
    let items: i32 = input
        .cart()
        .lines()
        .iter()
        .map(|line| *line.quantity())
        .sum();
    let body = JsonValue::Object(BTreeMap::from([
        ("subtotal".to_string(), JsonValue::Number(cost.amount().0)),
        (
            "currency".to_string(),
            JsonValue::String(cost.currency_code().clone()),
        ),
        ("items".to_string(), JsonValue::Number(items as f64)),
    ]));
    Ok(schema::CartLinesDiscountsGenerateFetchResult {
        request: Some(schema::HttpRequest {
            method: schema::HttpRequestMethod::Post,
            url: settings.url,
            headers: vec![
                schema::HttpRequestHeader {
                    name: "accept".to_string(),
                    value: "application/json".to_string(),
                },
                schema::HttpRequestHeader {
                    name: "content-type".to_string(),
                    value: "application/json".to_string(),
                },
            ],
            body: None,
            json_body: Some(body),
            policy: schema::HttpRequestPolicy {
                read_timeout_ms: 2000,
            },
        }),
    })
}

#[shopify_function]
fn cart_lines_discounts_generate_run(
    input: schema::cart_lines_discounts_generate_run::Input,
) -> Result<schema::CartLinesDiscountsGenerateRunResult> {
    let nothing = Ok(schema::CartLinesDiscountsGenerateRunResult { operations: vec![] });
    let Some(response) = input.fetch_result() else {
        log!("no response: the .fetch target asked for no request");
        return nothing;
    };
    if *response.status() != 200 {
        log!(
            "the server answered {}: no partner price",
            response.status()
        );
        return nothing;
    }
    let Some(JsonValue::Object(body)) = response.json_body() else {
        log!("the server did not answer JSON");
        return nothing;
    };
    let percentage = match body.get("percentage") {
        Some(JsonValue::Number(percentage)) => *percentage,
        _ => 0.0,
    };
    let message = match body.get("message") {
        Some(JsonValue::String(message)) => message.clone(),
        _ => "Partner price".to_string(),
    };
    log!("the server answered {percentage}%");
    if percentage <= 0.0
        || !input
            .discount()
            .discount_classes()
            .contains(&schema::DiscountClass::Order)
    {
        return nothing;
    }
    Ok(schema::CartLinesDiscountsGenerateRunResult {
        operations: vec![schema::CartOperation::OrderDiscountsAdd(
            schema::OrderDiscountsAddOperation {
                selection_strategy: schema::OrderDiscountSelectionStrategy::First,
                candidates: vec![schema::OrderDiscountCandidate {
                    targets: vec![schema::OrderDiscountCandidateTarget::OrderSubtotal(
                        schema::OrderSubtotalTarget {
                            excluded_cart_line_ids: vec![],
                        },
                    )],
                    message: Some(format!("{message}: {percentage}% off")),
                    value: schema::OrderDiscountCandidateValue::Percentage(schema::Percentage {
                        value: Decimal(percentage),
                    }),
                    conditions: None,
                    associated_discount_code: None,
                }],
            },
        )],
    })
}
