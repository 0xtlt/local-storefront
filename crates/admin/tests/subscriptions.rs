//! The Subscriptions API end to end, through `AdminApi::handle`: a subscription bought at the
//! local checkout and one the backend creates, billed cycle after cycle (paid, declined, after
//! a 3D Secure challenge), skipped and unskipped, paused, changed, cancelled; every change
//! delivered as Shopify delivers it, signed, to a listener of the test.

mod support;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration;

use lsf_admin::api::{AdminApi, AdminOptions, HttpRequest, HttpResponse, ThrottleSetting};
use lsf_admin::auth::signing::verify_webhook_hmac;
use lsf_admin::domains::orders::PlacedOrder;
use lsf_admin::webhooks::DeliveryOptions;
use lsf_checkout::{
    Address, CheckoutResult, Cost, DeliveryGroup, DeliveryOption, LineSellingPlan, ResultLine,
};
use serde_json::{Value, json};

const SECRET: &str = "test-secret";
const GRAPHQL: &str = "/admin/api/2026-10/graphql.json";

/// The demo store, its clock frozen at noon (UTC) on 2026-01-15.
fn api() -> AdminApi {
    let mut store = support::demo_store();
    store.now = Some("2026-01-15T12:00:00Z".parse().unwrap());
    AdminApi::new(
        &store,
        AdminOptions {
            app: None,
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

fn request(
    api: &AdminApi,
    method: &str,
    path: &str,
    query: &[(&str, &str)],
    body: Value,
) -> HttpResponse {
    api.handle(&HttpRequest {
        method: method.into(),
        path: path.into(),
        query: query
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
        headers: HashMap::from([("content-type".to_string(), "application/json".to_string())]),
        body: if body.is_null() {
            Vec::new()
        } else {
            body.to_string().into_bytes()
        },
    })
    .expect("the path is routed")
}

/// The answer to a GraphQL request with the default token; it must have no errors.
fn graphql(api: &AdminApi, query: &str, variables: Value) -> Value {
    let token = api.token(&api.default_shop());
    let response = api
        .handle(&HttpRequest {
            method: "POST".into(),
            path: GRAPHQL.into(),
            query: Vec::new(),
            headers: HashMap::from([
                ("content-type".to_string(), "application/json".to_string()),
                ("x-shopify-access-token".to_string(), token),
            ]),
            body: json!({"query": query, "variables": variables})
                .to_string()
                .into_bytes(),
        })
        .expect("the Admin API is routed");
    assert_eq!(response.status, 200);
    let answer: Value = serde_json::from_slice(&response.body).unwrap();
    assert!(answer.get("errors").is_none(), "{answer:#}");
    answer
}

/// A field of a mutation's answer, its user errors checked empty.
fn mutate(api: &AdminApi, field: &str, query: &str, variables: Value) -> Value {
    let answer = graphql(api, query, variables);
    let payload = answer["data"][field].clone();
    assert_eq!(payload["userErrors"], json!([]), "{field}: {payload:#}");
    payload
}

/// A webhook receiver: an HTTP listener that answers `200` and hands over each request.
struct Receiver {
    url: String,
    requests: mpsc::Receiver<(HashMap<String, String>, Vec<u8>)>,
}

impl Receiver {
    fn start() -> Receiver {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (sender, requests) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut headers = HashMap::new();
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    let trimmed = line.trim_end();
                    if trimmed.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = trimmed.split_once(':') {
                        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
                    }
                }
                let length: usize = headers
                    .get("content-length")
                    .and_then(|length| length.parse().ok())
                    .unwrap_or(0);
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
                if sender.send((headers, body)).is_err() {
                    break;
                }
            }
        });
        Receiver { url, requests }
    }

    /// The next deliveries: their topics and payloads, their signatures checked.
    fn take(&self, count: usize) -> Vec<(String, Value)> {
        (0..count)
            .map(|_| {
                let (headers, body) = self
                    .requests
                    .recv_timeout(Duration::from_secs(10))
                    .expect("a webhook was delivered");
                assert!(verify_webhook_hmac(
                    SECRET,
                    &body,
                    &headers["x-shopify-hmac-sha256"]
                ));
                (
                    headers["x-shopify-topic"].clone(),
                    serde_json::from_slice(&body).unwrap(),
                )
            })
            .collect()
    }

    /// The next deliveries, which must be of these topics, in this order.
    fn expect(&self, topics: &[&str]) -> Vec<Value> {
        let delivered = self.take(topics.len());
        let got: Vec<&str> = delivered.iter().map(|(topic, _)| topic.as_str()).collect();
        assert_eq!(got, topics);
        delivered.into_iter().map(|(_, payload)| payload).collect()
    }

    /// Nothing more was delivered.
    fn quiet(&self) {
        assert!(
            self.requests
                .recv_timeout(Duration::from_millis(300))
                .is_err(),
            "a webhook more was delivered"
        );
    }
}

const TOPICS: [&str; 22] = [
    "CUSTOMER_PAYMENT_METHODS_CREATE",
    "CUSTOMER_PAYMENT_METHODS_UPDATE",
    "CUSTOMER_PAYMENT_METHODS_REVOKE",
    "SUBSCRIPTION_CONTRACTS_CREATE",
    "SUBSCRIPTION_CONTRACTS_UPDATE",
    "SUBSCRIPTION_CONTRACTS_ACTIVATE",
    "SUBSCRIPTION_CONTRACTS_PAUSE",
    "SUBSCRIPTION_CONTRACTS_CANCEL",
    "SUBSCRIPTION_CONTRACTS_EXPIRE",
    "SUBSCRIPTION_CONTRACTS_FAIL",
    "SUBSCRIPTION_BILLING_ATTEMPTS_SUCCESS",
    "SUBSCRIPTION_BILLING_ATTEMPTS_FAILURE",
    "SUBSCRIPTION_BILLING_ATTEMPTS_CHALLENGED",
    "SUBSCRIPTION_BILLING_CYCLES_SKIP",
    "SUBSCRIPTION_BILLING_CYCLES_UNSKIP",
    "SUBSCRIPTION_BILLING_CYCLE_EDITS_CREATE",
    "SUBSCRIPTION_BILLING_CYCLE_EDITS_UPDATE",
    "SUBSCRIPTION_BILLING_CYCLE_EDITS_DELETE",
    "SELLING_PLAN_GROUPS_CREATE",
    "ORDERS_CREATE",
    "ORDERS_PAID",
    "INVENTORY_LEVELS_UPDATE",
];

fn subscribe(api: &AdminApi, receiver: &Receiver) {
    for topic in TOPICS {
        mutate(
            api,
            "webhookSubscriptionCreate",
            "mutation($topic: WebhookSubscriptionTopic!, $uri: String!) { webhookSubscriptionCreate(topic: $topic, webhookSubscription: {uri: $uri}) { userErrors { message } } }",
            json!({"topic": topic, "uri": format!("{}/webhooks", receiver.url)}),
        );
    }
}

/// A card of lsf's vault, its session made over HTTP as Shopify's vault makes one.
fn session(api: &AdminApi, number: &str) -> String {
    let response = request(
        api,
        "POST",
        "/__lsf/admin/card-sessions",
        &[],
        json!({"credit_card": {"number": number, "first_name": "Jane", "last_name": "Doe", "month": 8, "year": 2030, "verification_value": "123"}}),
    );
    assert_eq!(response.status, 201);
    let body: Value = serde_json::from_slice(&response.body).unwrap();
    body["id"].as_str().unwrap().to_string()
}

const ATTEMPT: &str = r#"mutation($id: ID!, $key: String!, $index: Int!) {
  subscriptionBillingAttemptCreate(subscriptionContractId: $id, subscriptionBillingAttemptInput: {idempotencyKey: $key, billingCycleSelector: {index: $index}}) {
    subscriptionBillingAttempt { id ready } userErrors { field message code }
  }
}"#;

const READ_ATTEMPT: &str = r#"query($id: ID!) { subscriptionBillingAttempt(id: $id) { ready errorCode order { id totalPriceSet { shopMoney { amount } } } state { __typename ... on SubscriptionBillingAttemptActionRequiredState { action { ... on SubscriptionBillingAttemptPaymentChallenge { nextActionUrl status } } } } } }"#;

#[test]
fn a_subscription_is_bought_billed_skipped_paused_changed_and_cancelled() {
    let receiver = Receiver::start();
    let api = api();
    subscribe(&api, &receiver);
    let store = support::demo_store();
    let jane = store.customer_by_email("jane.doe@example.com").unwrap();
    let customer = format!("gid://shopify/Customer/{}", jane.id);
    let socks = store.product_by_handle("merino-crew-socks").unwrap();

    // 1. A selling plan group of the app, on the socks: what the checkout sells them with.
    let group = mutate(
        &api,
        "sellingPlanGroupCreate",
        r#"mutation($product: ID!) { sellingPlanGroupCreate(input: {name: "Sock club", options: ["Every 2 weeks"], sellingPlansToCreate: [{name: "Every 2 weeks, 15% off", options: ["Every 2 weeks"], category: SUBSCRIPTION,
            billingPolicy: {recurring: {interval: WEEK, intervalCount: 2, minCycles: 2}}, deliveryPolicy: {recurring: {interval: WEEK, intervalCount: 2}},
            pricingPolicies: [{fixed: {adjustmentType: PERCENTAGE, adjustmentValue: {percentage: 15}}}]}]}, resources: {productIds: [$product]}) {
          sellingPlanGroup { id sellingPlans(first: 1) { nodes { id name } } } userErrors { field message } } }"#,
        json!({"product": format!("gid://shopify/Product/{}", socks.id)}),
    );
    let plan = group["sellingPlanGroup"]["sellingPlans"]["nodes"][0].clone();
    receiver.expect(&["selling_plan_groups/create"]);

    // 2. Bought at the local checkout: the order, the card vaulted, the contract.
    let variant = &socks.variants[0];
    let result = CheckoutResult {
        lines: vec![ResultLine {
            id: "gid://shopify/CartLine/1".into(),
            variant_id: variant.id,
            product_id: socks.id,
            title: socks.title.clone(),
            quantity: 3,
            unit_price: 1360,
            subtotal: 4080,
            total: 4080,
            requires_shipping: true,
            selling_plan: Some(LineSellingPlan {
                id: plan["id"].as_str().unwrap().to_string(),
                name: plan["name"].as_str().unwrap().to_string(),
                recurring_deliveries: true,
            }),
            ..ResultLine::default()
        }],
        delivery_groups: vec![DeliveryGroup {
            handle: "group-1".into(),
            line_ids: vec!["gid://shopify/CartLine/1".into()],
            options: vec![DeliveryOption {
                handle: "standard".into(),
                title: "Standard".into(),
                description: None,
                kind: Default::default(),
                original_cost: 500,
                cost: 500,
            }],
            selected: Some("standard".into()),
            discount_allocations: Vec::new(),
        }],
        cost: Cost {
            subtotal: 4080,
            discounts: 0,
            shipping: 500,
            tax: 0,
            total: 4580,
            due_now: 4580,
            due_later: 0,
            checkout_charge_amount: 4080,
            currency_code: "USD".into(),
        },
        ..CheckoutResult::default()
    };
    let address = Address {
        first_name: Some("Jane".into()),
        last_name: Some("Doe".into()),
        address1: Some("12 Maple Street".into()),
        city: Some("Portland".into()),
        country_code: Some("us".into()),
        province_code: Some("OR".into()),
        zip: Some("97201".into()),
        ..Address::default()
    };
    let recorded = api
        .record_order(&PlacedOrder {
            id: None,
            number: Some(1004),
            processed_at: "2026-01-15T12:00:00Z".parse().unwrap(),
            result: &result,
            customer_id: Some(jane.id),
            company_location_id: None,
            email: Some(&jane.email),
            phone: None,
            shipping_address: Some(&address),
            note: None,
            attributes: &[],
            payment: Some("Credit card"),
        })
        .unwrap();
    let delivered = receiver.expect(&[
        "orders/create",
        "orders/paid",
        "customer_payment_methods/create",
        "subscription_contracts/create",
    ]);
    let bought = &delivered[3];
    assert_eq!(
        bought["admin_graphql_api_origin_order_id"],
        json!(recorded.id)
    );
    assert_eq!(
        bought["billing_policy"],
        json!({"interval": "week", "interval_count": 2, "min_cycles": 2, "max_cycles": null})
    );
    assert_eq!(bought["status"], json!("active"));
    let checkout_contract = bought["admin_graphql_api_id"].as_str().unwrap().to_string();
    let read = graphql(
        &api,
        r#"query($id: ID!, $order: ID!) { subscriptionContract(id: $id) { nextBillingDate deliveryPrice { amount } originOrder { id } customerPaymentMethod { instrument { ... on CustomerCreditCard { lastDigits } } }
            lines(first: 1) { nodes { quantity currentPrice { amount } sellingPlanName pricingPolicy { basePrice { amount } cycleDiscounts { computedPrice { amount } } } } } }
          order(id: $order) { lineItems(first: 1) { nodes { sellingPlan { name } } } } }"#,
        json!({"id": checkout_contract, "order": recorded.id}),
    );
    let bought = &read["data"]["subscriptionContract"];
    assert_eq!(bought["nextBillingDate"], json!("2026-01-29T12:00:00Z"));
    assert_eq!(bought["deliveryPrice"]["amount"], json!("5.0"));
    assert_eq!(
        bought["customerPaymentMethod"]["instrument"]["lastDigits"],
        json!("4242")
    );
    assert_eq!(bought["lines"]["nodes"][0]["quantity"], json!(3));
    assert_eq!(
        bought["lines"]["nodes"][0]["pricingPolicy"],
        json!({"basePrice": {"amount": "16.0"}, "cycleDiscounts": [{"computedPrice": {"amount": "13.6"}}]})
    );
    assert_eq!(
        read["data"]["order"]["lineItems"]["nodes"][0]["sellingPlan"]["name"],
        json!("Every 2 weeks, 15% off")
    );

    // 3. The backend's own contract, atomically, on a card it vaulted.
    let card = mutate(
        &api,
        "customerPaymentMethodCreditCardCreate",
        r#"mutation($customer: ID!, $session: String!) { customerPaymentMethodCreditCardCreate(customerId: $customer, sessionId: $session, billingAddress: {city: "Portland", countryCode: US}) { customerPaymentMethod { id } processing userErrors { message } } }"#,
        json!({"customer": customer, "session": session(&api, "4242424242424242")}),
    );
    let method = card["customerPaymentMethod"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let vaulted = receiver.expect(&["customer_payment_methods/create"]);
    assert_eq!(
        vaulted[0]["payment_instrument"],
        json!({"last_digits": "4242", "month": 8, "year": 2030, "name": "Jane Doe", "brand": "Visa"})
    );
    let created = mutate(
        &api,
        "subscriptionContractAtomicCreate",
        r#"mutation($input: SubscriptionContractAtomicCreateInput!) { subscriptionContractAtomicCreate(input: $input) { contract { id } userErrors { field message } } }"#,
        json!({"input": {
            "customerId": customer, "currencyCode": "USD", "nextBillingDate": "2026-02-15T12:00:00Z",
            "contract": {"paymentMethodId": method, "billingPolicy": {"interval": "MONTH", "intervalCount": 1}, "deliveryPolicy": {"interval": "MONTH", "intervalCount": 1}, "deliveryPrice": "5.00",
                "deliveryMethod": {"shipping": {"address": {"address1": "12 Maple Street", "city": "Portland", "countryCode": "US", "zip": "97201"}, "shippingOption": {"title": "Standard"}}}},
            "lines": [{"line": {"productVariantId": format!("gid://shopify/ProductVariant/{}", variant.id), "quantity": 2, "currentPrice": "10.00", "sellingPlanId": plan["id"]}, "discounts": []}],
        }}),
    );
    let contract = created["contract"]["id"].as_str().unwrap().to_string();
    receiver.expect(&["subscription_contracts/create"]);

    // 4. Its cycles, from its creation and the shop's clock.
    let cycles = graphql(
        &api,
        r#"query($id: ID!) { subscriptionBillingCycles(contractId: $id, first: 3, billingCyclesIndexRangeSelector: {startIndex: 1, endIndex: 3}) { nodes { cycleIndex billingAttemptExpectedDate status } } }"#,
        json!({"id": contract}),
    );
    assert_eq!(
        cycles["data"]["subscriptionBillingCycles"]["nodes"],
        json!([
            {"cycleIndex": 1, "billingAttemptExpectedDate": "2026-02-15T12:00:00Z", "status": "UNBILLED"},
            {"cycleIndex": 2, "billingAttemptExpectedDate": "2026-03-15T11:00:00Z", "status": "UNBILLED"},
            {"cycleIndex": 3, "billingAttemptExpectedDate": "2026-04-15T11:00:00Z", "status": "UNBILLED"},
        ])
    );

    // 5. Cycle 1 billed: the order, its stock committed, `/success`.
    let attempt = mutate(
        &api,
        "subscriptionBillingAttemptCreate",
        ATTEMPT,
        json!({"id": contract, "key": "c1", "index": 1}),
    );
    assert_eq!(attempt["subscriptionBillingAttempt"]["ready"], json!(false));
    let delivered = receiver.expect(&[
        "orders/create",
        "inventory_levels/update",
        "orders/paid",
        "subscription_billing_attempts/success",
    ]);
    let order = delivered[0]["admin_graphql_api_id"].clone();
    assert_eq!(delivered[0]["source_name"], json!("subscription_contract"));
    assert_eq!(delivered[3]["admin_graphql_api_order_id"], order);
    assert_eq!(delivered[3]["idempotency_key"], json!("c1"));
    let read = graphql(
        &api,
        READ_ATTEMPT,
        json!({"id": attempt["subscriptionBillingAttempt"]["id"]}),
    );
    assert_eq!(
        read["data"]["subscriptionBillingAttempt"]["order"]["totalPriceSet"]["shopMoney"]["amount"],
        json!("25.0")
    );

    // 6. A declined card: cycle 2 fails, `/failure`, with Shopify's code.
    mutate(
        &api,
        "customerPaymentMethodCreditCardUpdate",
        r#"mutation($id: ID!, $session: String!) { customerPaymentMethodCreditCardUpdate(id: $id, sessionId: $session, billingAddress: {city: "Portland", countryCode: US}) { customerPaymentMethod { id } userErrors { message } } }"#,
        json!({"id": method, "session": session(&api, "4000000000000002")}),
    );
    receiver.expect(&["customer_payment_methods/update"]);
    mutate(
        &api,
        "subscriptionBillingAttemptCreate",
        ATTEMPT,
        json!({"id": contract, "key": "c2", "index": 2}),
    );
    let failure = receiver.expect(&["subscription_billing_attempts/failure"]);
    assert_eq!(failure[0]["error_code"], json!("card_declined"));
    assert_eq!(failure[0]["order_id"], Value::Null);

    // 7. A 3D Secure card: cycle 2 again, challenged; the customer completes the challenge on
    // lsf's page, and the attempt bills.
    let page = request(
        &api,
        "POST",
        &format!(
            "/__lsf/admin/payment-methods/{}/update",
            method.rsplit('/').next().unwrap()
        ),
        &[],
        json!({"credit_card": {"number": "4000000000003220", "month": 8, "year": 2030}}),
    );
    assert_eq!(page.status, 200);
    receiver.expect(&["customer_payment_methods/update"]);
    let challenged = mutate(
        &api,
        "subscriptionBillingAttemptCreate",
        ATTEMPT,
        json!({"id": contract, "key": "c2-retry", "index": 2}),
    );
    receiver.expect(&["subscription_billing_attempts/challenged"]);
    let read = graphql(
        &api,
        READ_ATTEMPT,
        json!({"id": challenged["subscriptionBillingAttempt"]["id"]}),
    );
    let action = &read["data"]["subscriptionBillingAttempt"]["state"]["action"];
    assert_eq!(action["status"], json!("OFF_SESSION_REJECTED"));
    let url = action["nextActionUrl"].as_str().unwrap();
    let path = url
        .split("http://127.0.0.1:9292")
        .nth(1)
        .unwrap_or(url)
        .split('?')
        .next()
        .unwrap();
    assert_eq!(request(&api, "GET", path, &[], Value::Null).status, 200);
    let done = request(&api, "POST", &format!("{path}/complete"), &[], Value::Null);
    let done: Value = serde_json::from_slice(&done.body).unwrap();
    assert_eq!(
        done["state"],
        json!("SubscriptionBillingAttemptSuccessState")
    );
    receiver.expect(&[
        "orders/create",
        "inventory_levels/update",
        "orders/paid",
        "subscription_billing_attempts/success",
    ]);

    // 8. Cycle 3 skipped, then unskipped; its billing date moved; its edits deleted.
    let cycle = json!({"contractId": contract, "selector": {"index": 3}});
    mutate(
        &api,
        "subscriptionBillingCycleSkip",
        "mutation($input: SubscriptionBillingCycleInput!) { subscriptionBillingCycleSkip(billingCycleInput: $input) { billingCycle { skipped } userErrors { message } } }",
        json!({"input": cycle}),
    );
    let skipped = receiver.expect(&["subscription_billing_cycles/skip"]);
    assert_eq!(skipped[0]["cycle_index"], json!(3));
    assert_eq!(skipped[0]["skipped"], json!(true));
    assert_eq!(
        skipped[0]["cycle_end_at"],
        json!("2026-04-15T07:00:00-04:00")
    );
    mutate(
        &api,
        "subscriptionBillingCycleUnskip",
        "mutation($input: SubscriptionBillingCycleInput!) { subscriptionBillingCycleUnskip(billingCycleInput: $input) { billingCycle { skipped } userErrors { message } } }",
        json!({"input": cycle}),
    );
    receiver.expect(&["subscription_billing_cycles/unskip"]);
    let schedule = "mutation($input: SubscriptionBillingCycleInput!, $date: DateTime!) { subscriptionBillingCycleScheduleEdit(billingCycleInput: $input, input: {billingDate: $date, reason: BUYER_INITIATED}) { billingCycle { billingAttemptExpectedDate } userErrors { message } } }";
    mutate(
        &api,
        "subscriptionBillingCycleScheduleEdit",
        schedule,
        json!({"input": cycle, "date": "2026-04-10T12:00:00Z"}),
    );
    receiver.expect(&["subscription_billing_cycle_edits/update"]);
    mutate(
        &api,
        "subscriptionBillingCycleEditDelete",
        "mutation($input: SubscriptionBillingCycleInput!) { subscriptionBillingCycleEditDelete(billingCycleInput: $input) { billingCycles { edited } userErrors { message } } }",
        json!({"input": cycle}),
    );
    receiver.expect(&["subscription_billing_cycle_edits/delete"]);

    // 9. Paused, activated, its product changed, its next billing date set, cancelled.
    let status = |name: &str| {
        format!(
            "mutation($id: ID!) {{ {name}(subscriptionContractId: $id) {{ contract {{ status }} userErrors {{ message }} }} }}"
        )
    };
    mutate(
        &api,
        "subscriptionContractPause",
        &status("subscriptionContractPause"),
        json!({"id": contract}),
    );
    assert_eq!(
        receiver.expect(&["subscription_contracts/pause"])[0]["status"],
        json!("paused")
    );
    mutate(
        &api,
        "subscriptionContractActivate",
        &status("subscriptionContractActivate"),
        json!({"id": contract}),
    );
    receiver.expect(&["subscription_contracts/activate"]);
    let line = graphql(
        &api,
        "query($id: ID!) { subscriptionContract(id: $id) { lines(first: 1) { nodes { id } } } }",
        json!({"id": contract}),
    )["data"]["subscriptionContract"]["lines"]["nodes"][0]["id"]
        .clone();
    mutate(
        &api,
        "subscriptionContractProductChange",
        "mutation($id: ID!, $line: ID!, $variant: ID!) { subscriptionContractProductChange(subscriptionContractId: $id, lineId: $line, input: {productVariantId: $variant, currentPrice: \"11.00\"}) { lineUpdated { variantId } userErrors { message } } }",
        json!({"id": contract, "line": line, "variant": format!("gid://shopify/ProductVariant/{}", socks.variants[1].id)}),
    );
    receiver.expect(&["subscription_contracts/update"]);
    mutate(
        &api,
        "subscriptionContractSetNextBillingDate",
        "mutation($id: ID!) { subscriptionContractSetNextBillingDate(contractId: $id, date: \"2026-05-15T12:00:00Z\") { contract { nextBillingDate } userErrors { message } } }",
        json!({"id": contract}),
    );
    receiver.expect(&["subscription_contracts/update"]);
    mutate(
        &api,
        "subscriptionContractCancel",
        &status("subscriptionContractCancel"),
        json!({"id": contract}),
    );
    assert_eq!(
        receiver.expect(&["subscription_contracts/cancel"])[0]["status"],
        json!("cancelled")
    );

    // 10. The checkout's contract fails, then expires; the cancelled contract's card is revoked.
    mutate(
        &api,
        "subscriptionContractFail",
        &status("subscriptionContractFail"),
        json!({"id": checkout_contract}),
    );
    receiver.expect(&["subscription_contracts/fail"]);
    mutate(
        &api,
        "subscriptionContractExpire",
        &status("subscriptionContractExpire"),
        json!({"id": checkout_contract}),
    );
    receiver.expect(&["subscription_contracts/expire"]);
    mutate(
        &api,
        "customerPaymentMethodRevoke",
        "mutation($id: ID!) { customerPaymentMethodRevoke(customerPaymentMethodId: $id) { revokedCustomerPaymentMethodId userErrors { message } } }",
        json!({"id": method}),
    );
    receiver.expect(&["customer_payment_methods/revoke"]);
    receiver.quiet();

    // 11. The log: what was charged, and what an email would have said.
    mutate(
        &api,
        "paymentInstrumentSendAddEmail",
        "mutation($id: String!) { paymentInstrumentSendAddEmail(mandate: {resourceId: $id, resourceType: SUBSCRIPTIONS}) { customer { id } userErrors { message } } }",
        json!({"id": checkout_contract}),
    );
    let log = request(&api, "GET", "/__lsf/admin/subscriptions", &[], Value::Null);
    let log: Value = serde_json::from_slice(&log.body).unwrap();
    let states: Vec<&str> = log["charges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|charge| charge["state"].as_str().unwrap())
        .collect();
    assert_eq!(
        states,
        [
            "SubscriptionBillingAttemptSuccessState",
            "SubscriptionBillingAttemptFailedState",
            "SubscriptionBillingAttemptSuccessState"
        ]
    );
    assert_eq!(log["emails"][0]["what"], json!("payment_instrument_add"));
    assert_eq!(log["emails"][0]["to"], json!("jane.doe@example.com"));
    // Nothing of it reaches the storefront's store data but the stock the orders committed.
    let status = api.status();
    assert!(
        status["mutations"]
            .as_array()
            .unwrap()
            .contains(&json!("subscriptionBillingCycleBulkCharge"))
    );
}

#[test]
fn every_mutation_of_the_subscriptions_api_is_simulated() {
    let api = api();
    let mutations = api.status()["mutations"].clone();
    for name in [
        "customerPaymentMethodCreditCardCreate",
        "customerPaymentMethodCreditCardUpdate",
        "customerPaymentMethodGetUpdateUrl",
        "customerPaymentMethodPaypalBillingAgreementCreate",
        "customerPaymentMethodPaypalBillingAgreementUpdate",
        "customerPaymentMethodRemoteCreate",
        "customerPaymentMethodRevoke",
        "customerPaymentMethodSendUpdateEmail",
        "paymentInstrumentSendAddEmail",
        "subscriptionBillingAttemptCreate",
        "subscriptionBillingCycleBulkCharge",
        "subscriptionBillingCycleBulkSearch",
        "subscriptionBillingCycleCharge",
        "subscriptionBillingCycleContractDraftCommit",
        "subscriptionBillingCycleContractDraftConcatenate",
        "subscriptionBillingCycleContractEdit",
        "subscriptionBillingCycleContractEditCalculate",
        "subscriptionBillingCycleEditDelete",
        "subscriptionBillingCycleEditsDelete",
        "subscriptionBillingCycleScheduleEdit",
        "subscriptionBillingCycleSkip",
        "subscriptionBillingCycleUnskip",
        "subscriptionContractActivate",
        "subscriptionContractAtomicCreate",
        "subscriptionContractCalculationCommit",
        "subscriptionContractCancel",
        "subscriptionContractCreate",
        "subscriptionContractCreateCalculate",
        "subscriptionContractExpire",
        "subscriptionContractFail",
        "subscriptionContractPause",
        "subscriptionContractProductChange",
        "subscriptionContractSetNextBillingDate",
        "subscriptionContractUpdate",
        "subscriptionContractUpdateCalculate",
        "subscriptionDraftCommit",
        "subscriptionDraftDiscountAdd",
        "subscriptionDraftDiscountCodeApply",
        "subscriptionDraftDiscountRemove",
        "subscriptionDraftDiscountUpdate",
        "subscriptionDraftFreeShippingDiscountAdd",
        "subscriptionDraftFreeShippingDiscountUpdate",
        "subscriptionDraftLineAdd",
        "subscriptionDraftLineRemove",
        "subscriptionDraftLineUpdate",
        "subscriptionDraftUpdate",
    ] {
        assert!(
            mutations.as_array().unwrap().contains(&json!(name)),
            "{name} is not simulated"
        );
    }
}

#[test]
fn the_card_vault_refuses_what_is_not_a_card() {
    let api = api();
    let response = request(
        &api,
        "POST",
        "/__lsf/admin/card-sessions",
        &[],
        json!({"credit_card": {"number": "", "month": 1, "year": 2030}}),
    );
    assert_eq!(response.status, 422);
    let response = request(&api, "GET", "/__lsf/admin/card-sessions", &[], Value::Null);
    assert_eq!(response.status, 405);
    let response = request(
        &api,
        "POST",
        "/__lsf/admin/billing-attempts/1/challenge/complete",
        &[],
        Value::Null,
    );
    assert_eq!(response.status, 404);
}

#[test]
fn the_root_queries_of_the_area_answer_without_defaults_on_the_demo_store() {
    let api = api();
    let answer = graphql(
        &api,
        r#"{ subscriptionContracts(first: 5) { nodes { id } } subscriptionBillingAttempts(first: 5) { nodes { id } } }"#,
        json!({}),
    );
    assert!(
        answer["extensions"].get("lsf").is_none(),
        "{:#}",
        answer["extensions"]
    );
}
