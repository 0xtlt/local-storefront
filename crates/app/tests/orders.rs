//! The orders family end to end, through `AdminApi::handle`: an order placed in the local
//! checkout, then fulfilled, refunded and edited by the backend, each change delivered as
//! Shopify delivers it, signed, to a listener of the test.

mod support;

use std::collections::HashMap;
use std::time::Duration;

use lsf_app::admin::api::{AdminApi, AdminOptions, HttpRequest, ThrottleSetting};
use lsf_app::admin::domains::orders::PlacedOrder;
use lsf_app::admin::webhooks::DeliveryOptions;
use lsf_app::checkout::{
    Address, CheckoutResult, Cost, DeliveryGroup, DeliveryOption, Metafield, ResultLine,
};
use serde_json::{Value, json};
use support::receiver::Receiver;

const SECRET: &str = "test-secret";

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
    let response = api
        .handle(&support::graphql_request(&token, query, variables))
        .expect("the Admin API is routed");
    assert_eq!(response.status, 200);
    serde_json::from_slice(&response.body).unwrap()
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
    place_order_with(api, number, &[])
}

/// [`place_order_numbered`] from a cart with `metafields`.
fn place_order_with(api: &AdminApi, number: u64, metafields: &[Metafield]) -> Option<String> {
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
        number: Some(number),
        processed_at: "2026-05-01T09:30:00Z".parse().unwrap(),
        result: &result,
        customer_id: Some(jane.id),
        company_location_id: None,
        email: Some(&jane.email),
        phone: None,
        shipping_address: Some(&address),
        note: None,
        attributes: &[],
        payment: Some("Credit card"),
        metafields,
    })
    .map(|recorded| recorded.expect("the order is recorded").id)
}

#[test]
fn a_checkout_order_is_delivered_then_fulfilled_refunded_and_edited() {
    let receiver = Receiver::start(SECRET);
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
    assert_eq!(created["payment_gateway_names"], json!(["Credit card"]));
    let (_, transaction) = &delivered[1];
    assert_eq!(transaction["kind"], json!("sale"));
    assert_eq!(transaction["gateway"], json!("Credit card"));
    assert_eq!(transaction["amount"], json!("45.00"));
    assert_eq!(
        transaction["order_id"],
        json!(lsf_app::admin::state::parse_gid(&order).unwrap().1)
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
        json!(lsf_app::admin::state::parse_gid(&order).unwrap().1)
    );
    assert_eq!(
        edit["line_items"]["additions"],
        json!([{"id": lsf_app::admin::state::parse_gid(lines[1]["id"].as_str().unwrap()).unwrap().1, "delta": 1}])
    );
    assert_eq!(delivered[1].1["line_items"].as_array().unwrap().len(), 2);
}

/// Creates a definition of `owner_type`, `cartToOrderCopyable` as `copyable` says.
fn define(
    api: &AdminApi,
    owner_type: &str,
    namespace: &str,
    key: &str,
    kind: &str,
    copyable: bool,
) {
    let answer = graphql(
        api,
        "mutation($definition: MetafieldDefinitionInput!) { metafieldDefinitionCreate(definition: $definition) { createdDefinition { id } userErrors { message } } }",
        json!({"definition": {
            "name": key, "namespace": namespace, "key": key, "type": kind, "ownerType": owner_type,
            "capabilities": {"cartToOrderCopyable": {"enabled": copyable}},
            "validations": if kind == "number_integer" { json!([{"name": "max", "value": "5"}]) } else { json!([]) },
        }}),
    );
    assert_eq!(
        answer["data"]["metafieldDefinitionCreate"]["userErrors"],
        json!([]),
        "{answer}"
    );
}

fn cart_metafield(namespace: &str, key: &str, kind: &str, value: &str) -> Metafield {
    Metafield {
        namespace: namespace.into(),
        key: key.into(),
        kind: kind.into(),
        value: value.into(),
    }
}

#[test]
fn the_cart_metafields_an_order_definition_copies_are_the_orders() {
    let shop = api();
    // Copied: the app's namespace, as the extension writes it, and a merchant's one.
    define(
        &shop,
        "ORDER",
        "$app:gift",
        "message",
        "single_line_text_field",
        true,
    );
    define(&shop, "ORDER", "custom", "count", "number_integer", true);
    define(&shop, "ORDER", "custom", "too_many", "number_integer", true);
    // Not copied: the capability is off, the definition is of another owner type (where the
    // capability cannot be enabled), there is no definition.
    define(&shop, "ORDER", "custom", "wrap", "boolean", false);
    define(
        &shop,
        "PRODUCT",
        "custom",
        "engraving",
        "single_line_text_field",
        false,
    );
    let cart = [
        cart_metafield(
            "$app:gift",
            "message",
            "single_line_text_field",
            "Happy birthday",
        ),
        // The cart's type is not the definition's: the order's is the definition's.
        cart_metafield("custom", "count", "single_line_text_field", "3"),
        // Over the definition's `max`.
        cart_metafield("custom", "too_many", "number_integer", "9"),
        cart_metafield("custom", "wrap", "boolean", "true"),
        cart_metafield("custom", "engraving", "single_line_text_field", "J.D."),
        cart_metafield(
            "custom",
            "undefined",
            "single_line_text_field",
            "kept on the cart",
        ),
    ];
    let order = place_order_with(&shop, 1004, &cart).expect("the order is recorded");
    let answer = graphql(
        &shop,
        r#"query($id: ID!) { order(id: $id) {
            message: metafield(namespace: "$app:gift", key: "message") { namespace value type definition { key } }
            count: metafield(namespace: "custom", key: "count") { value type }
            metafields(first: 10) { nodes { namespace key } }
        } }"#,
        json!({"id": order}),
    );
    let order = &answer["data"]["order"];
    let message = &order["message"];
    assert!(
        message["namespace"].as_str().is_some_and(
            |namespace| namespace.starts_with("app--") && namespace.ends_with("--gift")
        ),
        "{answer}"
    );
    assert_eq!(message["value"], "Happy birthday", "{answer}");
    assert_eq!(message["type"], "single_line_text_field");
    assert_eq!(
        message["definition"]["key"], "message",
        "linked to its definition"
    );
    assert_eq!(
        order["count"],
        json!({"value": "3", "type": "number_integer"})
    );
    let keys: Vec<&str> = order["metafields"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["key"].as_str())
        .collect();
    assert_eq!(keys, ["message", "count"], "{answer}");

    // Without a definition, nothing is copied.
    let bare = api();
    let order = place_order_with(&bare, 1004, &cart).expect("the order is recorded");
    let answer = graphql(
        &bare,
        "query($id: ID!) { order(id: $id) { metafields(first: 10) { nodes { key } } } }",
        json!({"id": order}),
    );
    assert_eq!(
        answer["data"]["order"]["metafields"]["nodes"],
        json!([]),
        "{answer}"
    );
}

#[test]
fn the_order_definitions_of_the_app_toml_copy_the_cart_metafields() {
    // `[order.metafields.app.gift_message]` with `capabilities.cart_to_order_copyable = true`,
    // and `[order.metafields.custom.wrap]` without.
    let mut app = support::fixture_app();
    let declared = |namespace: &str, key: &str, kind: &str, copyable: bool| {
        lsf_app::config::DeclaredMetafieldDefinition {
            namespace: namespace.into(),
            key: key.into(),
            kind: kind.into(),
            name: None,
            description: None,
            cart_to_order_copyable: copyable,
        }
    };
    app.config.order_metafields = vec![
        declared("$app", "gift_message", "single_line_text_field", true),
        declared("custom", "wrap", "boolean", false),
    ];
    let shop = AdminApi::new(
        &support::demo_store(),
        AdminOptions {
            app: Some(app),
            client_secret: SECRET.to_string(),
            app_url: None,
            throttle: ThrottleSetting::Store,
            delivery: DeliveryOptions::default(),
            token_seed: Some(7),
        },
    );
    let answer = graphql(
        &shop,
        "{ metafieldDefinitions(first: 10, ownerType: ORDER) { nodes { namespace key name type { name } capabilities { cartToOrderCopyable { enabled } } } } }",
        json!({}),
    );
    let nodes = answer["data"]["metafieldDefinitions"]["nodes"]
        .as_array()
        .unwrap_or_else(|| panic!("{answer}"));
    let summary: Vec<(String, &str, bool)> = nodes
        .iter()
        .map(|node| {
            (
                format!(
                    "{}.{}",
                    node["namespace"].as_str().unwrap(),
                    node["key"].as_str().unwrap()
                ),
                node["type"]["name"].as_str().unwrap(),
                node["capabilities"]["cartToOrderCopyable"]["enabled"]
                    .as_bool()
                    .unwrap(),
            )
        })
        .collect();
    let app_id = answer_app_id(&shop);
    assert_eq!(
        summary,
        [
            (
                format!("app--{app_id}.gift_message"),
                "single_line_text_field",
                true
            ),
            ("custom.wrap".to_string(), "boolean", false),
        ]
    );
    assert_eq!(nodes[0]["name"], "Gift message", "the key, humanized");

    let cart = [
        cart_metafield(
            "$app",
            "gift_message",
            "single_line_text_field",
            "Happy birthday",
        ),
        cart_metafield("custom", "wrap", "boolean", "true"),
    ];
    let order = place_order_with(&shop, 1004, &cart).expect("the order is recorded");
    let answer = graphql(
        &shop,
        r#"query($id: ID!) { order(id: $id) { metafields(first: 10) { nodes { namespace key value } } } }"#,
        json!({"id": order}),
    );
    assert_eq!(
        answer["data"]["order"]["metafields"]["nodes"],
        json!([{"namespace": format!("app--{app_id}"), "key": "gift_message", "value": "Happy birthday"}]),
        "{answer}"
    );
}

/// The number of the app's GID: what its reserved namespace is made of (`app--<id>`).
fn answer_app_id(api: &AdminApi) -> String {
    let answer = graphql(api, "{ app { id } }", json!({}));
    answer["data"]["app"]["id"]
        .as_str()
        .and_then(|id| id.rsplit('/').next())
        .unwrap_or_else(|| panic!("{answer}"))
        .to_string()
}

#[test]
fn a_checkout_order_keeps_its_discounts_and_its_gateway() {
    use lsf_app::checkout::{
        AllocationMethod, AppliedDiscount, DiscountAllocation, DiscountClass, DiscountDetails,
        DiscountTarget, DiscountValue, TargetSelection,
    };
    let receiver = Receiver::start(SECRET);
    let api = api();
    subscribe(&api, &receiver, &["ORDERS_CREATE"]);
    let store = support::demo_store();
    let mug = store.product_by_handle("ceramic-mug").unwrap();
    // 20 % off the mugs (an app's automatic discount), and the shipping free with a code.
    let result = CheckoutResult {
        lines: vec![ResultLine {
            id: "gid://shopify/CartLine/1".into(),
            variant_id: mug.variants[0].id,
            product_id: mug.id,
            title: mug.title.clone(),
            quantity: 2,
            unit_price: 2000,
            subtotal: 4000,
            total: 3200,
            requires_shipping: true,
            discount_allocations: vec![DiscountAllocation {
                title: "20% off".into(),
                code: None,
                amount: 800,
                class: DiscountClass::Product,
                discount: 0,
            }],
            ..ResultLine::default()
        }],
        discounts: vec![
            AppliedDiscount {
                title: "20% off".into(),
                code: None,
                class: DiscountClass::Product,
                amount: 800,
                function: "discount-tag-percentage".into(),
                details: DiscountDetails {
                    allocation_method: AllocationMethod::Across,
                    target_selection: TargetSelection::Explicit,
                    target_type: DiscountTarget::LineItem,
                    value: DiscountValue::Percentage(20.0),
                    metafields: Vec::new(),
                },
                code_discount: None,
            },
            AppliedDiscount {
                title: "Free shipping".into(),
                code: Some("FREESHIP".into()),
                class: DiscountClass::Shipping,
                amount: 500,
                function: "discount-free-shipping".into(),
                details: DiscountDetails {
                    allocation_method: AllocationMethod::Each,
                    target_selection: TargetSelection::All,
                    target_type: DiscountTarget::ShippingLine,
                    value: DiscountValue::Fixed(500),
                    metafields: Vec::new(),
                },
                code_discount: None,
            },
        ],
        discount_codes: vec![lsf_app::checkout::DiscountCodeStatus {
            code: "FREESHIP".into(),
            applicable: true,
            reason: None,
            took_nothing: false,
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
                cost: 0,
            }],
            selected: Some("standard".into()),
            discount_allocations: vec![DiscountAllocation {
                title: "Free shipping".into(),
                code: Some("FREESHIP".into()),
                amount: 500,
                class: DiscountClass::Shipping,
                discount: 1,
            }],
        }],
        cost: Cost {
            subtotal: 4000,
            discounts: 1300,
            shipping: 0,
            tax: 0,
            total: 3200,
            due_now: 3200,
            due_later: 0,
            checkout_charge_amount: 4000,
            currency_code: "USD".into(),
        },
        ..CheckoutResult::default()
    };
    let order = api
        .record_order(&PlacedOrder {
            id: None,
            number: Some(1004),
            processed_at: "2026-05-01T09:30:00Z".parse().unwrap(),
            result: &result,
            customer_id: None,
            company_location_id: None,
            email: Some("buyer@example.com"),
            phone: None,
            shipping_address: None,
            note: None,
            attributes: &[],
            payment: Some("Credit card"),
            metafields: &[],
        })
        .expect("the order is recorded")
        .expect("no code is refused")
        .id;

    let answer = graphql(
        &api,
        r#"query($id: ID!) { order(id: $id) {
            paymentGatewayNames
            discountApplications(first: 5) { nodes {
                __typename index allocationMethod targetSelection targetType
                value { __typename ... on PricingPercentageValue { percentage } ... on MoneyV2 { amount } }
                ... on AutomaticDiscountApplication { title }
                ... on DiscountCodeApplication { code } } }
            lineItems(first: 1) { nodes { discountAllocations {
                allocatedAmountSet { shopMoney { amount } } discountApplication { index } } } }
            shippingLines(first: 1) { nodes { discountAllocations {
                allocatedAmountSet { shopMoney { amount } } discountApplication { index } } } } } }"#,
        json!({"id": order}),
    );
    let order_data = &answer["data"]["order"];
    assert_eq!(
        order_data["paymentGatewayNames"],
        json!(["Credit card"]),
        "{answer}"
    );
    assert_eq!(
        order_data["discountApplications"]["nodes"],
        json!([
            {"__typename": "AutomaticDiscountApplication", "index": 0, "allocationMethod": "ACROSS",
             "targetSelection": "EXPLICIT", "targetType": "LINE_ITEM",
             "value": {"__typename": "PricingPercentageValue", "percentage": 20.0}, "title": "20% off"},
            {"__typename": "DiscountCodeApplication", "index": 1, "allocationMethod": "EACH",
             "targetSelection": "ALL", "targetType": "SHIPPING_LINE",
             "value": {"__typename": "MoneyV2", "amount": "5.0"}, "code": "FREESHIP"},
        ]),
        "{answer}"
    );
    assert_eq!(
        order_data["lineItems"]["nodes"][0]["discountAllocations"],
        json!([{"allocatedAmountSet": {"shopMoney": {"amount": "8.0"}}, "discountApplication": {"index": 0}}])
    );
    assert_eq!(
        order_data["shippingLines"]["nodes"][0]["discountAllocations"],
        json!([{"allocatedAmountSet": {"shopMoney": {"amount": "5.0"}}, "discountApplication": {"index": 1}}])
    );

    // The webhook says the same, as REST writes it.
    let (topic, created) = receiver.next();
    assert_eq!(topic, "orders/create");
    assert_eq!(created["payment_gateway_names"], json!(["Credit card"]));
    let applications = created["discount_applications"].as_array().unwrap();
    assert_eq!(applications.len(), 2, "{created}");
    assert_eq!(applications[0]["type"], json!("automatic"));
    assert_eq!(applications[0]["value"], json!("20.0"));
    assert_eq!(applications[0]["value_type"], json!("percentage"));
    assert_eq!(applications[1]["type"], json!("discount_code"));
    assert_eq!(applications[1]["code"], json!("FREESHIP"));
    let allocation = &created["line_items"][0]["discount_allocations"][0];
    assert_eq!(allocation["amount"], json!("8.00"));
    assert_eq!(allocation["discount_application_index"], json!(0));
    assert_eq!(
        created["shipping_lines"][0]["discount_allocations"][0]["discount_application_index"],
        json!(1)
    );
    assert_eq!(created["discount_codes"][0]["code"], json!("FREESHIP"));
    assert_eq!(created["discount_codes"][0]["amount"], json!("5.00"));
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
    let receiver = Receiver::start(SECRET);
    let app = lsf_app::config::load(&support::fixture("admin-app")).unwrap();
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

/// The issue's bundle: a line a cart transform expanded is ordered as its components, each in
/// a `LineItemGroup` that names the bundle, as Shopify stores a bundle in an order.
#[test]
fn a_bundle_is_ordered_as_its_components_in_a_line_item_group() {
    let receiver = Receiver::start(SECRET);
    let api = api();
    subscribe(&api, &receiver, &["ORDERS_CREATE"]);
    let store = support::demo_store();
    let tee = store.product_by_handle("organic-cotton-t-shirt").unwrap();
    let mug = store.product_by_handle("ceramic-mug").unwrap();
    let component = |variant: &lsf_core::store::Variant, unit_price: i64| ResultLine {
        variant_id: variant.id,
        product_id: mug.id,
        title: mug.title.clone(),
        sku: Some(variant.sku.clone()),
        quantity: 1,
        unit_price,
        subtotal: unit_price,
        total: unit_price,
        requires_shipping: true,
        ..ResultLine::default()
    };
    // The kit costs 30.00, 3.00 off: its components share what it costs, in proportion.
    let kit = ResultLine {
        id: "gid://shopify/CartLine/1".into(),
        variant_id: tee.variants[0].id,
        product_id: tee.id,
        title: "Kit".into(),
        sku: Some(tee.variants[0].sku.clone()),
        quantity: 1,
        unit_price: 3000,
        subtotal: 3000,
        total: 2700,
        requires_shipping: true,
        components: vec![
            component(&mug.variants[0], 2000),
            component(&mug.variants[1], 1000),
        ],
        transformed_by: Some("expand".into()),
        ..ResultLine::default()
    };
    let result = CheckoutResult {
        lines: vec![kit],
        cost: Cost {
            subtotal: 3000,
            discounts: 300,
            total: 2700,
            due_now: 2700,
            checkout_charge_amount: 3000,
            currency_code: "USD".into(),
            ..Cost::default()
        },
        ..CheckoutResult::default()
    };
    let order = api
        .record_order(&PlacedOrder {
            id: None,
            number: Some(1004),
            processed_at: "2026-05-01T09:30:00Z".parse().unwrap(),
            result: &result,
            customer_id: None,
            company_location_id: None,
            email: Some("buyer@example.com"),
            phone: None,
            shipping_address: None,
            note: None,
            attributes: &[],
            payment: Some("Credit card"),
            metafields: &[],
        })
        .expect("the order is recorded")
        .expect("no code is refused")
        .id;

    let answer = graphql(
        &api,
        r#"query($id: ID!) { order(id: $id) { subtotalPriceSet { shopMoney { amount } }
            lineItems(first: 5) { nodes { sku quantity discountedTotalSet { shopMoney { amount } }
                lineItemGroup { id title quantity variantSku } } } } }"#,
        json!({"id": order}),
    );
    let order_data = &answer["data"]["order"];
    let items = order_data["lineItems"]["nodes"].as_array().unwrap();
    assert_eq!(items.len(), 2, "{answer}");
    let group = &items[0]["lineItemGroup"];
    assert_eq!(group["title"], "Kit", "{answer}");
    assert_eq!(group["quantity"], 1);
    assert_eq!(group["variantSku"], json!(tee.variants[0].sku));
    assert_eq!(items[1]["lineItemGroup"], *group);
    assert_eq!(
        items
            .iter()
            .map(|item| (
                item["sku"].clone(),
                item["discountedTotalSet"]["shopMoney"]["amount"].clone()
            ))
            .collect::<Vec<_>>(),
        [
            (json!(mug.variants[0].sku), json!("18.0")),
            (json!(mug.variants[1].sku), json!("9.0")),
        ]
    );
    assert_eq!(
        order_data["subtotalPriceSet"]["shopMoney"]["amount"],
        "27.0"
    );

    let (_, created) = &receiver.take(1)[0];
    let lines = created["line_items"].as_array().unwrap();
    let group_id = lsf_app::admin::state::parse_gid(group["id"].as_str().unwrap())
        .unwrap()
        .1;
    assert_eq!(lines.len(), 2);
    for line in lines {
        assert_eq!(
            line["sales_line_item_group_id"],
            json!(group_id),
            "{created}"
        );
    }
}
