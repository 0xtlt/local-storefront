//! The catalog's mutations from the outside: through `AdminApi::handle` with the webhooks
//! delivered to a listener of the test, the records they make checked against the schema, and
//! what they change projected into the store the storefront sees.

mod support;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration;

use lsf_admin::api::{AdminApi, AdminOptions, HttpRequest, ThrottleSetting};
use lsf_admin::auth::signing::verify_webhook_hmac;
use lsf_admin::domains::catalog;
use lsf_admin::graphql::{self, Request};
use lsf_admin::resolve::{Registry, ShopResolver};
use lsf_admin::state::ShopState;
use lsf_admin::sync;
use lsf_admin::webhooks::DeliveryOptions;
use lsf_core::store::load::{DataSource, LoadOptions, load_with_overlays};
use serde_json::{Value, json};
use support::shape::Schema;

const SECRET: &str = "test-secret";
const GRAPHQL: &str = "/admin/api/2026-10/graphql.json";

// --- Through the HTTP API ---

fn demo_api() -> AdminApi {
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

fn graphql_request(api: &AdminApi, token: &str, query: &str, variables: Value) -> Value {
    let body = json!({"query": query, "variables": variables}).to_string();
    let response = api
        .handle(&HttpRequest {
            method: "POST".into(),
            path: GRAPHQL.into(),
            query: Vec::new(),
            headers: HashMap::from([
                ("content-type".to_string(), "application/json".to_string()),
                ("x-shopify-access-token".to_string(), token.to_string()),
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
}

fn subscribe(api: &AdminApi, token: &str, topic: &str, receiver: &Receiver) {
    let answer = graphql_request(
        api,
        token,
        "mutation($topic: WebhookSubscriptionTopic!, $uri: String!) { webhookSubscriptionCreate(topic: $topic, webhookSubscription: {uri: $uri}) { userErrors { message } } }",
        json!({"topic": topic, "uri": format!("{}/webhooks", receiver.url)}),
    );
    assert_eq!(
        answer["data"]["webhookSubscriptionCreate"]["userErrors"],
        json!([]),
        "{answer}"
    );
}

#[test]
fn a_backend_syncs_a_product_and_its_stock_and_gets_the_webhooks() {
    let receiver = Receiver::start();
    let api = demo_api();
    let token = api.token(&api.default_shop());
    for topic in [
        "PRODUCTS_CREATE",
        "INVENTORY_LEVELS_UPDATE",
        "PRODUCT_PUBLICATIONS_CREATE",
        "VARIANTS_OUT_OF_STOCK",
    ] {
        subscribe(&api, &token, topic, &receiver);
    }
    let locations = graphql_request(
        &api,
        &token,
        "{ locations(first: 1) { nodes { id } } }",
        Value::Null,
    );
    let location = locations["data"]["locations"]["nodes"][0]["id"].clone();

    // `productSet`, as a PIM integration sends it.
    let created = graphql_request(
        &api,
        &token,
        r#"mutation($input: ProductSetInput!) { productSet(input: $input) {
            product { id variants(first: 2) { nodes { id inventoryItem { id } } } } userErrors { field message } } }"#,
        json!({"input": {
            "title": "Trail shoe", "vendor": "Acme", "productType": "Shoes",
            "productOptions": [{"name": "Size", "values": [{"name": "42"}, {"name": "43"}]}],
            "variants": [
                {"optionValues": [{"optionName": "Size", "name": "42"}], "price": "120.00", "sku": "TRAIL-42",
                 "inventoryItem": {"tracked": true}, "inventoryQuantities": [{"locationId": location, "name": "available", "quantity": 2}]},
                {"optionValues": [{"optionName": "Size", "name": "43"}], "price": "120.00", "sku": "TRAIL-43",
                 "inventoryItem": {"tracked": true}, "inventoryQuantities": [{"locationId": location, "name": "available", "quantity": 1}]},
            ],
        }}),
    );
    let set = &created["data"]["productSet"];
    assert_eq!(set["userErrors"], json!([]), "{created}");
    let product = set["product"]["id"].as_str().unwrap().to_string();
    let item = set["product"]["variants"]["nodes"][1]["inventoryItem"]["id"].clone();
    let (topic, payload) = receiver.next();
    assert_eq!(topic, "products/create");
    assert_eq!(payload["title"], json!("Trail shoe"));
    assert_eq!(payload["variants"].as_array().unwrap().len(), 2);
    assert_eq!(payload["variants"][0]["sku"], json!("TRAIL-42"));
    assert_eq!(payload["variants"][0]["price"], json!("120.00"));

    // An order taken elsewhere: the last 43 goes.
    let adjusted = graphql_request(
        &api,
        &token,
        r#"mutation($input: InventoryAdjustQuantitiesInput!) { inventoryAdjustQuantities(input: $input) @idempotent(key: "order-1001") {
            inventoryAdjustmentGroup { changes { name delta } } userErrors { message code } } }"#,
        json!({"input": {"reason": "correction", "name": "available", "changes": [
            {"inventoryItemId": item, "locationId": location, "delta": -1, "changeFromQuantity": 1}]}}),
    );
    assert_eq!(
        adjusted["data"]["inventoryAdjustQuantities"]["userErrors"],
        json!([]),
        "{adjusted}"
    );
    let mut received: HashMap<String, Value> = HashMap::new();
    for _ in 0..2 {
        let (topic, payload) = receiver.next();
        received.insert(topic, payload);
    }
    let level = &received["inventory_levels/update"];
    assert_eq!(level["available"], json!(0));
    assert_eq!(
        level["location_id"],
        json!(
            lsf_admin::state::parse_gid(location.as_str().unwrap())
                .unwrap()
                .1
        )
    );
    let variant = &received["variants/out_of_stock"];
    assert_eq!(variant["sku"], json!("TRAIL-43"));
    assert_eq!(variant["inventory_quantity"], json!(0));

    // Published on the Online Store.
    let publications = graphql_request(
        &api,
        &token,
        "{ publications(first: 1) { nodes { id } } }",
        Value::Null,
    );
    let publication = publications["data"]["publications"]["nodes"][0]["id"].clone();
    let published = graphql_request(
        &api,
        &token,
        "mutation($id: ID!, $publication: ID!) { publishablePublish(id: $id, input: [{publicationId: $publication}]) { userErrors { message } } }",
        json!({"id": product, "publication": publication}),
    );
    assert_eq!(
        published["data"]["publishablePublish"]["userErrors"],
        json!([])
    );
    let (topic, payload) = receiver.next();
    assert_eq!(topic, "product_publications/create");
    assert_eq!(
        payload["product_id"],
        json!(lsf_admin::state::parse_gid(&product).unwrap().1)
    );
    assert_eq!(
        payload["publication_id"],
        json!(
            lsf_admin::state::parse_gid(publication.as_str().unwrap())
                .unwrap()
                .1
        )
    );
    assert_eq!(payload["published"], json!(true));
}

// --- On the state ---

fn run(shop: &mut ShopState, query: &str, variables: Value) -> Value {
    let mut resolver = ShopResolver::new(shop, Registry::standard());
    let mut request = Request::new(query);
    if let Value::Object(variables) = variables {
        request.variables = variables;
    }
    let answer = graphql::run(lsf_admin::schema::admin(), &mut resolver, &request, None);
    let answer = answer.to_json();
    assert!(answer.get("errors").is_none(), "{answer}");
    answer
}

/// The payload of a mutation, checked to have no user errors.
fn ok(answer: &Value, name: &str) -> Value {
    let payload = answer["data"][name].clone();
    for key in [
        "userErrors",
        "mediaUserErrors",
        "locationDeactivateUserErrors",
    ] {
        if let Some(errors) = payload.get(key) {
            assert_eq!(errors, &json!([]), "{name}: {answer}");
        }
    }
    payload
}

/// Runs one of each kind of catalog mutation on a shop: the records they leave.
fn exercise(shop: &mut ShopState) -> String {
    let location = shop.keys("Location").next().unwrap().to_string();
    let created = ok(
        &run(
            shop,
            r#"mutation($input: ProductSetInput!) { productSet(input: $input) { product { id variants(first: 3) { nodes { id } } } userErrors { message } } }"#,
            json!({"input": {
                "title": "Linen apron", "productType": "Kitchen", "tags": ["linen", "sale"], "status": "ACTIVE",
                "productOptions": [{"name": "Color", "values": [{"name": "Sand"}, {"name": "Rust"}]}],
                "variants": [
                    {"optionValues": [{"optionName": "Color", "name": "Sand"}], "price": "32.00", "compareAtPrice": "40.00", "sku": "APRON-SND",
                     "inventoryItem": {"tracked": true}, "inventoryQuantities": [{"locationId": location, "name": "available", "quantity": 6}]},
                    {"optionValues": [{"optionName": "Color", "name": "Rust"}], "price": "32.00", "sku": "APRON-RST"},
                ],
                "metafields": [{"namespace": "custom", "key": "care", "type": "single_line_text_field", "value": "Wash cold"}],
                "files": [{"originalSource": "https://cdn.example.com/apron.jpg", "alt": "Apron", "contentType": "IMAGE"}],
            }}),
        ),
        "productSet",
    );
    let product = created["product"]["id"].as_str().unwrap().to_string();
    let variant = created["product"]["variants"]["nodes"][0]["id"].clone();
    for (query, variables, name) in [
        (
            r#"mutation($id: ID!, $variants: [ProductVariantsBulkInput!]!) { productVariantsBulkCreate(productId: $id, variants: $variants) { userErrors { message } } }"#,
            json!({"id": product, "variants": [{"optionValues": [{"optionName": "Color", "name": "Olive"}], "price": "30", "barcodes": [{"value": "123456789012"}]}]}),
            "productVariantsBulkCreate",
        ),
        (
            r#"mutation($id: ID!, $variants: [ProductVariantsBulkInput!]!) { productVariantsBulkUpdate(productId: $id, variants: $variants) { userErrors { message } } }"#,
            json!({"id": product, "variants": [{"id": variant, "price": "29.50", "unitPriceMeasurement": {"quantityUnit": "G", "quantityValue": 250.0, "referenceUnit": "KG", "referenceValue": 1}}]}),
            "productVariantsBulkUpdate",
        ),
        (
            r#"mutation($id: ID!) { productOptionsCreate(productId: $id, options: [{name: "Size", values: [{name: "One size"}]}]) { userErrors { message } } }"#,
            json!({"id": product}),
            "productOptionsCreate",
        ),
        (
            r#"mutation($id: ID!) { productCreateMedia(productId: $id, media: [{originalSource: "https://example.com/v.mp4", mediaContentType: VIDEO}, {originalSource: "https://vimeo.com/123", mediaContentType: EXTERNAL_VIDEO}, {originalSource: "https://example.com/m.glb", mediaContentType: MODEL_3D}]) { mediaUserErrors { message } } }"#,
            json!({"id": product}),
            "productCreateMedia",
        ),
        (
            r#"mutation { collectionCreate(input: {title: "Sale", ruleSet: {appliedDisjunctively: false, rules: [{column: TAG, relation: EQUALS, condition: "sale"}]}}) { userErrors { message } } }"#,
            Value::Null,
            "collectionCreate",
        ),
        (
            r#"mutation($id: ID!) { collectionCreate(input: {title: "Aprons", products: [$id], image: {src: "https://example.com/c.jpg", altText: "Aprons"}}) { userErrors { message } } }"#,
            json!({"id": product}),
            "collectionCreate",
        ),
        (
            r#"mutation($id: ID!, $location: ID!) { inventorySetQuantities(input: {name: "on_hand", reason: "received", quantities: [{inventoryItemId: $id, locationId: $location, quantity: 9, changeFromQuantity: null}]}) @idempotent(key: "s") { userErrors { message } } }"#,
            json!({"id": shop_item(shop, &variant), "location": location}),
            "inventorySetQuantities",
        ),
        (
            r#"mutation { locationAdd(input: {name: "Depot", address: {countryCode: DE, city: "Berlin"}}) { userErrors { message } } }"#,
            Value::Null,
            "locationAdd",
        ),
        (
            r#"mutation($id: ID!) { sellingPlanGroupCreate(input: {name: "Refills", options: ["Every"], sellingPlansToCreate: [
                {name: "Monthly", options: ["month"], category: SUBSCRIPTION, billingPolicy: {recurring: {interval: MONTH, intervalCount: 1, anchors: [{type: MONTHDAY, day: 1}]}},
                 deliveryPolicy: {recurring: {interval: MONTH, intervalCount: 1, anchors: [{type: MONTHDAY, day: 1}]}},
                 pricingPolicies: [{fixed: {adjustmentType: PERCENTAGE, adjustmentValue: {percentage: 5}}}, {recurring: {afterCycle: 3, adjustmentType: FIXED_AMOUNT, adjustmentValue: {fixedValue: "2.00"}}}]},
                {name: "Pre-order", options: ["later"], category: PRE_ORDER, billingPolicy: {fixed: {checkoutCharge: {type: PERCENTAGE, value: {percentage: 20}}, remainingBalanceChargeTrigger: ON_FULFILLMENT}},
                 deliveryPolicy: {fixed: {fulfillmentTrigger: UNKNOWN}}}]}, resources: {productIds: [$id]}) { userErrors { message } } }"#,
            json!({"id": product}),
            "sellingPlanGroupCreate",
        ),
        (
            r#"mutation { giftCardCreate(input: {initialValue: "50", code: "LSFTEST12345678"}) { userErrors { message } } }"#,
            Value::Null,
            "giftCardCreate",
        ),
        (
            r#"mutation($id: ID!) { productDuplicate(productId: $id, newTitle: "Linen apron 2", includeImages: true) { userErrors { message } } }"#,
            json!({"id": product}),
            "productDuplicate",
        ),
        (
            r#"mutation($id: ID!) { productSet(synchronous: false, identifier: {id: $id}, input: {title: "Linen apron"}) { userErrors { message } } }"#,
            json!({"id": product}),
            "productSet",
        ),
    ] {
        ok(&run(shop, query, variables), name);
    }
    let publication = catalog::online_store_publication_gid();
    ok(
        &run(
            shop,
            "mutation($id: ID!, $publication: ID!) { publishablePublish(id: $id, input: [{publicationId: $publication}]) { userErrors { message } } }",
            json!({"id": product, "publication": publication}),
        ),
        "publishablePublish",
    );
    ok(
        &run(
            shop,
            r#"mutation($id: ID!) { giftCardCredit(id: $id, creditInput: {creditAmount: {amount: "5", currencyCode: EUR}}) { userErrors { message } } }"#,
            json!({"id": shop.keys("GiftCard").last().unwrap()}),
        ),
        "giftCardCredit",
    );
    product
}

fn shop_item(shop: &ShopState, variant: &Value) -> String {
    shop.get(variant.as_str().unwrap()).unwrap()["inventoryItem"]
        .as_str()
        .unwrap()
        .to_string()
}

fn check_shape(shop: &ShopState) {
    let Some(schema) = Schema::load() else {
        return;
    };
    let problems = schema.check(shop);
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn the_records_the_catalog_mutations_make_are_in_the_shape_of_their_types() {
    let store = support::fixture_store("rich");
    let app = support::fixture_app();
    let mut shop = ShopState::seeded(&store, Some(&app));
    let currency = shop.shop().unwrap()["currencyCode"].clone();
    assert_eq!(currency, json!("EUR"), "the gift card is credited in EUR");
    exercise(&mut shop);
    check_shape(&shop);
}

#[test]
fn what_the_catalog_mutations_change_is_in_the_store() {
    let base = support::fixture_store("rich");
    let app = support::fixture_app();
    let mut shop = ShopState::seeded(&base, Some(&app));
    let product = exercise(&mut shop);
    let overlay = sync::overlay(&shop, &base).expect("something to sync");
    let (store, diagnostics) = load_with_overlays(
        &DataSource::Directory(support::fixture("rich")),
        &LoadOptions::default(),
        std::slice::from_ref(&overlay),
    );
    let own: Vec<_> = diagnostics
        .items
        .iter()
        .filter(|item| item.file == sync::OVERLAY_NAME || item.file.contains("admin"))
        .collect();
    assert!(own.is_empty(), "{own:?}\n{overlay:#?}");
    let apron = store
        .product_by_handle("linen-apron")
        .expect("the product is in the store");
    assert_eq!(catalog::product_gid(apron), product);
    // Published and active: on the storefront.
    assert!(apron.listed(), "{apron:?}");
    let skus: Vec<&str> = apron
        .variants
        .iter()
        .map(|variant| variant.sku.as_str())
        .collect();
    assert!(skus.contains(&"APRON-SND"), "{skus:?}");
    let sand = apron
        .variants
        .iter()
        .find(|variant| variant.sku == "APRON-SND")
        .unwrap();
    assert_eq!(sand.price, 2950);
    assert_eq!(sand.inventory_quantity, 9);
    // In the collection made of it, and in the smart one its tag matches (written as manual).
    for handle in ["aprons", "sale"] {
        let collection = store
            .collection_by_handle(handle)
            .unwrap_or_else(|| panic!("the collection {handle} is in the store"));
        assert!(
            collection
                .products
                .iter()
                .any(|index| store.products[*index].handle == "linen-apron"),
            "{handle}"
        );
    }
    // The location added.
    assert!(
        store
            .locations
            .iter()
            .any(|location| location.name == "Depot")
    );
}
