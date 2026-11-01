//! The Admin API end to end, through `AdminApi::handle`: one HTTP request in, one answer out,
//! as `lsf` serves it. Webhooks are delivered to a listener of the test, over a real socket.

mod support;

use std::time::Duration;

use lsf_admin::api::{
    AdminApi, AdminOptions, HttpRequest, HttpResponse, INVALID_TOKEN, ThrottleSetting,
};
use lsf_admin::auth::signing::verify_webhook_hmac;
use lsf_admin::throttle::Plan;
use lsf_admin::webhooks::DeliveryOptions;
use serde_json::{Value, json};
use support::json_of;
use support::receiver::Receiver;

const SECRET: &str = "test-secret";
const GRAPHQL: &str = "/admin/api/2026-10/graphql.json";

fn options(app: Option<lsf_checkout::app::App>, app_url: Option<String>) -> AdminOptions {
    AdminOptions {
        app,
        client_secret: SECRET.to_string(),
        app_url,
        throttle: ThrottleSetting::Store,
        delivery: DeliveryOptions {
            timeout: Duration::from_secs(3),
            retry_delays: Vec::new(),
            ..DeliveryOptions::default()
        },
        token_seed: Some(7),
    }
}

fn demo_api() -> AdminApi {
    AdminApi::new(&support::demo_store(), options(None, None))
}

fn request(method: &str, path: &str, headers: &[(&str, &str)], body: &[u8]) -> HttpRequest {
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    HttpRequest {
        method: method.to_string(),
        path: path.to_string(),
        query: form_urlencoded::parse(query.as_bytes())
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect(),
        headers: headers
            .iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value.to_string()))
            .collect(),
        body: body.to_vec(),
    }
}

fn send(api: &AdminApi, method: &str, path: &str, body: Value) -> HttpResponse {
    let body = if body.is_null() {
        Vec::new()
    } else {
        body.to_string().into_bytes()
    };
    api.handle(&request(
        method,
        path,
        &[("content-type", "application/json")],
        &body,
    ))
    .unwrap_or_else(|| panic!("{path} is not routed"))
}

fn header<'a>(response: &'a HttpResponse, name: &str) -> Option<&'a str> {
    response
        .headers
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// The answer to a GraphQL request with `token`.
fn graphql(api: &AdminApi, token: &str, query: &str, variables: Value) -> (u16, Value) {
    let body = json!({"query": query, "variables": variables}).to_string();
    let response = api
        .handle(&request(
            "POST",
            GRAPHQL,
            &[
                ("content-type", "application/json"),
                ("x-shopify-access-token", token),
            ],
            body.as_bytes(),
        ))
        .expect("the Admin API is routed");
    (response.status, json_of(&response))
}

fn default_token(api: &AdminApi) -> String {
    api.token(&api.default_shop())
}

#[test]
fn answers_the_introspection_query() {
    let api = demo_api();
    let query = "query { __schema { queryType { name } mutationType { name } types { name } } }";
    let (status, answer) = graphql(&api, &default_token(&api), query, Value::Null);
    assert_eq!(status, 200);
    let schema = &answer["data"]["__schema"];
    assert_eq!(schema["queryType"]["name"], json!("QueryRoot"));
    assert_eq!(schema["mutationType"]["name"], json!("Mutation"));
    assert_eq!(schema["types"].as_array().unwrap().len(), 3699);
    // graphql-js's `getIntrospectionQuery()`, as the GraphiQL of a client sends it.
    let full = include_str!("fixtures/introspection-query.graphql");
    let (status, answer) = graphql(&api, &default_token(&api), full, Value::Null);
    assert_eq!(status, 200);
    assert!(answer.get("errors").is_none(), "{}", answer["errors"]);
    assert_eq!(answer["extensions"]["cost"]["requestedQueryCost"], json!(0));
}

#[test]
fn pages_products_with_cursors_and_a_query() {
    let api = demo_api();
    let token = default_token(&api);
    let query = "query Page($after: String, $query: String) {
        products(first: 2, after: $after, query: $query) {
            nodes { title } pageInfo { hasNextPage endCursor }
        }
    }";
    let (status, first) = graphql(&api, &token, query, json!({}));
    assert_eq!(status, 200);
    let page = &first["data"]["products"];
    assert_eq!(page["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(page["pageInfo"]["hasNextPage"], json!(true));
    let mut seen: Vec<Value> = page["nodes"].as_array().unwrap().clone();
    let mut after = page["pageInfo"]["endCursor"].clone();
    loop {
        let (_, next) = graphql(&api, &token, query, json!({"after": after}));
        let page = &next["data"]["products"];
        seen.extend(page["nodes"].as_array().unwrap().iter().cloned());
        if page["pageInfo"]["hasNextPage"] != json!(true) {
            break;
        }
        after = page["pageInfo"]["endCursor"].clone();
    }
    let (_, all) = graphql(
        &api,
        &token,
        "{ products(first: 250) { nodes { title } } }",
        Value::Null,
    );
    assert_eq!(Value::Array(seen), all["data"]["products"]["nodes"]);

    let (_, wool) = graphql(&api, &token, query, json!({"query": "tag:wool"}));
    assert_eq!(
        wool["data"]["products"]["nodes"],
        json!([{"title": "Merino Crew Socks"}, {"title": "Wool Beanie"}])
    );
    let cost = &wool["extensions"]["cost"];
    assert_eq!(cost["requestedQueryCost"], json!(4));
    // The demo store has companies: a Plus shop, whose bucket is 20000.
    assert_eq!(cost["throttleStatus"]["maximumAvailable"], json!(20000.0));
}

#[test]
fn reads_any_record_by_node() {
    let api = demo_api();
    let token = default_token(&api);
    let (_, answer) = graphql(
        &api,
        &token,
        "query($id: ID!) { node(id: $id) { id __typename ... on Product { handle } } }",
        json!({"id": "gid://shopify/Product/4511006830056"}),
    );
    assert_eq!(
        answer["data"]["node"],
        json!({"id": "gid://shopify/Product/4511006830056", "__typename": "Product", "handle": "wool-beanie"})
    );
    let (_, invalid) = graphql(
        &api,
        &token,
        "query($id: ID!) { node(id: $id) { id } }",
        json!({"id": "123"}),
    );
    assert_eq!(
        invalid["errors"][0]["message"],
        json!("Variable $id of type ID! was provided invalid value")
    );
}

#[test]
fn denies_what_the_scopes_of_the_app_do_not_cover() {
    // The fixture app: read_products, write_products, read_orders, read_customers, write_discounts.
    let api = AdminApi::new(
        &support::demo_store(),
        options(Some(support::fixture_app()), None),
    );
    let token = default_token(&api);
    let (_, allowed) = graphql(
        &api,
        &token,
        "{ products(first: 1) { nodes { id } } }",
        Value::Null,
    );
    assert!(allowed.get("errors").is_none(), "{allowed}");
    let (status, denied) = graphql(
        &api,
        &token,
        "{ locations(first: 1) { nodes { name } } }",
        Value::Null,
    );
    assert_eq!(status, 200);
    assert_eq!(denied["data"], Value::Null);
    let error = &denied["errors"][0];
    assert_eq!(error["path"], json!(["locations"]));
    assert_eq!(error["extensions"]["code"], json!("ACCESS_DENIED"));
    assert_eq!(
        error["extensions"]["documentation"],
        json!("https://shopify.dev/api/usage/access-scopes")
    );
    assert!(error["message"].as_str().unwrap().starts_with(
        "Access denied for locations field. Required access: `read_locations` access scope"
    ));

    // Installed again with fewer scopes: the same token reads less.
    let installed = send(
        &api,
        "POST",
        "/__lsf/admin/install",
        json!({"scopes": "read_orders"}),
    );
    assert_eq!(installed.status, 200);
    assert_eq!(json_of(&installed)["scopesChanged"], json!(true));
    let (_, now_denied) = graphql(
        &api,
        &token,
        "{ products(first: 1) { nodes { id } } }",
        Value::Null,
    );
    assert_eq!(
        now_denied["errors"][0]["extensions"]["code"],
        json!("ACCESS_DENIED")
    );
    let (_, app) = graphql(
        &api,
        &token,
        "{ currentAppInstallation { accessScopes { handle } } }",
        Value::Null,
    );
    assert_eq!(
        app["data"]["currentAppInstallation"]["accessScopes"],
        json!([{"handle": "read_orders"}])
    );
}

#[test]
fn throttles_and_refuses_queries_over_the_cost_limit() {
    let mut options = options(None, None);
    options.throttle = ThrottleSetting::Plan(Plan {
        maximum_available: 100.0,
        restore_rate: 1.0,
    });
    let api = AdminApi::new(&support::demo_store(), options);
    let token = default_token(&api);
    let (status, throttled) = graphql(
        &api,
        &token,
        "{ products(first: 200) { nodes { id } } }",
        Value::Null,
    );
    assert_eq!(status, 200);
    assert_eq!(throttled.get("data"), None);
    assert_eq!(throttled["errors"][0]["message"], json!("Throttled"));
    assert_eq!(
        throttled["errors"][0]["extensions"]["code"],
        json!("THROTTLED")
    );
    assert_eq!(
        throttled["extensions"]["cost"]["actualQueryCost"],
        Value::Null
    );

    let expensive = "{ products(first: 250) { nodes { variants(first: 250) { nodes { id } } } } }";
    let (_, refused) = graphql(&api, &token, expensive, Value::Null);
    assert_eq!(
        refused["errors"][0]["extensions"]["code"],
        json!("MAX_COST_EXCEEDED")
    );
    assert!(
        refused["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("exceeds the single query max cost limit (1000)")
    );

    // `--admin-throttle off`.
    let mut off = self::options(None, None);
    off.throttle = ThrottleSetting::Off;
    let api = AdminApi::new(&support::demo_store(), off);
    let (_, answer) = graphql(
        &api,
        &default_token(&api),
        "{ products(first: 200) { nodes { id } } }",
        Value::Null,
    );
    assert!(answer["data"]["products"].is_object());
}

#[test]
fn delivers_the_webhook_of_a_created_product() {
    let receiver = Receiver::start(SECRET);
    let api = demo_api();
    let token = default_token(&api);
    let (_, subscribed) = graphql(
        &api,
        &token,
        "mutation($uri: String!) { webhookSubscriptionCreate(topic: PRODUCTS_CREATE, webhookSubscription: {uri: $uri, includeFields: [\"id\", \"title\", \"handle\"]}) {
            webhookSubscription { id topic uri includeFields } userErrors { field message } } }",
        json!({"uri": format!("{}/webhooks/products", receiver.url)}),
    );
    let subscription = &subscribed["data"]["webhookSubscriptionCreate"];
    assert_eq!(subscription["userErrors"], json!([]), "{subscribed}");
    assert_eq!(
        subscription["webhookSubscription"]["topic"],
        json!("PRODUCTS_CREATE")
    );
    let (_, listed) = graphql(
        &api,
        &token,
        "{ webhookSubscriptions(first: 5) { nodes { id topic } } webhookSubscriptionsCount { count } }",
        Value::Null,
    );
    assert_eq!(
        listed["data"]["webhookSubscriptionsCount"]["count"],
        json!(1)
    );

    let (_, created) = graphql(
        &api,
        &token,
        "mutation { productCreate(product: {title: \"Rain Jacket\"}) { product { id } userErrors { message } } }",
        Value::Null,
    );
    let product = created["data"]["productCreate"]["product"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (headers, body) = receiver.next_raw();
    assert_eq!(headers["x-shopify-topic"], "products/create");
    assert_eq!(headers["x-shopify-shop-domain"], api.default_shop());
    assert_eq!(headers["x-shopify-api-version"], "2026-10");
    assert!(verify_webhook_hmac(
        SECRET,
        &body,
        &headers["x-shopify-hmac-sha256"]
    ));
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        payload["admin_graphql_api_id"],
        Value::Null,
        "include_fields keeps id, title, handle"
    );
    assert_eq!(payload["title"], json!("Rain Jacket"));
    assert_eq!(payload["handle"], json!("rain-jacket"));
    assert_eq!(
        payload["id"],
        json!(lsf_admin::state::parse_gid(&product).unwrap().1)
    );

    let log = send(
        &api,
        "GET",
        "/__lsf/webhooks?wait=1&topic=products/create",
        Value::Null,
    );
    assert_eq!(log.status, 200);
    assert_eq!(json_of(&log)["deliveries"][0]["state"], json!("delivered"));
}

#[test]
fn exchanges_a_session_token_for_an_access_token() {
    let api = demo_api();
    let minted = send(&api, "POST", "/__lsf/admin/session-token", json!({}));
    assert_eq!(minted.status, 200);
    let session_token = json_of(&minted)["token"].as_str().unwrap().to_string();
    let exchanged = send(
        &api,
        "POST",
        "/admin/oauth/access_token",
        json!({
            "client_id": api.client_id(),
            "client_secret": SECRET,
            "grant_type": "urn:ietf:params:oauth:grant-type:token-exchange",
            "subject_token": session_token,
            "subject_token_type": "urn:ietf:params:oauth:token-type:id_token",
            "requested_token_type": "urn:shopify:params:oauth:token-type:online-access-token",
        }),
    );
    assert_eq!(
        exchanged.status,
        200,
        "{}",
        String::from_utf8_lossy(&exchanged.body)
    );
    let issued = json_of(&exchanged);
    let token = issued["access_token"].as_str().unwrap();
    assert!(token.starts_with("shpat_"));
    assert_ne!(token, default_token(&api));
    assert_eq!(issued["associated_user"]["account_owner"], json!(true));
    let (status, answer) = graphql(&api, token, "{ shop { myshopifyDomain } }", Value::Null);
    assert_eq!(status, 200);
    assert_eq!(
        answer["data"]["shop"]["myshopifyDomain"],
        json!(api.default_shop())
    );
}

#[test]
fn isolates_the_shops_it_creates() {
    let api = demo_api();
    let created = send(
        &api,
        "POST",
        "/__lsf/admin/shops",
        json!({"shop": "test-1"}),
    );
    assert_eq!(created.status, 201);
    let created = json_of(&created);
    assert_eq!(created["shop"], json!("test-1.myshopify.com"));
    let token = created["token"].as_str().unwrap().to_string();
    assert_eq!(token, api.token("test-1.myshopify.com"));
    let (_, shop) = graphql(&api, &token, "{ shop { myshopifyDomain } }", Value::Null);
    assert_eq!(
        shop["data"]["shop"]["myshopifyDomain"],
        json!("test-1.myshopify.com")
    );

    let count = "{ productsCount { count } }";
    let (_, before) = graphql(&api, &token, count, Value::Null);
    graphql(
        &api,
        &token,
        "mutation { productCreate(product: {title: \"Only in test-1\"}) { product { id } } }",
        Value::Null,
    );
    let (_, after) = graphql(&api, &token, count, Value::Null);
    let (_, default) = graphql(&api, &default_token(&api), count, Value::Null);
    let number = |answer: &Value| answer["data"]["productsCount"]["count"].as_u64().unwrap();
    assert_eq!(number(&after), number(&before) + 1);
    assert_eq!(number(&default), number(&before));

    assert_eq!(
        send(
            &api,
            "POST",
            "/__lsf/admin/shops",
            json!({"shop": "test-1"})
        )
        .status,
        409
    );
    assert_eq!(
        send(
            &api,
            "POST",
            "/__lsf/admin/shops",
            json!({"shop": "not a shop"})
        )
        .status,
        422
    );

    // A reset puts every shop back, and keeps them.
    let reset = send(&api, "DELETE", "/__lsf/admin", Value::Null);
    assert_eq!(
        json_of(&reset)["shops"],
        json!([api.default_shop(), "test-1.myshopify.com"])
    );
    let (_, again) = graphql(&api, &token, count, Value::Null);
    assert_eq!(number(&again), number(&before));
}

#[test]
fn deletes_the_shops_it_creates() {
    let api = demo_api();
    let created = json_of(&send(
        &api,
        "POST",
        "/__lsf/admin/shops",
        json!({"shop": "test-1"}),
    ));
    let token = created["token"].as_str().unwrap().to_string();
    let (_, subscribed) = graphql(
        &api,
        &token,
        "mutation { webhookSubscriptionCreate(topic: PRODUCTS_UPDATE, webhookSubscription: {uri: \"https://example.com/hooks\"}) { userErrors { message } } }",
        Value::Null,
    );
    assert_eq!(
        subscribed["data"]["webhookSubscriptionCreate"]["userErrors"],
        json!([])
    );

    let deleted = send(&api, "DELETE", "/__lsf/admin/shops/test-1", Value::Null);
    assert_eq!(deleted.status, 200);
    assert_eq!(
        json_of(&deleted),
        json!({"ok": true, "shop": "test-1.myshopify.com", "revoked": 1, "deletedSubscriptions": 1})
    );
    // Its token goes with it, and the server lists the default shop alone.
    let (status, refused) = graphql(&api, &token, "{ shop { name } }", Value::Null);
    assert_eq!(status, 401);
    assert_eq!(refused, json!({"errors": INVALID_TOKEN}));
    let status = json_of(&send(&api, "GET", "/__lsf/admin", Value::Null));
    assert_eq!(status["shops"].as_array().unwrap().len(), 1);

    let missing = send(
        &api,
        "DELETE",
        "/__lsf/admin/shops/test-1.myshopify.com",
        Value::Null,
    );
    assert_eq!(missing.status, 404);
    let default = send(
        &api,
        "DELETE",
        &format!("/__lsf/admin/shops/{}", api.default_shop()),
        Value::Null,
    );
    assert_eq!(default.status, 409);
    assert!(
        String::from_utf8_lossy(&default.body).contains("is the default shop"),
        "{}",
        String::from_utf8_lossy(&default.body)
    );
    assert_eq!(
        send(&api, "GET", "/__lsf/admin/shops/test-1", Value::Null).status,
        405
    );
    // The default shop still answers.
    let (status, _) = graphql(&api, &default_token(&api), "{ shop { name } }", Value::Null);
    assert_eq!(status, 200);

    // Made again under the name, it is a new shop with the app installed.
    let again = send(
        &api,
        "POST",
        "/__lsf/admin/shops",
        json!({"shop": "test-1"}),
    );
    assert_eq!(again.status, 201);
    let (status, answer) = graphql(
        &api,
        &token,
        "{ webhookSubscriptionsCount { count } }",
        Value::Null,
    );
    assert_eq!(status, 200);
    assert_eq!(
        answer["data"]["webhookSubscriptionsCount"]["count"],
        json!(0)
    );
}

#[test]
fn uninstalls_and_installs_the_app() {
    let receiver = Receiver::start(SECRET);
    let api = demo_api();
    let token = default_token(&api);
    graphql(
        &api,
        &token,
        "mutation($uri: String!) { webhookSubscriptionCreate(topic: APP_UNINSTALLED, webhookSubscription: {uri: $uri}) { userErrors { message } } }",
        json!({"uri": format!("{}/webhooks/app/uninstalled", receiver.url)}),
    );
    let uninstalled = send(&api, "POST", "/__lsf/admin/uninstall", Value::Null);
    assert_eq!(uninstalled.status, 200);
    assert_eq!(json_of(&uninstalled)["revoked"], json!(1));
    let (headers, body) = receiver.next_raw();
    assert_eq!(headers["x-shopify-topic"], "app/uninstalled");
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["myshopify_domain"], json!(api.default_shop()));

    let (status, refused) = graphql(&api, &token, "{ shop { name } }", Value::Null);
    assert_eq!(status, 401);
    assert_eq!(refused, json!({"errors": INVALID_TOKEN}));

    let installed = send(&api, "POST", "/__lsf/admin/install", Value::Null);
    assert_eq!(json_of(&installed)["token"], json!(token));
    let (status, _) = graphql(&api, &token, "{ shop { name } }", Value::Null);
    assert_eq!(status, 200);
}

#[test]
fn refuses_requests_without_a_valid_token() {
    let api = demo_api();
    for headers in [
        vec![],
        vec![(
            "x-shopify-access-token",
            "shpat_00000000000000000000000000000000",
        )],
    ] {
        let mut headers = headers;
        headers.push(("content-type", "application/json"));
        let response = api
            .handle(&request(
                "POST",
                GRAPHQL,
                &headers,
                br#"{"query": "{ shop { name } }"}"#,
            ))
            .unwrap();
        assert_eq!(response.status, 401);
        assert_eq!(json_of(&response), json!({"errors": INVALID_TOKEN}));
        assert_eq!(header(&response, "x-shopify-api-version"), Some("2026-10"));
        assert!(header(&response, "x-lsf-reason").is_some());
    }
    // A query in `application/graphql`, and a missing one.
    let token = default_token(&api);
    let raw = api
        .handle(&request(
            "POST",
            GRAPHQL,
            &[
                ("content-type", "application/graphql"),
                ("x-shopify-access-token", &token),
            ],
            b"{ shop { name } }",
        ))
        .unwrap();
    assert_eq!(
        json_of(&raw)["data"]["shop"]["name"],
        json!("Local Supply Co.")
    );
    assert!(header(&raw, "x-request-id").is_some());
    let missing = api
        .handle(&request(
            "POST",
            GRAPHQL,
            &[("x-shopify-access-token", &token)],
            b"{}",
        ))
        .unwrap();
    assert_eq!(missing.status, 400);
    assert_eq!(
        json_of(&missing),
        json!({"errors": {"query": "Required parameter missing or invalid"}})
    );
}

#[test]
fn says_the_rest_api_and_other_versions_are_not_served() {
    let api = demo_api();
    let rest = send(&api, "GET", "/admin/api/2026-10/products.json", Value::Null);
    assert_eq!(rest.status, 404);
    assert!(
        json_of(&rest)["errors"]
            .as_str()
            .unwrap()
            .contains("does not serve the REST Admin API")
    );
    let other = send(
        &api,
        "POST",
        "/admin/api/2026-07/graphql.json",
        json!({"query": "{ shop { name } }"}),
    );
    assert_eq!(other.status, 404);
    assert_eq!(
        json_of(&other),
        json!({"errors": "lsf serves the Admin API for API version 2026-10 only, not 2026-07"})
    );
    assert!(
        api.handle(&request("GET", "/products/x", &[], b""))
            .is_none()
    );
}

#[test]
fn reports_its_status_and_triggers_webhooks_from_the_shop() {
    let receiver = Receiver::start(SECRET);
    let api = demo_api();
    let status = json_of(&send(&api, "GET", "/__lsf/admin", Value::Null));
    assert_eq!(status["defaultShop"], json!(api.default_shop()));
    assert_eq!(status["shops"][0]["token"], json!(default_token(&api)));
    assert!(status["shops"][0]["counts"]["Product"].as_u64().unwrap() > 0);
    assert!(
        status["mutations"]
            .as_array()
            .unwrap()
            .contains(&json!("productCreate"))
    );

    let triggered = send(
        &api,
        "POST",
        "/__lsf/webhooks/trigger",
        json!({"topic": "products/update", "id": "gid://shopify/Product/4511006830056", "address": format!("{}/hooks", receiver.url), "wait": true}),
    );
    assert_eq!(
        triggered.status,
        200,
        "{}",
        String::from_utf8_lossy(&triggered.body)
    );
    let (headers, body) = receiver.next_raw();
    assert_eq!(headers["x-shopify-topic"], "products/update");
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["title"], json!("Wool Beanie"));
    let missing = send(
        &api,
        "POST",
        "/__lsf/webhooks/trigger",
        json!({"topic": "products/update", "id": "gid://shopify/Product/1", "address": receiver.url}),
    );
    assert_eq!(missing.status, 404);

    // A payload of its own is sent as is, for a topic lsf builds no payload of, to the default
    // shop or to the one named (its domain written in full).
    for (shop, domain) in [
        (None, api.default_shop()),
        (Some("other"), "other.myshopify.com".to_string()),
    ] {
        let triggered = send(
            &api,
            "POST",
            "/__lsf/webhooks/trigger",
            json!({"topic": "carts/create", "shop": shop, "payload": {"a": 1}, "address": receiver.url, "wait": true}),
        );
        assert_eq!(
            triggered.status,
            200,
            "{}",
            String::from_utf8_lossy(&triggered.body)
        );
        let (headers, body) = receiver.next_raw();
        assert_eq!(headers["x-shopify-shop-domain"], domain);
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap(),
            json!({"a": 1})
        );
    }
}

#[test]
fn delivers_the_webhooks_of_metafields_and_metaobjects() {
    let receiver = Receiver::start(SECRET);
    let api = demo_api();
    let token = default_token(&api);
    for topic in [
        "METAFIELD_DEFINITIONS_CREATE",
        "PRODUCTS_UPDATE",
        "METAOBJECTS_CREATE",
    ] {
        let (_, subscribed) = graphql(
            &api,
            &token,
            "mutation($topic: WebhookSubscriptionTopic!, $uri: String!) { webhookSubscriptionCreate(topic: $topic, webhookSubscription: {uri: $uri}) { userErrors { message } } }",
            json!({"topic": topic, "uri": format!("{}/webhooks", receiver.url)}),
        );
        assert_eq!(
            subscribed["data"]["webhookSubscriptionCreate"]["userErrors"],
            json!([]),
            "{subscribed}"
        );
    }
    let (_, products) = graphql(
        &api,
        &token,
        "{ products(first: 1) { nodes { id } } }",
        Value::Null,
    );
    let product = products["data"]["products"]["nodes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // A definition, then a value it checks, on a product.
    let (_, defined) = graphql(
        &api,
        &token,
        "mutation { metafieldDefinitionCreate(definition: {name: \"Care\", namespace: \"$app\", key: \"care\", ownerType: PRODUCT, type: \"multi_line_text_field\"}) {
            createdDefinition { id namespace } userErrors { field message code } } }",
        Value::Null,
    );
    let definition = &defined["data"]["metafieldDefinitionCreate"]["createdDefinition"];
    assert!(
        definition["namespace"]
            .as_str()
            .unwrap()
            .starts_with("app--"),
        "{defined}"
    );
    let (_, set) = graphql(
        &api,
        &token,
        "mutation($metafields: [MetafieldsSetInput!]!) { metafieldsSet(metafields: $metafields) {
            metafields { key value definition { id } } userErrors { field message code } } }",
        json!({"metafields": [{"ownerId": product, "namespace": "$app", "key": "care", "value": "Wash cold."}]}),
    );
    let set = &set["data"]["metafieldsSet"];
    assert_eq!(set["userErrors"], json!([]));
    assert_eq!(set["metafields"][0]["definition"]["id"], definition["id"]);
    let (_, read) = graphql(
        &api,
        &token,
        "query($id: ID!) { product(id: $id) { metafield(namespace: \"$app\", key: \"care\") { value } } }",
        json!({"id": product}),
    );
    assert_eq!(read["data"]["product"]["metafield"]["value"], "Wash cold.");

    // A metaobject type and an entry.
    let (_, created) = graphql(
        &api,
        &token,
        "mutation { metaobjectDefinitionCreate(definition: {type: \"$app:badge\", fieldDefinitions: [{key: \"label\", type: \"single_line_text_field\"}]}) { userErrors { message } }
            metaobjectCreate(metaobject: {type: \"$app:badge\", handle: \"eco\", fields: [{key: \"label\", value: \"Eco\"}]}) {
            metaobject { id handle } userErrors { message } } }",
        Value::Null,
    );
    let entry = &created["data"]["metaobjectCreate"]["metaobject"];
    assert_eq!(entry["handle"], "eco", "{created}");

    let mut deliveries = Vec::new();
    for _ in 0..3 {
        let (headers, body) = receiver.next_raw();
        assert!(verify_webhook_hmac(
            SECRET,
            &body,
            &headers["x-shopify-hmac-sha256"]
        ));
        let payload: Value = serde_json::from_slice(&body).unwrap();
        deliveries.push((headers["x-shopify-topic"].clone(), payload));
    }
    let topics: Vec<&str> = deliveries.iter().map(|(topic, _)| topic.as_str()).collect();
    assert_eq!(
        topics,
        [
            "metafield_definitions/create",
            "products/update",
            "metaobjects/create"
        ]
    );
    let (_, definition_payload) = &deliveries[0];
    assert_eq!(definition_payload["key"], "care");
    assert_eq!(definition_payload["owner_type"], "Product");
    assert_eq!(definition_payload["type_name"], "multi_line_text_field");
    let (_, product_payload) = &deliveries[1];
    assert_eq!(product_payload["admin_graphql_api_id"], json!(product));
    let (_, metaobject_payload) = &deliveries[2];
    assert_eq!(metaobject_payload["id"], entry["id"]);
    assert_eq!(metaobject_payload["handle"], "eco");
    assert_eq!(metaobject_payload["fields"], json!({"label": "Eco"}));
    assert!(
        metaobject_payload["type"]
            .as_str()
            .unwrap()
            .ends_with("--badge")
    );
}

#[test]
fn a_request_nested_too_deeply_is_answered_not_crashed() {
    // Each of these, 20,000 levels deep, overflowed the stack of the request's thread, and the
    // server aborted.
    let api = demo_api();
    let token = default_token(&api);
    let deep = "(".repeat(20_000);

    // A search query is read up to the limit: the rest is ignored, with a warning.
    let (status, searched) = graphql(
        &api,
        &token,
        "query($q: String) { products(first: 1, query: $q) { nodes { id } } }",
        json!({"q": deep}),
    );
    assert_eq!(status, 200);
    assert_eq!(
        searched["data"]["products"]["nodes"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        searched["extensions"]["search"][0]["warnings"],
        json!([
            {"message": "Parentheses nest more than 100 deep: what they hold is ignored."},
            {"message": "A `(` is not closed."},
        ])
    );
    let (_, negated) = graphql(
        &api,
        &token,
        "query($q: String) { products(first: 1, query: $q) { nodes { id } } }",
        json!({"q": format!("{}title:nothing", "NOT ".repeat(20_001))}),
    );
    assert_eq!(
        negated["data"]["products"]["nodes"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    // A webhook filter must follow the grammar: it is refused.
    let (_, subscribed) = graphql(
        &api,
        &token,
        "mutation($f: String) { webhookSubscriptionCreate(topic: PRODUCTS_UPDATE, webhookSubscription: {uri: \"https://example.com/hooks\", filter: $f}) { webhookSubscription { id } userErrors { field message } } }",
        json!({"f": format!("{deep}title:a{}", ")".repeat(20_000))}),
    );
    let errors = &subscribed["data"]["webhookSubscriptionCreate"]["userErrors"];
    assert_eq!(errors[0]["field"], json!(["webhookSubscription", "filter"]));
    assert!(
        errors[0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("nest more than 100 deep")),
        "{subscribed}"
    );

    // The type of a variable is GraphQL's nesting: the query is too large.
    let (_, typed) = graphql(
        &api,
        &token,
        &format!(
            "query($a: {}Int{}) {{ shop {{ name }} }}",
            "[".repeat(20_000),
            "]".repeat(20_000)
        ),
        Value::Null,
    );
    assert_eq!(
        typed,
        json!({"errors": [{"message": "This query is too large to execute."}]})
    );

    // Variables nested past serde_json's limit (128) are no JSON body lsf reads.
    let variables = format!("{}1{}", "[".repeat(20_000), "]".repeat(20_000));
    let response = api
        .handle(&request(
            "POST",
            GRAPHQL,
            &[
                ("content-type", "application/json"),
                ("x-shopify-access-token", &token),
            ],
            format!(r#"{{"query": "{{ shop {{ name }} }}", "variables": {{"a": {variables}}}}}"#)
                .as_bytes(),
        ))
        .unwrap();
    assert_eq!(response.status, 400);
}
