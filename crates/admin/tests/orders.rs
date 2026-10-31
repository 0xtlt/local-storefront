//! The orders family end to end, through `AdminApi::handle`: an order placed in the local
//! checkout, then fulfilled, refunded and edited by the backend, each change delivered as
//! Shopify delivers it, signed, to a listener of the test.

mod support;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration;

use lsf_admin::api::{AdminApi, AdminOptions, HttpRequest, ThrottleSetting};
use lsf_admin::auth::signing::verify_webhook_hmac;
use lsf_admin::domains::orders::PlacedOrder;
use lsf_admin::webhooks::DeliveryOptions;
use lsf_checkout::{Address, CheckoutResult, Cost, DeliveryGroup, DeliveryOption, ResultLine};
use serde_json::{Value, json};

const SECRET: &str = "test-secret";
const GRAPHQL: &str = "/admin/api/2026-10/graphql.json";

fn api() -> AdminApi {
    AdminApi::new(
        &support::demo_store(),
        AdminOptions {
            app: None,
            client_secret: SECRET.to_string(),
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

/// The answer to a GraphQL request with the default token.
fn graphql(api: &AdminApi, query: &str, variables: Value) -> Value {
    let token = api.token(&api.default_shop());
    let body = json!({"query": query, "variables": variables}).to_string();
    let response = api
        .handle(&HttpRequest {
            method: "POST".into(),
            path: GRAPHQL.into(),
            query: Vec::new(),
            headers: HashMap::from([
                ("content-type".to_string(), "application/json".to_string()),
                ("x-shopify-access-token".to_string(), token),
            ]),
            body: body.into_bytes(),
        })
        .expect("the Admin API is routed");
    assert_eq!(response.status, 200);
    serde_json::from_slice(&response.body).unwrap()
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

    /// The next delivery: its topic and its payload, its signature checked.
    fn next(&self) -> (String, Value) {
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
    }

    /// The next `count` deliveries, by topic.
    fn take(&self, count: usize) -> Vec<(String, Value)> {
        (0..count).map(|_| self.next()).collect()
    }
}

fn subscribe(api: &AdminApi, receiver: &Receiver, topics: &[&str]) {
    for topic in topics {
        let answer = graphql(
            api,
            "mutation($topic: WebhookSubscriptionTopic!, $uri: String!) { webhookSubscriptionCreate(topic: $topic, webhookSubscription: {uri: $uri}) { userErrors { message } } }",
            json!({"topic": topic, "uri": format!("{}/webhooks", receiver.url)}),
        );
        assert_eq!(
            answer["data"]["webhookSubscriptionCreate"]["userErrors"],
            json!([]),
            "{answer}"
        );
    }
}

/// An order of two mugs placed in the local checkout, paid by card.
fn place_order(api: &AdminApi) -> String {
    place_order_numbered(api, 1004).expect("the order is recorded")
}

/// [`place_order`] as the order `number`; `None` when the Admin API did not record it.
fn place_order_numbered(api: &AdminApi, number: u64) -> Option<String> {
    let store = support::demo_store();
    let jane = store.customer_by_email("jane.doe@example.com").unwrap();
    let mug = store.product_by_handle("ceramic-mug").unwrap();
    let variant = &mug.variants[0];
    let result = CheckoutResult {
        lines: vec![ResultLine {
            id: "gid://shopify/CartLine/1".into(),
            variant_id: variant.id,
            product_id: mug.id,
            title: mug.title.clone(),
            variant_title: Some(variant.title.clone()),
            quantity: 2,
            unit_price: 2000,
            subtotal: 4000,
            total: 4000,
            requires_shipping: true,
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
            subtotal: 4000,
            discounts: 0,
            shipping: 500,
            tax: 0,
            total: 4500,
            due_now: 4500,
            due_later: 0,
            checkout_charge_amount: 4000,
            currency_code: "USD".into(),
        },
        ..CheckoutResult::default()
    };
    let address = Address {
        first_name: Some("Jane".into()),
        last_name: Some("Doe".into()),
        address1: Some("1 Main St".into()),
        city: Some("Portland".into()),
        country_code: Some("us".into()),
        province_code: Some("OR".into()),
        zip: Some("97201".into()),
        ..Address::default()
    };
    let id = format!("gid://shopify/Order/{number}");
    api.record_order(&PlacedOrder {
        id: Some(&id),
        number,
        processed_at: "2026-05-01T09:30:00Z".parse().unwrap(),
        result: &result,
        customer_id: Some(jane.id),
        email: Some(&jane.email),
        phone: None,
        shipping_address: Some(&address),
        note: None,
        attributes: &[],
        payment: Some("Credit card"),
    })
}

#[test]
fn a_checkout_order_is_delivered_then_fulfilled_refunded_and_edited() {
    let receiver = Receiver::start();
    let api = api();
    subscribe(
        &api,
        &receiver,
        &[
            "ORDERS_CREATE",
            "ORDER_TRANSACTIONS_CREATE",
            "ORDERS_PAID",
            "ORDERS_UPDATED",
            "ORDERS_FULFILLED",
            "FULFILLMENTS_CREATE",
            "REFUNDS_CREATE",
            "ORDERS_EDITED",
        ],
    );

    // A paid online order: the order, its payment, `orders/paid`.
    let order = place_order(&api);
    let delivered = receiver.take(3);
    let topics: Vec<&str> = delivered.iter().map(|(topic, _)| topic.as_str()).collect();
    assert_eq!(
        topics,
        ["orders/create", "order_transactions/create", "orders/paid"]
    );
    let (_, created) = &delivered[0];
    assert_eq!(created["admin_graphql_api_id"], json!(order));
    assert_eq!(created["financial_status"], json!("paid"));
    assert_eq!(created["fulfillment_status"], Value::Null);
    assert_eq!(created["line_items"][0]["quantity"], json!(2));
    let (_, transaction) = &delivered[1];
    assert_eq!(transaction["kind"], json!("sale"));
    assert_eq!(transaction["gateway"], json!("Credit card"));
    assert_eq!(transaction["amount"], json!("45.00"));
    assert_eq!(
        transaction["order_id"],
        json!(lsf_admin::state::parse_gid(&order).unwrap().1)
    );

    // The backend ships it.
    let fulfillment_order = graphql(
        &api,
        "query($id: ID!) { order(id: $id) { fulfillmentOrders(first: 1) { nodes { id } } } }",
        json!({"id": order}),
    )["data"]["order"]["fulfillmentOrders"]["nodes"][0]["id"]
        .clone();
    let shipped = graphql(
        &api,
        "mutation($fo: ID!) { fulfillmentCreate(fulfillment: {lineItemsByFulfillmentOrder: [{fulfillmentOrderId: $fo}], trackingInfo: {company: \"UPS\", number: \"1Z1\"}}) { fulfillment { id } userErrors { message } } }",
        json!({"fo": fulfillment_order}),
    );
    assert_eq!(
        shipped["data"]["fulfillmentCreate"]["userErrors"],
        json!([]),
        "{shipped}"
    );
    let delivered = receiver.take(3);
    let topics: Vec<&str> = delivered.iter().map(|(topic, _)| topic.as_str()).collect();
    assert_eq!(
        topics,
        ["fulfillments/create", "orders/fulfilled", "orders/updated"]
    );
    assert_eq!(delivered[0].1["tracking_number"], json!("1Z1"));
    assert_eq!(
        delivered[0].1["tracking_url"],
        json!("https://www.ups.com/WebTracking?loc=en_US&requester=ST&trackNums=1Z1")
    );
    assert_eq!(delivered[1].1["fulfillment_status"], json!("fulfilled"));

    // One mug comes back, refunded to the card.
    let detail = graphql(
        &api,
        "query($id: ID!) { order(id: $id) { lineItems(first: 1) { nodes { id } } transactions { id } } }",
        json!({"id": order}),
    );
    let line = detail["data"]["order"]["lineItems"]["nodes"][0]["id"].clone();
    let sale = detail["data"]["order"]["transactions"][0]["id"].clone();
    let refunded = graphql(
        &api,
        "mutation($input: RefundInput!) { refundCreate(input: $input) { refund { id } userErrors { field message } } }",
        json!({"input": {
            "orderId": order,
            "refundLineItems": [{"lineItemId": line, "quantity": 1, "restockType": "RETURN"}],
            "transactions": [{"orderId": order, "kind": "REFUND", "gateway": "Credit card", "parentId": sale, "amount": "20.00"}],
        }}),
    );
    assert_eq!(
        refunded["data"]["refundCreate"]["userErrors"],
        json!([]),
        "{refunded}"
    );
    let delivered = receiver.take(3);
    let topics: Vec<&str> = delivered.iter().map(|(topic, _)| topic.as_str()).collect();
    assert_eq!(
        topics,
        [
            "order_transactions/create",
            "refunds/create",
            "orders/updated"
        ]
    );
    assert_eq!(delivered[0].1["kind"], json!("refund"));
    let refund = &delivered[1].1;
    assert_eq!(refund["restock"], json!(true));
    assert_eq!(
        refund["refund_line_items"][0]["restock_type"],
        json!("return")
    );
    assert_eq!(refund["refund_line_items"][0]["quantity"], json!(1));
    assert_eq!(refund["transactions"][0]["amount"], json!("20.00"));
    assert_eq!(
        delivered[2].1["financial_status"],
        json!("partially_refunded")
    );

    // A custom item is added by an order edit.
    let begun = graphql(
        &api,
        "mutation($id: ID!) { orderEditBegin(id: $id) { calculatedOrder { id } } }",
        json!({"id": order}),
    );
    let calculated = begun["data"]["orderEditBegin"]["calculatedOrder"]["id"].clone();
    let added = graphql(
        &api,
        "mutation($id: ID!) { orderEditAddCustomItem(id: $id, title: \"Gift note\", quantity: 1, price: {amount: \"3.00\", currencyCode: USD}) { calculatedLineItem { id } userErrors { message } } }",
        json!({"id": calculated}),
    );
    assert_eq!(
        added["data"]["orderEditAddCustomItem"]["userErrors"],
        json!([]),
        "{added}"
    );
    let committed = graphql(
        &api,
        "mutation($id: ID!) { orderEditCommit(id: $id, staffNote: \"Added a note\") { order { lineItems(first: 5) { nodes { id title } } } userErrors { message } } }",
        json!({"id": calculated}),
    );
    let lines = committed["data"]["orderEditCommit"]["order"]["lineItems"]["nodes"].clone();
    assert_eq!(lines[1]["title"], json!("Gift note"), "{committed}");
    let delivered = receiver.take(2);
    let topics: Vec<&str> = delivered.iter().map(|(topic, _)| topic.as_str()).collect();
    assert_eq!(topics, ["orders/edited", "orders/updated"]);
    let edit = &delivered[0].1["order_edit"];
    assert_eq!(edit["staff_note"], json!("Added a note"));
    assert_eq!(
        edit["order_id"],
        json!(lsf_admin::state::parse_gid(&order).unwrap().1)
    );
    assert_eq!(
        edit["line_items"]["additions"],
        json!([{"id": lsf_admin::state::parse_gid(lines[1]["id"].as_str().unwrap()).unwrap().1, "delta": 1}])
    );
    assert_eq!(delivered[1].1["line_items"].as_array().unwrap().len(), 2);
}

/// The deliveries of the log, by topic, oldest first.
fn delivered_topics(api: &AdminApi) -> Vec<String> {
    let response = api
        .handle(&HttpRequest {
            method: "GET".into(),
            path: "/__lsf/webhooks".into(),
            query: Vec::new(),
            headers: HashMap::new(),
            body: Vec::new(),
        })
        .expect("the log is routed");
    let log: Value = serde_json::from_slice(&response.body).unwrap();
    let mut deliveries: Vec<&Value> = log["deliveries"].as_array().unwrap().iter().collect();
    deliveries.sort_by_key(|delivery| delivery["id"].as_u64());
    deliveries
        .iter()
        .map(|delivery| delivery["topic"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn after_an_uninstall_an_order_delivers_nothing_and_a_reinstall_brings_the_app_subscriptions_back()
{
    let receiver = Receiver::start();
    let app = lsf_checkout::app::load(&support::fixture("app")).unwrap();
    let api = AdminApi::new(
        &support::demo_store(),
        AdminOptions {
            app: Some(app),
            client_secret: SECRET.to_string(),
            app_url: Some(receiver.url.clone()),
            throttle: ThrottleSetting::Store,
            delivery: DeliveryOptions {
                timeout: Duration::from_secs(3),
                retry_delays: Vec::new(),
                ..DeliveryOptions::default()
            },
            token_seed: Some(7),
        },
    );
    let post = |path: &str, body: Value| {
        let response = api
            .handle(&HttpRequest {
                method: "POST".into(),
                path: path.into(),
                query: Vec::new(),
                headers: HashMap::from([(
                    "content-type".to_string(),
                    "application/json".to_string(),
                )]),
                body: if body.is_null() {
                    Vec::new()
                } else {
                    body.to_string().into_bytes()
                },
            })
            .expect("routed");
        assert_eq!(
            response.status,
            200,
            "{}",
            String::from_utf8_lossy(&response.body)
        );
        serde_json::from_slice::<Value>(&response.body).unwrap()
    };
    let control = |method: &str, path: &str| {
        assert_eq!(method, "POST");
        post(path, Value::Null)
    };
    // `orders/create` of `shopify.app.toml`, and two subscriptions of the shop.
    subscribe(&api, &receiver, &["ORDERS_CREATE", "APP_UNINSTALLED"]);
    place_order_numbered(&api, 1004).unwrap();
    let topics: Vec<String> = receiver
        .take(2)
        .into_iter()
        .map(|(topic, _)| topic)
        .collect();
    assert_eq!(topics, ["orders/create", "orders/create"]);

    // Uninstalled: `app/uninstalled` through the subscription the shop had, which is deleted.
    let uninstalled = control("POST", "/__lsf/admin/uninstall");
    assert_eq!(uninstalled["deletedSubscriptions"], json!(2));
    let (topic, payload) = receiver.next();
    assert_eq!(topic, "app/uninstalled");
    assert_eq!(payload["myshopify_domain"], json!(api.default_shop()));

    // An order placed at the checkout now delivers nothing, the app's subscription included.
    place_order_numbered(&api, 1005).expect("the shop still records it");
    assert_eq!(
        delivered_topics(&api),
        ["orders/create", "orders/create", "app/uninstalled"]
    );
    // Uninstalling again says nothing more.
    let again = control("POST", "/__lsf/admin/uninstall");
    assert_eq!(again["revoked"], json!(0));
    assert_eq!(delivered_topics(&api).len(), 3);

    // Installed again: the subscriptions of `shopify.app.toml` deliver, the shop's are gone.
    control("POST", "/__lsf/admin/install");
    let listed = graphql(
        &api,
        "{ webhookSubscriptions(first: 10) { nodes { topic } } }",
        json!({}),
    );
    assert_eq!(listed["data"]["webhookSubscriptions"]["nodes"], json!([]));
    place_order_numbered(&api, 1006).unwrap();
    let (topic, payload) = receiver.next();
    assert_eq!(topic, "orders/create");
    assert_eq!(payload["name"], json!("#1006"));
    assert_eq!(
        delivered_topics(&api),
        [
            "orders/create",
            "orders/create",
            "app/uninstalled",
            "orders/create"
        ]
    );

    // A token exchange installs the app as well (App Bridge runs once Shopify installed it).
    control("POST", "/__lsf/admin/uninstall");
    place_order_numbered(&api, 1007).unwrap();
    assert_eq!(delivered_topics(&api).len(), 4);
    let minted = post("/__lsf/admin/session-token", json!({}));
    post(
        "/admin/oauth/access_token",
        json!({
            "client_id": api.client_id(),
            "client_secret": SECRET,
            "grant_type": "urn:ietf:params:oauth:grant-type:token-exchange",
            "subject_token": minted["token"],
            "subject_token_type": "urn:ietf:params:oauth:token-type:id_token",
            "requested_token_type": "urn:shopify:params:oauth:token-type:offline-access-token",
        }),
    );
    place_order_numbered(&api, 1008).unwrap();
    let (topic, payload) = receiver.next();
    assert_eq!(topic, "orders/create");
    assert_eq!(payload["name"], json!("#1008"));
}
