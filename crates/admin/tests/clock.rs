//! Time passing: what waits on the shop's clock happens on the next request of the Admin API,
//! as when the clock is moved, without a mutation to wake it up.

mod support;

use std::collections::HashMap;
use std::time::Duration;

use lsf_admin::api::{AdminApi, AdminOptions, HttpRequest, ThrottleSetting};
use lsf_admin::state::Clock;
use lsf_admin::webhooks::DeliveryOptions;
use serde_json::{Value, json};

fn api() -> AdminApi {
    AdminApi::new(
        &support::demo_store(),
        AdminOptions {
            app: None,
            client_secret: "secret".to_string(),
            app_url: None,
            throttle: ThrottleSetting::Store,
            delivery: DeliveryOptions {
                timeout: Duration::from_secs(3),
                retry_delays: Vec::new(),
                ..DeliveryOptions::default()
            },
            token_seed: Some(7),
        },
    )
}

/// A GraphQL request with the default token.
fn graphql(api: &AdminApi, query: &str, variables: Value) -> Value {
    let token = api.token(&api.default_shop());
    let response = api
        .handle(&HttpRequest {
            method: "POST".into(),
            path: format!("/admin/api/{}/graphql.json", lsf_admin::version::SERVED[0]),
            query: Vec::new(),
            headers: HashMap::from([
                ("content-type".to_string(), "application/json".to_string()),
                ("x-shopify-access-token".to_string(), token),
            ]),
            body: json!({"query": query, "variables": variables})
                .to_string()
                .into_bytes(),
        })
        .expect("the path is routed");
    serde_json::from_slice(&response.body).unwrap()
}

#[test]
fn a_data_erasure_due_is_processed_by_the_next_request() {
    let api = api();
    let shop = api.default_shop();
    let customer = api
        .with_shop(&shop, |state| {
            state
                .all("Customer")
                .find(|customer| customer["email"].is_string())
                .and_then(|customer| customer["id"].as_str())
                .map(str::to_string)
        })
        .flatten()
        .expect("a customer with an email");
    let requested = graphql(
        &api,
        "mutation($id: ID!) { customerRequestDataErasure(customerId: $id) { customerId userErrors { message } } }",
        json!({"id": customer}),
    );
    assert_eq!(
        requested["data"]["customerRequestDataErasure"]["userErrors"],
        json!([]),
        "{requested}"
    );
    // Eleven days pass, with nothing to say so but the clock (as a system clock does).
    api.change_shop(&shop, |state| {
        let later = state.now() + chrono::Duration::days(11);
        state.set_clock(Clock::Frozen(later));
    });
    // Any request then finds the customer erased.
    graphql(&api, "{ shop { name } }", Value::Null);
    let (email, outbox) = api
        .with_shop(&shop, |state| {
            (
                state.get(&customer).unwrap()["email"].clone(),
                lsf_admin::outbox::entries(state, Some(lsf_admin::outbox::Channel::DataErasure)),
            )
        })
        .unwrap();
    assert_eq!(email, Value::Null);
    let whats: Vec<&str> = outbox
        .iter()
        .filter_map(|entry| entry["what"].as_str())
        .collect();
    assert_eq!(whats, ["data_erasure_requested", "customer_data_erased"]);
}
