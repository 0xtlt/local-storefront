//! Deliveries to a backend: a local HTTP server of the test's own.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::TimeZone;
use lsf_checkout::app::WebhookSubscriptionConfig;
use serde_json::{Value, json};

use super::*;
use crate::api::HttpRequest;
use crate::state::Event;

/// A request the backend received.
#[derive(Clone, Debug)]
struct Received {
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Received {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

/// A backend on a free port: `answer(n)` is the status and the delay of its answer to the n-th
/// request (from 0).
struct Backend {
    url: String,
    received: Arc<Mutex<Vec<Received>>>,
}

impl Backend {
    fn start(answer: impl Fn(usize) -> (u16, Duration) + Send + 'static) -> Backend {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let received = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&received);
        std::thread::spawn(move || {
            for (index, stream) in listener.incoming().enumerate() {
                let Ok(mut stream) = stream else { return };
                let Some(request) = read_request(&mut stream) else {
                    continue;
                };
                log.lock().unwrap().push(request);
                let (status, delay) = answer(index);
                std::thread::sleep(delay);
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} Answer\r\ncontent-length: 7\r\nconnection: close\r\n\r\nthanks!"
                );
            }
        });
        Backend { url, received }
    }

    fn received(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }
}

fn read_request(stream: &mut std::net::TcpStream) -> Option<Received> {
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 4096];
    let head_end = loop {
        let read = stream.read(&mut buffer).ok()?;
        if read == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..read]);
        if let Some(position) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break position;
        }
    };
    let head = String::from_utf8_lossy(&bytes[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let path = lines.next()?.split(' ').nth(1)?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);
    let mut body = bytes[head_end + 4..].to_vec();
    while body.len() < length {
        let read = stream.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..read]);
    }
    Some(Received {
        path,
        headers,
        body,
    })
}

const SHOP: &str = "my-shop.myshopify.com";
const SECRET: &str = "s3cr3t";

fn options() -> DeliveryOptions {
    DeliveryOptions {
        timeout: Duration::from_secs(3),
        retry_delays: Vec::new(),
        ..DeliveryOptions::default()
    }
}

fn webhooks(
    subscriptions: Vec<WebhookSubscriptionConfig>,
    app_url: &str,
    options: DeliveryOptions,
) -> Webhooks {
    let config = AppConfig {
        application_url: Some("https://tunnel.trycloudflare.com".to_string()),
        webhooks_api_version: Some(API_VERSION.to_string()),
        webhook_subscriptions: subscriptions,
        ..Default::default()
    };
    Webhooks::for_app(
        &config,
        Some(app_url.to_string()),
        SECRET.to_string(),
        options,
    )
}

fn subscription(topics: &[&str], uri: &str) -> WebhookSubscriptionConfig {
    WebhookSubscriptionConfig {
        topics: topics.iter().map(|topic| topic.to_string()).collect(),
        uri: uri.to_string(),
        ..Default::default()
    }
}

fn product() -> Record {
    json!({
        "__typename": "Product",
        "id": "gid://shopify/Product/1",
        "title": "Organic cotton t-shirt",
        "handle": "organic-cotton-t-shirt",
        "status": "ACTIVE",
        "tags": ["cotton"],
        "updatedAt": "2026-01-02T00:00:00Z",
        "variants": [],
    })
    .as_object()
    .unwrap()
    .clone()
}

fn event(topic: &str, gid: &str) -> Event {
    Event {
        topic: topic.to_string(),
        shop: SHOP.to_string(),
        gid: gid.to_string(),
        at: Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap(),
        id: "event-1".to_string(),
        snapshot: None,
    }
}

fn lookup() -> Records {
    Records::of([product()], None)
}

fn wait_for(webhooks: &Webhooks, count: usize) -> Vec<Delivery> {
    let waited =
        webhooks
            .deliveries()
            .wait(&DeliveryFilter::default(), count, Duration::from_secs(10));
    assert!(waited.complete, "{:#?}", waited.deliveries);
    waited.deliveries
}

#[test]
fn an_event_is_delivered_signed_to_each_subscription_of_its_topic() {
    let backend = Backend::start(|_| (200, Duration::ZERO));
    let named = WebhookSubscriptionConfig {
        name: Some("product-sync".to_string()),
        ..subscription(&["products/update"], "/webhooks/products")
    };
    let trimmed = WebhookSubscriptionConfig {
        include_fields: vec!["id".to_string(), "title".to_string()],
        ..subscription(&["products/update", "products/create"], "/webhooks/trimmed")
    };
    let other_topic = subscription(&["orders/create"], "/webhooks/orders");
    let webhooks = webhooks(vec![named, trimmed, other_topic], &backend.url, options());

    let ids = webhooks.emit(
        &event("products/update", "gid://shopify/Product/1"),
        &lookup(),
    );
    assert_eq!(ids.len(), 2);
    let deliveries = wait_for(&webhooks, 2);
    assert!(
        deliveries
            .iter()
            .all(|delivery| delivery.state == DeliveryState::Delivered),
        "{deliveries:#?}"
    );
    assert_eq!(deliveries[0].attempts[0].status, Some(200));
    assert_eq!(
        deliveries[0].attempts[0].response.as_deref(),
        Some("thanks!")
    );

    let mut received = backend.received();
    received.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(received.len(), 2);
    let (full, cut) = (&received[0], &received[1]);
    assert_eq!(full.path, "/webhooks/products");
    assert_eq!(cut.path, "/webhooks/trimmed");
    for request in [full, cut] {
        assert_eq!(request.header("x-shopify-topic"), Some("products/update"));
        assert_eq!(request.header("x-shopify-shop-domain"), Some(SHOP));
        assert_eq!(request.header("x-shopify-api-version"), Some("2026-10"));
        assert_eq!(request.header("x-shopify-event-id"), Some("event-1"));
        assert_eq!(
            request.header("x-shopify-triggered-at"),
            Some("2026-01-02T03:04:05.000000Z")
        );
        assert_eq!(request.header("content-type"), Some("application/json"));
        assert_eq!(request.header("user-agent"), Some("Shopify-Captain-Hook"));
        let signature = request.header("x-shopify-hmac-sha256").unwrap();
        assert!(sign::verify(SECRET, &request.body, signature));
    }
    // One webhook id per delivery, one event id for both.
    assert_ne!(
        full.header("x-shopify-webhook-id"),
        cut.header("x-shopify-webhook-id")
    );
    assert_eq!(full.header("x-shopify-name"), Some("product-sync"));
    assert_eq!(cut.header("x-shopify-name"), None);
    assert_eq!(full.json()["tags"], "cotton");
    assert_eq!(full.json()["status"], "active");
    assert_eq!(
        cut.json(),
        json!({"id": 1, "title": "Organic cotton t-shirt"})
    );
    // The log has what was sent.
    let logged = deliveries
        .iter()
        .find(|delivery| {
            delivery.url.as_deref() == Some(&format!("{}/webhooks/trimmed", backend.url))
        })
        .unwrap();
    let request = logged.request.as_ref().unwrap();
    assert_eq!(request.body.as_bytes(), cut.body);
    assert_eq!(request.headers["X-Shopify-Topic"], "products/update");
}

#[test]
fn a_delivery_is_sent_from_a_worker_not_from_the_caller() {
    let backend = Backend::start(|_| (200, Duration::from_millis(600)));
    let webhooks = webhooks(
        vec![subscription(&["products/update"], "/hooks")],
        &backend.url,
        options(),
    );
    let start = Instant::now();
    webhooks.emit(
        &event("products/update", "gid://shopify/Product/1"),
        &lookup(),
    );
    assert!(start.elapsed() < Duration::from_millis(300));
    assert_eq!(
        webhooks.deliveries().list(&DeliveryFilter::default())[0].state,
        DeliveryState::Pending
    );
    assert_eq!(wait_for(&webhooks, 1)[0].state, DeliveryState::Delivered);
}

#[test]
fn a_failing_backend_is_retried_then_the_delivery_fails() {
    // Fails twice, then answers.
    let backend = Backend::start(|index| (if index < 2 { 500 } else { 200 }, Duration::ZERO));
    let retrying = DeliveryOptions {
        retry_delays: vec![Duration::from_millis(10), Duration::from_millis(10)],
        ..options()
    };
    let webhooks = webhooks(
        vec![subscription(&["products/update"], "/hooks")],
        &backend.url,
        retrying,
    );
    webhooks.emit(
        &event("products/update", "gid://shopify/Product/1"),
        &lookup(),
    );
    let delivery = &wait_for(&webhooks, 1)[0];
    assert_eq!(delivery.state, DeliveryState::Delivered);
    let statuses: Vec<Option<u16>> = delivery
        .attempts
        .iter()
        .map(|attempt| attempt.status)
        .collect();
    assert_eq!(statuses, [Some(500), Some(500), Some(200)]);
    assert_eq!(delivery.attempts[0].error.as_deref(), Some("answered 500"));
    // Each attempt is the same request: the same webhook id.
    let received = backend.received();
    assert_eq!(received.len(), 3);
    assert_eq!(
        received[0].header("x-shopify-webhook-id"),
        received[2].header("x-shopify-webhook-id")
    );

    // Always failing: as many attempts as retries and one, then failed.
    let backend = Backend::start(|_| (503, Duration::ZERO));
    let once_more = DeliveryOptions {
        retry_delays: vec![Duration::from_millis(10)],
        ..options()
    };
    let webhooks = self::webhooks(
        vec![subscription(&["products/update"], "/hooks")],
        &backend.url,
        once_more,
    );
    webhooks.emit(
        &event("products/update", "gid://shopify/Product/1"),
        &lookup(),
    );
    let delivery = &wait_for(&webhooks, 1)[0];
    assert_eq!(delivery.state, DeliveryState::Failed);
    assert_eq!(delivery.attempts.len(), 2);
    assert_eq!(
        delivery.reason.as_deref(),
        Some("answered 503 (2 attempts)")
    );
}

#[test]
fn a_redirect_is_a_failure_as_on_shopify() {
    let backend = Backend::start(|_| (302, Duration::ZERO));
    let webhooks = webhooks(
        vec![subscription(&["products/update"], "/hooks")],
        &backend.url,
        options(),
    );
    webhooks.emit(
        &event("products/update", "gid://shopify/Product/1"),
        &lookup(),
    );
    let delivery = &wait_for(&webhooks, 1)[0];
    assert_eq!(delivery.state, DeliveryState::Failed);
    assert!(
        delivery.reason.as_deref().unwrap().contains("redirect"),
        "{delivery:?}"
    );
}

#[test]
fn a_backend_slower_than_the_timeout_fails_the_delivery() {
    let backend = Backend::start(|_| (200, Duration::from_millis(1500)));
    let impatient = DeliveryOptions {
        timeout: Duration::from_millis(200),
        ..options()
    };
    let webhooks = webhooks(
        vec![subscription(&["products/update"], "/hooks")],
        &backend.url,
        impatient,
    );
    webhooks.emit(
        &event("products/update", "gid://shopify/Product/1"),
        &lookup(),
    );
    let delivery = &wait_for(&webhooks, 1)[0];
    assert_eq!(delivery.state, DeliveryState::Failed);
    let attempt = &delivery.attempts[0];
    assert_eq!(attempt.status, None);
    assert!(
        attempt
            .error
            .as_deref()
            .unwrap()
            .contains("no answer within 0.2 s"),
        "{attempt:?}"
    );
    assert!(attempt.duration_ms < 1500, "{attempt:?}");
}

#[test]
fn a_backend_that_is_not_listening_fails_the_delivery() {
    // A port that was free a moment ago.
    let url = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    let webhooks = webhooks(
        vec![subscription(&["products/update"], "/hooks")],
        &url,
        options(),
    );
    webhooks.emit(
        &event("products/update", "gid://shopify/Product/1"),
        &lookup(),
    );
    let delivery = &wait_for(&webhooks, 1)[0];
    assert_eq!(delivery.state, DeliveryState::Failed);
    let reason = delivery.reason.as_deref().unwrap();
    assert!(
        reason.contains("is the backend of the app listening on"),
        "{reason}"
    );
}

#[test]
fn a_wait_ends_at_its_timeout_and_says_so() {
    let webhooks = webhooks(Vec::new(), "http://localhost:1", options());
    let start = Instant::now();
    let waited =
        webhooks
            .deliveries()
            .wait(&DeliveryFilter::default(), 1, Duration::from_millis(150));
    assert!(!waited.complete);
    assert!(waited.deliveries.is_empty());
    assert!(start.elapsed() >= Duration::from_millis(150));
    // Nothing to wait for: done at once.
    let waited = webhooks
        .deliveries()
        .wait(&DeliveryFilter::default(), 0, Duration::from_secs(10));
    assert!(waited.complete);
}

#[test]
fn what_cannot_be_sent_is_logged_with_why() {
    let backend = Backend::start(|_| (200, Duration::ZERO));
    let webhooks = webhooks(
        vec![
            subscription(&["carts/create"], "/hooks"),
            subscription(&["products/update"], "pubsub://project:topic"),
            subscription(
                &["products/update"],
                "arn:aws:events:us-east-1::event-source/aws.partner/shopify.com/1/x",
            ),
        ],
        &backend.url,
        options(),
    );
    webhooks.emit(&event("carts/create", "gid://shopify/Cart/1"), &lookup());
    webhooks.emit(
        &event("products/update", "gid://shopify/Product/1"),
        &lookup(),
    );
    // A record the shop does not have.
    webhooks.emit(
        &event("products/update", "gid://shopify/Product/404"),
        &lookup(),
    );
    let deliveries = webhooks.deliveries().list(&DeliveryFilter::default());
    assert_eq!(deliveries.len(), 5);
    assert!(
        deliveries
            .iter()
            .all(|delivery| delivery.state == DeliveryState::NotSent)
    );
    let reasons: Vec<&str> = deliveries
        .iter()
        .map(|delivery| delivery.reason.as_deref().unwrap())
        .collect();
    assert_eq!(
        reasons[0],
        "lsf does not build the payload of `carts/create`"
    );
    assert!(reasons[1].contains("Pub/Sub"), "{}", reasons[1]);
    assert!(reasons[2].contains("EventBridge"), "{}", reasons[2]);
    assert!(reasons[3].contains("`gid://shopify/Product/404` is not a record of the shop"));
    // What would have been sent to Pub/Sub is in the log all the same.
    assert!(deliveries[1].request.is_some());
    assert!(deliveries[1].url.is_none());
    assert!(backend.received().is_empty());

    // Without an app URL, a relative uri goes nowhere.
    let config = AppConfig {
        webhook_subscriptions: vec![subscription(&["products/update"], "/hooks")],
        ..Default::default()
    };
    let nowhere = Webhooks::for_app(&config, None, SECRET.to_string(), options());
    nowhere.emit(
        &event("products/update", "gid://shopify/Product/1"),
        &lookup(),
    );
    let delivery = &nowhere.deliveries().list(&DeliveryFilter::default())[0];
    assert!(delivery.reason.as_deref().unwrap().contains("--app-url"));
}

#[test]
fn a_filter_is_noted_until_an_evaluator_is_plugged_in() {
    let backend = Backend::start(|_| (200, Duration::ZERO));
    let filtered = || WebhookSubscriptionConfig {
        filter: Some("status:active".to_string()),
        ..subscription(&["products/update"], "/hooks")
    };
    let unevaluated = webhooks(vec![filtered()], &backend.url, options());
    unevaluated.emit(
        &event("products/update", "gid://shopify/Product/1"),
        &lookup(),
    );
    let delivery = &wait_for(&unevaluated, 1)[0];
    assert_eq!(delivery.state, DeliveryState::Delivered);
    assert_eq!(
        delivery.notes,
        ["the filter `status:active` was not evaluated: lsf delivered as if it matched"]
    );

    // The evaluator sees the payload after `include_fields`.
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&seen);
    let evaluator = move |filter: &str, payload: &Value| -> Result<bool, String> {
        log.lock()
            .unwrap()
            .push((filter.to_string(), payload.clone()));
        match payload["status"].as_str() {
            Some(status) => Ok(filter == format!("status:{status}")),
            None => Err("the payload has no `status`".to_string()),
        }
    };
    let evaluated = webhooks(
        vec![
            filtered(),
            WebhookSubscriptionConfig {
                filter: Some("status:draft".to_string()),
                ..subscription(&["products/update"], "/hooks")
            },
            WebhookSubscriptionConfig {
                filter: Some("status:active".to_string()),
                include_fields: vec!["id".to_string()],
                ..subscription(&["products/update"], "/hooks")
            },
        ],
        &backend.url,
        options(),
    )
    .with_filter(Arc::new(evaluator));
    evaluated.emit(
        &event("products/update", "gid://shopify/Product/1"),
        &lookup(),
    );
    let deliveries = wait_for(&evaluated, 3);
    let states: Vec<DeliveryState> = deliveries.iter().map(|delivery| delivery.state).collect();
    assert_eq!(
        states,
        [
            DeliveryState::Delivered,
            DeliveryState::NotSent,
            DeliveryState::NotSent
        ]
    );
    assert!(deliveries[0].notes.is_empty());
    assert_eq!(
        deliveries[1].reason.as_deref(),
        Some("the filter `status:draft` does not match the payload")
    );
    assert!(
        deliveries[2]
            .reason
            .as_deref()
            .unwrap()
            .contains("cannot be evaluated (the payload has no `status`)")
    );
    assert_eq!(seen.lock().unwrap()[2].1, json!({"id": 1}));
}

#[test]
fn a_delete_is_built_from_the_snapshot_of_the_record() {
    let backend = Backend::start(|_| (200, Duration::ZERO));
    let webhooks = webhooks(
        vec![subscription(
            &["products/delete", "discounts/delete"],
            "/hooks",
        )],
        &backend.url,
        options(),
    );
    let mut deleted = event("products/delete", "gid://shopify/Product/1");
    deleted.snapshot = Some(product());
    // The record is gone from the state.
    webhooks.emit(&deleted, &Records::default());
    let mut discount = event("discounts/delete", "gid://shopify/DiscountCodeNode/4");
    discount.snapshot = Some(
        json!({"id": "gid://shopify/DiscountCodeNode/4"})
            .as_object()
            .unwrap()
            .clone(),
    );
    webhooks.emit(&discount, &Records::default());
    wait_for(&webhooks, 2);
    let mut bodies: Vec<Value> = backend.received().iter().map(Received::json).collect();
    bodies.sort_by_key(|body| body.get("id").is_none());
    assert_eq!(bodies[0], json!({"id": 1}));
    assert_eq!(
        bodies[1],
        json!({"admin_graphql_api_id": "gid://shopify/DiscountCodeNode/4", "deleted_at": "2026-01-02T03:04:05Z"})
    );
}

#[test]
fn scopes_update_reports_the_scopes_before_and_after() {
    let backend = Backend::start(|_| (200, Duration::ZERO));
    let webhooks = webhooks(
        vec![subscription(&["app/scopes_update"], "/hooks")],
        &backend.url,
        options(),
    );
    let installation = |scopes: &[&str]| {
        json!({
            "id": "gid://shopify/AppInstallation/3",
            "accessScopes": scopes.iter().map(|handle| json!({"handle": handle})).collect::<Vec<_>>(),
            "updatedAt": "2026-01-02T03:04:05Z",
        })
        .as_object()
        .unwrap()
        .clone()
    };
    let mut changed = event("app/scopes_update", "gid://shopify/AppInstallation/3");
    changed.snapshot = Some(installation(&["read_products"]));
    let shop = json!({"id": "gid://shopify/Shop/9", "myshopifyDomain": SHOP})
        .as_object()
        .unwrap()
        .clone();
    webhooks.emit(
        &changed,
        &Records::of(
            [installation(&["read_products", "write_orders"])],
            Some(shop),
        ),
    );
    wait_for(&webhooks, 1);
    assert_eq!(
        backend.received()[0].json(),
        json!({
            "id": 3,
            "shop_id": "gid://shopify/Shop/9",
            "previous": ["read_products"],
            "current": ["read_products", "write_orders"],
            "updated_at": "2026-01-02T03:04:05.000Z",
        })
    );
}

#[test]
fn compliance_topics_are_sent_on_demand_only() {
    let backend = Backend::start(|_| (200, Duration::ZERO));
    let webhooks = webhooks(
        vec![WebhookSubscriptionConfig {
            compliance_topics: vec!["shop/redact".to_string()],
            uri: "/compliance".to_string(),
            ..Default::default()
        }],
        &backend.url,
        options(),
    );
    assert!(
        webhooks
            .emit(&event("shop/redact", "gid://shopify/Shop/1"), &lookup())
            .is_empty()
    );
    let payload = json!({"shop_id": 1, "shop_domain": SHOP});
    let ids = webhooks.trigger(&Trigger {
        topic: "shop/redact",
        shop: SHOP,
        payload: &payload,
        address: None,
        at: Utc::now(),
    });
    assert_eq!(ids.len(), 1);
    wait_for(&webhooks, 1);
    assert_eq!(backend.received()[0].path, "/compliance");
    assert_eq!(backend.received()[0].json(), payload);
}

#[test]
fn a_trigger_to_an_address_sends_the_whole_payload_there() {
    let backend = Backend::start(|_| (200, Duration::ZERO));
    // A subscription with `include_fields`, which an address ignores.
    let webhooks = webhooks(
        vec![WebhookSubscriptionConfig {
            include_fields: vec!["id".to_string()],
            ..subscription(&["orders/create"], "/hooks")
        }],
        "http://localhost:1",
        options(),
    );
    let payload = json!({"id": 1, "name": "#1001"});
    let address = format!("{}/direct", backend.url);
    let ids = webhooks.trigger(&Trigger {
        topic: "orders/create",
        shop: SHOP,
        payload: &payload,
        address: Some(&address),
        at: Utc::now(),
    });
    assert_eq!(ids.len(), 1);
    let delivery = &wait_for(&webhooks, 1)[0];
    assert_eq!(delivery.state, DeliveryState::Delivered);
    assert!(delivery.subscription.is_none());
    assert_eq!(backend.received()[0].path, "/direct");
    assert_eq!(backend.received()[0].json(), payload);
    // No subscription to the topic, no address: nothing.
    assert!(
        webhooks
            .trigger(&Trigger {
                topic: "products/create",
                shop: SHOP,
                payload: &payload,
                address: None,
                at: Utc::now(),
            })
            .is_empty()
    );
}

#[test]
fn a_subscription_of_the_admin_api_is_delivered_for_its_shop_only() {
    let backend = Backend::start(|_| (200, Duration::ZERO));
    let webhooks = webhooks(Vec::new(), "http://localhost:1", options());
    let created = webhooks
        .registry()
        .create(
            SHOP,
            "PRODUCTS_UPDATE",
            SubscriptionInput {
                uri: Some(format!("{}/api-hooks", backend.url)),
                ..Default::default()
            },
            API_VERSION,
            Utc::now(),
        )
        .unwrap();
    let mut elsewhere = event("products/update", "gid://shopify/Product/1");
    elsewhere.shop = "other.myshopify.com".to_string();
    assert!(webhooks.emit(&elsewhere, &lookup()).is_empty());
    assert_eq!(
        webhooks
            .emit(
                &event("products/update", "gid://shopify/Product/1"),
                &lookup()
            )
            .len(),
        1
    );
    let delivery = &wait_for(&webhooks, 1)[0];
    assert_eq!(delivery.state, DeliveryState::Delivered);
    assert_eq!(delivery.subscription.as_ref().unwrap().id, created.id);
    assert_eq!(backend.received()[0].path, "/api-hooks");
}

#[test]
fn the_summary_says_where_the_webhooks_go() {
    let webhooks = webhooks(
        vec![
            subscription(&["orders/create", "orders/paid"], "/hooks"),
            subscription(&["app/uninstalled"], "/x"),
        ],
        "http://localhost:3000",
        options(),
    );
    assert_eq!(
        webhooks.summary(),
        "3 subscriptions of shopify.app.toml → http://localhost:3000"
    );
}

// --- /__lsf/webhooks ---

fn request(method: &str, path: &str, query: &[(&str, &str)], body: Value) -> HttpRequest {
    HttpRequest {
        method: method.to_string(),
        path: path.to_string(),
        query: query
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
        body: if body.is_null() {
            Vec::new()
        } else {
            serde_json::to_vec(&body).unwrap()
        },
        ..Default::default()
    }
}

fn body(response: &crate::api::HttpResponse) -> Value {
    serde_json::from_slice(&response.body).unwrap()
}

/// What the api layer gives the trigger: a product of the store for `products/*`, or the
/// trigger's own payload.
fn build(trigger: &TriggerRequest) -> Result<(String, Value), TriggerError> {
    if let Some(payload) = &trigger.payload {
        let shop = trigger.shop.clone().unwrap_or_else(|| SHOP.to_string());
        return Ok((shop, payload.clone()));
    }
    match trigger.topic.as_str() {
        "products/update" => {
            let record = product();
            if trigger
                .id
                .as_deref()
                .is_some_and(|id| id != "gid://shopify/Product/1")
            {
                return Err(TriggerError::NotFound(format!(
                    "no record {}",
                    trigger.id.as_deref().unwrap_or("")
                )));
            }
            Ok((
                SHOP.to_string(),
                payload(&trigger.topic, &record, &lookup()).unwrap(),
            ))
        }
        topic => Err(TriggerError::NotBuilt(format!(
            "lsf does not build `{topic}`"
        ))),
    }
}

#[test]
fn the_control_endpoints_list_wait_trigger_and_forget() {
    let backend = Backend::start(|_| (200, Duration::ZERO));
    let webhooks = webhooks(
        vec![
            subscription(&["products/update"], "/hooks"),
            subscription(&["orders/create"], "/orders"),
        ],
        &backend.url,
        options(),
    );
    let handle = |request: HttpRequest| webhooks.handle(&request, &build).unwrap();

    // A trigger, built by the api layer, answered once delivered.
    let triggered = handle(request(
        "POST",
        "/__lsf/webhooks/trigger",
        &[],
        json!({"topic": "PRODUCTS_UPDATE", "wait": true}),
    ));
    assert_eq!(
        triggered.status,
        200,
        "{}",
        String::from_utf8_lossy(&triggered.body)
    );
    let triggered = body(&triggered);
    assert_eq!(triggered["deliveries"][0]["state"], "delivered");
    assert_eq!(triggered["deliveries"][0]["topic"], "products/update");
    assert_eq!(
        backend.received()[0].json()["title"],
        "Organic cotton t-shirt"
    );

    // A payload given as is.
    let given = handle(request(
        "POST",
        "/__lsf/webhooks/trigger",
        &[],
        json!({"topic": "orders/create", "shop": SHOP, "payload": {"id": 7}}),
    ));
    assert_eq!(given.status, 200);

    // Listing, by topic, waiting.
    let waited = handle(request(
        "GET",
        "/__lsf/webhooks",
        &[("topic", "orders/create"), ("wait", "1")],
        Value::Null,
    ));
    assert_eq!(waited.status, 200);
    let waited = body(&waited);
    assert_eq!(waited["count"], 1);
    assert_eq!(waited["waited"]["complete"], true);
    assert_eq!(waited["deliveries"][0]["request"]["body"], r#"{"id":7}"#);
    let all = body(&handle(request("GET", "/__lsf/webhooks", &[], Value::Null)));
    assert_eq!(all["count"], 2);
    let delivered = body(&handle(request(
        "GET",
        "/__lsf/webhooks",
        &[("state", "delivered")],
        Value::Null,
    )));
    assert_eq!(delivered["count"], 2);

    // Waiting for more than will come: 408, with what there is.
    let late = handle(request(
        "GET",
        "/__lsf/webhooks",
        &[("wait", "5"), ("timeout", "50")],
        Value::Null,
    ));
    assert_eq!(late.status, 408);
    assert_eq!(body(&late)["waited"]["complete"], false);
    assert_eq!(body(&late)["count"], 2);

    // Forgetting.
    let cleared = handle(request("DELETE", "/__lsf/webhooks", &[], Value::Null));
    assert_eq!(body(&cleared), json!({"ok": true, "deliveries": 2}));
    assert_eq!(
        body(&handle(request("GET", "/__lsf/webhooks", &[], Value::Null)))["count"],
        0
    );
}

#[test]
fn the_control_endpoints_say_what_is_wrong() {
    let webhooks = webhooks(
        vec![subscription(&["products/update"], "/hooks")],
        "http://localhost:1",
        options(),
    );
    let handle = |request: HttpRequest| webhooks.handle(&request, &build).unwrap();
    let error = |response: crate::api::HttpResponse| {
        (
            response.status,
            body(&response)["error"].as_str().unwrap().to_string(),
        )
    };

    let (status, message) = error(handle(request(
        "POST",
        "/__lsf/webhooks/trigger",
        &[],
        json!({"topic": "products/updated"}),
    )));
    assert_eq!(status, 422);
    assert!(
        message.contains("did you mean `products/update`?"),
        "{message}"
    );
    let (status, message) = error(handle(request(
        "POST",
        "/__lsf/webhooks/trigger",
        &[],
        json!({"topic": "products/update", "id": "gid://shopify/Product/9"}),
    )));
    assert_eq!(
        (status, message.as_str()),
        (404, "no record gid://shopify/Product/9")
    );
    let (status, _) = error(handle(request(
        "POST",
        "/__lsf/webhooks/trigger",
        &[],
        json!({"topic": "carts/create"}),
    )));
    assert_eq!(status, 422);
    // No subscription to the topic.
    let (status, message) = error(handle(request(
        "POST",
        "/__lsf/webhooks/trigger",
        &[],
        json!({"topic": "orders/create", "shop": SHOP, "payload": {}}),
    )));
    assert_eq!(status, 404);
    assert!(
        message.contains("no subscription to `orders/create`"),
        "{message}"
    );
    let (status, _) = error(handle(request(
        "POST",
        "/__lsf/webhooks/trigger",
        &[],
        json!({"topic": "x", "extra": 1}),
    )));
    assert_eq!(status, 422);
    let (status, _) = error(handle(request(
        "GET",
        "/__lsf/webhooks",
        &[("state", "lost")],
        Value::Null,
    )));
    assert_eq!(status, 422);
    let (status, _) = error(handle(request(
        "GET",
        "/__lsf/webhooks",
        &[("wait", "many")],
        Value::Null,
    )));
    assert_eq!(status, 422);
    let (status, _) = error(handle(request("PUT", "/__lsf/webhooks", &[], Value::Null)));
    assert_eq!(status, 405);
    assert!(
        webhooks
            .handle(&request("GET", "/__lsf/other", &[], Value::Null), &build)
            .is_none()
    );
}
