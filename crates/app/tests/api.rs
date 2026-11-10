//! The Admin API end to end, through `AdminApi::handle`: one HTTP request in, one answer out,
//! as `lsf` serves it. Webhooks are delivered to a listener of the test, over a real socket.

mod support;

use std::time::Duration;

use lsf_app::admin::api::{
    AdminApi, AdminOptions, HttpRequest, HttpResponse, INVALID_TOKEN, ThrottleSetting,
};
use lsf_app::admin::auth::signing::verify_webhook_hmac;
use lsf_app::admin::throttle::Plan;
use lsf_app::admin::webhooks::DeliveryOptions;
use serde_json::{Value, json};
use support::json_of;
use support::receiver::Receiver;

const SECRET: &str = "test-secret";
const GRAPHQL: &str = "/admin/api/2026-10/graphql.json";

fn options(app: Option<lsf_app::config::App>, app_url: Option<String>) -> AdminOptions {
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
        json!(lsf_app::admin::state::parse_gid(&product).unwrap().1)
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

/// The objects that run the app's Functions go with the uninstall, those the store data declares
/// included, and neither a reseed (the data changed on disk) nor a reinstall brings them back.
#[test]
fn an_uninstall_removes_the_owners_of_the_functions_for_good() {
    let store = support::fixture_store("rich");
    let api = AdminApi::new(&store, options(Some(support::fixture_app()), None));
    let owners = |api: &AdminApi| {
        api.with_default_shop(|state| {
            lsf_app::admin::domains::app::FUNCTION_OWNER_TYPES
                .iter()
                .map(|type_name| state.count(type_name))
                .sum::<usize>()
                + state.count("DiscountAutomaticNode")
                + state.count("DiscountCodeNode")
        })
        .unwrap()
    };
    // The data's: a payment, a delivery customization, a validation, a cart transform, and
    // two app discounts.
    assert_eq!(owners(&api), 6);
    let uninstalled = send(&api, "POST", "/__lsf/admin/uninstall", Value::Null);
    assert_eq!(json_of(&uninstalled)["removedFunctionOwners"], json!(6));
    assert_eq!(owners(&api), 0);
    // Uninstalled again: nothing more.
    let again = send(&api, "POST", "/__lsf/admin/uninstall", Value::Null);
    assert_eq!(json_of(&again)["removedFunctionOwners"], json!(0));

    api.reseed(&store);
    assert_eq!(owners(&api), 0, "a new seed does not bring them back");
    send(&api, "POST", "/__lsf/admin/install", Value::Null);
    assert_eq!(owners(&api), 0, "nor does a reinstall");
    // Installed, a new seed is the data's again.
    api.reseed(&store);
    assert_eq!(owners(&api), 6);
}

/// The terminal says when the app is uninstalled from a shop and installed on it again: what
/// stops (its UI extensions, its Functions) and what comes back. An uninstall of an app that is
/// not installed says nothing.
#[test]
fn an_uninstall_and_a_reinstall_are_said_in_the_terminal() {
    let store = support::fixture_store("rich");
    let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sink = lines.clone();
    let api = AdminApi::new(&store, options(Some(support::fixture_app()), None)).with_notices(
        std::sync::Arc::new(move |line: &str| sink.lock().unwrap().push(line.to_string())),
    );
    let shop = api.default_shop();
    send(&api, "POST", "/__lsf/admin/uninstall", Value::Null);
    send(&api, "POST", "/__lsf/admin/uninstall", Value::Null);
    send(&api, "POST", "/__lsf/admin/install", Value::Null);
    let lines = lines.lock().unwrap().clone();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(
        lines[0],
        format!(
            "{shop}: the app is not installed on the shop: its UI extensions do not render and its \
             Functions do not run until it is installed again (`POST /__lsf/admin/install`, OAuth \
             or a token exchange installs it again)"
        )
    );
    assert!(
        lines[1].starts_with(&format!(
            "{shop}: the app is installed again: its UI extensions render again"
        )),
        "{}",
        lines[1]
    );
}

/// The definitions of the app's reserved namespaces, as a test reads them: `(owner type,
/// namespace, key, id)` of each, in the order of their ids.
fn definitions(api: &AdminApi) -> Vec<(String, String, String, String)> {
    api.with_default_shop(|state| {
        state
            .all("MetafieldDefinition")
            .map(|definition| {
                let text = |field: &str| definition[field].as_str().unwrap_or_default().to_string();
                (
                    text("ownerType"),
                    text("namespace"),
                    text("key"),
                    text("id"),
                )
            })
            .collect()
    })
    .unwrap()
}

/// What shopify.dev says an uninstall does to an app's metafield definitions ("Manage metafield
/// definitions", "App uninstall and reinstall"): those of its reserved namespaces are deleted,
/// declared in `shopify.app.toml` or made through the Admin API, and their metafields stay without
/// a definition; a reinstall creates them again under new ids and gives them back their
/// metafields. A definition of another namespace is the merchant's: it stays, under its id. The
/// terminal says what comes back, and what happens to the app proxy.
#[test]
fn an_uninstall_deletes_the_apps_own_metafield_definitions_and_a_reinstall_brings_them_back() {
    let store = support::fixture_store("rich");
    let mut app = support::fixture_app();
    app.config
        .order_metafields
        .push(lsf_app::config::DeclaredMetafieldDefinition {
            namespace: "$app".to_string(),
            key: "gift_note".to_string(),
            kind: "single_line_text_field".to_string(),
            name: Some("Gift note".to_string()),
            description: None,
            cart_to_order_copyable: true,
        });
    app.config.app_proxy = Some(lsf_app::config::AppProxyConfig {
        url: "/proxy".to_string(),
        subpath: "reviews".to_string(),
        prefix: "apps".to_string(),
    });
    let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sink = lines.clone();
    let api = AdminApi::new(&store, options(Some(app), None)).with_notices(std::sync::Arc::new(
        move |line: &str| sink.lock().unwrap().push(line.to_string()),
    ));
    let shop = api.default_shop();
    let token = default_token(&api);
    let (_, products) = graphql(
        &api,
        &token,
        "{ products(first: 1) { nodes { id } } }",
        Value::Null,
    );
    let product = products["data"]["products"]["nodes"][0]["id"].clone();
    for (namespace, key, kind) in [
        ("$app:loyalty", "tier", "single_line_text_field"),
        ("loyalty", "points", "number_integer"),
    ] {
        let (_, created) = graphql(
            &api,
            &token,
            "mutation($definition: MetafieldDefinitionInput!) { metafieldDefinitionCreate(definition: $definition) { createdDefinition { id } userErrors { message } } }",
            json!({"definition": {
                "name": key, "namespace": namespace, "key": key, "type": kind,
                "ownerType": "PRODUCT",
            }}),
        );
        assert_eq!(
            created["data"]["metafieldDefinitionCreate"]["userErrors"],
            json!([]),
            "{created}"
        );
    }
    let (_, set) = graphql(
        &api,
        &token,
        "mutation($metafields: [MetafieldsSetInput!]!) { metafieldsSet(metafields: $metafields) { userErrors { message } } }",
        json!({"metafields": [
            {"ownerId": product, "namespace": "$app:loyalty", "key": "tier", "type": "single_line_text_field", "value": "gold"},
            {"ownerId": product, "namespace": "loyalty", "key": "points", "type": "number_integer", "value": "10"},
        ]}),
    );
    assert_eq!(
        set["data"]["metafieldsSet"]["userErrors"],
        json!([]),
        "{set}"
    );
    let before = definitions(&api);
    let app_owned = |list: &[(String, String, String, String)]| -> Vec<(String, String, String)> {
        list.iter()
            .filter(|(_, namespace, _, _)| namespace.starts_with("app--"))
            .map(|(owner, namespace, key, _)| (owner.clone(), namespace.clone(), key.clone()))
            .collect()
    };
    let merchant =
        |list: &[(String, String, String, String)]| -> Vec<(String, String, String, String)> {
            list.iter()
                .filter(|(_, namespace, _, _)| !namespace.starts_with("app--"))
                .cloned()
                .collect()
        };
    let owned = app_owned(&before);
    assert_eq!(owned.len(), 2, "{before:?}");
    assert!(
        owned
            .iter()
            .any(|(owner, _, key)| owner == "ORDER" && key == "gift_note")
    );
    assert!(
        owned
            .iter()
            .any(|(owner, namespace, key)| owner == "PRODUCT"
                && namespace.ends_with("--loyalty")
                && key == "tier")
    );
    let tier = |api: &AdminApi| -> (Option<String>, Value) {
        api.with_default_shop(|state| {
            let metafield = state
                .all("Metafield")
                .find(|metafield| {
                    metafield["owner"] == product
                        && metafield["key"] == "tier"
                        && metafield["namespace"]
                            .as_str()
                            .is_some_and(|namespace| namespace.starts_with("app--"))
                })
                .expect("the metafield stays");
            (
                metafield["value"].as_str().map(str::to_string),
                metafield["definition"].clone(),
            )
        })
        .unwrap()
    };

    let uninstalled = send(&api, "POST", "/__lsf/admin/uninstall", Value::Null);
    assert_eq!(
        json_of(&uninstalled)["removedMetafieldDefinitions"],
        json!(2)
    );
    let after = definitions(&api);
    assert_eq!(app_owned(&after), Vec::<(String, String, String)>::new());
    assert_eq!(
        merchant(&after),
        merchant(&before),
        "the merchant's stay, ids included"
    );
    let (value, definition) = tier(&api);
    assert_eq!(value.as_deref(), Some("gold"), "the value stays");
    assert_eq!(definition, Value::Null, "without a definition");

    send(&api, "POST", "/__lsf/admin/install", Value::Null);
    let back = definitions(&api);
    assert_eq!(app_owned(&back), owned, "in the order they were created");
    for (owner, namespace, key, id) in back.iter().filter(|(_, ns, _, _)| ns.starts_with("app--")) {
        let old = before
            .iter()
            .find(|(o, n, k, _)| (o, n, k) == (owner, namespace, key))
            .map(|(_, _, _, id)| id)
            .unwrap();
        assert_ne!(id, old, "{key}: a new id");
    }
    assert_eq!(merchant(&back), merchant(&before));
    let (value, definition) = tier(&api);
    assert_eq!(value.as_deref(), Some("gold"));
    let (_, read) = graphql(
        &api,
        &token,
        "{ metafieldDefinitions(first: 5, ownerType: PRODUCT, namespace: \"$app:loyalty\") { nodes { id key metafieldsCount } } }",
        Value::Null,
    );
    let nodes = &read["data"]["metafieldDefinitions"]["nodes"];
    assert_eq!(nodes[0]["key"], "tier", "{read}");
    assert_eq!(nodes[0]["metafieldsCount"], 1, "{read}");
    assert_eq!(definition, nodes[0]["id"], "the metafield has it again");

    // A new seed while the app is uninstalled: the declared definition is deleted again, the one
    // made through the API is gone with the data the seed dropped; a reinstall brings back the
    // declared one.
    send(&api, "POST", "/__lsf/admin/uninstall", Value::Null);
    api.reseed(&store);
    assert_eq!(
        app_owned(&definitions(&api)),
        Vec::<(String, String, String)>::new()
    );
    send(&api, "POST", "/__lsf/admin/install", Value::Null);
    let reseeded = app_owned(&definitions(&api));
    assert_eq!(
        reseeded,
        vec![(
            "ORDER".to_string(),
            owned
                .iter()
                .find(|(o, _, _)| o == "ORDER")
                .unwrap()
                .1
                .clone(),
            "gift_note".to_string()
        )]
    );

    let lines = lines.lock().unwrap().clone();
    assert_eq!(
        lines[1],
        format!(
            "{shop}: the app is not installed on the shop: uninstalling it removed its app proxy, \
             so /apps/reviews is the storefront's 404 and no request reaches the backend until it \
             is installed again (`POST /__lsf/admin/install`, OAuth or a token exchange installs \
             it again)"
        ),
        "{lines:?}"
    );
    assert!(
        lines[2].ends_with(
            "; its 2 metafield definitions are back, under new ids; /apps/reviews goes to its app \
             proxy again"
        ),
        "{}",
        lines[2]
    );
    assert!(
        lines.last().unwrap().ends_with(
            "; its metafield definition is back, under a new id; /apps/reviews goes to its app \
             proxy again"
        ),
        "{lines:?}"
    );
}

/// The metaobject definitions of the store, as a test reads them: `(type, id, entries)` of each,
/// in the order of their ids, the entries by handle.
fn metaobject_definitions(api: &AdminApi) -> Vec<(String, String, Vec<String>)> {
    api.with_default_shop(|state| {
        state
            .all("MetaobjectDefinition")
            .map(|definition| {
                let id = definition["id"].as_str().unwrap_or_default().to_string();
                let mut entries: Vec<String> = state
                    .all("Metaobject")
                    .filter(|entry| entry["definition"] == definition["id"])
                    .map(|entry| entry["handle"].as_str().unwrap_or_default().to_string())
                    .collect();
                entries.sort();
                (
                    definition["type"].as_str().unwrap_or_default().to_string(),
                    id,
                    entries,
                )
            })
            .collect()
    })
    .unwrap()
}

/// What shopify.dev says an uninstall does to an app's metaobject definitions ("Manage metaobject
/// definitions", "App uninstall and reinstall"): those of its reserved types (`$app:<type>`) are
/// deleted, and their metaobjects are kept without a definition; a reinstall creates the
/// definitions again under new ids and gives them back their metaobjects. A type without the
/// prefix is the merchant's: it stays, under its id, with its entries. lsf keeps the entries out
/// of reach meanwhile (a `Metaobject` has a non-null `definition`), and a metafield definition
/// whose validation names the old definition names the new one.
#[test]
fn an_uninstall_deletes_the_apps_own_metaobject_definitions_and_a_reinstall_brings_them_back() {
    let store = support::fixture_store("rich");
    let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let sink = lines.clone();
    let mut app = support::fixture_app();
    app.config
        .scopes
        .extend(["write_metaobject_definitions", "write_metaobjects"].map(str::to_string));
    let api = AdminApi::new(&store, options(Some(app), None)).with_notices(std::sync::Arc::new(
        move |line: &str| sink.lock().unwrap().push(line.to_string()),
    ));
    let shop = api.default_shop();
    let token = default_token(&api);
    let gql = |query: &str, variables: Value| -> Value {
        let (status, answer) = graphql(&api, &token, query, variables);
        assert_eq!(status, 200, "{answer}");
        answer
    };
    let mut created = Vec::new();
    for kind in ["$app:author", "size_chart"] {
        let answer = gql(
            "mutation($definition: MetaobjectDefinitionCreateInput!) { metaobjectDefinitionCreate(definition: $definition) { metaobjectDefinition { id type } userErrors { message } } }",
            json!({"definition": {"type": kind, "name": kind, "displayNameKey": "name",
                "fieldDefinitions": [{"key": "name", "name": "Name", "type": "single_line_text_field"}]}}),
        );
        let payload = &answer["data"]["metaobjectDefinitionCreate"];
        assert_eq!(payload["userErrors"], json!([]), "{answer}");
        created.push(
            payload["metaobjectDefinition"]["id"]
                .as_str()
                .unwrap()
                .to_string(),
        );
        for name in ["Ada", "Grace"] {
            let entry = gql(
                "mutation($metaobject: MetaobjectCreateInput!) { metaobjectCreate(metaobject: $metaobject) { metaobject { id } userErrors { message } } }",
                json!({"metaobject": {"type": kind, "fields": [{"key": "name", "value": name}]}}),
            );
            assert_eq!(
                entry["data"]["metaobjectCreate"]["userErrors"],
                json!([]),
                "{entry}"
            );
        }
    }
    // A merchant's product definition whose entries must be authors: its validation names the
    // app's definition by id.
    let referring = gql(
        "mutation($definition: MetafieldDefinitionInput!) { metafieldDefinitionCreate(definition: $definition) { createdDefinition { id } userErrors { message } } }",
        json!({"definition": {"name": "Author", "namespace": "custom", "key": "author",
            "type": "metaobject_reference", "ownerType": "PRODUCT",
            "validations": [{"name": "metaobject_definition_id", "value": created[0]}]}}),
    );
    assert_eq!(
        referring["data"]["metafieldDefinitionCreate"]["userErrors"],
        json!([]),
        "{referring}"
    );
    let author_type = api
        .with_default_shop(|state| format!("app--{}--author", state.app_id()))
        .unwrap();
    let before = metaobject_definitions(&api);
    let app_owned = |list: &[(String, String, Vec<String>)]| -> Vec<(String, Vec<String>)> {
        list.iter()
            .filter(|(kind, _, _)| kind.starts_with("app--"))
            .map(|(kind, _, entries)| (kind.clone(), entries.clone()))
            .collect()
    };
    let merchant = |list: &[(String, String, Vec<String>)]| -> Vec<(String, String, Vec<String>)> {
        list.iter()
            .filter(|(kind, _, _)| !kind.starts_with("app--"))
            .cloned()
            .collect()
    };
    let owned = app_owned(&before);
    assert!(
        owned.contains(&(
            author_type.clone(),
            vec!["ada".to_string(), "grace".to_string()]
        )),
        "{before:?}"
    );
    let entry_ids = |api: &AdminApi| -> Vec<String> {
        let mut ids: Vec<String> = api
            .with_default_shop(|state| {
                state
                    .all("Metaobject")
                    .filter(|entry| entry["type"] == json!(author_type))
                    .map(|entry| entry["id"].as_str().unwrap().to_string())
                    .collect()
            })
            .unwrap();
        ids.sort();
        ids
    };
    let authors = entry_ids(&api);
    assert_eq!(authors.len(), 2);

    let uninstalled = json_of(&send(&api, "POST", "/__lsf/admin/uninstall", Value::Null));
    assert_eq!(
        uninstalled["removedMetaobjectDefinitions"],
        json!(owned.len()),
        "{uninstalled}"
    );
    assert_eq!(
        uninstalled["retainedMetaobjects"],
        json!(
            owned
                .iter()
                .map(|(_, entries)| entries.len())
                .sum::<usize>()
        )
    );
    let after = metaobject_definitions(&api);
    assert_eq!(app_owned(&after), Vec::<(String, Vec<String>)>::new());
    assert_eq!(
        merchant(&after),
        merchant(&before),
        "the merchant's stay, ids included"
    );
    assert_eq!(entry_ids(&api), Vec::<String>::new(), "kept out of reach");

    send(&api, "POST", "/__lsf/admin/install", Value::Null);
    let back = metaobject_definitions(&api);
    assert_eq!(
        app_owned(&back),
        owned,
        "in the order they were created, with their entries"
    );
    assert_eq!(merchant(&back), merchant(&before));
    let new_id = back
        .iter()
        .find(|(kind, _, _)| *kind == author_type)
        .map(|(_, id, _)| id.clone())
        .unwrap();
    assert_ne!(new_id, created[0], "a new id");
    assert_eq!(entry_ids(&api), authors, "the same metaobjects");
    let read = gql(
        "{ metaobjectDefinitionByType(type: \"$app:author\") { id metaobjectsCount } metaobjects(type: \"$app:author\", first: 5) { nodes { handle definition { id } } } }",
        Value::Null,
    );
    assert_eq!(
        read["data"]["metaobjectDefinitionByType"]["id"],
        json!(new_id),
        "{read}"
    );
    assert_eq!(
        read["data"]["metaobjectDefinitionByType"]["metaobjectsCount"],
        json!(2)
    );
    let nodes = read["data"]["metaobjects"]["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 2, "{read}");
    assert!(
        nodes
            .iter()
            .all(|node| node["definition"]["id"] == json!(new_id))
    );
    let validations = api
        .with_default_shop(|state| {
            state
                .all("MetafieldDefinition")
                .find(|definition| definition["key"] == "author")
                .map(|definition| definition["validations"].clone())
        })
        .flatten()
        .unwrap();
    assert_eq!(validations[0]["value"], json!(new_id), "{validations}");

    // A new seed while uninstalled drops the definition the app made, with its entries.
    send(&api, "POST", "/__lsf/admin/uninstall", Value::Null);
    api.reseed(&store);
    send(&api, "POST", "/__lsf/admin/install", Value::Null);
    assert!(
        !metaobject_definitions(&api)
            .iter()
            .any(|(kind, _, _)| *kind == author_type)
    );

    let lines = lines.lock().unwrap().clone();
    let reinstalled = lines
        .iter()
        .find(|line| line.starts_with(&format!("{shop}: the app is installed again")))
        .unwrap();
    assert!(
        reinstalled.contains(&format!(
            "; its {} metaobject definition{} back, under new ids, with their metaobjects",
            owned.len(),
            if owned.len() == 1 { " is" } else { "s are" }
        )) || (owned.len() == 1
            && reinstalled.contains(
                "; its metaobject definition is back, under a new id, with its metaobjects"
            )),
        "{reinstalled}"
    );
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

#[test]
fn a_revoked_token_says_what_revoked_it_and_how_to_recover_once_in_the_log() {
    let said = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let log = std::sync::Arc::clone(&said);
    let api = demo_api().with_log(std::sync::Arc::new(move |line: &str| {
        log.lock().unwrap().push(line.to_string())
    }));
    let refused = |token: &str| {
        let response = api
            .handle(&support::graphql_request(
                token,
                "{ shop { name } }",
                Value::Null,
            ))
            .unwrap();
        assert_eq!(response.status, 401);
        assert_eq!(json_of(&response), json!({"errors": INVALID_TOKEN}));
        header(&response, "x-lsf-reason").unwrap().to_string()
    };
    let recover = ": run the token exchange or OAuth again for a new one (a backend that stored \
                   this one in its session storage must forget it), or use a new shop name";

    // A token exchange, then a reset.
    let session_token =
        json_of(&send(&api, "POST", "/__lsf/admin/session-token", json!({})))["token"]
            .as_str()
            .unwrap()
            .to_string();
    let exchanged = json_of(&send(
        &api,
        "POST",
        "/admin/oauth/access_token",
        json!({
            "client_id": api.client_id(),
            "client_secret": SECRET,
            "grant_type": "urn:ietf:params:oauth:grant-type:token-exchange",
            "subject_token": session_token,
            "subject_token_type": "urn:ietf:params:oauth:token-type:id_token",
            "requested_token_type": "urn:shopify:params:oauth:token-type:offline-access-token",
        }),
    ))["access_token"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        send(&api, "DELETE", "/__lsf/admin", Value::Null).status,
        200
    );
    let shop = api.default_shop();
    let reason = refused(&exchanged);
    assert_eq!(
        reason,
        format!(
            "this access token of {shop} was revoked by a reset of the Admin API (DELETE \
             /__lsf/admin){recover}"
        )
    );
    // Every refusal says it; the log, once.
    assert_eq!(refused(&exchanged), reason);
    assert_eq!(
        *said.lock().unwrap(),
        [format!("401 for a revoked access token: {reason}")]
    );
    // The default token is the app's again.
    let (status, _) = graphql(&api, &default_token(&api), "{ shop { name } }", Value::Null);
    assert_eq!(status, 200);

    // A deleted shop.
    let created = json_of(&send(
        &api,
        "POST",
        "/__lsf/admin/shops",
        json!({"shop": "test-1"}),
    ));
    send(&api, "DELETE", "/__lsf/admin/shops/test-1", Value::Null);
    assert_eq!(
        refused(created["token"].as_str().unwrap()),
        format!(
            "this access token of test-1.myshopify.com was revoked when the shop was deleted \
             (DELETE /__lsf/admin/shops/test-1){recover}"
        )
    );

    // An uninstall.
    let created = json_of(&send(
        &api,
        "POST",
        "/__lsf/admin/shops",
        json!({"shop": "test-2"}),
    ));
    send(
        &api,
        "POST",
        "/__lsf/admin/uninstall",
        json!({"shop": "test-2"}),
    );
    assert_eq!(
        refused(created["token"].as_str().unwrap()),
        format!(
            "this access token of test-2.myshopify.com was revoked when the app was uninstalled \
             from the shop (POST /__lsf/admin/uninstall, or appUninstall){recover}"
        )
    );
    assert_eq!(said.lock().unwrap().len(), 3);

    // A token lsf never issued says so.
    assert!(
        refused("shpat_00000000000000000000000000000000")
            .starts_with("lsf did not issue this access token")
    );
    assert_eq!(said.lock().unwrap().len(), 3);
}
