//! The app platform end to end, through `AdminApi::handle` as `lsf` serves it: an app that
//! uninstalls itself, revokes scopes, and delegates its access, and the tokens that follow.

mod support;

use std::collections::HashMap;
use std::time::Duration;

use lsf_admin::api::{AdminApi, AdminOptions, HttpRequest, HttpResponse, ThrottleSetting};
use lsf_admin::webhooks::DeliveryOptions;
use serde_json::{Value, json};

const GRAPHQL: &str = "/admin/api/2026-10/graphql.json";

fn api() -> AdminApi {
    let app = lsf_checkout::app::load(&support::fixture("platform-app")).expect("the app loads");
    AdminApi::new(
        &support::demo_store(),
        AdminOptions {
            app: Some(app),
            client_secret: "test-secret".to_string(),
            app_url: None,
            throttle: ThrottleSetting::Off,
            delivery: DeliveryOptions {
                timeout: Duration::from_secs(1),
                retry_delays: Vec::new(),
                ..DeliveryOptions::default()
            },
            token_seed: Some(11),
        },
    )
}

fn send(
    api: &AdminApi,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> HttpResponse {
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let mut headers = HashMap::new();
    headers.insert("host".to_string(), "127.0.0.1:9292".to_string());
    headers.insert("content-type".to_string(), "application/json".to_string());
    if let Some(token) = token {
        headers.insert("x-shopify-access-token".to_string(), token.to_string());
    }
    api.handle(&HttpRequest {
        method: method.to_string(),
        path: path.to_string(),
        query: form_urlencoded::parse(query.as_bytes())
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect(),
        headers,
        body: body.to_string().into_bytes(),
    })
    .unwrap_or_else(|| panic!("{path} is not routed"))
}

fn graphql(api: &AdminApi, token: &str, query: &str) -> (u16, Value) {
    let response = send(api, "POST", GRAPHQL, Some(token), json!({"query": query}));
    let body = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
    (response.status, body)
}

#[test]
fn an_app_that_uninstalls_itself_loses_its_tokens_and_subscriptions() {
    let api = api();
    let token = api.token(&api.default_shop());
    let (_, created) = graphql(
        &api,
        &token,
        "mutation { webhookSubscriptionCreate(topic: PRODUCTS_UPDATE, webhookSubscription: {uri: \"http://127.0.0.1:9/hooks\"}) { webhookSubscription { id } userErrors { message } } }",
    );
    assert!(
        created["data"]["webhookSubscriptionCreate"]["webhookSubscription"]["id"].is_string(),
        "{created}"
    );
    let (status, answer) = graphql(
        &api,
        &token,
        "mutation { appUninstall { app { title } userErrors { message } } }",
    );
    assert_eq!(status, 200);
    assert_eq!(
        answer["data"]["appUninstall"]["app"]["title"], "Platform fixtures",
        "{answer}"
    );
    let (status, _) = graphql(&api, &token, "{ shop { name } }");
    assert_eq!(status, 401, "the token is revoked");
    let shop = api.default_shop();
    assert!(
        api.webhooks().registry().list(&shop).is_empty(),
        "the shop's subscriptions are gone"
    );
    let installed = send(&api, "POST", "/__lsf/admin/install", None, json!({}));
    assert_eq!(installed.status, 200);
    let (status, _) = graphql(&api, &token, "{ shop { name } }");
    assert_eq!(status, 200, "installed again, the token works");
}

#[test]
fn a_revoked_scope_is_gone_from_every_token_of_the_installation() {
    let api = api();
    let installed = send(
        &api,
        "POST",
        "/__lsf/admin/install",
        None,
        json!({"scopes": "read_products,write_products,write_customers,read_discounts"}),
    );
    assert_eq!(installed.status, 200);
    let token = api.token(&api.default_shop());
    let (_, before) = graphql(&api, &token, "{ customers(first: 1) { nodes { id } } }");
    assert!(before.get("errors").is_none(), "{before}");
    let (_, answer) = graphql(
        &api,
        &token,
        "mutation { appRevokeAccessScopes(scopes: [\"write_customers\"]) { revoked { handle } userErrors { message } } }",
    );
    assert_eq!(
        answer["data"]["appRevokeAccessScopes"]["revoked"],
        json!([{"handle": "write_customers"}]),
        "{answer}"
    );
    let (_, after) = graphql(&api, &token, "{ customers(first: 1) { nodes { id } } }");
    assert_eq!(
        after["errors"][0]["extensions"]["code"], "ACCESS_DENIED",
        "{after}"
    );
}

#[test]
fn a_delegate_token_authenticates_with_its_scopes_only_until_destroyed() {
    let api = api();
    let token = api.token(&api.default_shop());
    let (_, created) = graphql(
        &api,
        &token,
        "mutation { delegateAccessTokenCreate(input: {delegateAccessScope: [\"read_products\"]}) { delegateAccessToken { accessToken accessScopes expiresIn } userErrors { message } } }",
    );
    let delegate =
        created["data"]["delegateAccessTokenCreate"]["delegateAccessToken"]["accessToken"]
            .as_str()
            .unwrap_or_else(|| panic!("{created}"))
            .to_string();
    assert_eq!(
        created["data"]["delegateAccessTokenCreate"]["delegateAccessToken"]["expiresIn"],
        Value::Null,
        "the parent never expires"
    );
    let (status, products) = graphql(&api, &delegate, "{ products(first: 1) { nodes { id } } }");
    assert_eq!(status, 200);
    assert!(products.get("errors").is_none(), "{products}");
    let (_, denied) = graphql(
        &api,
        &delegate,
        "mutation { productCreate(product: {title: \"No\"}) { product { id } } }",
    );
    assert_eq!(
        denied["errors"][0]["extensions"]["code"], "ACCESS_DENIED",
        "{denied}"
    );
    let (_, nested) = graphql(
        &api,
        &delegate,
        "mutation { delegateAccessTokenCreate(input: {delegateAccessScope: [\"read_products\"]}) { delegateAccessToken { accessToken } userErrors { code } } }",
    );
    assert_eq!(
        nested["data"]["delegateAccessTokenCreate"]["userErrors"][0]["code"],
        "DELEGATE_ACCESS_TOKEN"
    );
    let (_, destroyed) = graphql(
        &api,
        &token,
        &format!(
            "mutation {{ delegateAccessTokenDestroy(accessToken: \"{delegate}\") {{ status userErrors {{ code }} }} }}"
        ),
    );
    assert_eq!(
        destroyed["data"]["delegateAccessTokenDestroy"]["status"], true,
        "{destroyed}"
    );
    let (status, _) = graphql(&api, &delegate, "{ shop { name } }");
    assert_eq!(status, 401);
}

#[test]
fn the_outbox_lists_what_flow_would_have_received() {
    let api = api();
    let token = api.token(&api.default_shop());
    // The app may not read customers: the id is the shop's first one's.
    let customer = api
        .with_default_shop(|shop| {
            let gid = shop.keys("Customer").next().unwrap().to_string();
            gid.rsplit('/').next().unwrap().to_string()
        })
        .unwrap();
    let (_, answer) = graphql(
        &api,
        &token,
        &format!(
            "mutation {{ flowTriggerReceive(handle: \"auction-bid-placed\", payload: {{Amount: \"30\", customer_id: {customer}}}) {{ userErrors {{ message }} }} }}"
        ),
    );
    assert_eq!(
        answer["data"]["flowTriggerReceive"]["userErrors"],
        json!([]),
        "{answer}"
    );
    let outbox = send(&api, "GET", "/__lsf/admin/outbox", None, Value::Null);
    let outbox: Value = serde_json::from_slice(&outbox.body).unwrap();
    assert_eq!(
        outbox["outbox"][0]["target"], "flow_trigger/auction-bid-placed",
        "{outbox}"
    );
}
