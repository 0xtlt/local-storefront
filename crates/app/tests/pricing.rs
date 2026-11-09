//! Managed pricing (Shopify App Pricing): the plans of the app, the merchant's choice of one on
//! the plan selection page of lsf's admin, the subscription it makes in the Admin API, its
//! webhook, its welcome link, and the Billing API it closes. Through the resolver, then through
//! `AdminApi::handle`.

mod support;

use std::collections::HashMap;
use std::time::Duration;

use lsf_app::admin::api::{AdminApi, AdminOptions, HttpRequest, HttpResponse, ThrottleSetting};
use lsf_app::admin::domains::app::pricing::{self, Billing, Pricing, SelectError};
use lsf_app::admin::graphql::{self, Request};
use lsf_app::admin::resolve::{Registry, ShopResolver};
use lsf_app::admin::state::ShopState;
use lsf_app::admin::webhooks::DeliveryOptions;
use serde_json::{Map, Value, json};
use support::json_of;
use support::receiver::Receiver;

const SECRET: &str = "test-secret";
const HOST: &str = "127.0.0.1:9292";
const ADMIN_APP: &str = "http://127.0.0.1:9292/admin/apps/admin-fixtures";

const PLANS: &str = r#"
[[plans]]
handle = "free"
name = "Free"
price = 0
features = ["One market"]

[[plans]]
handle = "pro"
name = "Pro"
price = 4.99
trial_days = 7
features = ["Every market", "Rules by customer tag"]
welcome_link = "/app/welcome"

[[plans]]
handle = "pro-annual"
name = "Pro (annual)"
price = 49
interval = "annual"
"#;

fn plans() -> Pricing {
    Pricing::from_toml(PLANS).unwrap()
}

fn shop() -> ShopState {
    ShopState::seeded(
        &support::fixture_store("rich"),
        Some(&support::fixture_app()),
    )
}

/// The merchant chooses `handle` on the page: what it did, or why not.
fn choose(
    shop: &mut ShopState,
    pricing: &Pricing,
    handle: &str,
) -> Result<pricing::Selected, SelectError> {
    let domain = shop.domain().to_string();
    pricing::select(shop, pricing, handle, |plan, number| {
        pricing::welcome_url(plan, ADMIN_APP, &domain, pricing.legacy, number)
    })
}

/// The answer to a request through the resolver, the app billing as `billing` says.
fn run(shop: &mut ShopState, billing: Billing, query: &str, variables: Value) -> Value {
    let mut resolver = ShopResolver::new(shop, Registry::standard()).with_billing(billing);
    let variables = match variables {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    let request = Request {
        query: query.to_string(),
        variables,
        operation_name: None,
    };
    graphql::run(
        lsf_app::admin::schema::admin(),
        &mut resolver,
        &request,
        None,
    )
    .to_json()
}

/// The topics and the records of the events taken from the shop.
fn events(shop: &mut ShopState) -> Vec<(String, String)> {
    shop.take_events()
        .into_iter()
        .map(|event| (event.topic, event.gid))
        .collect()
}

const ACTIVE: &str = r#"{ currentAppInstallation { activeSubscriptions { id name status test trialDays currentPeriodEnd returnUrl
  lineItems { plan { pricingDetails { ... on AppRecurringPricing { price { amount currencyCode } interval planHandle } } } } } } }"#;

fn active(shop: &mut ShopState) -> Vec<Value> {
    let answer = run(shop, Billing::Legacy, ACTIVE, json!({}));
    answer["data"]["currentAppInstallation"]["activeSubscriptions"]
        .as_array()
        .unwrap_or_else(|| panic!("{answer}"))
        .clone()
}

#[test]
fn a_plan_chosen_is_an_active_test_subscription_of_the_admin_api_with_its_plan_handle() {
    let mut shop = shop();
    let pricing = plans();
    let selected = choose(&mut shop, &pricing, "pro").unwrap();
    assert_eq!(selected.replaced, Vec::<String>::new());
    assert_eq!(
        selected.redirect,
        format!(
            "{ADMIN_APP}/app/welcome?shop={}&charge_id={}&plan_handle=pro",
            shop.domain(),
            selected.number
        )
    );
    assert_eq!(
        events(&mut shop),
        [(
            "app_subscriptions/update".to_string(),
            selected.subscription.clone()
        )]
    );
    let subscriptions = active(&mut shop);
    let [subscription] = subscriptions.as_slice() else {
        panic!("{subscriptions:?}");
    };
    assert_eq!(subscription["id"], json!(selected.subscription));
    assert_eq!(
        (
            &subscription["name"],
            &subscription["status"],
            &subscription["test"],
            &subscription["trialDays"],
        ),
        (&json!("Pro"), &json!("ACTIVE"), &json!(true), &json!(7))
    );
    assert_eq!(subscription["returnUrl"], json!(selected.redirect));
    assert_eq!(
        subscription["lineItems"][0]["plan"]["pricingDetails"],
        json!({"price": {"amount": "4.99", "currencyCode": "USD"}, "interval": "EVERY_30_DAYS", "planHandle": "pro"})
    );
    // The trial is the period: it ends in 7 days.
    let ends =
        chrono::DateTime::parse_from_rfc3339(subscription["currentPeriodEnd"].as_str().unwrap())
            .unwrap();
    assert_eq!((ends.to_utc() - shop.now()).num_days(), 7);
    assert_eq!(
        pricing::current(&shop).map(|current| current.plan),
        Some(Some("pro".to_string()))
    );
}

#[test]
fn a_new_plan_replaces_the_current_one_at_once_with_a_webhook_for_each() {
    let mut shop = shop();
    let pricing = plans();
    let pro = choose(&mut shop, &pricing, "pro").unwrap();
    shop.take_events();
    let annual = choose(&mut shop, &pricing, "pro-annual").unwrap();
    assert_eq!(annual.replaced, std::slice::from_ref(&pro.subscription));
    assert_eq!(annual.number, pro.number + 1);
    // As Shopify's Billing API replaces: the old one cancelled, then the new one active.
    assert_eq!(
        events(&mut shop),
        [
            (
                "app_subscriptions/update".to_string(),
                pro.subscription.clone()
            ),
            (
                "app_subscriptions/update".to_string(),
                annual.subscription.clone()
            ),
        ]
    );
    let subscriptions = active(&mut shop);
    assert_eq!(subscriptions.len(), 1);
    assert_eq!(subscriptions[0]["name"], json!("Pro (annual)"));
    assert_eq!(
        subscriptions[0]["lineItems"][0]["plan"]["pricingDetails"]["interval"],
        json!("ANNUAL")
    );
    let old = run(
        &mut shop,
        Billing::Legacy,
        r#"query($id: ID!) { node(id: $id) { ... on AppSubscription { status currentPeriodEnd } } }"#,
        json!({"id": pro.subscription}),
    );
    assert_eq!(
        old["data"]["node"],
        json!({"status": "CANCELLED", "currentPeriodEnd": null})
    );
}

#[test]
fn a_downgrade_to_the_free_plan_keeps_the_period_the_paid_plan_started() {
    let mut shop = shop();
    let pricing = plans();
    choose(&mut shop, &pricing, "pro-annual").unwrap();
    let paid_until = active(&mut shop)[0]["currentPeriodEnd"].clone();
    let free = choose(&mut shop, &pricing, "free").unwrap();
    let subscriptions = active(&mut shop);
    assert_eq!(subscriptions[0]["id"], json!(free.subscription));
    assert_eq!(subscriptions[0]["name"], json!("Free"));
    assert_eq!(
        subscriptions[0]["lineItems"][0]["plan"]["pricingDetails"]["price"],
        json!({"amount": "0.0", "currencyCode": "USD"})
    );
    assert_eq!(subscriptions[0]["trialDays"], json!(0));
    assert_eq!(subscriptions[0]["currentPeriodEnd"], paid_until);
    // An upgrade from it starts a period of its own.
    choose(&mut shop, &pricing, "pro").unwrap();
    assert_ne!(active(&mut shop)[0]["currentPeriodEnd"], paid_until);
}

#[test]
fn the_current_plan_and_an_unknown_one_cannot_be_chosen_and_change_nothing() {
    let mut shop = shop();
    let pricing = plans();
    choose(&mut shop, &pricing, "free").unwrap();
    shop.take_events();
    let revision = shop.revision();
    assert_eq!(
        choose(&mut shop, &pricing, "free"),
        Err(SelectError::Current)
    );
    assert_eq!(
        choose(&mut shop, &pricing, "enterprise"),
        Err(SelectError::UnknownPlan)
    );
    assert_eq!(shop.revision(), revision);
    assert!(events(&mut shop).is_empty());
}

#[test]
fn without_the_legacy_features_the_welcome_link_has_the_plan_alone_and_no_webhook_is_sent() {
    let mut shop = shop();
    let mut pricing = plans();
    pricing.legacy = false;
    let selected = choose(&mut shop, &pricing, "pro").unwrap();
    assert_eq!(
        selected.redirect,
        format!(
            "{ADMIN_APP}/app/welcome?shop={}&plan_handle=pro",
            shop.domain()
        )
    );
    let topics: Vec<String> = events(&mut shop)
        .into_iter()
        .map(|(topic, _)| topic)
        .collect();
    assert_eq!(topics, [lsf_app::admin::domains::app::CHANGED]);
    assert_eq!(active(&mut shop)[0]["test"], json!(false));
}

const SUBSCRIPTION_CREATE: &str = r#"mutation { appSubscriptionCreate(name: "Pro", returnUrl: "http://localhost:3000/", test: true,
  lineItems: [{plan: {appRecurringPricingDetails: {price: {amount: 5, currencyCode: USD}}}}]) {
  appSubscription { id } confirmationUrl userErrors { field message } } }"#;

const PURCHASE_CREATE: &str = r#"mutation { appPurchaseOneTimeCreate(name: "Credits", returnUrl: "http://localhost:3000/",
  price: {amount: 5, currencyCode: USD}) { appPurchaseOneTime { id } confirmationUrl userErrors { field message } } }"#;

#[test]
fn an_app_on_managed_pricing_creates_no_charge_with_the_billing_api_but_cancels_its_subscription() {
    let mut shop = shop();
    let refused = json!([{"field": null, "message": pricing::BILLING_API_REFUSED}]);
    let created = run(&mut shop, Billing::Legacy, SUBSCRIPTION_CREATE, json!({}));
    assert_eq!(
        created["data"]["appSubscriptionCreate"],
        json!({"appSubscription": null, "confirmationUrl": null, "userErrors": refused})
    );
    let purchased = run(&mut shop, Billing::Legacy, PURCHASE_CREATE, json!({}));
    assert_eq!(
        purchased["data"]["appPurchaseOneTimeCreate"]["userErrors"],
        refused
    );
    assert!(events(&mut shop).is_empty());
    // An app on manual pricing creates its charges.
    let manual = run(
        &mut shop,
        Billing::BillingApi,
        SUBSCRIPTION_CREATE,
        json!({}),
    );
    assert_eq!(
        manual["data"]["appSubscriptionCreate"]["userErrors"],
        json!([])
    );

    let selected = choose(&mut shop, &plans(), "pro").unwrap();
    shop.take_events();
    let cancelled = run(
        &mut shop,
        Billing::Legacy,
        r#"mutation($id: ID!) { appSubscriptionCancel(id: $id) { appSubscription { status } userErrors { message } } }"#,
        json!({"id": selected.subscription}),
    );
    assert_eq!(
        cancelled["data"]["appSubscriptionCancel"]["appSubscription"]["status"],
        json!("CANCELLED")
    );
    assert!(active(&mut shop).is_empty());
    assert_eq!(pricing::current(&shop), None);
    // With the legacy features, the cancellation is an `app_subscriptions/update`.
    assert_eq!(
        events(&mut shop),
        [(
            "app_subscriptions/update".to_string(),
            selected.subscription.clone()
        )]
    );
}

const CANCEL: &str = r#"mutation($id: ID!) { appSubscriptionCancel(id: $id) { appSubscription { status } userErrors { message } } }"#;

/// Shopify App Pricing "no longer sends webhooks for subscription changes" after April 28, 2026:
/// the app's own cancellation sends none either, nor does an uninstall in any mode.
#[test]
fn without_the_legacy_features_cancelling_a_subscription_sends_no_webhook() {
    let mut shop = shop();
    let mut pricing = plans();
    pricing.legacy = false;
    assert_eq!(Billing::of(Some(&pricing)), Billing::AppPricing);
    let selected = choose(&mut shop, &pricing, "pro").unwrap();
    shop.take_events();
    let cancelled = run(
        &mut shop,
        Billing::AppPricing,
        CANCEL,
        json!({"id": selected.subscription}),
    );
    assert_eq!(
        cancelled["data"]["appSubscriptionCancel"],
        json!({"appSubscription": {"status": "CANCELLED"}, "userErrors": []})
    );
    let topics: Vec<String> = events(&mut shop)
        .into_iter()
        .map(|(topic, _)| topic)
        .collect();
    assert_eq!(topics, [lsf_app::admin::domains::app::CHANGED]);
    assert!(active(&mut shop).is_empty());

    // The merchant uninstalls the app: the subscription is cancelled, without a webhook.
    let again = choose(&mut shop, &pricing, "pro").unwrap();
    shop.take_events();
    lsf_app::admin::domains::app::billing::cancel_on_uninstall(&mut shop);
    assert!(events(&mut shop).is_empty());
    assert!(active(&mut shop).is_empty());
    assert_ne!(again.subscription, selected.subscription);

    // Without plans, or with the legacy features, the Billing API's webhook.
    assert_eq!(Billing::of(None), Billing::BillingApi);
    assert_eq!(Billing::of(Some(&plans())), Billing::Legacy);
    assert_eq!(
        Billing::of(Some(&Pricing::default())),
        Billing::BillingApi,
        "no plans: the Billing API"
    );
}

/// The merchant's plan ends without the app doing it (lsf's stand-in: Shopify's page has no such
/// button): `CANCELLED` at once, `app_subscriptions/update` with the legacy features, nothing
/// without them.
#[test]
fn the_plan_ended_outside_the_app_is_cancelled_with_a_webhook_only_with_the_legacy_features() {
    let mut shop = shop();
    let pricing = plans();
    assert_eq!(pricing::cancel(&mut shop, &pricing), None, "no plan yet");
    let selected = choose(&mut shop, &pricing, "pro").unwrap();
    shop.take_events();
    let ended = pricing::cancel(&mut shop, &pricing).unwrap();
    assert_eq!(
        ended,
        pricing::Ended {
            subscription: selected.subscription.clone(),
            number: selected.number,
            plan: Some("pro".to_string()),
            webhook: Some("app_subscriptions/update"),
        }
    );
    assert_eq!(
        events(&mut shop),
        [(
            "app_subscriptions/update".to_string(),
            selected.subscription.clone()
        )]
    );
    assert!(active(&mut shop).is_empty());
    assert_eq!(pricing::current(&shop), None);
    let record = shop.get(&selected.subscription).unwrap();
    assert_eq!(
        (&record["status"], &record["currentPeriodEnd"]),
        (&json!("CANCELLED"), &Value::Null)
    );
    assert_eq!(pricing::cancel(&mut shop, &pricing), None, "ended already");

    // Without the legacy features: no webhook, as for every change of the subscription.
    let mut app_pricing = plans();
    app_pricing.legacy = false;
    let again = choose(&mut shop, &app_pricing, "free").unwrap();
    shop.take_events();
    let ended = pricing::cancel(&mut shop, &app_pricing).unwrap();
    assert_eq!(
        (
            ended.subscription.as_str(),
            ended.plan.as_deref(),
            ended.webhook
        ),
        (again.subscription.as_str(), Some("free"), None)
    );
    let topics: Vec<String> = events(&mut shop)
        .into_iter()
        .map(|(topic, _)| topic)
        .collect();
    assert_eq!(topics, [lsf_app::admin::domains::app::CHANGED]);
    assert!(active(&mut shop).is_empty());

    // A new plan can be chosen after: the one just ended is not the current one.
    assert!(choose(&mut shop, &pricing, "free").is_ok());
}

// ---- through `AdminApi::handle` ---------------------------------------------------------------

fn api() -> AdminApi {
    AdminApi::new(
        &support::demo_store(),
        AdminOptions {
            app: Some(support::fixture_app()),
            client_secret: SECRET.to_string(),
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
    all.insert("host".into(), HOST.into());
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

fn get(api: &AdminApi, path: &str) -> HttpResponse {
    send(api, "GET", path, &[], b"")
}

fn text(response: &HttpResponse) -> String {
    String::from_utf8_lossy(&response.body).into_owned()
}

fn location(response: &HttpResponse) -> &str {
    response
        .headers
        .iter()
        .find(|(name, _)| name == "location")
        .map(|(_, value)| value.as_str())
        .unwrap_or_default()
}

fn graphql(api: &AdminApi, query: &str, variables: Value) -> Value {
    let token = api.token(&api.default_shop());
    let response = api
        .handle(&support::graphql_request(&token, query, variables))
        .unwrap();
    assert_eq!(response.status, 200);
    json_of(&response)
}

fn subscribe(api: &AdminApi, receiver: &Receiver) {
    let answer = graphql(
        api,
        "mutation($uri: String!) { webhookSubscriptionCreate(topic: APP_SUBSCRIPTIONS_UPDATE, webhookSubscription: {uri: $uri}) { userErrors { message } } }",
        json!({"uri": format!("{}/webhooks", receiver.url)}),
    );
    assert_eq!(
        answer["data"]["webhookSubscriptionCreate"]["userErrors"],
        json!([])
    );
}

fn plans_json() -> Value {
    json!({"plans": [
        {"handle": "free", "name": "Free", "price": 0, "features": ["One market"]},
        {"handle": "pro", "name": "Pro", "price": "4.99", "trial_days": 7, "welcome_link": "/app/welcome"},
    ]})
}

const PAGE: &str = "/admin/charges/admin-fixtures/pricing_plans";

#[test]
fn without_plans_the_page_says_how_to_declare_them_and_the_billing_api_creates_charges() {
    let api = api();
    let status = json_of(&get(&api, "/__lsf/admin/pricing-plans"));
    assert_eq!(
        (
            &status["managedPricing"],
            &status["plans"],
            &status["source"]
        ),
        (&json!(false), &json!([]), &Value::Null)
    );
    assert_eq!(status["page"], json!(PAGE));
    let page = get(&api, PAGE);
    assert_eq!(page.status, 404);
    let html = text(&page);
    assert!(
        html.contains("No pricing plans")
            && html.contains("--pricing-plans")
            && html.contains("PUT /__lsf/admin/pricing-plans"),
        "{html}"
    );
    let created = graphql(&api, SUBSCRIPTION_CREATE, json!({}));
    assert_eq!(
        created["data"]["appSubscriptionCreate"]["userErrors"],
        json!([])
    );
    assert_eq!(
        send(
            &api,
            "POST",
            "/__lsf/admin/pricing-plans/pro/select",
            &[],
            b""
        )
        .status,
        404
    );
}

#[test]
fn the_merchant_chooses_and_changes_plans_on_the_page_and_the_backend_hears_of_it() {
    let receiver = Receiver::start(SECRET);
    let api = api();
    subscribe(&api, &receiver);
    let shop = api.default_shop();

    // Declared through the control API.
    let set = send(
        &api,
        "PUT",
        "/__lsf/admin/pricing-plans",
        &[],
        plans_json().to_string().as_bytes(),
    );
    assert_eq!(set.status, 200, "{}", text(&set));
    let status = json_of(&set);
    assert_eq!(
        (
            &status["ok"],
            &status["managedPricing"],
            &status["legacy"],
            &status["source"]
        ),
        (&json!(true), &json!(true), &json!(true), &json!("request"))
    );
    assert_eq!(status["plans"][1]["price"], json!("4.99"));
    assert_eq!(status["current"], Value::Null);
    assert_eq!(
        json_of(&get(&api, "/__lsf/admin"))["app"]["pricingPlans"],
        json!({"plans": 2, "legacy": true, "source": "request"})
    );

    // The page: a card per plan, a button to choose each.
    let page = get(&api, PAGE);
    assert_eq!(page.status, 200);
    let html = text(&page);
    assert!(html.contains("<title>Pricing plans · "), "{html}");
    for part in [
        "data-plan=\"free\"",
        "data-plan=\"pro\"",
        "4.99 USD",
        "every 30 days",
        "7-day free trial",
        "One market",
        &format!("href=\"{PAGE}/pro\""),
    ] {
        assert!(html.contains(part), "no {part} in the page");
    }
    // By client id it is a 404, as on Shopify, which names the app by its handle there.
    let by_client_id = get(
        &api,
        "/admin/charges/11111111111111111111111111111111/pricing_plans",
    );
    assert_eq!(by_client_id.status, 404);
    assert!(text(&by_client_id).contains("Use the app's handle"));
    assert!(
        by_client_id
            .headers
            .iter()
            .any(|(name, reason)| name == "x-lsf-reason" && reason.contains("admin-fixtures")),
        "{:?}",
        by_client_id.headers
    );
    let other = get(&api, "/admin/charges/another-app/pricing_plans");
    assert_eq!(other.status, 404);
    assert!(text(&other).contains("lsf has no such app"));
    assert_eq!(get(&api, &format!("{PAGE}/enterprise")).status, 404);
    assert_eq!(get(&api, &format!("{PAGE}/pro/more")).status, 404);

    // The plan's page, its form, the approval: back to the welcome link.
    let confirm = text(&get(&api, &format!("{PAGE}/pro?shop={shop}")));
    assert!(confirm.contains("Approve subscription") && confirm.contains("7-day free trial"));
    let action = format!("/__lsf/admin/pricing-plans/pro/select?shop={shop}");
    assert!(
        confirm.contains(&format!("action=\"{}\"", action.replace('&', "&amp;"))),
        "{confirm}"
    );
    let approved = send(
        &api,
        "POST",
        &action,
        &[("content-type", "application/x-www-form-urlencoded")],
        format!("shop={shop}").as_bytes(),
    );
    assert_eq!(approved.status, 303);
    assert_eq!(
        location(&approved),
        format!(
            "http://{HOST}/admin/apps/admin-fixtures/app/welcome?shop={shop}&charge_id=1&plan_handle=pro"
        )
    );
    let (topic, payload) = receiver.next();
    assert_eq!(topic, "app_subscriptions/update");
    let subscription = &payload["app_subscription"];
    assert_eq!(
        (
            &subscription["admin_graphql_api_id"],
            &subscription["name"],
            &subscription["status"],
            &subscription["price"],
            &subscription["currency"],
            &subscription["interval"],
            &subscription["plan_handle"],
        ),
        (
            &json!("gid://shopify/AppSubscription/1"),
            &json!("Pro"),
            &json!("ACTIVE"),
            &json!("4.99"),
            &json!("USD"),
            &json!("every_30_days"),
            &json!("pro"),
        )
    );
    let active = graphql(
        &api,
        "{ currentAppInstallation { activeSubscriptions { id name status test } } }",
        json!({}),
    );
    assert_eq!(
        active["data"]["currentAppInstallation"]["activeSubscriptions"],
        json!([{"id": "gid://shopify/AppSubscription/1", "name": "Pro", "status": "ACTIVE", "test": true}])
    );

    // The page marks the plan, which cannot be chosen again.
    let html = text(&get(&api, PAGE));
    assert!(html.contains("lsf-plan lsf-plan-current"));
    assert!(html.contains("Current plan") && !html.contains(&format!("href=\"{PAGE}/pro\"")));
    let again = send(
        &api,
        "POST",
        "/__lsf/admin/pricing-plans/pro/select",
        &[],
        b"",
    );
    assert_eq!(again.status, 409);
    let unknown = send(
        &api,
        "POST",
        "/__lsf/admin/pricing-plans/gold/select",
        &[],
        b"",
    );
    assert_eq!(unknown.status, 404);
    assert!(text(&unknown).contains("the plans are free, pro"));

    // The Billing API creates no charge while the app is on managed pricing.
    let refused = graphql(&api, SUBSCRIPTION_CREATE, json!({}));
    assert_eq!(
        refused["data"]["appSubscriptionCreate"]["userErrors"],
        json!([{"field": null, "message": pricing::BILLING_API_REFUSED}])
    );

    // Down to the free plan, without the page: the old one cancelled, then the new one.
    let free = send(
        &api,
        "POST",
        "/__lsf/admin/pricing-plans/free/select",
        &[],
        json!({"shop": shop}).to_string().as_bytes(),
    );
    assert_eq!(free.status, 200, "{}", text(&free));
    let free = json_of(&free);
    assert_eq!(free["replaced"], json!(["gid://shopify/AppSubscription/1"]));
    assert_eq!(free["subscription"]["chargeId"], json!(2));
    assert_eq!(free["subscription"]["status"], json!("ACTIVE"));
    assert_eq!(
        free["redirect"],
        json!(format!(
            "http://{HOST}/admin/apps/admin-fixtures?shop={shop}&charge_id=2&plan_handle=free"
        ))
    );
    let statuses: Vec<(Value, Value)> = (0..2)
        .map(|_| receiver.next().1["app_subscription"].clone())
        .map(|subscription| {
            (
                subscription["plan_handle"].clone(),
                subscription["status"].clone(),
            )
        })
        .collect();
    assert_eq!(
        statuses,
        [
            (json!("pro"), json!("CANCELLED")),
            (json!("free"), json!("ACTIVE"))
        ]
    );
    assert_eq!(
        json_of(&get(&api, "/__lsf/admin/pricing-plans"))["current"],
        json!({"id": "gid://shopify/AppSubscription/2", "name": "Free", "plan": "free"})
    );

    // Without the legacy features, the app cancels the free plan: no webhook. The next one the
    // backend hears of is the plan chosen once they are back.
    let mut plans = plans_json();
    plans["legacy"] = json!(false);
    let set = send(
        &api,
        "PUT",
        "/__lsf/admin/pricing-plans",
        &[],
        plans.to_string().as_bytes(),
    );
    assert_eq!(set.status, 200, "{}", text(&set));
    assert_eq!(api.billing(), pricing::Billing::AppPricing);
    let cancelled = graphql(
        &api,
        CANCEL,
        json!({"id": "gid://shopify/AppSubscription/2"}),
    );
    assert_eq!(
        cancelled["data"]["appSubscriptionCancel"]["appSubscription"]["status"],
        json!("CANCELLED")
    );
    let set = send(
        &api,
        "PUT",
        "/__lsf/admin/pricing-plans",
        &[],
        plans_json().to_string().as_bytes(),
    );
    assert_eq!(set.status, 200, "{}", text(&set));
    assert_eq!(api.billing(), pricing::Billing::Legacy);
    let pro = send(
        &api,
        "POST",
        "/__lsf/admin/pricing-plans/pro/select",
        &[],
        json!({"shop": shop}).to_string().as_bytes(),
    );
    assert_eq!(pro.status, 200, "{}", text(&pro));
    let next = receiver.next().1["app_subscription"].clone();
    assert_eq!(
        (&next["plan_handle"], &next["status"]),
        (&json!("pro"), &json!("ACTIVE")),
        "the cancellation sent nothing: {next}"
    );

    // Gone: back to no plans, and the Billing API again.
    let deleted = json_of(&send(
        &api,
        "DELETE",
        "/__lsf/admin/pricing-plans",
        &[],
        b"",
    ));
    assert_eq!(
        (&deleted["managedPricing"], &deleted["source"]),
        (&json!(false), &Value::Null)
    );
    let created = graphql(&api, SUBSCRIPTION_CREATE, json!({}));
    assert_eq!(
        created["data"]["appSubscriptionCreate"]["userErrors"],
        json!([])
    );
}

#[test]
fn a_wrong_declaration_is_refused_and_the_plans_of_the_file_come_back_after_a_request() {
    let api = api().with_pricing(plans(), "pricing.toml");
    let status = json_of(&get(&api, "/__lsf/admin/pricing-plans"));
    assert_eq!(
        (
            &status["source"],
            &status["plans"].as_array().unwrap().len()
        ),
        (&json!("pricing.toml"), &3)
    );
    let not_json = send(
        &api,
        "PUT",
        "/__lsf/admin/pricing-plans",
        &[],
        b"plans = []",
    );
    assert_eq!(not_json.status, 400);
    let wrong = send(
        &api,
        "PUT",
        "/__lsf/admin/pricing-plans",
        &[],
        br#"{"plans": [{"handle": "pro", "name": "Pro", "price": -1}]}"#,
    );
    assert_eq!(wrong.status, 422);
    assert!(text(&wrong).contains("plans[0] (pro): price is 0 or more"));
    // Unchanged: the file's.
    assert_eq!(
        json_of(&get(&api, "/__lsf/admin/pricing-plans"))["source"],
        json!("pricing.toml")
    );
    let set = send(
        &api,
        "PUT",
        "/__lsf/admin/pricing-plans",
        &[],
        br#"{"legacy": false, "plans": [{"handle": "solo", "name": "Solo", "price": 1}]}"#,
    );
    assert_eq!(json_of(&set)["legacy"], json!(false));
    // A reset of the shops keeps the plans: they are the app's.
    assert_eq!(send(&api, "DELETE", "/__lsf/admin", &[], b"").status, 200);
    assert_eq!(
        json_of(&get(&api, "/__lsf/admin/pricing-plans"))["plans"][0]["handle"],
        json!("solo")
    );
    let back = json_of(&send(
        &api,
        "DELETE",
        "/__lsf/admin/pricing-plans",
        &[],
        b"",
    ));
    assert_eq!(
        (
            &back["source"],
            &back["legacy"],
            &back["plans"][0]["handle"]
        ),
        (&json!("pricing.toml"), &json!(true), &json!("free"))
    );
    assert_eq!(
        send(&api, "PATCH", "/__lsf/admin/pricing-plans", &[], b"").status,
        405
    );
    assert_eq!(
        send(
            &api,
            "GET",
            "/__lsf/admin/pricing-plans/free/select",
            &[],
            b""
        )
        .status,
        405
    );
}

/// `POST /__lsf/admin/pricing-plans/cancel`: the plan ends for a test, the backend hears of it
/// with the legacy features and not without them, and the shop has no plan after.
#[test]
fn the_control_api_ends_the_shops_plan_as_if_it_ended_outside_the_app() {
    let receiver = Receiver::start(SECRET);
    let api = api();
    subscribe(&api, &receiver);
    let shop = api.default_shop();
    let body = json!({"shop": shop}).to_string();
    let cancel = |api: &AdminApi| {
        send(
            api,
            "POST",
            "/__lsf/admin/pricing-plans/cancel",
            &[],
            body.as_bytes(),
        )
    };

    // Without plans: managed pricing is off, nothing to end.
    let none = cancel(&api);
    assert_eq!(none.status, 404);
    assert!(text(&none).contains("--pricing-plans"));

    let set = send(
        &api,
        "PUT",
        "/__lsf/admin/pricing-plans",
        &[],
        plans_json().to_string().as_bytes(),
    );
    assert_eq!(set.status, 200, "{}", text(&set));
    let nothing = cancel(&api);
    assert_eq!(nothing.status, 409);
    assert!(text(&nothing).contains("has no active subscription"));
    assert_eq!(
        get(&api, "/__lsf/admin/pricing-plans/cancel").status,
        405,
        "POST only"
    );

    // With the legacy features: the backend hears of it.
    let pro = send(
        &api,
        "POST",
        "/__lsf/admin/pricing-plans/pro/select",
        &[],
        body.as_bytes(),
    );
    assert_eq!(pro.status, 200, "{}", text(&pro));
    assert_eq!(
        receiver.next().1["app_subscription"]["status"],
        json!("ACTIVE")
    );
    let ended = cancel(&api);
    assert_eq!(ended.status, 200, "{}", text(&ended));
    let ended = json_of(&ended);
    assert_eq!(
        (
            &ended["ok"],
            &ended["shop"],
            &ended["plan"]["handle"],
            &ended["webhook"]
        ),
        (
            &json!(true),
            &json!(shop),
            &json!("pro"),
            &json!("app_subscriptions/update")
        )
    );
    assert_eq!(
        (
            &ended["subscription"]["id"],
            &ended["subscription"]["chargeId"],
            &ended["subscription"]["name"],
            &ended["subscription"]["status"],
            &ended["subscription"]["currentPeriodEnd"],
        ),
        (
            &json!("gid://shopify/AppSubscription/1"),
            &json!(1),
            &json!("Pro"),
            &json!("CANCELLED"),
            &Value::Null
        )
    );
    let delivered = receiver.next().1["app_subscription"].clone();
    assert_eq!(
        (&delivered["plan_handle"], &delivered["status"]),
        (&json!("pro"), &json!("CANCELLED"))
    );
    assert_eq!(
        json_of(&get(&api, "/__lsf/admin/pricing-plans"))["current"],
        Value::Null
    );
    let active = graphql(
        &api,
        "{ currentAppInstallation { activeSubscriptions { id } } }",
        json!({}),
    );
    assert_eq!(
        active["data"]["currentAppInstallation"]["activeSubscriptions"],
        json!([])
    );
    assert_eq!(cancel(&api).status, 409, "ended already");

    // Without them (`legacy = false`): no webhook. The next one the backend hears of is the plan
    // chosen once they are back.
    let mut plans = plans_json();
    plans["legacy"] = json!(false);
    let set = send(
        &api,
        "PUT",
        "/__lsf/admin/pricing-plans",
        &[],
        plans.to_string().as_bytes(),
    );
    assert_eq!(set.status, 200, "{}", text(&set));
    let free = send(
        &api,
        "POST",
        "/__lsf/admin/pricing-plans/free/select",
        &[],
        body.as_bytes(),
    );
    assert_eq!(free.status, 200, "{}", text(&free));
    let ended = json_of(&cancel(&api));
    assert_eq!(
        (
            &ended["subscription"]["name"],
            &ended["subscription"]["status"],
            &ended["webhook"]
        ),
        (&json!("Free"), &json!("CANCELLED"), &Value::Null)
    );
    let set = send(
        &api,
        "PUT",
        "/__lsf/admin/pricing-plans",
        &[],
        plans_json().to_string().as_bytes(),
    );
    assert_eq!(set.status, 200, "{}", text(&set));
    let pro = send(
        &api,
        "POST",
        "/__lsf/admin/pricing-plans/pro/select",
        &[],
        body.as_bytes(),
    );
    assert_eq!(pro.status, 200, "{}", text(&pro));
    let next = receiver.next().1["app_subscription"].clone();
    assert_eq!(
        (&next["plan_handle"], &next["status"]),
        (&json!("pro"), &json!("ACTIVE")),
        "the plan ended under Shopify App Pricing sent something: {next}"
    );
}

#[test]
fn the_file_of_the_plans_is_read_again_when_it_changes_and_on_delete() {
    let dir = std::env::temp_dir().join(format!("lsf-pricing-reload-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("pricing.toml");
    let source = file.display().to_string();
    let one = "[[plans]]\nhandle = \"free\"\nname = \"Free\"\nprice = 0\n";
    let two = format!("{one}\n[[plans]]\nhandle = \"pro\"\nname = \"Pro\"\nprice = 4.99\n");
    std::fs::write(&file, one).unwrap();
    let api = api().with_pricing(Pricing::read(&file).unwrap(), source.clone());
    let handles = |api: &AdminApi| -> Vec<String> {
        api.pricing()
            .unwrap()
            .0
            .plans
            .iter()
            .map(|plan| plan.handle.clone())
            .collect()
    };
    // Unchanged: nothing to read.
    assert_eq!(api.reload_pricing_file_if_changed(), Ok(None));

    // Changed: read again, once.
    std::fs::write(&file, &two).unwrap();
    assert_eq!(
        api.reload_pricing_file_if_changed(),
        Ok(Some(source.clone()))
    );
    assert_eq!(handles(&api), ["free", "pro"]);
    assert_eq!(api.reload_pricing_file_if_changed(), Ok(None));

    // Plans a request set stay over the file until the DELETE, which reads the file again.
    let set = send(
        &api,
        "PUT",
        "/__lsf/admin/pricing-plans",
        &[],
        br#"{"plans": [{"handle": "solo", "name": "Solo", "price": 1}]}"#,
    );
    assert_eq!(set.status, 200);
    std::fs::write(&file, one).unwrap();
    assert_eq!(
        api.reload_pricing_file_if_changed(),
        Ok(Some(source.clone()))
    );
    assert_eq!(handles(&api), ["solo"]);
    std::fs::write(&file, &two).unwrap();
    let back = send(&api, "DELETE", "/__lsf/admin/pricing-plans", &[], b"");
    assert_eq!(back.status, 200);
    assert_eq!(json_of(&back)["source"], json!(source));
    assert_eq!(handles(&api), ["free", "pro"]);

    // A file that no longer reads: said once, and the plans stay; the DELETE is a 422.
    std::fs::write(&file, "[[plans]]\nhandle = ").unwrap();
    assert!(api.reload_pricing_file_if_changed().is_err());
    assert_eq!(api.reload_pricing_file_if_changed(), Ok(None));
    assert_eq!(handles(&api), ["free", "pro"]);
    assert_eq!(
        send(&api, "DELETE", "/__lsf/admin/pricing-plans", &[], b"").status,
        422
    );

    // A file that is gone: the plans read last stay.
    std::fs::remove_file(&file).unwrap();
    assert_eq!(api.reload_pricing_file_if_changed(), Ok(None));
    assert_eq!(
        send(&api, "DELETE", "/__lsf/admin/pricing-plans", &[], b"").status,
        200
    );
    assert_eq!(handles(&api), ["free", "pro"]);
    std::fs::remove_dir_all(&dir).ok();
}
