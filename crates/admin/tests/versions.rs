//! Two versions of the Admin API served side by side, through `AdminApi::handle`: 2026-10 (the
//! default) and 2027-01 (the release candidate, served with `--admin-versions 2026-10,2027-01`).
//! What exists in one only is refused in the other, introspection differs accordingly, a webhook
//! subscription gets its version's header, a version Shopify no longer supports falls forward, and
//! the adapter of 2027-01 shows a record written in 2026-10 its own way.

mod support;

use lsf_admin::api::{AdminApi, AdminOptions, HttpRequest, HttpResponse, ThrottleSetting};
use lsf_admin::version::{ApiVersion, Served};
use lsf_admin::webhooks::DeliveryOptions;
use serde_json::{Value, json};

const OLD: &str = "2026-10";
const NEW: &str = "2027-01";

fn api(versions: &str) -> AdminApi {
    AdminApi::new(
        &support::demo_store(),
        AdminOptions {
            client_secret: "test-secret".to_string(),
            throttle: ThrottleSetting::Off,
            delivery: DeliveryOptions {
                retry_delays: Vec::new(),
                ..DeliveryOptions::default()
            },
            token_seed: Some(7),
            ..AdminOptions::default()
        },
    )
    .with_versions(Served::parse(versions).unwrap())
}

fn both() -> AdminApi {
    api(&format!("{OLD},{NEW}"))
}

fn post(api: &AdminApi, version: &str, query: &str, variables: Value) -> HttpResponse {
    let token = api.token(&api.default_shop());
    let body = json!({"query": query, "variables": variables}).to_string();
    api.handle(&HttpRequest {
        method: "POST".into(),
        path: format!("/admin/api/{version}/graphql.json"),
        query: Default::default(),
        headers: [
            ("content-type".to_string(), "application/json".to_string()),
            ("x-shopify-access-token".to_string(), token),
        ]
        .into(),
        body: body.into_bytes(),
    })
    .expect("the Admin API is routed")
}

fn json_of(response: &HttpResponse) -> Value {
    serde_json::from_slice(&response.body).unwrap()
}

fn header<'a>(response: &'a HttpResponse, name: &str) -> Option<&'a str> {
    response
        .headers
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// The answer of a query in a version, which must have answered in that version.
fn query(api: &AdminApi, version: &str, query: &str) -> Value {
    let response = post(api, version, query, Value::Null);
    assert_eq!(response.status, 200);
    assert_eq!(header(&response, "x-shopify-api-version"), Some(version));
    json_of(&response)
}

/// The first error of an answer that has no data: a validation error.
fn refused(answer: &Value) -> (&str, &str) {
    assert!(answer.get("data").is_none(), "{answer}");
    let error = &answer["errors"][0];
    (
        error["message"].as_str().unwrap(),
        error["extensions"]["code"].as_str().unwrap(),
    )
}

#[test]
fn a_field_or_a_mutation_of_one_version_is_refused_in_the_other_as_shopify_refuses_it() {
    let api = both();
    // `automaticDiscounts`: deprecated in 2026-10 ("This will be removed in 2027-01"), removed.
    let discounts = "{ automaticDiscounts(first: 5) { nodes { __typename } } }";
    let old = query(&api, OLD, discounts);
    assert!(old.get("errors").is_none(), "{old}");
    assert!(old["data"]["automaticDiscounts"]["nodes"].is_array());
    let new = query(&api, NEW, discounts);
    assert_eq!(
        refused(&new),
        (
            "Field 'automaticDiscounts' doesn't exist on type 'QueryRoot'",
            "undefinedField"
        )
    );

    // `InventoryItemMeasurement.packedDimensions`: new in 2027-01.
    let dimensions = "{ productVariants(first: 1) { nodes { inventoryItem { measurement {
        weight { value } packedDimensions { length width height unit } } } } } }";
    let new = query(&api, NEW, dimensions);
    assert!(new.get("errors").is_none(), "{new}");
    let measurement = &new["data"]["productVariants"]["nodes"][0]["inventoryItem"]["measurement"];
    assert_eq!(measurement["packedDimensions"]["length"], json!(0.0));
    // lsf has no dimensions to give: the default says so.
    assert!(
        new["extensions"]["lsf"]["defaultedFields"]
            .as_array()
            .unwrap()
            .contains(&json!("InventoryItemMeasurement.packedDimensions"))
    );
    assert_eq!(
        refused(&query(&api, OLD, dimensions)),
        (
            "Field 'packedDimensions' doesn't exist on type 'InventoryItemMeasurement'",
            "undefinedField"
        )
    );

    // `appDowngradeAccessScopes`: a mutation of 2027-01 only. It runs there (`write_products` is
    // a required scope of this app, which cannot be downgraded), and is unknown in 2026-10.
    let downgrade = "mutation { appDowngradeAccessScopes(scopes: [\"write_products\"]) {
        userErrors { message code } } }";
    let new = query(&api, NEW, downgrade);
    assert!(new.get("errors").is_none(), "{new}");
    assert_eq!(
        new["data"]["appDowngradeAccessScopes"]["userErrors"][0]["code"],
        json!("CANNOT_DOWNGRADE_REQUIRED_SCOPES"),
        "{new}"
    );
    assert_eq!(
        refused(&query(&api, OLD, downgrade)),
        (
            "Field 'appDowngradeAccessScopes' doesn't exist on type 'Mutation'",
            "undefinedField"
        )
    );
}

#[test]
fn introspection_answers_the_schema_of_the_version() {
    let api = both();
    let fields = "{ measurement: __type(name: \"InventoryItemMeasurement\") { fields { name } }
        box: __type(name: \"BoxDimensions\") { name }
        access: __type(name: \"MetaobjectAdminAccess\") { enumValues(includeDeprecated: true) { name } }
        root: __type(name: \"QueryRoot\") { fields(includeDeprecated: true) { name } } }";
    let names = |answer: &Value, key: &str, list: &str| -> Vec<String> {
        answer["data"][key][list]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["name"].as_str().unwrap().to_string())
            .collect()
    };
    let old = query(&api, OLD, fields);
    let new = query(&api, NEW, fields);
    assert_eq!(names(&old, "measurement", "fields"), ["id", "weight"]);
    assert_eq!(
        names(&new, "measurement", "fields"),
        ["id", "packedDimensions", "weight"]
    );
    assert_eq!(old["data"]["box"], Value::Null);
    assert_eq!(new["data"]["box"]["name"], json!("BoxDimensions"));
    assert_eq!(
        names(&old, "access", "enumValues"),
        [
            "MERCHANT_READ",
            "MERCHANT_READ_WRITE",
            "PRIVATE",
            "PUBLIC_READ",
            "PUBLIC_READ_WRITE"
        ]
    );
    assert_eq!(
        names(&new, "access", "enumValues"),
        ["MERCHANT_READ", "MERCHANT_READ_WRITE", "PUBLIC_READ_WRITE"]
    );
    assert!(names(&old, "root", "fields").contains(&"automaticDiscounts".to_string()));
    assert!(!names(&new, "root", "fields").contains(&"automaticDiscounts".to_string()));
}

#[test]
fn a_webhook_subscription_is_delivered_in_the_version_that_created_it() {
    let api = both();
    let subscribe = "mutation($uri: String!) {
        webhookSubscriptionCreate(topic: PRODUCTS_UPDATE, webhookSubscription: {uri: $uri}) {
            webhookSubscription { id apiVersion { handle supported } } userErrors { message } } }";
    // Nobody listens there: the delivery's request is in the log either way.
    for version in [OLD, NEW] {
        let answer = json_of(&post(
            &api,
            version,
            subscribe,
            json!({"uri": format!("https://example.invalid/{version}")}),
        ));
        let created = &answer["data"]["webhookSubscriptionCreate"];
        assert_eq!(created["userErrors"], json!([]), "{answer}");
        assert_eq!(
            created["webhookSubscription"]["apiVersion"],
            json!({"handle": version, "supported": true})
        );
    }
    let product =
        query(&api, OLD, "{ products(first: 1) { nodes { id } } }")["data"]["products"]["nodes"][0]
            ["id"]
            .clone();
    let updated = json_of(&post(
        &api,
        NEW,
        "mutation($id: ID!) { productUpdate(product: {id: $id, title: \"Renamed\"}) {
            userErrors { message } } }",
        json!({"id": product}),
    ));
    assert_eq!(
        updated["data"]["productUpdate"]["userErrors"],
        json!([]),
        "{updated}"
    );
    let deliveries = api.webhooks().deliveries().list(&Default::default());
    let mut versions: Vec<(String, String, String)> = deliveries
        .iter()
        .filter(|delivery| delivery.topic == "products/update")
        .map(|delivery| {
            let request = delivery.request.as_ref().expect("a request was built");
            (
                delivery.url.clone().unwrap_or_default(),
                delivery.api_version.clone(),
                request.headers["X-Shopify-API-Version"].clone(),
            )
        })
        .collect();
    versions.sort();
    assert_eq!(
        versions,
        [
            (
                format!("https://example.invalid/{OLD}"),
                OLD.to_string(),
                OLD.to_string()
            ),
            (
                format!("https://example.invalid/{NEW}"),
                NEW.to_string(),
                NEW.to_string()
            ),
        ]
    );
}

#[test]
fn a_webhook_asking_for_a_version_the_server_does_not_serve_says_what_it_got() {
    let mut app = support::fixture_app();
    app.config.webhooks_api_version = Some(NEW.to_string());
    app.config.webhook_subscriptions = vec![lsf_checkout::app::WebhookSubscriptionConfig {
        topics: vec!["products/update".into()],
        uri: "https://example.invalid/toml".into(),
        ..Default::default()
    }];
    let api = AdminApi::new(
        &support::demo_store(),
        AdminOptions {
            app: Some(app),
            client_secret: "test-secret".to_string(),
            throttle: ThrottleSetting::Off,
            delivery: DeliveryOptions {
                retry_delays: Vec::new(),
                ..DeliveryOptions::default()
            },
            token_seed: Some(7),
            ..AdminOptions::default()
        },
    );
    let product =
        query(&api, OLD, "{ products(first: 1) { nodes { id } } }")["data"]["products"]["nodes"][0]
            ["id"]
            .clone();
    post(
        &api,
        OLD,
        "mutation($id: ID!) { productUpdate(product: {id: $id, title: \"Renamed\"}) { userErrors { message } } }",
        json!({"id": product}),
    );
    let deliveries = api.webhooks().deliveries().list(&Default::default());
    let delivery = deliveries
        .iter()
        .find(|delivery| delivery.topic == "products/update")
        .unwrap();
    assert_eq!(delivery.api_version, OLD);
    assert!(
        delivery
            .notes
            .iter()
            .any(|note| note.contains("--admin-versions 2026-10,2027-01")),
        "{:?}",
        delivery.notes
    );
}

#[test]
fn a_version_shopify_no_longer_supports_falls_forward_to_the_oldest_stable_one_served() {
    // shopify.dev ("API versioning"): "If your app targets an inaccessible version, Shopify falls
    // forward and responds using the oldest accessible stable version", and
    // `X-Shopify-API-Version` says which.
    let api = both();
    let response = post(
        &api,
        "2025-07",
        "{ automaticDiscounts(first: 1) { nodes { __typename } } }",
        Value::Null,
    );
    assert_eq!(response.status, 200);
    // 2026-10, the stable one, not 2027-01, the release candidate: `automaticDiscounts` exists.
    assert_eq!(header(&response, "x-shopify-api-version"), Some(OLD));
    assert!(json_of(&response).get("errors").is_none());
    assert!(
        header(&response, "x-lsf-reason")
            .unwrap()
            .contains("no longer supported")
    );
}

#[test]
fn a_version_lsf_knows_but_does_not_serve_says_how_to_serve_it() {
    let api = api(OLD);
    let response = post(&api, NEW, "{ shop { name } }", Value::Null);
    assert_eq!(response.status, 404);
    assert_eq!(header(&response, "x-shopify-api-version"), Some(OLD));
    assert_eq!(
        json_of(&response),
        json!({"errors": "lsf knows API version 2027-01 (release candidate) but this server serves 2026-10 only: start it with `--admin-versions 2026-10,2027-01`"})
    );
    let status = json_of(
        &api.handle(&HttpRequest {
            method: "GET".into(),
            path: "/__lsf/admin".into(),
            query: Default::default(),
            headers: Default::default(),
            body: Vec::new(),
        })
        .unwrap(),
    );
    assert_eq!(status["apiVersion"], json!(OLD));
    assert_eq!(
        status["apiVersions"],
        json!([
            {"version": OLD, "status": "stable", "served": true, "graphql": "/admin/api/2026-10/graphql.json"},
            {"version": NEW, "status": "release candidate", "served": false, "graphql": "/admin/api/2027-01/graphql.json"},
        ])
    );
}

#[test]
fn a_record_written_in_2026_10_reads_through_the_adapter_of_2027_01() {
    // A definition made before 2027-01 keeps `PUBLIC_READ`, which 2027-01 removed ("Use
    // MERCHANT_READ instead"): 2026-10 reads the record as it is, 2027-01 through its adapter.
    let api = both();
    let created = json_of(&post(
        &api,
        OLD,
        "mutation { metaobjectDefinitionCreate(definition: {type: \"lookbook\",
            fieldDefinitions: [{key: \"title\", type: \"single_line_text_field\"}]}) {
            metaobjectDefinition { id } userErrors { message } } }",
        Value::Null,
    ));
    let id = created["data"]["metaobjectDefinitionCreate"]["metaobjectDefinition"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{created}"))
        .to_string();
    api.change_shop(&api.default_shop(), |state| {
        let record = state.get_mut(&id).unwrap();
        record["access"]["admin"] = json!("PUBLIC_READ");
    });
    let read =
        format!("{{ metaobjectDefinition(id: \"{id}\") {{ access {{ admin storefront }} }} }}");
    let old = query(&api, OLD, &read);
    assert_eq!(
        old["data"]["metaobjectDefinition"]["access"]["admin"],
        json!("PUBLIC_READ"),
        "{old}"
    );
    let new = query(&api, NEW, &read);
    assert!(new.get("errors").is_none(), "{new}");
    assert_eq!(
        new["data"]["metaobjectDefinition"]["access"]["admin"],
        json!("MERCHANT_READ")
    );
    // The value the record holds is not one of 2027-01's enum: without the adapter, the executor
    // would refuse it.
    let schema = ApiVersion::get(NEW).unwrap().schema();
    assert!(
        !schema
            .enum_values("MetaobjectAdminAccess")
            .unwrap()
            .any(|value| value.name == "PUBLIC_READ")
    );
    // A value 2027-01 has passes through.
    assert_eq!(
        new["data"]["metaobjectDefinition"]["access"]["storefront"],
        old["data"]["metaobjectDefinition"]["access"]["storefront"]
    );
}
