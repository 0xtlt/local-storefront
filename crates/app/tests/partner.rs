//! The Partner API on lsf (`lsf_app::admin::partner`): what an app on Shopify App Pricing reads of
//! its plan (`activeSubscription`) and of its history (`events`, `app { events }`), from the
//! plans chosen, changed and ended, the trials, the installs and the uninstalls, through
//! `AdminApi::handle` as a backend reaches it.

mod support;

use std::collections::HashMap;
use std::time::Duration;

use lsf_app::admin::api::{AdminApi, AdminOptions, HttpRequest, HttpResponse, ThrottleSetting};
use lsf_app::admin::partner;
use lsf_app::admin::webhooks::DeliveryOptions;
use serde_json::{Value, json};
use support::json_of;

fn api() -> AdminApi {
    AdminApi::new(
        &support::demo_store(),
        AdminOptions {
            app: Some(support::fixture_app()),
            client_secret: "test-secret".to_string(),
            app_url: Some("http://localhost:3000".to_string()),
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

fn send(
    api: &AdminApi,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> HttpResponse {
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let mut all: HashMap<String, String> = headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.to_string()))
        .collect();
    all.insert("host".into(), "127.0.0.1:9292".into());
    api.handle(&HttpRequest {
        method: method.to_string(),
        path: path.to_string(),
        query: form_urlencoded::parse(query.as_bytes())
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect(),
        headers: all,
        body: body.to_vec(),
    })
    .unwrap_or_else(|| panic!("{path} is not routed"))
}

/// A control endpoint, its JSON answer.
fn control(api: &AdminApi, method: &str, path: &str, body: Value) -> Value {
    let body = if body.is_null() {
        Vec::new()
    } else {
        body.to_string().into_bytes()
    };
    let response = send(
        api,
        method,
        path,
        &[("content-type", "application/json")],
        &body,
    );
    assert!(
        response.status < 300,
        "{path}: {}",
        String::from_utf8_lossy(&response.body)
    );
    json_of(&response)
}

/// What a test reads in `GET /__lsf/admin/partner`: the path of the endpoint, the token, the
/// app's Shopify GID and the default shop's.
struct Org {
    path: String,
    token: String,
    app: String,
    shop: String,
}

fn org(api: &AdminApi) -> Org {
    let answer = control(api, "GET", "/__lsf/admin/partner", Value::Null);
    let url = answer["url"].as_str().unwrap();
    Org {
        path: url
            .strip_prefix("http://127.0.0.1:9292")
            .unwrap()
            .to_string(),
        token: answer["token"].as_str().unwrap().to_string(),
        app: answer["app"]["id"].as_str().unwrap().to_string(),
        shop: answer["shops"][0]["id"].as_str().unwrap().to_string(),
    }
}

/// A request of the Partner API, as a backend sends it: its status and its JSON.
fn partner_request(api: &AdminApi, query: &str, variables: Value) -> (u16, Value) {
    let org = org(api);
    let body = json!({"query": query, "variables": variables}).to_string();
    let response = send(
        api,
        "POST",
        &org.path,
        &[
            ("content-type", "application/json"),
            ("x-shopify-access-token", &org.token),
        ],
        body.as_bytes(),
    );
    (response.status, json_of(&response))
}

fn partner_api(api: &AdminApi, query: &str, variables: Value) -> Value {
    let (status, answer) = partner_request(api, query, variables);
    assert_eq!(status, 200, "{answer}");
    answer
}

const ACTIVE: &str = r#"query($appId: ID!, $shopId: ID!) { activeSubscription(appId: $appId, shopId: $shopId) {
  app { id name apiKey } shop { id myshopifyDomain } billingPeriod cancelAtEndOfCycle trialEndsAt
  currentBillingCycle { startTime endTime }
  items { handle description price { __typename active currency ... on FlatRatePrice { amount } } discount { amount } usage { quantity } }
  pendingUpdate { billingPeriod } legacySubscriptionId } }"#;

/// `activeSubscription` of the default shop.
fn active(api: &AdminApi) -> Value {
    let org = org(api);
    let answer = partner_api(api, ACTIVE, json!({"appId": org.app, "shopId": org.shop}));
    assert!(answer.get("errors").is_none(), "{answer}");
    answer["data"]["activeSubscription"].clone()
}

const EVENTS: &str = r#"query($filter: EventFilterInput, $order: EventOrder, $first: Int, $after: String) {
  events(filter: $filter, orderBy: $order, first: $first, after: $after) {
    edges { cursor node { id occurredAt eventType shop { id myshopifyDomain } subject { ... on AppReference { id apiKey } }
      ... on Relationship { relationship: state }
      ... on SubscriptionStatus { state cancelEffectiveOn plan { handle billingPeriod trialDays trialDaysRemaining
        prices { __typename currencyCode ... on FlatRatePlanPrice { amount } } } } } }
    pageInfo { hasNextPage hasPreviousPage startCursor endCursor } } }"#;

/// The `eventType` of each event `events` lists with `variables`.
fn events(api: &AdminApi, variables: Value) -> Vec<String> {
    let answer = partner_api(api, EVENTS, variables);
    assert!(answer.get("errors").is_none(), "{answer}");
    answer["data"]["events"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| edge["node"]["eventType"].as_str().unwrap().to_string())
        .collect()
}

const APP_EVENTS: &str = r#"query($id: ID!, $types: [AppEventTypes!], $shopId: ID) { app(id: $id) { id apiKey
  events(first: 50, types: $types, shopId: $shopId) { edges { node { __typename type occurredAt shop { id myshopifyDomain }
    ... on AppSubscriptionEvent { charge { id name test billingOn amount { amount currencyCode } } } } }
    pageInfo { hasNextPage hasPreviousPage } } } }"#;

/// `(type, charge name)` of each event `App.events` lists, newest first.
fn app_events(api: &AdminApi, types: Value) -> Vec<(String, Option<String>)> {
    let app = org(api).app.replace("gid://shopify/", "gid://partners/");
    let answer = partner_api(api, APP_EVENTS, json!({"id": app, "types": types}));
    assert!(answer.get("errors").is_none(), "{answer}");
    answer["data"]["app"]["events"]["edges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|edge| {
            (
                edge["node"]["type"].as_str().unwrap().to_string(),
                edge["node"]["charge"]["name"].as_str().map(str::to_string),
            )
        })
        .collect()
}

fn plans(legacy: bool) -> Value {
    json!({"legacy": legacy, "plans": [
        {"handle": "free", "name": "Free", "price": 0},
        {"handle": "pro", "name": "Pro", "price": "4.99", "trial_days": 7},
        {"handle": "pro-annual", "name": "Pro (annual)", "price": 49, "interval": "annual"},
    ]})
}

fn select(api: &AdminApi, plan: &str) -> Value {
    control(
        api,
        "POST",
        &format!("/__lsf/admin/pricing-plans/{plan}/select"),
        json!({}),
    )
}

/// The end-to-end case: a plan chosen, read through the Partner API, ended, read again.
#[test]
fn a_plan_chosen_then_ended_is_read_from_the_partner_api() {
    let api = api();
    control(&api, "PUT", "/__lsf/admin/pricing-plans", plans(true));
    assert_eq!(active(&api), Value::Null, "no plan yet");

    let selected = select(&api, "pro");
    let subscription = selected["subscription"]["id"].as_str().unwrap();
    let now = api.with_default_shop(|state| state.now()).unwrap();
    let trial_ends =
        (now + chrono::Duration::days(7)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let org = org(&api);
    assert_eq!(
        active(&api),
        json!({
            "app": {"id": org.app, "name": "Admin fixtures", "apiKey": api.client_id()},
            "shop": {"id": org.shop, "myshopifyDomain": api.default_shop()},
            "billingPeriod": "EVERY_30_DAYS",
            "cancelAtEndOfCycle": false,
            "trialEndsAt": trial_ends,
            "currentBillingCycle": null,
            "items": [{
                "handle": "pro",
                "description": "Pro",
                "price": {"__typename": "FlatRatePrice", "active": true, "currency": "USD", "amount": "4.99"},
                "discount": null,
                "usage": null,
            }],
            "pendingUpdate": null,
            "legacySubscriptionId": subscription,
        })
    );

    let ended = control(&api, "POST", "/__lsf/admin/pricing-plans/cancel", json!({}));
    assert_eq!(ended["subscription"]["status"], "CANCELLED");
    assert_eq!(active(&api), Value::Null, "the plan ended");

    let answer = partner_api(
        &api,
        EVENTS,
        json!({"filter": {"subjectId": org.app, "shopId": org.shop}}),
    );
    let edges = answer["data"]["events"]["edges"].as_array().unwrap();
    assert_eq!(edges.len(), 2, "{answer}");
    let today = now.format("%Y-%m-%d").to_string();
    let canceled = &edges[0]["node"];
    assert_eq!(
        canceled["eventType"], "SUBSCRIPTION_CANCELED",
        "newest first"
    );
    assert_eq!(canceled["state"], "CANCELED");
    assert_eq!(canceled["cancelEffectiveOn"], json!(today));
    assert_eq!(
        canceled["shop"],
        json!({"id": org.shop, "myshopifyDomain": api.default_shop()})
    );
    assert_eq!(
        canceled["subject"],
        json!({"id": org.app, "apiKey": api.client_id()})
    );
    let created = &edges[1]["node"];
    assert_eq!(created["eventType"], "SUBSCRIPTION_CREATED");
    assert_eq!(created["state"], "CREATED");
    assert_eq!(created["cancelEffectiveOn"], Value::Null);
    assert_eq!(
        created["plan"],
        json!({"handle": "pro", "billingPeriod": "EVERY_30_DAYS", "trialDays": 7, "trialDaysRemaining": 7,
            "prices": [{"__typename": "FlatRatePlanPrice", "currencyCode": "USD", "amount": "4.99"}]})
    );
    assert_ne!(canceled["id"], created["id"]);
}

#[test]
fn without_the_legacy_features_the_subscription_has_no_legacy_id() {
    let api = api();
    control(&api, "PUT", "/__lsf/admin/pricing-plans", plans(false));
    select(&api, "pro-annual");
    let active = active(&api);
    assert_eq!(active["legacySubscriptionId"], Value::Null);
    assert_eq!(active["billingPeriod"], "ANNUAL");
    assert_eq!(active["trialEndsAt"], Value::Null);
    let now = api.with_default_shop(|state| state.now()).unwrap();
    // The cycle starts when the plan was picked: a moment before `now`, maybe a second earlier.
    let time = |key: &str| {
        chrono::DateTime::parse_from_rfc3339(active["currentBillingCycle"][key].as_str().unwrap())
            .unwrap()
            .with_timezone(&chrono::Utc)
    };
    let start = time("startTime");
    assert!(
        (now - start).num_seconds().abs() <= 2,
        "{start} is not {now}"
    );
    assert_eq!(time("endTime"), start + chrono::Duration::days(365));
    assert_eq!(active["items"][0]["price"]["amount"], "49.00");
    assert_eq!(events(&api, json!({})), ["SUBSCRIPTION_CREATED"]);
}

/// A change of plan is one `SUBSCRIPTION_UPDATED`; `App.events` has the end of the old charge
/// and the start of the new one.
#[test]
fn a_change_of_plan_is_one_update_and_two_charges() {
    let api = api();
    control(&api, "PUT", "/__lsf/admin/pricing-plans", plans(true));
    select(&api, "pro");
    select(&api, "free");
    assert_eq!(active(&api)["items"][0]["handle"], "free");
    assert_eq!(
        events(&api, json!({})),
        ["SUBSCRIPTION_UPDATED", "SUBSCRIPTION_CREATED"]
    );
    let answer = partner_api(
        &api,
        EVENTS,
        json!({"filter": {"eventTypes": ["SUBSCRIPTION_UPDATED"]}}),
    );
    let updated = &answer["data"]["events"]["edges"][0]["node"];
    assert_eq!(updated["state"], "UPDATED");
    assert_eq!(updated["plan"]["handle"], "free");
    assert_eq!(updated["plan"]["prices"][0]["amount"], "0.00");
    let charge = |name: &str| Some(name.to_string());
    assert_eq!(
        app_events(&api, Value::Null),
        [
            ("SUBSCRIPTION_CHARGE_CANCELED".to_string(), charge("Pro")),
            ("SUBSCRIPTION_CHARGE_ACTIVATED".to_string(), charge("Free")),
            ("SUBSCRIPTION_CHARGE_ACCEPTED".to_string(), charge("Free")),
            ("SUBSCRIPTION_CHARGE_ACTIVATED".to_string(), charge("Pro")),
            ("SUBSCRIPTION_CHARGE_ACCEPTED".to_string(), charge("Pro")),
            ("RELATIONSHIP_INSTALLED".to_string(), None),
        ]
    );
    // The charge as the event saw it: the test subscription of a development store, its amount,
    // its next billing date (none once cancelled).
    let app = org(&api).app.replace("gid://shopify/", "gid://partners/");
    let answer = partner_api(
        &api,
        APP_EVENTS,
        json!({"id": app, "types": ["SUBSCRIPTION_CHARGE_ACTIVATED"]}),
    );
    let pro = &answer["data"]["app"]["events"]["edges"][1]["node"];
    assert_eq!(pro["__typename"], "SubscriptionChargeActivated");
    assert_eq!(pro["charge"]["test"], true);
    assert_eq!(
        pro["charge"]["amount"],
        json!({"amount": "4.99", "currencyCode": "USD"})
    );
    assert!(pro["charge"]["billingOn"].is_string(), "{pro}");
    assert_eq!(
        pro["shop"]["id"],
        json!(org(&api).shop.replace("gid://shopify/", "gid://partners/"))
    );
}

/// The trial runs on the shop's clock: `trialEndsAt` then `null`, the billing cycle `null` then
/// the one the clock is in.
#[test]
fn the_trial_ends_with_the_shops_clock() {
    let api = api();
    control(&api, "PUT", "/__lsf/admin/pricing-plans", plans(true));
    let start = api.with_default_shop(|state| state.now()).unwrap();
    select(&api, "pro");
    let date =
        |at: chrono::DateTime<chrono::Utc>| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let trial_ends = start + chrono::Duration::days(7);
    assert_eq!(active(&api)["trialEndsAt"], json!(date(trial_ends)));
    assert_eq!(active(&api)["currentBillingCycle"], Value::Null);

    control(
        &api,
        "POST",
        "/__lsf/admin/clock",
        json!({"now": date(start + chrono::Duration::days(8))}),
    );
    let after = active(&api);
    assert_eq!(after["trialEndsAt"], Value::Null);
    assert_eq!(
        after["currentBillingCycle"],
        json!({"startTime": date(trial_ends), "endTime": date(trial_ends + chrono::Duration::days(30))})
    );
    // Two cycles on.
    control(
        &api,
        "POST",
        "/__lsf/admin/clock",
        json!({"now": date(trial_ends + chrono::Duration::days(65))}),
    );
    assert_eq!(
        active(&api)["currentBillingCycle"],
        json!({"startTime": date(trial_ends + chrono::Duration::days(60)), "endTime": date(trial_ends + chrono::Duration::days(90))})
    );
}

/// An uninstall ends the plan and the relationship; a reinstall starts it again. Both event
/// systems show it, in both modes of managed pricing.
#[test]
fn uninstalls_and_installs_are_in_the_history() {
    for legacy in [true, false] {
        let api = api();
        control(&api, "PUT", "/__lsf/admin/pricing-plans", plans(legacy));
        select(&api, "pro");
        control(&api, "POST", "/__lsf/admin/uninstall", json!({}));
        assert_eq!(active(&api), Value::Null, "legacy {legacy}");
        control(&api, "POST", "/__lsf/admin/install", json!({}));
        assert_eq!(
            events(&api, json!({})),
            [
                "RELATIONSHIP_INSTALLED",
                "RELATIONSHIP_UNINSTALLED",
                "SUBSCRIPTION_CANCELED",
                "SUBSCRIPTION_CREATED"
            ],
            "legacy {legacy}"
        );
        let answer = partner_api(
            &api,
            EVENTS,
            json!({"filter": {"eventTypes": ["RELATIONSHIP_UNINSTALLED"]}}),
        );
        let node = &answer["data"]["events"]["edges"][0]["node"];
        assert_eq!(node["relationship"], "UNINSTALLED", "{answer}");
        assert_eq!(
            app_events(
                &api,
                json!(["RELATIONSHIP_INSTALLED", "RELATIONSHIP_UNINSTALLED"])
            ),
            [
                ("RELATIONSHIP_INSTALLED".to_string(), None),
                ("RELATIONSHIP_UNINSTALLED".to_string(), None),
                ("RELATIONSHIP_INSTALLED".to_string(), None),
            ]
        );
        // The install the shop starts with is dated as the seed's records, outside the default
        // window of 30 days: an explicit one finds it.
        assert_eq!(
            events(
                &api,
                json!({"filter": {"occurredAtMin": "2024-01-01T00:00:00Z", "occurredAtMax": "2024-01-31T00:00:00Z"}})
            ),
            ["RELATIONSHIP_INSTALLED"]
        );
    }
}

/// A subscription of the Billing API is not a contract of managed pricing: `activeSubscription`
/// is `null` and `events` has no subscription event; `App.events` reports its charge, as it did
/// before managed pricing.
#[test]
fn a_billing_api_subscription_is_in_app_events_alone() {
    let api = api();
    let token = api.token(&api.default_shop());
    let created = api
        .handle(&support::graphql_request(
            &token,
            r#"mutation { appSubscriptionCreate(name: "Basic", returnUrl: "http://localhost:3000/", test: true,
              lineItems: [{plan: {appRecurringPricingDetails: {price: {amount: 10, currencyCode: USD}}}}]) {
              appSubscription { id } userErrors { message } } }"#,
            Value::Null,
        ))
        .unwrap();
    let created = json_of(&created);
    let id = created["data"]["appSubscriptionCreate"]["appSubscription"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{created}"))
        .to_string();
    let number = id.rsplit('/').next().unwrap();
    control(
        &api,
        "POST",
        &format!("/__lsf/admin/charges/{number}/approve"),
        json!({}),
    );
    assert_eq!(active(&api), Value::Null);
    assert_eq!(events(&api, json!({})), Vec::<String>::new());
    assert_eq!(
        app_events(&api, json!(["SUBSCRIPTION_CHARGE_ACTIVATED"])),
        [(
            "SUBSCRIPTION_CHARGE_ACTIVATED".to_string(),
            Some("Basic".to_string())
        )]
    );
}

/// Shopify's answers to what the Partner API refuses, and lsf's for what it does not simulate.
#[test]
fn refuses_as_shopify_does() {
    let api = api();
    let org = org(&api);
    let post = |path: &str, token: &str, body: &str| {
        send(
            &api,
            "POST",
            path,
            &[
                ("content-type", "application/json"),
                ("x-shopify-access-token", token),
            ],
            body.as_bytes(),
        )
    };
    let query = r#"{"query": "{ app(id: \"gid://partners/App/1\") { id } }"}"#;
    let invalid =
        json!({"errors": [{"message": "Invalid access token", "extensions": {"code": "401"}}]});
    // Another token of the format, made here: secret scanners stop a literal one.
    let other = format!("prtapi_{}", "0".repeat(32));
    for (path, token) in [
        (org.path.as_str(), ""),
        (org.path.as_str(), other.as_str()),
        ("/1/api/2026-10/graphql.json", org.token.as_str()),
    ] {
        let response = post(path, token, query);
        assert_eq!(response.status, 401, "{path} {token}");
        assert_eq!(json_of(&response), invalid);
        assert!(
            response
                .headers
                .iter()
                .any(|(name, _)| name == "x-lsf-reason")
        );
    }
    let version = post(&org.path.replace("2026-10", "nope"), &org.token, query);
    assert_eq!(version.status, 404);
    assert_eq!(
        json_of(&version),
        json!({"errors": [{"message": "Invalid API version", "extensions": {"code": "404"}}]})
    );
    let older = post(&org.path.replace("2026-10", "2026-04"), &org.token, query);
    assert_eq!(older.status, 404);
    assert!(
        json_of(&older)["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("2026-07, 2026-10 and 2027-01")
    );
    for version in partner::VERSIONS {
        let served = post(&org.path.replace("2026-10", version), &org.token, query);
        assert_eq!(served.status, 200, "{version}");
    }
    let long = json!({"query": format!("{{ app(id: \"x\") {{ id }} }} #{}", "x".repeat(50_000))});
    let too_long = post(&org.path, &org.token, &long.to_string());
    assert_eq!(too_long.status, 400);
    assert_eq!(
        json_of(&too_long),
        json!({"errors": [{"message": "Maximum query length is 50000 characters", "extensions": {"code": "400"}}]})
    );

    let error = |query: &str, variables: Value| -> Value {
        let answer = partner_api(&api, query, variables);
        answer["errors"][0].clone()
    };
    let not_simulated = error(
        "{ transactions(first: 1) { edges { node { id } } } }",
        Value::Null,
    );
    assert_eq!(not_simulated["extensions"]["code"], "LSF_NOT_SIMULATED");
    assert_eq!(not_simulated["path"], json!(["transactions"]));
    let mutation = error(
        r#"mutation { appSubscriptionCancel(appId: "gid://shopify/App/1", shopId: "gid://shopify/Shop/1", prorate: false, skipFinalUsageCharge: false, deferCancellation: false) { userErrors { message } } }"#,
        Value::Null,
    );
    assert_eq!(mutation["extensions"]["code"], "LSF_NOT_SIMULATED");
    // A field Shopify does not have is a validation error, as graphql-ruby words it.
    assert_eq!(
        error("{ shop { id } }", Value::Null)["message"],
        "Field 'shop' doesn't exist on type 'QueryRoot'"
    );
    assert_eq!(
        error(
            ACTIVE,
            json!({"appId": org.app, "shopId": "gid://shopify/Shop/1"})
        )["message"],
        "Shop not found"
    );
    let partners_id = org.app.replace("gid://shopify/", "gid://partners/");
    assert!(
        error(ACTIVE, json!({"appId": partners_id, "shopId": org.shop}))["message"]
            .as_str()
            .unwrap()
            .contains("takes the app's Shopify GID")
    );
    assert!(
        error(
            ACTIVE,
            json!({"appId": "gid://shopify/App/1", "shopId": org.shop})
        )["message"]
            .as_str()
            .unwrap()
            .starts_with("lsf runs one app")
    );
    assert_eq!(
        error(EVENTS, json!({"filter": {"shopId": org.shop}}))["message"],
        "You must specify a subjectId when filtering by shopId."
    );
    assert!(
        error(EVENTS, json!({"first": 251}))["message"]
            .as_str()
            .unwrap()
            .contains("250 at most")
    );
    assert!(
        error(
            EVENTS,
            json!({"filter": {"occurredAtMin": "2024-01-01T00:00:00Z", "occurredAtMax": "2025-06-01T00:00:00Z"}})
        )["message"]
            .as_str()
            .unwrap()
            .contains("365 days at most")
    );
    // An app of another organization: nothing; a theme: nothing.
    assert_eq!(
        partner_api(
            &api,
            "{ app(id: \"gid://partners/App/1\") { id } }",
            Value::Null
        )["data"]["app"],
        Value::Null
    );
    assert_eq!(
        events(
            &api,
            json!({"filter": {"subjectId": "gid://shopify/App/1"}})
        ),
        Vec::<String>::new()
    );
    assert_eq!(
        events(&api, json!({"filter": {"subjectType": "THEME"}})),
        Vec::<String>::new()
    );
}

/// `events` spans every shop of the organization, in pages, filtered by shop, newest or oldest
/// first; a reset puts the history back to the seed's.
#[test]
fn events_span_the_shops_in_pages() {
    let api = api();
    control(&api, "PUT", "/__lsf/admin/pricing-plans", plans(true));
    let test_shop = control(
        &api,
        "POST",
        "/__lsf/admin/shops",
        json!({"shop": "partner-1"}),
    )["shop"]
        .as_str()
        .unwrap()
        .to_string();
    select(&api, "pro");
    control(
        &api,
        "POST",
        "/__lsf/admin/pricing-plans/free/select",
        json!({"shop": test_shop}),
    );
    control(
        &api,
        "POST",
        "/__lsf/admin/uninstall",
        json!({"shop": test_shop}),
    );
    let all = events(&api, json!({}));
    assert_eq!(
        all,
        [
            "RELATIONSHIP_UNINSTALLED",
            "SUBSCRIPTION_CANCELED",
            "SUBSCRIPTION_CREATED",
            "SUBSCRIPTION_CREATED"
        ]
    );
    let mut oldest_first = events(&api, json!({"order": "OCCURRED_AT_ASC"}));
    oldest_first.reverse();
    assert_eq!(oldest_first, all);

    let listed = control(&api, "GET", "/__lsf/admin/partner", Value::Null);
    let test_shop_id = listed["shops"]
        .as_array()
        .unwrap()
        .iter()
        .find(|shop| shop["shop"] == json!(test_shop))
        .map(|shop| shop["id"].clone())
        .unwrap();
    let org = org(&api);
    assert_eq!(
        events(
            &api,
            json!({"filter": {"subjectId": org.app, "shopId": org.shop}})
        ),
        ["SUBSCRIPTION_CREATED"]
    );
    assert_eq!(
        events(
            &api,
            json!({"filter": {"subjectId": org.app, "shopId": test_shop_id}})
        ),
        [
            "RELATIONSHIP_UNINSTALLED",
            "SUBSCRIPTION_CANCELED",
            "SUBSCRIPTION_CREATED"
        ]
    );

    // Pages of one.
    let mut seen = Vec::new();
    let mut after = Value::Null;
    loop {
        let answer = partner_api(&api, EVENTS, json!({"first": 1, "after": after}));
        let connection = &answer["data"]["events"];
        let edges = connection["edges"].as_array().unwrap();
        assert_eq!(edges.len(), 1, "{answer}");
        seen.push(edges[0]["node"]["eventType"].as_str().unwrap().to_string());
        assert_eq!(
            connection["pageInfo"]["hasPreviousPage"],
            json!(!seen.is_empty() && seen.len() > 1)
        );
        assert_eq!(connection["pageInfo"]["endCursor"], edges[0]["cursor"]);
        if connection["pageInfo"]["hasNextPage"] != json!(true) {
            break;
        }
        after = connection["pageInfo"]["endCursor"].clone();
    }
    assert_eq!(seen, all);
    let bad = partner_api(&api, EVENTS, json!({"after": "bm9wZQ=="}));
    assert!(
        bad["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("not a cursor")
    );

    // A reset: every shop back to the seed's history, the install the shop starts with.
    control(&api, "DELETE", "/__lsf/admin", Value::Null);
    assert_eq!(events(&api, json!({})), Vec::<String>::new());
    assert_eq!(
        app_events(&api, Value::Null),
        [
            ("RELATIONSHIP_INSTALLED".to_string(), None),
            ("RELATIONSHIP_INSTALLED".to_string(), None),
        ],
        "one per shop"
    );
}

/// Where a test finds the Partner API: `GET /__lsf/admin/partner`, and `GET /__lsf/admin`.
#[test]
fn the_control_api_says_where_the_partner_api_is() {
    let api = api();
    let answer = control(&api, "GET", "/__lsf/admin/partner", Value::Null);
    let id = partner::partner_id(api.client_id());
    let app = lsf_app::admin::domains::app::app_id(api.app());
    let shop = api
        .with_default_shop(|state| state.shop_gid().unwrap().to_string())
        .unwrap();
    assert_eq!(
        answer,
        json!({
            "url": format!("http://127.0.0.1:9292/{id}/api/2026-10/graphql.json"),
            "partnerId": id,
            "token": partner::token(api.client_id()),
            "versions": ["2026-07", "2026-10", "2027-01"],
            "app": {
                "id": format!("gid://shopify/App/{app}"),
                "partnersId": format!("gid://partners/App/{app}"),
                "apiKey": api.client_id(),
                "name": "Admin fixtures",
            },
            "shops": [{
                "shop": api.default_shop(),
                "id": shop,
                "partnersId": shop.replace("gid://shopify/", "gid://partners/"),
            }],
        })
    );
    // The app's GID is the Admin API's.
    let token = api.token(&api.default_shop());
    let admin = json_of(
        &api.handle(&support::graphql_request(
            &token,
            "{ currentAppInstallation { app { id } } shop { id } }",
            Value::Null,
        ))
        .unwrap(),
    );
    assert_eq!(
        admin["data"]["currentAppInstallation"]["app"]["id"],
        answer["app"]["id"]
    );
    assert_eq!(admin["data"]["shop"]["id"], json!(shop));
    let status = control(&api, "GET", "/__lsf/admin", Value::Null);
    assert_eq!(
        status["partner"],
        json!({
            "graphql": format!("/{id}/api/2026-10/graphql.json"),
            "partnerId": id,
            "token": partner::token(api.client_id()),
            "appId": format!("gid://shopify/App/{app}"),
        })
    );
    let response = send(&api, "POST", "/__lsf/admin/partner", &[], b"");
    assert_eq!(response.status, 405);
}
