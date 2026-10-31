//! The orders family through queries and mutations over the demo store and the rich fixture.

use serde_json::{Value, json};

use super::model;
use crate::graphql::{self, Request};
use crate::resolve::{Registry, ShopResolver};
use crate::state::ShopState;

fn demo() -> ShopState {
    let (store, _) = lsf_core::store::load::load(
        &lsf_core::store::load::DataSource::Demo,
        &Default::default(),
    );
    ShopState::seeded(&store, None)
}

fn rich() -> ShopState {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rich");
    let (store, _) = lsf_core::store::load::load(
        &lsf_core::store::load::DataSource::Directory(path),
        &Default::default(),
    );
    ShopState::seeded(&store, None)
}

/// Every seeded order is left as the seed made it by `refresh_order`: the mutations and the
/// seed derive the same values.
#[test]
fn refreshing_a_seeded_order_changes_nothing() {
    for mut shop in [demo(), rich()] {
        let orders: Vec<String> = shop.keys("Order").map(str::to_string).collect();
        assert!(!orders.is_empty());
        for order in orders {
            let status = shop.get(&order).unwrap()["displayFinancialStatus"].clone();
            let before = shop.clone();
            model::refresh_order(&mut shop, &order);
            for (key, record) in shop.iter() {
                let old = before.get(key).unwrap();
                for (field, value) in record {
                    // A refunded order of the data has a refund transaction but no refund: its
                    // outstanding amount is the seed's (nothing), the refresh's its total.
                    if status == "REFUNDED" && field == "totalOutstandingSet" {
                        continue;
                    }
                    assert_eq!(
                        old.get(field),
                        Some(value),
                        "{order}: {key}.{field} changed"
                    );
                }
            }
        }
    }
}

/// Runs a request on the shop: its answer (`extensions` without `cost`).
fn run(shop: &mut ShopState, query: &str, variables: Value) -> Value {
    let mut resolver = ShopResolver::new(shop, Registry::standard());
    let mut answer = graphql::run(
        crate::schema::admin(),
        &mut resolver,
        &Request::new(query).with_variables(variables),
        None,
    );
    answer.extensions.extend(resolver.extensions());
    answer.extensions.remove("cost");
    answer.to_json()
}

/// The topics of the events emitted since the last call, with the GIDs they are about.
fn events(shop: &mut ShopState) -> Vec<(String, String)> {
    shop.take_events()
        .into_iter()
        .map(|event| (event.topic, event.gid))
        .collect()
}

fn topics(shop: &mut ShopState) -> Vec<String> {
    events(shop).into_iter().map(|(topic, _)| topic).collect()
}

/// The orders of the demo store: `#1001` (paid, fulfilled), `#1002` and `#1003` (paid,
/// unfulfilled).
fn order_named(shop: &ShopState, name: &str) -> String {
    shop.keys("Order")
        .find(|key| shop.get(key).unwrap()["name"] == json!(name))
        .unwrap()
        .to_string()
}

/// An order of the demo store by its name, as [`ORDER_FIELDS`] reads it.
fn named(shop: &mut ShopState, name: &str) -> Value {
    let id = order_named(shop, name);
    order(shop, &id)
}

/// What is available of an inventory item, over its locations.
fn available(shop: &ShopState, item: &str) -> i64 {
    crate::domains::catalog::gids(shop.get(item).unwrap().get("inventoryLevels"))
        .iter()
        .map(|level| crate::domains::catalog::available_of(shop.get(level).unwrap()))
        .sum()
}

fn amount(value: &Value) -> &str {
    value["shopMoney"]["amount"].as_str().unwrap()
}

const ORDER_FIELDS: &str = "id name displayFinancialStatus displayFulfillmentStatus closed closedAt cancelledAt cancelReason
    currentSubtotalPriceSet { shopMoney { amount } } currentTotalPriceSet { shopMoney { amount } }
    totalReceivedSet { shopMoney { amount } } totalRefundedSet { shopMoney { amount } }
    totalOutstandingSet { shopMoney { amount } } totalCapturableSet { shopMoney { amount } }
    lineItems(first: 10) { nodes { id quantity currentQuantity unfulfilledQuantity refundableQuantity sku } }
    fulfillmentOrders(first: 10) { nodes { id status supportedActions { action }
      assignedLocation { location { id } }
      lineItems(first: 10) { nodes { id totalQuantity remainingQuantity lineItem { id } } } } }
    fulfillments { id name status totalQuantity trackingInfo { company number url } }
    transactions { id kind status gateway amountSet { shopMoney { amount } } parentTransaction { id } }
    refunds { id totalRefundedSet { shopMoney { amount } } }";

fn order(shop: &mut ShopState, id: &str) -> Value {
    let answer = run(
        shop,
        &format!("query($id: ID!) {{ order(id: $id) {{ {ORDER_FIELDS} }} }}"),
        json!({"id": id}),
    );
    answer["data"]["order"].clone()
}

#[test]
fn orders_are_searched_and_sorted_by_their_documented_filters() {
    let mut shop = demo();
    let names = |shop: &mut ShopState, query: &str| -> Vec<String> {
        let answer = run(
            shop,
            "query($q: String) { orders(first: 10, query: $q, sortKey: ORDER_NUMBER) { nodes { name } } }",
            json!({"q": query}),
        );
        answer["data"]["orders"]["nodes"]
            .as_array()
            .unwrap_or_else(|| panic!("{answer}"))
            .iter()
            .map(|node| node["name"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(names(&mut shop, "fulfillment_status:fulfilled"), ["#1001"]);
    assert_eq!(names(&mut shop, "fulfillment_status:shipped"), ["#1001"]);
    assert_eq!(
        names(&mut shop, "fulfillment_status:unfulfilled"),
        ["#1002", "#1003"]
    );
    assert_eq!(names(&mut shop, "financial_status:paid").len(), 3);
    assert!(names(&mut shop, "financial_status:pending").is_empty());
    assert_eq!(names(&mut shop, "status:open").len(), 3);
    assert!(names(&mut shop, "status:cancelled").is_empty());
    assert_eq!(names(&mut shop, "name:#1002"), ["#1002"]);
    assert_eq!(names(&mut shop, "name:1003"), ["#1003"]);
    assert_eq!(names(&mut shop, "1003"), ["#1003"]);
    let first_sku = named(&mut shop, "#1002")["lineItems"]["nodes"][0]["sku"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(names(&mut shop, &format!("sku:{first_sku}")).contains(&"#1002".to_string()));
    let reversed = run(
        &mut shop,
        "{ orders(first: 3, sortKey: ORDER_NUMBER, reverse: true) { nodes { name } } ordersCount(query: \"fulfillment_status:unshipped\") { count } }",
        Value::Null,
    );
    assert_eq!(reversed["data"]["orders"]["nodes"][0]["name"], json!("#1003"));
    assert_eq!(reversed["data"]["ordersCount"]["count"], json!(2));
}

#[test]
fn fulfillment_orders_say_what_they_support() {
    let mut shop = demo();
    let unfulfilled = named(&mut shop, "#1002");
    let fo = &unfulfilled["fulfillmentOrders"]["nodes"][0];
    assert_eq!(fo["status"], json!("OPEN"));
    let actions: Vec<&str> = fo["supportedActions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|action| action["action"].as_str().unwrap())
        .collect();
    assert!(actions.starts_with(&["CREATE_FULFILLMENT", "HOLD", "MOVE"]), "{actions:?}");
    let fulfilled = named(&mut shop, "#1001");
    assert_eq!(fulfilled["fulfillmentOrders"]["nodes"][0]["status"], json!("CLOSED"));
    assert_eq!(fulfilled["fulfillmentOrders"]["nodes"][0]["supportedActions"], json!([]));
    let open = run(
        &mut shop,
        "{ fulfillmentOrders(first: 10) { nodes { id status } } all: fulfillmentOrders(first: 10, includeClosed: true) { nodes { id } } }",
        Value::Null,
    );
    assert_eq!(open["data"]["fulfillmentOrders"]["nodes"].as_array().unwrap().len(), 2);
    assert_eq!(open["data"]["all"]["nodes"].as_array().unwrap().len(), 3);
}

const FULFILLMENT_CREATE: &str = "mutation($input: FulfillmentInput!) { fulfillmentCreate(fulfillment: $input) {
    fulfillment { id name status displayStatus totalQuantity location { id }
      trackingInfo { company number url } fulfillmentLineItems(first: 5) { nodes { quantity lineItem { id } } } }
    userErrors { field message } } }";

#[test]
fn fulfillment_create_ships_part_then_all_of_an_order() {
    let mut shop = demo();
    let id = order_named(&shop, "#1002");
    let before = order(&mut shop, &id);
    let fo = before["fulfillmentOrders"]["nodes"][0].clone();
    let fo_line = fo["lineItems"]["nodes"][0].clone();
    let total = fo_line["totalQuantity"].as_i64().unwrap();
    assert!(total >= 1);
    // An order of one unit ships at once; otherwise one unit first.
    let first = if total > 1 { 1 } else { total };
    let answer = run(
        &mut shop,
        FULFILLMENT_CREATE,
        json!({"input": {
            "lineItemsByFulfillmentOrder": [{"fulfillmentOrderId": fo["id"], "fulfillmentOrderLineItems": [{"id": fo_line["id"], "quantity": first}]}],
            "trackingInfo": {"company": "UPS", "number": "1Z999"},
        }}),
    );
    let created = &answer["data"]["fulfillmentCreate"];
    assert_eq!(created["userErrors"], json!([]), "{answer}");
    let fulfillment = &created["fulfillment"];
    assert_eq!(fulfillment["name"], json!("#1002-F1"));
    assert_eq!(fulfillment["status"], json!("SUCCESS"));
    assert_eq!(fulfillment["totalQuantity"], json!(first));
    assert_eq!(fulfillment["location"]["id"], fo["assignedLocation"]["location"]["id"]);
    assert_eq!(
        fulfillment["trackingInfo"],
        json!([{"company": "UPS", "number": "1Z999", "url": "https://www.ups.com/WebTracking?loc=en_US&requester=ST&trackNums=1Z999"}])
    );
    let after = order(&mut shop, &id);
    let partial = first < total;
    assert_eq!(
        after["displayFulfillmentStatus"],
        json!(if partial { "PARTIALLY_FULFILLED" } else { "FULFILLED" })
    );
    assert_eq!(
        after["fulfillmentOrders"]["nodes"][0]["status"],
        json!(if partial { "IN_PROGRESS" } else { "CLOSED" })
    );
    assert_eq!(
        after["lineItems"]["nodes"][0]["unfulfilledQuantity"],
        json!(total - first)
    );
    let fired = topics(&mut shop);
    assert_eq!(
        fired,
        [
            "fulfillments/create",
            if partial { "orders/partially_fulfilled" } else { "orders/fulfilled" },
            "orders/updated"
        ]
    );
    if partial {
        // Without line items, what remains of the fulfillment order ships.
        let answer = run(
            &mut shop,
            FULFILLMENT_CREATE,
            json!({"input": {"lineItemsByFulfillmentOrder": [{"fulfillmentOrderId": fo["id"]}]}}),
        );
        assert_eq!(answer["data"]["fulfillmentCreate"]["userErrors"], json!([]));
        assert_eq!(
            answer["data"]["fulfillmentCreate"]["fulfillment"]["name"],
            json!("#1002-F2")
        );
        let done = order(&mut shop, &id);
        assert_eq!(done["displayFulfillmentStatus"], json!("FULFILLED"));
        assert_eq!(done["fulfillmentOrders"]["nodes"][0]["status"], json!("CLOSED"));
        assert_eq!(
            topics(&mut shop),
            ["fulfillments/create", "orders/fulfilled", "orders/updated"]
        );
    }
}

#[test]
fn fulfillment_create_refuses_what_cannot_ship() {
    let mut shop = demo();
    let closed = named(&mut shop, "#1001");
    let closed_fo = closed["fulfillmentOrders"]["nodes"][0]["id"].as_str().unwrap().to_string();
    let number = crate::state::parse_gid(&closed_fo).unwrap().1;
    let answer = run(
        &mut shop,
        FULFILLMENT_CREATE,
        json!({"input": {"lineItemsByFulfillmentOrder": [{"fulfillmentOrderId": closed_fo}]}}),
    );
    assert_eq!(
        answer["data"]["fulfillmentCreate"],
        json!({"fulfillment": null, "userErrors": [{"field": ["fulfillment"], "message": format!("Fulfillment order {number} has an unfulfillable status= closed.")}]})
    );
    let open = named(&mut shop, "#1003");
    let fo = &open["fulfillmentOrders"]["nodes"][0];
    let answer = run(
        &mut shop,
        FULFILLMENT_CREATE,
        json!({"input": {"lineItemsByFulfillmentOrder": [{"fulfillmentOrderId": fo["id"], "fulfillmentOrderLineItems": [{"id": fo["lineItems"]["nodes"][0]["id"], "quantity": 99}]}]}}),
    );
    assert_eq!(
        answer["data"]["fulfillmentCreate"]["userErrors"],
        json!([{"field": ["fulfillment"], "message": "Invalid fulfillment order line item quantity requested."}])
    );
    assert_eq!(named(&mut shop, "#1003"), open);
    assert!(shop.take_events().is_empty());
}

#[test]
fn fulfillments_are_tracked_and_cancelled() {
    let mut shop = demo();
    let id = order_named(&shop, "#1003");
    let fo = order(&mut shop, &id)["fulfillmentOrders"]["nodes"][0]["id"].clone();
    let created = run(
        &mut shop,
        FULFILLMENT_CREATE,
        json!({"input": {"lineItemsByFulfillmentOrder": [{"fulfillmentOrderId": fo}]}}),
    );
    let fulfillment = created["data"]["fulfillmentCreate"]["fulfillment"]["id"].clone();
    events(&mut shop);
    let tracked = run(
        &mut shop,
        "mutation($id: ID!) { fulfillmentTrackingInfoUpdate(fulfillmentId: $id, trackingInfoInput: {company: \"DHL\", numbers: [\"A1\", \"A2\"], urls: [\"https://dhl.example/A1\"]}) {
            fulfillment { trackingInfo { company number url } } userErrors { field message } } }",
        json!({"id": fulfillment}),
    );
    assert_eq!(
        tracked["data"]["fulfillmentTrackingInfoUpdate"]["fulfillment"]["trackingInfo"],
        json!([
            {"company": "DHL", "number": "A1", "url": "https://dhl.example/A1"},
            {"company": "DHL", "number": "A2", "url": null},
        ])
    );
    assert_eq!(topics(&mut shop), ["fulfillments/update"]);
    let event = run(
        &mut shop,
        "mutation($id: ID!) { fulfillmentEventCreate(fulfillmentEvent: {fulfillmentId: $id, status: IN_TRANSIT, happenedAt: \"2026-02-01T10:00:00Z\"}) {
            fulfillmentEvent { id status happenedAt } userErrors { field message } } }",
        json!({"id": fulfillment}),
    );
    assert_eq!(
        event["data"]["fulfillmentEventCreate"]["fulfillmentEvent"]["status"],
        json!("IN_TRANSIT")
    );
    assert_eq!(topics(&mut shop), ["fulfillment_events/create", "fulfillments/update"]);
    let status = run(
        &mut shop,
        "query($id: ID!) { fulfillment(id: $id) { displayStatus inTransitAt events(first: 5) { nodes { status } } } }",
        json!({"id": fulfillment}),
    );
    assert_eq!(
        status["data"]["fulfillment"],
        json!({"displayStatus": "IN_TRANSIT", "inTransitAt": "2026-02-01T10:00:00Z", "events": {"nodes": [{"status": "IN_TRANSIT"}]}})
    );

    let cancel = "mutation($id: ID!) { fulfillmentCancel(id: $id) { fulfillment { status displayStatus } userErrors { field message } } }";
    let cancelled = run(&mut shop, cancel, json!({"id": fulfillment}));
    assert_eq!(
        cancelled["data"]["fulfillmentCancel"],
        json!({"fulfillment": {"status": "CANCELLED", "displayStatus": "CANCELED"}, "userErrors": []})
    );
    let after = order(&mut shop, &id);
    assert_eq!(after["displayFulfillmentStatus"], json!("UNFULFILLED"));
    // The fulfillment order had closed: the units are on a new one, open.
    let fos = after["fulfillmentOrders"]["nodes"].as_array().unwrap();
    assert_eq!(fos.len(), 2);
    assert_eq!(fos[0]["status"], json!("CLOSED"));
    assert_eq!(fos[1]["status"], json!("OPEN"));
    assert_eq!(topics(&mut shop), ["fulfillments/update", "orders/updated"]);
    let again = run(&mut shop, cancel, json!({"id": fulfillment}));
    assert_eq!(
        again["data"]["fulfillmentCancel"]["userErrors"][0]["message"],
        json!("Fulfillment cannot be cancelled")
    );
}

#[test]
fn fulfillment_orders_are_held_released_and_moved() {
    let mut shop = demo();
    let id = order_named(&shop, "#1002");
    let fo = order(&mut shop, &id)["fulfillmentOrders"]["nodes"][0].clone();
    let hold = "mutation($id: ID!, $hold: FulfillmentOrderHoldInput!) { fulfillmentOrderHold(id: $id, fulfillmentHold: $hold) {
        fulfillmentHold { id reason reasonNotes displayReason handle heldByRequestingApp }
        fulfillmentOrder { status } remainingFulfillmentOrder { id } userErrors { field message code } } }";
    let held = run(
        &mut shop,
        hold,
        json!({"id": fo["id"], "hold": {"reason": "AWAITING_PAYMENT", "reasonNotes": "Bank transfer", "handle": "payment"}}),
    );
    let payload = &held["data"]["fulfillmentOrderHold"];
    assert_eq!(payload["userErrors"], json!([]), "{held}");
    assert_eq!(payload["fulfillmentOrder"]["status"], json!("ON_HOLD"));
    assert_eq!(payload["fulfillmentHold"]["displayReason"], json!("Awaiting payment"));
    assert_eq!(payload["remainingFulfillmentOrder"], Value::Null);
    assert_eq!(order(&mut shop, &id)["displayFulfillmentStatus"], json!("ON_HOLD"));
    assert_eq!(
        topics(&mut shop),
        ["fulfillment_holds/added", "fulfillment_orders/placed_on_hold"]
    );
    let again = run(
        &mut shop,
        hold,
        json!({"id": fo["id"], "hold": {"reason": "OTHER", "handle": "payment"}}),
    );
    assert_eq!(
        again["data"]["fulfillmentOrderHold"]["userErrors"][0]["code"],
        json!("DUPLICATE_FULFILLMENT_HOLD_HANDLE")
    );
    let released = run(
        &mut shop,
        "mutation($id: ID!) { fulfillmentOrderReleaseHold(id: $id) { fulfillmentOrder { status fulfillmentHolds { id } } userErrors { field message code } } }",
        json!({"id": fo["id"]}),
    );
    assert_eq!(
        released["data"]["fulfillmentOrderReleaseHold"],
        json!({"fulfillmentOrder": {"status": "OPEN", "fulfillmentHolds": []}, "userErrors": []})
    );
    assert_eq!(
        topics(&mut shop),
        ["fulfillment_holds/released", "fulfillment_orders/hold_released"]
    );

    let other = shop
        .keys("Location")
        .find(|location| Some(json!(location)) != Some(fo["assignedLocation"]["location"]["id"].clone()))
        .unwrap()
        .to_string();
    let moved = run(
        &mut shop,
        "mutation($id: ID!, $to: ID!) { fulfillmentOrderMove(id: $id, newLocationId: $to) {
            movedFulfillmentOrder { id assignedLocation { location { id } } } originalFulfillmentOrder { id } userErrors { field message } } }",
        json!({"id": fo["id"], "to": other}),
    );
    let payload = &moved["data"]["fulfillmentOrderMove"];
    assert_eq!(payload["userErrors"], json!([]), "{moved}");
    // Nothing of it shipped and all of it moved: the same fulfillment order, at the new place.
    assert_eq!(payload["movedFulfillmentOrder"]["id"], fo["id"]);
    assert_eq!(payload["movedFulfillmentOrder"]["assignedLocation"]["location"]["id"], json!(other));
    assert_eq!(topics(&mut shop), ["fulfillment_orders/moved"]);
}

const ORDER_CANCEL: &str = "mutation($id: ID!, $restock: Boolean!, $refund: Boolean!) {
    orderCancel(orderId: $id, reason: CUSTOMER, restock: $restock, refundMethod: {originalPaymentMethodsRefund: $refund}, staffNote: \"Asked by phone\") {
      job { id done } jobResult { id } orderCancelUserErrors { field message code } userErrors { field message } } }";

#[test]
fn order_cancel_voids_refunds_and_restocks() {
    let mut shop = demo();
    let id = order_named(&shop, "#1002");
    let before = order(&mut shop, &id);
    let line = before["lineItems"]["nodes"][0].clone();
    let item = model::inventory_item_of(&shop, shop.get(line["id"].as_str().unwrap()).unwrap()).unwrap();
    let available_before = available(&shop, &item);
    let answer = run(&mut shop, ORDER_CANCEL, json!({"id": id, "restock": true, "refund": true}));
    let payload = &answer["data"]["orderCancel"];
    assert_eq!(payload["orderCancelUserErrors"], json!([]), "{answer}");
    assert_eq!(payload["job"]["done"], json!(true));
    let result = run(
        &mut shop,
        "query($id: ID!) { node(id: $id) { ... on OrderCancelJobResult { done status order { id } } } }",
        json!({"id": payload["jobResult"]["id"]}),
    );
    assert_eq!(
        result["data"]["node"],
        json!({"done": true, "status": "SUCCEEDED", "order": {"id": id}})
    );
    let after = order(&mut shop, &id);
    assert_eq!(after["cancelReason"], json!("CUSTOMER"));
    assert!(after["cancelledAt"].is_string());
    assert_eq!(after["displayFinancialStatus"], json!("REFUNDED"));
    assert_eq!(after["totalRefundedSet"], before["totalReceivedSet"]);
    assert_eq!(amount(&after["currentTotalPriceSet"]), "0.0");
    assert_eq!(amount(&after["totalOutstandingSet"]), "0.0");
    assert_eq!(after["lineItems"]["nodes"][0]["currentQuantity"], json!(0));
    assert_eq!(after["fulfillmentOrders"]["nodes"][0]["status"], json!("CANCELLED"));
    let refund = after["transactions"].as_array().unwrap().last().unwrap().clone();
    assert_eq!(refund["kind"], json!("REFUND"));
    assert_eq!(refund["parentTransaction"]["id"], before["transactions"][0]["id"]);
    let tracked = line_is_tracked(&shop, &item);
    let restocked = if tracked { line["quantity"].as_i64().unwrap() } else { 0 };
    assert_eq!(available(&shop, &item), available_before + restocked);
    // The units go back in stock, then the money goes back.
    let mut expected = if tracked { vec!["inventory_levels/update"] } else { Vec::new() };
    expected.extend([
        "order_transactions/create",
        "refunds/create",
        "fulfillment_orders/cancelled",
        "orders/cancelled",
        "orders/updated",
    ]);
    assert_eq!(topics(&mut shop), expected);
    let again = run(&mut shop, ORDER_CANCEL, json!({"id": id, "restock": false, "refund": false}));
    assert_eq!(
        again["data"]["orderCancel"]["orderCancelUserErrors"],
        json!([{"field": ["orderId"], "message": "Order has already been cancelled", "code": "INVALID"}])
    );
    assert_eq!(again["data"]["orderCancel"]["userErrors"][0]["message"], json!("Order has already been cancelled"));
}

fn line_is_tracked(shop: &ShopState, item: &str) -> bool {
    shop.get(item).unwrap()["tracked"] == json!(true)
}

#[test]
fn order_close_open_and_delete() {
    let mut shop = demo();
    let id = order_named(&shop, "#1003");
    let delete = "mutation($id: ID!) { orderDelete(orderId: $id) { deletedId userErrors { field message code } } }";
    let refused = run(&mut shop, delete, json!({"id": id}));
    assert_eq!(refused["data"]["orderDelete"]["deletedId"], Value::Null);
    assert_eq!(refused["data"]["orderDelete"]["userErrors"][0]["code"], json!("INVALID"));
    let closed = run(
        &mut shop,
        "mutation($id: ID!) { orderClose(input: {id: $id}) { order { closed closedAt } userErrors { field message } } }",
        json!({"id": id}),
    );
    assert_eq!(closed["data"]["orderClose"]["order"]["closed"], json!(true));
    assert_eq!(topics(&mut shop), ["orders/updated"]);
    let opened = run(
        &mut shop,
        "mutation($id: ID!) { orderOpen(input: {id: $id}) { order { closed closedAt } userErrors { field message } } }",
        json!({"id": id}),
    );
    assert_eq!(opened["data"]["orderOpen"]["order"], json!({"closed": false, "closedAt": null}));
    run(
        &mut shop,
        "mutation($id: ID!) { orderClose(input: {id: $id}) { userErrors { message } } }",
        json!({"id": id}),
    );
    events(&mut shop);
    let customer = shop.get(&id).unwrap()["customer"].as_str().unwrap().to_string();
    let count = shop.get(&customer).unwrap()["numberOfOrders"].clone();
    let deleted = run(&mut shop, delete, json!({"id": id}));
    assert_eq!(deleted["data"]["orderDelete"], json!({"deletedId": id, "userErrors": []}));
    assert!(shop.get(&id).is_none());
    assert_ne!(shop.get(&customer).unwrap()["numberOfOrders"], count);
    let fired = shop.take_events();
    assert_eq!(fired.len(), 1);
    assert_eq!(fired[0].topic, "orders/delete");
    assert_eq!(fired[0].snapshot.as_ref().unwrap()["name"], json!("#1003"));
    let missing = run(&mut shop, delete, json!({"id": id}));
    assert_eq!(
        missing["data"]["orderDelete"]["userErrors"],
        json!([{"field": ["orderId"], "message": "Order does not exist", "code": "NOT_FOUND"}])
    );
}

#[test]
fn order_update_changes_what_is_given() {
    let mut shop = demo();
    let id = order_named(&shop, "#1002");
    let answer = run(
        &mut shop,
        "mutation($input: OrderInput!) { orderUpdate(input: $input) {
            order { note tags email poNumber customAttributes { key value } shippingAddress { address1 city countryCodeV2 formatted }
              fulfillmentOrders(first: 1) { nodes { destination { address1 city countryCode } } } }
            userErrors { field message } } }",
        json!({"input": {
            "id": id, "note": "Gift wrap", "tags": ["vip", "rush, vip"], "poNumber": "PO-7",
            "customAttributes": [{"key": "gift", "value": "yes"}],
            "shippingAddress": {"address1": "1 rue de Rivoli", "city": "Paris", "zip": "75001", "countryCode": "FR", "firstName": "Ana"},
        }}),
    );
    let order = &answer["data"]["orderUpdate"]["order"];
    assert_eq!(answer["data"]["orderUpdate"]["userErrors"], json!([]), "{answer}");
    assert_eq!(order["note"], json!("Gift wrap"));
    assert_eq!(order["tags"], json!(["vip", "rush"]));
    assert_eq!(order["poNumber"], json!("PO-7"));
    assert_eq!(order["customAttributes"], json!([{"key": "gift", "value": "yes"}]));
    assert_eq!(order["shippingAddress"]["city"], json!("Paris"));
    assert_eq!(order["shippingAddress"]["countryCodeV2"], json!("FR"));
    assert_eq!(
        order["fulfillmentOrders"]["nodes"][0]["destination"],
        json!({"address1": "1 rue de Rivoli", "city": "Paris", "countryCode": "FR"})
    );
    assert_eq!(topics(&mut shop), ["orders/updated"]);
    let missing = run(
        &mut shop,
        "mutation { orderUpdate(input: {id: \"gid://shopify/Order/1\", note: \"x\"}) { order { id } userErrors { field message } } }",
        Value::Null,
    );
    assert_eq!(
        missing["data"]["orderUpdate"],
        json!({"order": null, "userErrors": [{"field": ["id"], "message": "Order does not exist"}]})
    );
    assert!(shop.take_events().is_empty());
}

const ORDER_CREATE: &str = "mutation($order: OrderCreateOrderInput!, $options: OrderCreateOptionsInput) {
    orderCreate(order: $order, options: $options) { order { id } userErrors { field message code } } }";

fn first_variant(shop: &ShopState) -> (String, String) {
    let variant = shop.keys("ProductVariant").next().unwrap().to_string();
    let item = shop.get(&variant).unwrap()["inventoryItem"].as_str().unwrap().to_string();
    (variant, item)
}

#[test]
fn order_create_imports_an_order_with_its_money_and_stock() {
    let mut shop = demo();
    let (variant, item) = first_variant(&shop);
    let available = model::inventory_quantity(&shop, &item, None, "available");
    let customer = shop.keys("Customer").next().unwrap().to_string();
    let answer = run(
        &mut shop,
        ORDER_CREATE,
        json!({
            "order": {
                "lineItems": [
                    {"variantId": variant, "quantity": 2},
                    {"title": "Engraving", "quantity": 1, "priceSet": {"shopMoney": {"amount": "5.00", "currencyCode": "EUR"}}},
                ],
                "customer": {"toAssociate": {"id": customer}},
                "shippingLines": [{"title": "Express", "priceSet": {"shopMoney": {"amount": "10.00", "currencyCode": "EUR"}}}],
                "transactions": [{"kind": "SALE", "status": "SUCCESS", "gateway": "bogus", "amountSet": {"shopMoney": {"amount": "1.00", "currencyCode": "EUR"}}}],
                "tags": ["imported"],
                "test": true,
            },
            "options": {"inventoryBehaviour": "DECREMENT_IGNORING_POLICY"},
        }),
    );
    assert_eq!(answer["data"]["orderCreate"]["userErrors"], json!([]), "{answer}");
    let id = answer["data"]["orderCreate"]["order"]["id"].as_str().unwrap().to_string();
    let created = order(&mut shop, &id);
    assert_eq!(created["name"], json!("#1004"));
    assert_eq!(created["displayFinancialStatus"], json!("PARTIALLY_PAID"));
    assert_eq!(amount(&created["totalReceivedSet"]), "1.0");
    assert_eq!(created["transactions"][0]["gateway"], json!("bogus"));
    assert_eq!(shop.get(&id).unwrap()["customer"], json!(customer));
    assert_eq!(shop.get(&id).unwrap()["tags"], json!(["imported"]));
    let price = crate::domains::catalog::cents_of(shop.get(&variant).unwrap().get("price")).unwrap();
    assert_eq!(
        model::field_cents(shop.get(&id).unwrap(), "totalPriceSet"),
        2 * price + 500 + 1000
    );
    if line_is_tracked(&shop, &item) {
        assert_eq!(model::inventory_quantity(&shop, &item, None, "available"), available - 2);
        assert_eq!(model::inventory_quantity(&shop, &item, None, "committed"), 2);
    }
    let fired = topics(&mut shop);
    assert_eq!(fired[0], "orders/create");
    assert!(fired.contains(&"order_transactions/create".to_string()));
    assert!(!fired.contains(&"orders/paid".to_string()));

    let paid = run(
        &mut shop,
        ORDER_CREATE,
        json!({"order": {"lineItems": [{"variantId": variant, "quantity": 1}], "financialStatus": "PAID"}}),
    );
    let paid_id = paid["data"]["orderCreate"]["order"]["id"].as_str().unwrap().to_string();
    assert_eq!(order(&mut shop, &paid_id)["displayFinancialStatus"], json!("PAID"));
    assert_eq!(
        topics(&mut shop),
        ["orders/create", "order_transactions/create", "orders/paid"]
    );

    let wrong = run(
        &mut shop,
        ORDER_CREATE,
        json!({"order": {"lineItems": [{"title": " ", "quantity": 1, "priceSet": {"shopMoney": {"amount": "1", "currencyCode": "EUR"}}}, {"variantId": "gid://shopify/ProductVariant/1", "quantity": 1}]}}),
    );
    assert_eq!(
        wrong["data"]["orderCreate"],
        json!({"order": null, "userErrors": [
            {"field": ["order", "lineItems", "0", "title"], "message": "Title can't be blank", "code": "INVALID"},
            {"field": ["order", "lineItems", "1", "variantId"], "message": "Variant does not exist", "code": "INVALID"},
        ]})
    );
    assert!(shop.take_events().is_empty());
}

#[test]
fn pending_orders_are_marked_as_paid_and_authorizations_captured_or_voided() {
    let mut shop = demo();
    let (variant, _) = first_variant(&shop);
    let create = |shop: &mut ShopState, status: &str| -> String {
        let answer = run(
            shop,
            ORDER_CREATE,
            json!({"order": {"lineItems": [{"variantId": variant, "quantity": 1}], "financialStatus": status}}),
        );
        events(shop);
        answer["data"]["orderCreate"]["order"]["id"].as_str().unwrap().to_string()
    };
    let mark = "mutation($id: ID!) { orderMarkAsPaid(input: {id: $id}) { order { id displayFinancialStatus } userErrors { field message } } }";
    let pending = create(&mut shop, "PENDING");
    let marked = run(&mut shop, mark, json!({"id": pending}));
    assert_eq!(
        marked["data"]["orderMarkAsPaid"],
        json!({"order": {"id": pending, "displayFinancialStatus": "PAID"}, "userErrors": []})
    );
    let paid = order(&mut shop, &pending);
    assert_eq!(paid["transactions"][0]["kind"], json!("SALE"));
    assert_eq!(paid["transactions"][0]["gateway"], json!("manual"));
    assert_eq!(amount(&paid["totalOutstandingSet"]), "0.0");
    assert_eq!(
        topics(&mut shop),
        ["order_transactions/create", "orders/paid", "orders/updated"]
    );
    let again = run(&mut shop, mark, json!({"id": pending}));
    assert_eq!(
        again["data"]["orderMarkAsPaid"],
        json!({"order": {"id": pending, "displayFinancialStatus": "PAID"}, "userErrors": [{"field": ["id"], "message": "Order cannot be marked as paid."}]})
    );

    let authorized = create(&mut shop, "AUTHORIZED");
    let before = order(&mut shop, &authorized);
    assert_eq!(before["displayFinancialStatus"], json!("AUTHORIZED"));
    let authorization = before["transactions"][0]["id"].as_str().unwrap().to_string();
    let total = amount(&before["totalCapturableSet"]).to_string();
    let capture = "mutation($input: OrderCaptureInput!) { orderCapture(input: $input) { transaction { kind amountSet { shopMoney { amount } } parentTransaction { id } } userErrors { field message } } }";
    let too_much = run(
        &mut shop,
        capture,
        json!({"input": {"id": authorized, "parentTransactionId": authorization, "amount": "100000"}}),
    );
    assert_eq!(too_much["data"]["orderCapture"]["transaction"], Value::Null);
    assert_eq!(too_much["data"]["orderCapture"]["userErrors"][0]["field"], json!(["amount"]));
    let captured = run(
        &mut shop,
        capture,
        json!({"input": {"id": authorized, "parentTransactionId": authorization, "amount": total}}),
    );
    assert_eq!(
        captured["data"]["orderCapture"]["transaction"]["kind"],
        json!("CAPTURE")
    );
    assert_eq!(order(&mut shop, &authorized)["displayFinancialStatus"], json!("PAID"));
    assert_eq!(
        topics(&mut shop),
        ["order_transactions/create", "orders/paid", "orders/updated"]
    );

    let voidable = create(&mut shop, "AUTHORIZED");
    let authorization = order(&mut shop, &voidable)["transactions"][0]["id"].clone();
    let void = "mutation($id: ID!) { transactionVoid(parentTransactionId: $id) { transaction { kind } userErrors { field message code } } }";
    let voided = run(&mut shop, void, json!({"id": authorization}));
    assert_eq!(
        voided["data"]["transactionVoid"],
        json!({"transaction": {"kind": "VOID"}, "userErrors": []})
    );
    assert_eq!(order(&mut shop, &voidable)["displayFinancialStatus"], json!("VOIDED"));
    let twice = run(&mut shop, void, json!({"id": authorization}));
    assert_eq!(
        twice["data"]["transactionVoid"]["userErrors"],
        json!([{"field": ["parentTransactionId"], "message": "Transaction must be voidable.", "code": "AUTH_NOT_VOIDABLE"}])
    );
}

const REFUND_CREATE: &str = "mutation($input: RefundInput!) { refundCreate(input: $input) {
    refund { id totalRefundedSet { shopMoney { amount } } refundLineItems(first: 5) { nodes { quantity restockType restocked lineItem { id } subtotalSet { shopMoney { amount } } } } transactions(first: 5) { nodes { kind gateway } } }
    order { id displayFinancialStatus }
    userErrors { field message } } }";

#[test]
fn refund_create_takes_back_lines_and_money() {
    let mut shop = demo();
    let id = order_named(&shop, "#1001");
    let before = order(&mut shop, &id);
    let line = before["lineItems"]["nodes"][0].clone();
    let sale = before["transactions"][0].clone();
    let suggested = run(
        &mut shop,
        "query($id: ID!, $line: ID!) { order(id: $id) { suggestedRefund(refundLineItems: [{lineItemId: $line, quantity: 1, restockType: RETURN}]) {
            amountSet { shopMoney { amount } } subtotalSet { shopMoney { amount } }
            refundLineItems { quantity restockType lineItem { id } }
            suggestedTransactions { kind gateway amountSet { shopMoney { amount } } parentTransaction { id } } } } }",
        json!({"id": id, "line": line["id"]}),
    );
    let suggestion = &suggested["data"]["order"]["suggestedRefund"];
    let refund_amount = suggestion["amountSet"]["shopMoney"]["amount"].as_str().unwrap().to_string();
    assert_eq!(suggestion["suggestedTransactions"][0]["parentTransaction"]["id"], sale["id"]);
    assert_eq!(suggestion["suggestedTransactions"][0]["kind"], json!("SUGGESTED_REFUND"));
    let answer = run(
        &mut shop,
        REFUND_CREATE,
        json!({"input": {
            "orderId": id,
            "note": "Damaged",
            "refundLineItems": [{"lineItemId": line["id"], "quantity": 1, "restockType": "RETURN"}],
            "transactions": [{"orderId": id, "kind": "REFUND", "gateway": "manual", "parentId": sale["id"], "amount": refund_amount}],
        }}),
    );
    let payload = &answer["data"]["refundCreate"];
    assert_eq!(payload["userErrors"], json!([]), "{answer}");
    assert_eq!(payload["refund"]["totalRefundedSet"]["shopMoney"]["amount"], json!(refund_amount));
    assert_eq!(payload["refund"]["refundLineItems"]["nodes"][0]["restocked"], json!(true));
    assert_eq!(payload["order"]["displayFinancialStatus"], json!("PARTIALLY_REFUNDED"));
    let after = order(&mut shop, &id);
    assert_eq!(
        after["lineItems"]["nodes"][0]["currentQuantity"],
        json!(line["quantity"].as_i64().unwrap() - 1)
    );
    assert_eq!(amount(&after["totalOutstandingSet"]), "0.0");
    let fired = topics(&mut shop);
    assert!(fired.contains(&"order_transactions/create".to_string()), "{fired:?}");
    assert_eq!(&fired[fired.len() - 2..], ["refunds/create", "orders/updated"]);

    let errors = |shop: &mut ShopState, input: Value| -> Value {
        run(shop, REFUND_CREATE, json!({"input": input}))["data"]["refundCreate"].clone()
    };
    assert_eq!(
        errors(&mut shop, json!({"orderId": id}))["userErrors"],
        json!([{"field": null, "message": "either refund line items or refund duties or transactions or refund methods must be present"}])
    );
    assert_eq!(
        errors(&mut shop, json!({"orderId": id, "refundLineItems": [{"lineItemId": line["id"], "quantity": 99}]}))["userErrors"],
        json!([{"field": ["refundLineItems", "0", "quantity"], "message": "cannot refund more items than were purchased"}])
    );
    assert_eq!(
        errors(&mut shop, json!({"orderId": id, "refundLineItems": [{"lineItemId": line["id"], "quantity": 1}], "shipping": {"amount": "1000"}}))["userErrors"],
        json!([{"field": ["shipping"], "message": "Shipping must have amount less than or equal to remaining shipping"}])
    );
    assert!(shop.take_events().is_empty());
    assert_eq!(order(&mut shop, &id), after);
}
