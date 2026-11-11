//! Shopify's Events (`[[events.subscription]]` of `shopify.app.toml`), end to end through
//! `AdminApi::handle`: a variant's price changed through the Admin API is an Event of `Product`
//! `update`, signed, with the `Shopify-*` headers, the body of `fields_changed`,
//! `query_variables` and what the subscription's `query` answers.

mod support;

use std::time::Duration;

use lsf_app::admin::api::{AdminApi, AdminOptions, ThrottleSetting};
use lsf_app::admin::auth::signing::verify_webhook_hmac;
use lsf_app::admin::webhooks::DeliveryOptions;
use lsf_app::config::{App, AppConfig, EventSubscriptionConfig};
use serde_json::{Value, json};
use support::receiver::Receiver;
use support::{demo_store, graphql_request, json_of};

const SECRET: &str = "events-secret";

const QUERY: &str = r#"query product_variant_price_change($productId: ID!, $variantsId: ID!) {
  product(id: $productId) { id title status }
  productVariant(id: $variantsId) { id price }
}"#;

fn api(receiver: &Receiver, filter: Option<&str>) -> AdminApi {
    let app = App {
        client_id: Some("events-app".to_string()),
        config: AppConfig {
            scopes: vec!["write_products".to_string()],
            events_api_version: Some("2026-10".to_string()),
            event_subscriptions: vec![EventSubscriptionConfig {
                handle: "product-price-change".to_string(),
                topic: "Product".to_string(),
                actions: vec!["update".to_string()],
                triggers: vec!["product.variants.price".to_string()],
                uri: format!("{}/webhooks/products/update", receiver.url),
                query: Some(QUERY.to_string()),
                query_filter: filter.map(str::to_string),
            }],
            ..AppConfig::default()
        },
        ..App::default()
    };
    AdminApi::new(
        &demo_store(),
        AdminOptions {
            app: Some(app),
            client_secret: SECRET.to_string(),
            app_url: None,
            throttle: ThrottleSetting::Off,
            delivery: DeliveryOptions {
                timeout: Duration::from_secs(3),
                retry_delays: Vec::new(),
                ..DeliveryOptions::default()
            },
            token_seed: Some(7),
        },
    )
}

fn graphql(api: &AdminApi, query: &str, variables: Value) -> Value {
    let token = api.token(&api.default_shop());
    let answer = json_of(
        &api.handle(&graphql_request(&token, query, variables))
            .unwrap(),
    );
    assert!(answer.get("errors").is_none(), "{answer}");
    answer
}

/// The first product, its first variant and that variant's price.
fn first_variant(api: &AdminApi) -> (String, String, String) {
    let answer = graphql(
        api,
        "{ products(first: 1) { nodes { id variants(first: 1) { nodes { id price } } } } }",
        json!({}),
    );
    let product = &answer["data"]["products"]["nodes"][0];
    let variant = &product["variants"]["nodes"][0];
    (
        product["id"].as_str().unwrap().to_string(),
        variant["id"].as_str().unwrap().to_string(),
        variant["price"].as_str().unwrap().to_string(),
    )
}

fn set_price(api: &AdminApi, product: &str, variant: &str, price: &str) {
    let answer = graphql(
        api,
        "mutation($p: ID!, $v: [ProductVariantsBulkInput!]!) { productVariantsBulkUpdate(productId: $p, variants: $v) { userErrors { message } } }",
        json!({"p": product, "v": [{"id": variant, "price": price}]}),
    );
    assert_eq!(
        answer["data"]["productVariantsBulkUpdate"]["userErrors"],
        json!([])
    );
}

#[test]
fn a_price_change_is_an_event_with_the_answer_of_its_query() {
    let receiver = Receiver::start(SECRET);
    let api = api(&receiver, None);
    let (product, variant, _) = first_variant(&api);
    // A change no trigger names sends nothing.
    graphql(
        &api,
        "mutation($p: ProductUpdateInput!) { productUpdate(product: $p) { userErrors { message } } }",
        json!({"p": {"id": product, "title": "Renamed tote"}}),
    );
    set_price(&api, &product, &variant, "123.45");
    let (headers, body) = receiver.next_raw();
    assert!(verify_webhook_hmac(
        SECRET,
        &body,
        &headers["shopify-hmac-sha256"]
    ));
    assert_eq!(headers["shopify-topic"], "Product");
    assert_eq!(headers["shopify-action"], "update");
    assert_eq!(headers["shopify-handle"], "product-price-change");
    assert_eq!(headers["shopify-api-version"], "2026-10");
    assert_eq!(headers["shopify-shop-domain"], api.default_shop());
    assert!(headers.contains_key("shopify-webhook-id"));
    assert!(!headers.contains_key("x-shopify-topic"));
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["topic"], "Product");
    assert_eq!(body["action"], "update");
    assert_eq!(body["handle"], "product-price-change");
    assert_eq!(
        body["query_variables"],
        json!({"productId": product, "variantsId": variant})
    );
    assert_eq!(
        body["fields_changed"],
        json!({
            "added": [],
            "updated": [format!("product[id: '{product}'].variants[id: '{variant}'].price")],
            "removed": [],
        })
    );
    assert_eq!(body["data"]["productVariant"]["price"], "123.45");
    assert_eq!(body["data"]["product"]["title"], "Renamed tote");
    receiver.quiet();
}

#[test]
fn a_query_filter_that_does_not_match_sends_nothing() {
    let receiver = Receiver::start(SECRET);
    let api = api(&receiver, Some("product.status:'DRAFT'"));
    let (product, variant, _) = first_variant(&api);
    set_price(&api, &product, &variant, "99.00");
    receiver.quiet();
    let log = api.webhooks().deliveries().list(&Default::default());
    let event = log
        .iter()
        .find(|delivery| delivery.topic == "Product")
        .unwrap();
    assert!(format!("{event:?}").contains("does not match"), "{event:?}");
}
