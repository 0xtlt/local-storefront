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

/// The scenario of a delegate token that became a full one: exchanged as an offline token for an
/// expiring one, it came back `shpat_` with a refresh token, made delegates, and carried every
/// scope of the installation after an install.
#[test]
fn a_delegate_token_cannot_be_exchanged_for_a_token_of_the_installation() {
    let api = api();
    let token = api.token(&api.default_shop());
    let (_, created) = graphql(
        &api,
        &token,
        "mutation { delegateAccessTokenCreate(input: {delegateAccessScope: [\"read_products\"]}) { delegateAccessToken { accessToken } userErrors { message } } }",
    );
    let delegate =
        created["data"]["delegateAccessTokenCreate"]["delegateAccessToken"]["accessToken"]
            .as_str()
            .unwrap_or_else(|| panic!("{created}"))
            .to_string();
    let exchange = send(
        &api,
        "POST",
        "/admin/oauth/access_token",
        None,
        json!({
            "client_id": "22222222222222222222222222222222",
            "client_secret": "test-secret",
            "grant_type": "urn:ietf:params:oauth:grant-type:token-exchange",
            "subject_token": delegate,
            "subject_token_type": "urn:shopify:params:oauth:token-type:offline-access-token",
            "requested_token_type": "urn:shopify:params:oauth:token-type:offline-access-token",
            "expiring": "1",
        }),
    );
    let body: Value = serde_json::from_slice(&exchange.body).unwrap();
    assert_eq!(exchange.status, 400, "{body}");
    assert_eq!(body["error"], "invalid_subject_token");
    assert!(body.get("access_token").is_none());
    // The delegate is still a delegate: it makes no delegate, and an install leaves its scopes.
    let installed = send(&api, "POST", "/__lsf/admin/install", None, json!({}));
    assert_eq!(installed.status, 200);
    let (_, nested) = graphql(
        &api,
        &delegate,
        "mutation { delegateAccessTokenCreate(input: {delegateAccessScope: [\"read_products\"]}) { delegateAccessToken { accessToken } userErrors { code } } }",
    );
    assert_eq!(
        nested["data"]["delegateAccessTokenCreate"]["userErrors"][0]["code"],
        "DELEGATE_ACCESS_TOKEN",
        "{nested}"
    );
    let (_, denied) = graphql(
        &api,
        &delegate,
        "mutation { productCreate(product: {title: \"No\"}) { product { id } } }",
    );
    assert_eq!(
        denied["errors"][0]["extensions"]["code"], "ACCESS_DENIED",
        "{denied}"
    );
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
    let outbox = send(
        &api,
        "GET",
        "/__lsf/admin/outbox?channel=flow",
        None,
        Value::Null,
    );
    let outbox: Value = serde_json::from_slice(&outbox.body).unwrap();
    assert_eq!(outbox["count"], 1, "{outbox}");
    assert_eq!(outbox["outbox"][0]["to"], "auction-bid-placed", "{outbox}");
    assert_eq!(outbox["outbox"][0]["data"]["Amount"], "30", "{outbox}");
    // Forgotten on `DELETE`.
    let cleared = send(&api, "DELETE", "/__lsf/admin/outbox", None, Value::Null);
    let cleared: Value = serde_json::from_slice(&cleared.body).unwrap();
    assert_eq!(cleared["outbox"], 1, "{cleared}");
    let outbox = send(&api, "GET", "/__lsf/admin/outbox", None, Value::Null);
    let outbox: Value = serde_json::from_slice(&outbox.body).unwrap();
    assert_eq!(outbox["outbox"], json!([]), "{outbox}");
    // A channel lsf does not have is refused, with the ones it has.
    let unknown = send(
        &api,
        "GET",
        "/__lsf/admin/outbox?channel=fax",
        None,
        Value::Null,
    );
    assert_eq!(unknown.status, 422);
    assert!(String::from_utf8_lossy(&unknown.body).contains("data_erasure"));
}

#[test]
fn eventbridge_and_pubsub_subscriptions_are_made_and_their_deliveries_logged_not_sent() {
    // Without an app, every scope: `productCreate` may run.
    let api = AdminApi::new(
        &support::demo_store(),
        AdminOptions {
            app: None,
            client_secret: "test-secret".to_string(),
            app_url: None,
            throttle: ThrottleSetting::Off,
            delivery: DeliveryOptions::default(),
            token_seed: Some(12),
        },
    );
    let token = api.token(&api.default_shop());
    let arn =
        "arn:aws:events:us-east-1::event-source/aws.partner/shopify.com/1830279/test-event-source";
    let (_, bridge) = graphql(
        &api,
        &token,
        &format!(
            "mutation {{ eventBridgeWebhookSubscriptionCreate(topic: PRODUCTS_CREATE, webhookSubscription: {{arn: \"{arn}\", format: JSON}}) {{ webhookSubscription {{ id topic format endpoint {{ __typename ... on WebhookEventBridgeEndpoint {{ arn }} }} }} userErrors {{ field message }} }} }}"
        ),
    );
    let subscription =
        &bridge["data"]["eventBridgeWebhookSubscriptionCreate"]["webhookSubscription"];
    assert_eq!(
        subscription["endpoint"],
        json!({"__typename": "WebhookEventBridgeEndpoint", "arn": arn}),
        "{bridge}"
    );
    let bridge_id = subscription["id"].as_str().unwrap().to_string();
    let (_, pubsub) = graphql(
        &api,
        &token,
        "mutation { pubSubWebhookSubscriptionCreate(topic: PRODUCTS_CREATE, webhookSubscription: {pubSubProject: \"my-gcp-project\", pubSubTopic: \"my-gcp-topic\", format: JSON}) { webhookSubscription { id endpoint { __typename ... on WebhookPubSubEndpoint { pubSubProject pubSubTopic } } } userErrors { field message code } } }",
    );
    let pubsub_subscription =
        &pubsub["data"]["pubSubWebhookSubscriptionCreate"]["webhookSubscription"];
    assert_eq!(
        pubsub_subscription["endpoint"],
        json!({"__typename": "WebhookPubSubEndpoint", "pubSubProject": "my-gcp-project", "pubSubTopic": "my-gcp-topic"}),
        "{pubsub}"
    );
    let pubsub_id = pubsub_subscription["id"].as_str().unwrap().to_string();

    // The same address twice is taken; a blank project is refused; nothing more is made.
    let (_, taken) = graphql(
        &api,
        &token,
        "mutation { pubSubWebhookSubscriptionCreate(topic: PRODUCTS_CREATE, webhookSubscription: {pubSubProject: \"my-gcp-project\", pubSubTopic: \"my-gcp-topic\"}) { webhookSubscription { id } userErrors { field code } } }",
    );
    assert_eq!(
        taken["data"]["pubSubWebhookSubscriptionCreate"]["userErrors"][0]["code"], "TAKEN",
        "{taken}"
    );
    let (_, blank) = graphql(
        &api,
        &token,
        "mutation { pubSubWebhookSubscriptionCreate(topic: ORDERS_CREATE, webhookSubscription: {pubSubProject: \" \", pubSubTopic: \"t\"}) { webhookSubscription { id } userErrors { field code } } }",
    );
    assert_eq!(
        blank["data"]["pubSubWebhookSubscriptionCreate"]["userErrors"][0],
        json!({"field": ["webhookSubscription", "pubSubProject"], "code": "INVALID_PARAMETERS"}),
        "{blank}"
    );
    let shop = api.default_shop();
    assert_eq!(api.webhooks().registry().list(&shop).len(), 2);

    let (_, updated) = graphql(
        &api,
        &token,
        &format!(
            "mutation {{ pubSubWebhookSubscriptionUpdate(id: \"{pubsub_id}\", webhookSubscription: {{pubSubProject: \"other-project\", pubSubTopic: \"other-topic\"}}) {{ webhookSubscription {{ endpoint {{ ... on WebhookPubSubEndpoint {{ pubSubProject }} }} }} userErrors {{ code }} }} }}"
        ),
    );
    assert_eq!(
        updated["data"]["pubSubWebhookSubscriptionUpdate"]["webhookSubscription"]["endpoint"]["pubSubProject"],
        "other-project",
        "{updated}"
    );
    let (_, bridged) = graphql(
        &api,
        &token,
        &format!(
            "mutation {{ eventBridgeWebhookSubscriptionUpdate(id: \"{bridge_id}\", webhookSubscription: {{includeFields: [\"id\"]}}) {{ webhookSubscription {{ includeFields endpoint {{ ... on WebhookEventBridgeEndpoint {{ arn }} }} }} userErrors {{ message }} }} }}"
        ),
    );
    assert_eq!(
        bridged["data"]["eventBridgeWebhookSubscriptionUpdate"]["webhookSubscription"]["endpoint"]
            ["arn"],
        arn,
        "the ARN left out stays: {bridged}"
    );

    // A product created: both deliveries are logged, not sent.
    let (_, product) = graphql(
        &api,
        &token,
        "mutation { productCreate(product: {title: \"Delivered nowhere\"}) { product { id } userErrors { message } } }",
    );
    assert!(
        product["data"]["productCreate"]["product"]["id"].is_string(),
        "{product}"
    );
    let log = send(
        &api,
        "GET",
        "/__lsf/webhooks?topic=products/create&wait=2&timeout=3000",
        None,
        Value::Null,
    );
    let log: Value = serde_json::from_slice(&log.body).unwrap();
    let deliveries = log["deliveries"]
        .as_array()
        .unwrap_or_else(|| panic!("{log}"));
    assert_eq!(deliveries.len(), 2, "{log}");
    assert!(
        deliveries
            .iter()
            .all(|delivery| delivery["state"] == "not_sent"),
        "{log}"
    );
}

#[test]
fn the_status_lists_every_mutation_of_the_platform() {
    let api = api();
    let status = send(&api, "GET", "/__lsf/admin", None, Value::Null);
    let status: Value = serde_json::from_slice(&status.body).unwrap();
    let listed: Vec<&str> = status["mutations"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for mutation in [
        "appDowngradeAccessScopes",
        "appRevokeAccessScopes",
        "appUninstall",
        "bulkProductResourceFeedbackCreate",
        "checkoutAndAccountsConfigurationUpdate",
        "checkoutBrandingUpsert",
        "delegateAccessTokenCreate",
        "delegateAccessTokenDestroy",
        "discountBulkTagsAdd",
        "discountBulkTagsRemove",
        "eventBridgeServerPixelUpdate",
        "eventBridgeWebhookSubscriptionCreate",
        "eventBridgeWebhookSubscriptionUpdate",
        "fileAcknowledgeUpdateFailed",
        "flowTriggerReceive",
        "fulfillmentConstraintRuleCreate",
        "fulfillmentConstraintRuleDelete",
        "fulfillmentConstraintRuleUpdate",
        "mobilePlatformApplicationCreate",
        "mobilePlatformApplicationDelete",
        "mobilePlatformApplicationUpdate",
        "pubSubServerPixelUpdate",
        "pubSubWebhookSubscriptionCreate",
        "pubSubWebhookSubscriptionUpdate",
        "savedSearchCreate",
        "savedSearchDelete",
        "savedSearchUpdate",
        "scriptTagCreate",
        "scriptTagDelete",
        "scriptTagUpdate",
        "serverPixelCreate",
        "serverPixelDelete",
        "shopPolicyUpdate",
        "shopResourceFeedbackCreate",
        "stagedUploadTargetGenerate",
        "stagedUploadTargetsGenerate",
        "standardMetafieldDefinitionEnable",
        "standardMetaobjectDefinitionEnable",
        "storefrontAccessTokenCreate",
        "storefrontAccessTokenDelete",
        "themeCreate",
        "themeDelete",
        "themeDuplicate",
        "themeFilesCopy",
        "themeFilesDelete",
        "themeFilesUpsert",
        "themePublish",
        "themeUpdate",
        "urlRedirectBulkDeleteAll",
        "urlRedirectBulkDeleteByIds",
        "urlRedirectBulkDeleteBySavedSearch",
        "urlRedirectBulkDeleteBySearch",
        "urlRedirectImportCreate",
        "urlRedirectImportSubmit",
        "webPixelCreate",
        "webPixelDelete",
        "webPixelUpdate",
    ] {
        assert!(listed.contains(&mutation), "{mutation} is not simulated");
    }
}
