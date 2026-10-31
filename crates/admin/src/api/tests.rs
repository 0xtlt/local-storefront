//! The parts of the HTTP side that do not need a shop: reading a GraphQL request, the throttle
//! setting. `tests/api.rs` runs the endpoints end to end.

use serde_json::json;

use super::HttpRequest;
use super::admin::{ThrottleSetting, graphql_request};
use crate::throttle::Plan;

fn post(content_type: &str, body: &str) -> HttpRequest {
    HttpRequest {
        method: "POST".into(),
        path: "/admin/api/2026-10/graphql.json".into(),
        headers: [("content-type".to_string(), content_type.to_string())].into(),
        body: body.as_bytes().to_vec(),
        ..HttpRequest::default()
    }
}

#[test]
fn reads_the_graphql_request_of_every_form() {
    let json_body = graphql_request(&post(
        "application/json",
        r#"{"query": "query Q($n: Int) { shop { name } }", "variables": {"n": 1}, "operationName": "Q"}"#,
    ))
    .unwrap();
    assert_eq!(json_body.variables["n"], json!(1));
    assert_eq!(json_body.operation_name.as_deref(), Some("Q"));
    // Variables sent as a string of JSON, and an empty operation name.
    let string = graphql_request(&post(
        "application/json; charset=utf-8",
        r#"{"query": "{ shop { name } }", "variables": "{\"n\": 2}", "operationName": ""}"#,
    ))
    .unwrap();
    assert_eq!(string.variables["n"], json!(2));
    assert_eq!(string.operation_name, None);
    let raw = graphql_request(&post("application/graphql", "{ shop { name } }")).unwrap();
    assert_eq!(raw.query, "{ shop { name } }");
    let get = graphql_request(&HttpRequest {
        method: "GET".into(),
        query: vec![
            ("query".into(), "{ shop { name } }".into()),
            ("variables".into(), r#"{"a": true}"#.into()),
        ],
        ..HttpRequest::default()
    })
    .unwrap();
    assert_eq!(get.variables["a"], json!(true));

    assert!(graphql_request(&post("application/json", "{}")).is_err());
    assert!(graphql_request(&post("application/json", "not json")).is_err());
    assert!(
        graphql_request(&post(
            "application/json",
            r#"{"query": "{ a }", "variables": [1]}"#
        ))
        .is_err()
    );
}

#[test]
fn reads_the_throttle_setting() {
    assert_eq!(ThrottleSetting::parse("off"), Ok(ThrottleSetting::Off));
    assert_eq!(
        ThrottleSetting::parse("Plus"),
        Ok(ThrottleSetting::Plan(Plan::PLUS))
    );
    assert_eq!(ThrottleSetting::parse("store"), Ok(ThrottleSetting::Store));
    assert!(
        ThrottleSetting::parse("gold")
            .unwrap_err()
            .contains("off, store, standard")
    );
}

/// The Admin API of the demo store with the app of `tests/fixtures/platform-app`, its optional
/// scopes granted.
fn platform_api() -> std::sync::Arc<super::AdminApi> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/platform-app");
    let app = lsf_checkout::app::load(&root).expect("the app loads");
    let (store, _) = lsf_core::store::load::load(
        &lsf_core::store::load::DataSource::Demo,
        &Default::default(),
    );
    let api = super::AdminApi::new(
        &store,
        super::AdminOptions {
            app: Some(app),
            client_secret: "test-secret".to_string(),
            throttle: ThrottleSetting::Off,
            token_seed: Some(5),
            ..Default::default()
        },
    );
    let installed = send(
        &api,
        "POST",
        "/__lsf/admin/install",
        None,
        r#"{"scopes": "read_products,write_products,write_customers,read_discounts"}"#,
    );
    assert_eq!(installed.status, 200);
    std::sync::Arc::new(api)
}

fn send(
    api: &super::AdminApi,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: &str,
) -> super::HttpResponse {
    let mut headers: std::collections::HashMap<String, String> =
        [("content-type".to_string(), "application/json".to_string())].into();
    if let Some(token) = token {
        headers.insert("x-shopify-access-token".to_string(), token.to_string());
    }
    api.handle(&HttpRequest {
        method: method.into(),
        path: path.into(),
        headers,
        body: body.as_bytes().to_vec(),
        ..HttpRequest::default()
    })
    .expect("routed")
}

fn graphql(api: &super::AdminApi, token: &str, query: &str) -> (u16, serde_json::Value) {
    let response = send(
        api,
        "POST",
        "/admin/api/2026-10/graphql.json",
        Some(token),
        &json!({ "query": query }).to_string(),
    );
    (
        response.status,
        serde_json::from_slice(&response.body).unwrap_or_default(),
    )
}

/// A request that waited on the shop while `appUninstall`, `appRevokeAccessScopes` or a reset
/// ran ran after it with the grant it had looked up before: each change of the tokens moves the
/// shop's auth generation, and a request that sees it moved looks its token up again.
#[test]
fn a_request_that_waited_on_the_shop_runs_with_the_tokens_as_they_are_now() {
    // Uninstalled while it waited: refused.
    let api = platform_api();
    let token = api.token(&api.default_shop());
    let other = api.clone();
    let uninstalling = token.clone();
    api.at_next(super::admin::Hook::LookedUp, move || {
        let (_, answer) = graphql(
            &other,
            &uninstalling,
            "mutation { appUninstall { app { id } userErrors { message } } }",
        );
        assert_eq!(
            answer["data"]["appUninstall"]["userErrors"],
            json!([]),
            "{answer}"
        );
    });
    let (status, answer) = graphql(
        &api,
        &token,
        r#"mutation { productCreate(product: {title: "After the uninstall"}) { product { id } } }"#,
    );
    assert_eq!(status, 401, "{answer}");
    assert_eq!(
        api.with_default_shop(|state| state
            .all("Product")
            .filter(|product| product["title"] == "After the uninstall")
            .count()),
        Some(0),
        "no product made after the uninstall"
    );

    // A scope revoked while it waited: the request runs with the scopes left.
    let api = platform_api();
    let token = api.token(&api.default_shop());
    let other = api.clone();
    let revoking = token.clone();
    api.at_next(super::admin::Hook::LookedUp, move || {
        let (_, answer) = graphql(
            &other,
            &revoking,
            "mutation { appRevokeAccessScopes(scopes: [\"write_customers\"]) { revoked { handle } userErrors { message } } }",
        );
        assert_eq!(
            answer["data"]["appRevokeAccessScopes"]["revoked"],
            json!([{"handle": "write_customers"}]),
            "{answer}"
        );
    });
    let (status, answer) = graphql(
        &api,
        &token,
        r#"mutation { customerCreate(input: {email: "late@example.com"}) { customer { id } } }"#,
    );
    assert_eq!(status, 200);
    assert_eq!(
        answer["errors"][0]["extensions"]["code"], "ACCESS_DENIED",
        "{answer}"
    );

    // A reset while it waited: a token OAuth issued is gone.
    let api = platform_api();
    let shop = api.default_shop();
    let (_, created) = graphql(
        &api,
        &api.token(&shop),
        "mutation { delegateAccessTokenCreate(input: {delegateAccessScope: [\"read_products\"]}) { delegateAccessToken { accessToken } } }",
    );
    let delegate =
        created["data"]["delegateAccessTokenCreate"]["delegateAccessToken"]["accessToken"]
            .as_str()
            .unwrap()
            .to_string();
    let other = api.clone();
    api.at_next(super::admin::Hook::LookedUp, move || {
        assert_eq!(send(&other, "DELETE", "/__lsf/admin", None, "").status, 200);
    });
    let (status, answer) = graphql(&api, &delegate, "{ products(first: 1) { nodes { id } } }");
    assert_eq!(status, 401, "{answer}");
}

/// A request that came in after `appUninstall` unlocked the shop but before its effects reached
/// the tokens waits for them: it is refused, not run with the token being revoked.
#[test]
fn a_request_waits_for_the_tokens_a_request_before_it_changed() {
    let api = platform_api();
    let token = api.token(&api.default_shop());
    let other = api.clone();
    let late = token.clone();
    let (sender, received) = std::sync::mpsc::channel();
    api.at_next(super::admin::Hook::Unlocked, move || {
        std::thread::spawn(move || {
            let answer = graphql(
                &other,
                &late,
                r#"mutation { productCreate(product: {title: "During the uninstall"}) { product { id } } }"#,
            );
            let _ = sender.send(answer);
        });
        // Time for it to reach the shop, which it must not run on yet.
        std::thread::sleep(std::time::Duration::from_millis(100));
    });
    let (_, answer) = graphql(
        &api,
        &token,
        "mutation { appUninstall { app { id } userErrors { message } } }",
    );
    assert_eq!(
        answer["data"]["appUninstall"]["userErrors"],
        json!([]),
        "{answer}"
    );
    let (status, answer) = received
        .recv_timeout(std::time::Duration::from_secs(10))
        .expect("the late request answers");
    assert_eq!(status, 401, "{answer}");
}
