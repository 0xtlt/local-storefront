//! The orders family through queries and mutations over the demo store and the rich fixture.

use serde_json::{Value, json};

use super::model;
pub(super) use crate::domains::test_support::{demo, rich, run, topics};
use crate::graphql::{self, Request};
use crate::resolve::{Registry, ShopResolver};
use crate::state::ShopState;

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

/// The topics of the events emitted since the last call, with the GIDs they are about.
pub(super) fn events(shop: &mut ShopState) -> Vec<(String, String)> {
    shop.take_events()
        .into_iter()
        .map(|event| (event.topic, event.gid))
        .collect()
}

/// The orders of the demo store: `#1001` (paid, fulfilled), `#1002` and `#1003` (paid,
/// unfulfilled).
pub(super) fn order_named(shop: &ShopState, name: &str) -> String {
    shop.keys("Order")
        .find(|key| shop.get(key).unwrap()["name"] == json!(name))
        .unwrap()
        .to_string()
}

/// An order of the demo store by its name, as [`ORDER_FIELDS`] reads it.
pub(super) fn named(shop: &mut ShopState, name: &str) -> Value {
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

pub(super) fn amount(value: &Value) -> &str {
    value["shopMoney"]["amount"].as_str().unwrap()
}

pub(super) const ORDER_FIELDS: &str = "id name displayFinancialStatus displayFulfillmentStatus closed closedAt cancelledAt cancelReason
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

pub(super) fn order(shop: &mut ShopState, id: &str) -> Value {
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
    assert_eq!(
        reversed["data"]["orders"]["nodes"][0]["name"],
        json!("#1003")
    );
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
    assert!(
        actions.starts_with(&["CREATE_FULFILLMENT", "HOLD", "MOVE"]),
        "{actions:?}"
    );
    let fulfilled = named(&mut shop, "#1001");
    assert_eq!(
        fulfilled["fulfillmentOrders"]["nodes"][0]["status"],
        json!("CLOSED")
    );
    assert_eq!(
        fulfilled["fulfillmentOrders"]["nodes"][0]["supportedActions"],
        json!([])
    );
    let open = run(
        &mut shop,
        "{ fulfillmentOrders(first: 10) { nodes { id status } } all: fulfillmentOrders(first: 10, includeClosed: true) { nodes { id } } }",
        Value::Null,
    );
    assert_eq!(
        open["data"]["fulfillmentOrders"]["nodes"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
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
    assert_eq!(
        fulfillment["location"]["id"],
        fo["assignedLocation"]["location"]["id"]
    );
    assert_eq!(
        fulfillment["trackingInfo"],
        json!([{"company": "UPS", "number": "1Z999", "url": "https://www.ups.com/WebTracking?loc=en_US&requester=ST&trackNums=1Z999"}])
    );
    let after = order(&mut shop, &id);
    let partial = first < total;
    assert_eq!(
        after["displayFulfillmentStatus"],
        json!(if partial {
            "PARTIALLY_FULFILLED"
        } else {
            "FULFILLED"
        })
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
            if partial {
                "orders/partially_fulfilled"
            } else {
                "orders/fulfilled"
            },
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
        assert_eq!(
            done["fulfillmentOrders"]["nodes"][0]["status"],
            json!("CLOSED")
        );
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
    let closed_fo = closed["fulfillmentOrders"]["nodes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
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
    assert_eq!(
        topics(&mut shop),
        ["fulfillment_events/create", "fulfillments/update"]
    );
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
    assert_eq!(
        payload["fulfillmentHold"]["displayReason"],
        json!("Awaiting payment")
    );
    assert_eq!(payload["remainingFulfillmentOrder"], Value::Null);
    assert_eq!(
        order(&mut shop, &id)["displayFulfillmentStatus"],
        json!("ON_HOLD")
    );
    assert_eq!(
        topics(&mut shop),
        [
            "fulfillment_holds/added",
            "fulfillment_orders/placed_on_hold"
        ]
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
        [
            "fulfillment_holds/released",
            "fulfillment_orders/hold_released"
        ]
    );

    let other = shop
        .keys("Location")
        .find(|location| {
            Some(json!(location)) != Some(fo["assignedLocation"]["location"]["id"].clone())
        })
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
    assert_eq!(
        payload["movedFulfillmentOrder"]["assignedLocation"]["location"]["id"],
        json!(other)
    );
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
    let item =
        model::inventory_item_of(&shop, shop.get(line["id"].as_str().unwrap()).unwrap()).unwrap();
    let available_before = available(&shop, &item);
    let answer = run(
        &mut shop,
        ORDER_CANCEL,
        json!({"id": id, "restock": true, "refund": true}),
    );
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
    assert_eq!(
        after["fulfillmentOrders"]["nodes"][0]["status"],
        json!("CANCELLED")
    );
    let refund = after["transactions"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(refund["kind"], json!("REFUND"));
    assert_eq!(
        refund["parentTransaction"]["id"],
        before["transactions"][0]["id"]
    );
    // An order of the store data committed no stock: restocking it puts nothing back.
    assert_eq!(available(&shop, &item), available_before);
    assert_eq!(
        topics(&mut shop),
        [
            "order_transactions/create",
            "tender_transactions/create",
            "refunds/create",
            "fulfillment_orders/cancelled",
            "orders/cancelled",
            "orders/updated",
        ]
    );
    let again = run(
        &mut shop,
        ORDER_CANCEL,
        json!({"id": id, "restock": false, "refund": false}),
    );
    assert_eq!(
        again["data"]["orderCancel"]["orderCancelUserErrors"],
        json!([{"field": ["orderId"], "message": "Order has already been cancelled", "code": "INVALID"}])
    );
    assert_eq!(
        again["data"]["orderCancel"]["userErrors"][0]["message"],
        json!("Order has already been cancelled")
    );
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
    assert_eq!(
        refused["data"]["orderDelete"]["userErrors"][0]["code"],
        json!("INVALID")
    );
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
    assert_eq!(
        opened["data"]["orderOpen"]["order"],
        json!({"closed": false, "closedAt": null})
    );
    run(
        &mut shop,
        "mutation($id: ID!) { orderClose(input: {id: $id}) { userErrors { message } } }",
        json!({"id": id}),
    );
    events(&mut shop);
    let customer = shop.get(&id).unwrap()["customer"]
        .as_str()
        .unwrap()
        .to_string();
    let count = shop.get(&customer).unwrap()["numberOfOrders"].clone();
    let deleted = run(&mut shop, delete, json!({"id": id}));
    assert_eq!(
        deleted["data"]["orderDelete"],
        json!({"deletedId": id, "userErrors": []})
    );
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
    assert_eq!(
        answer["data"]["orderUpdate"]["userErrors"],
        json!([]),
        "{answer}"
    );
    assert_eq!(order["note"], json!("Gift wrap"));
    assert_eq!(order["tags"], json!(["vip", "rush"]));
    assert_eq!(order["poNumber"], json!("PO-7"));
    assert_eq!(
        order["customAttributes"],
        json!([{"key": "gift", "value": "yes"}])
    );
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
    let item = shop.get(&variant).unwrap()["inventoryItem"]
        .as_str()
        .unwrap()
        .to_string();
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
    assert_eq!(
        answer["data"]["orderCreate"]["userErrors"],
        json!([]),
        "{answer}"
    );
    let id = answer["data"]["orderCreate"]["order"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let created = order(&mut shop, &id);
    assert_eq!(created["name"], json!("#1004"));
    assert_eq!(created["displayFinancialStatus"], json!("PARTIALLY_PAID"));
    assert_eq!(amount(&created["totalReceivedSet"]), "1.0");
    assert_eq!(created["transactions"][0]["gateway"], json!("bogus"));
    assert_eq!(shop.get(&id).unwrap()["customer"], json!(customer));
    assert_eq!(shop.get(&id).unwrap()["tags"], json!(["imported"]));
    let price = crate::state::value::exact_cents(shop.get(&variant).unwrap().get("price")).unwrap();
    assert_eq!(
        model::field_cents(shop.get(&id).unwrap(), "totalPriceSet"),
        2 * price + 500 + 1000
    );
    if line_is_tracked(&shop, &item) {
        assert_eq!(
            model::inventory_quantity(&shop, &item, None, "available"),
            available - 2
        );
        assert_eq!(
            model::inventory_quantity(&shop, &item, None, "committed"),
            2
        );
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
    let paid_id = paid["data"]["orderCreate"]["order"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        order(&mut shop, &paid_id)["displayFinancialStatus"],
        json!("PAID")
    );
    assert_eq!(
        topics(&mut shop),
        [
            "orders/create",
            "order_transactions/create",
            "tender_transactions/create",
            "orders/paid"
        ]
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
        answer["data"]["orderCreate"]["order"]["id"]
            .as_str()
            .unwrap()
            .to_string()
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
        [
            "order_transactions/create",
            "tender_transactions/create",
            "orders/paid",
            "orders/updated"
        ]
    );
    let again = run(&mut shop, mark, json!({"id": pending}));
    assert_eq!(
        again["data"]["orderMarkAsPaid"],
        json!({"order": {"id": pending, "displayFinancialStatus": "PAID"}, "userErrors": [{"field": ["id"], "message": "Order cannot be marked as paid."}]})
    );

    let authorized = create(&mut shop, "AUTHORIZED");
    let before = order(&mut shop, &authorized);
    assert_eq!(before["displayFinancialStatus"], json!("AUTHORIZED"));
    let authorization = before["transactions"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let total = amount(&before["totalCapturableSet"]).to_string();
    let capture = "mutation($input: OrderCaptureInput!) { orderCapture(input: $input) { transaction { kind amountSet { shopMoney { amount } } parentTransaction { id } } userErrors { field message } } }";
    let too_much = run(
        &mut shop,
        capture,
        json!({"input": {"id": authorized, "parentTransactionId": authorization, "amount": "100000"}}),
    );
    assert_eq!(too_much["data"]["orderCapture"]["transaction"], Value::Null);
    assert_eq!(
        too_much["data"]["orderCapture"]["userErrors"][0]["field"],
        json!(["amount"])
    );
    let captured = run(
        &mut shop,
        capture,
        json!({"input": {"id": authorized, "parentTransactionId": authorization, "amount": total}}),
    );
    assert_eq!(
        captured["data"]["orderCapture"]["transaction"]["kind"],
        json!("CAPTURE")
    );
    assert_eq!(
        order(&mut shop, &authorized)["displayFinancialStatus"],
        json!("PAID")
    );
    assert_eq!(
        topics(&mut shop),
        [
            "order_transactions/create",
            "tender_transactions/create",
            "orders/paid",
            "orders/updated"
        ]
    );

    let voidable = create(&mut shop, "AUTHORIZED");
    let authorization = order(&mut shop, &voidable)["transactions"][0]["id"].clone();
    let void = "mutation($id: ID!) { transactionVoid(parentTransactionId: $id) { transaction { kind } userErrors { field message code } } }";
    let voided = run(&mut shop, void, json!({"id": authorization}));
    assert_eq!(
        voided["data"]["transactionVoid"],
        json!({"transaction": {"kind": "VOID"}, "userErrors": []})
    );
    assert_eq!(
        order(&mut shop, &voidable)["displayFinancialStatus"],
        json!("VOIDED")
    );
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
    let refund_amount = suggestion["amountSet"]["shopMoney"]["amount"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        suggestion["suggestedTransactions"][0]["parentTransaction"]["id"],
        sale["id"]
    );
    assert_eq!(
        suggestion["suggestedTransactions"][0]["kind"],
        json!("SUGGESTED_REFUND")
    );
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
    assert_eq!(
        payload["refund"]["totalRefundedSet"]["shopMoney"]["amount"],
        json!(refund_amount)
    );
    assert_eq!(
        payload["refund"]["refundLineItems"]["nodes"][0]["restocked"],
        json!(true)
    );
    assert_eq!(
        payload["order"]["displayFinancialStatus"],
        json!("PARTIALLY_REFUNDED")
    );
    let after = order(&mut shop, &id);
    assert_eq!(
        after["lineItems"]["nodes"][0]["currentQuantity"],
        json!(line["quantity"].as_i64().unwrap() - 1)
    );
    assert_eq!(amount(&after["totalOutstandingSet"]), "0.0");
    let fired = topics(&mut shop);
    assert!(
        fired.contains(&"order_transactions/create".to_string()),
        "{fired:?}"
    );
    assert_eq!(
        &fired[fired.len() - 2..],
        ["refunds/create", "orders/updated"]
    );

    let errors = |shop: &mut ShopState, input: Value| -> Value {
        run(shop, REFUND_CREATE, json!({"input": input}))["data"]["refundCreate"].clone()
    };
    assert_eq!(
        errors(&mut shop, json!({"orderId": id}))["userErrors"],
        json!([{"field": null, "message": "either refund line items or refund duties or transactions or refund methods must be present"}])
    );
    assert_eq!(
        errors(
            &mut shop,
            json!({"orderId": id, "refundLineItems": [{"lineItemId": line["id"], "quantity": 99}]})
        )["userErrors"],
        json!([{"field": ["refundLineItems", "0", "quantity"], "message": "cannot refund more items than were purchased"}])
    );
    assert_eq!(
        errors(
            &mut shop,
            json!({"orderId": id, "refundLineItems": [{"lineItemId": line["id"], "quantity": 1}], "shipping": {"amount": "1000"}})
        )["userErrors"],
        json!([{"field": ["shipping"], "message": "Shipping must have amount less than or equal to remaining shipping"}])
    );
    assert!(shop.take_events().is_empty());
    assert_eq!(order(&mut shop, &id), after);
}

#[test]
fn returns_are_requested_approved_processed_and_closed() {
    let mut shop = demo();
    let id = order_named(&shop, "#1001");
    let returnable = run(
        &mut shop,
        "query($id: ID!) { returnableFulfillments(orderId: $id, first: 5) { nodes { id fulfillment { id }
            returnableFulfillmentLineItems(first: 5) { nodes { quantity fulfillmentLineItem { id lineItem { id } } } } } } }",
        json!({"id": id}),
    );
    let fulfillment = &returnable["data"]["returnableFulfillments"]["nodes"][0];
    let line = fulfillment["returnableFulfillmentLineItems"]["nodes"][0].clone();
    let fulfillment_line = line["fulfillmentLineItem"]["id"].clone();
    let shipped = line["quantity"].as_i64().unwrap();
    let node = run(
        &mut shop,
        "query($id: ID!) { returnableFulfillment(id: $id) { id } }",
        json!({"id": fulfillment["id"]}),
    );
    assert_eq!(
        node["data"]["returnableFulfillment"]["id"],
        fulfillment["id"]
    );

    let request = "mutation($input: ReturnRequestInput!) { returnRequest(input: $input) {
        return { id name status totalQuantity returnLineItems(first: 5) { nodes { ... on ReturnLineItem { quantity processableQuantity customerNote } } } }
        userErrors { field message code } } }";
    let too_many = run(
        &mut shop,
        request,
        json!({"input": {"orderId": id, "returnLineItems": [{"fulfillmentLineItemId": fulfillment_line, "quantity": shipped + 1, "returnReason": "DEFECTIVE"}]}}),
    );
    assert_eq!(
        too_many["data"]["returnRequest"]["userErrors"],
        json!([{"field": ["input", "returnLineItems", "0", "quantity"], "message": "Return line item has an invalid quantity.", "code": "INVALID"}])
    );
    let unknown = run(
        &mut shop,
        request,
        json!({"input": {"orderId": id, "returnLineItems": [{"fulfillmentLineItemId": "gid://shopify/FulfillmentLineItem/1", "quantity": 1}]}}),
    );
    assert_eq!(
        unknown["data"]["returnRequest"]["userErrors"][0]["message"],
        json!("Fulfillment Line Item was not found.")
    );
    let requested = run(
        &mut shop,
        request,
        json!({"input": {"orderId": id, "returnLineItems": [{"fulfillmentLineItemId": fulfillment_line, "quantity": 1, "customerNote": "Too small"}]}}),
    );
    let created = &requested["data"]["returnRequest"]["return"];
    assert_eq!(created["status"], json!("REQUESTED"), "{requested}");
    assert_eq!(created["name"], json!("#1001-R1"));
    assert_eq!(
        created["returnLineItems"]["nodes"][0]["processableQuantity"],
        json!(0)
    );
    let return_id = created["id"].clone();
    assert_eq!(topics(&mut shop), ["returns/request"]);
    assert_eq!(
        run(
            &mut shop,
            "query($id: ID!) { order(id: $id) { returnStatus } }",
            json!({"id": id})
        )["data"]["order"]["returnStatus"],
        json!("RETURN_REQUESTED")
    );

    let approve = "mutation($id: ID!) { returnApproveRequest(input: {id: $id}) { return { status } userErrors { field message code } } }";
    let approved = run(&mut shop, approve, json!({"id": return_id}));
    assert_eq!(
        approved["data"]["returnApproveRequest"],
        json!({"return": {"status": "OPEN"}, "userErrors": []})
    );
    assert_eq!(topics(&mut shop), ["returns/approve"]);
    let twice = run(&mut shop, approve, json!({"id": return_id}));
    assert_eq!(
        twice["data"]["returnApproveRequest"]["userErrors"],
        json!([{"field": ["input", "id"], "message": "Return is not approvable. Only returns with status REQUESTED can be approved.", "code": "INVALID_STATE"}])
    );

    let return_line = run(
        &mut shop,
        "query($id: ID!) { return(id: $id) { returnLineItems(first: 1) { nodes { id } } } }",
        json!({"id": return_id}),
    )["data"]["return"]["returnLineItems"]["nodes"][0]["id"]
        .clone();
    let sale = order(&mut shop, &id)["transactions"][0]["id"].clone();
    let processed = run(
        &mut shop,
        "mutation($input: ReturnProcessInput!) { returnProcess(input: $input) {
            return { status closedAt refunds(first: 1) { nodes { totalRefundedSet { shopMoney { amount } } } } } userErrors { field message code } } }",
        json!({"input": {
            "returnId": return_id,
            "returnLineItems": [{"id": return_line, "quantity": 1}],
            "financialTransfer": {"issueRefund": {"orderTransactions": [{"parentId": sale, "transactionAmount": {"amount": "5.00", "currencyCode": "EUR"}}]}},
        }}),
    );
    let done = &processed["data"]["returnProcess"];
    assert_eq!(done["userErrors"], json!([]), "{processed}");
    assert_eq!(done["return"]["status"], json!("CLOSED"));
    assert_eq!(
        done["return"]["refunds"]["nodes"][0]["totalRefundedSet"]["shopMoney"]["amount"],
        json!("5.0")
    );
    let fired = topics(&mut shop);
    assert!(fired.contains(&"refunds/create".to_string()), "{fired:?}");
    assert!(fired.contains(&"returns/process".to_string()), "{fired:?}");
    let after = order(&mut shop, &id);
    assert_eq!(after["displayFinancialStatus"], json!("PARTIALLY_REFUNDED"));

    let reopen = run(
        &mut shop,
        "mutation($id: ID!) { returnReopen(id: $id) { return { status closedAt } userErrors { message } } }",
        json!({"id": return_id}),
    );
    assert_eq!(
        reopen["data"]["returnReopen"],
        json!({"return": {"status": "OPEN", "closedAt": null}, "userErrors": []})
    );
    let cancel = run(
        &mut shop,
        "mutation($id: ID!) { returnCancel(id: $id) { return { status } userErrors { message code } } }",
        json!({"id": return_id}),
    );
    assert_eq!(
        cancel["data"]["returnCancel"]["userErrors"][0]["code"],
        json!("INVALID_STATE")
    );
    let close = run(
        &mut shop,
        "mutation($id: ID!) { returnClose(id: $id) { return { status } userErrors { message } } }",
        json!({"id": return_id}),
    );
    assert_eq!(
        close["data"]["returnClose"]["return"]["status"],
        json!("CLOSED")
    );
    assert_eq!(topics(&mut shop), ["returns/reopen", "returns/close"]);
}

#[test]
fn a_requested_return_is_declined_and_an_open_one_cancelled() {
    let mut shop = demo();
    let id = order_named(&shop, "#1001");
    let fulfillment_line = shop
        .get(shop.get(&id).unwrap()["fulfillments"][0].as_str().unwrap())
        .unwrap()["fulfillmentLineItems"][0]
        .clone();
    let requested = run(
        &mut shop,
        "mutation($input: ReturnRequestInput!) { returnRequest(input: $input) { return { id } userErrors { message } } }",
        json!({"input": {"orderId": id, "returnLineItems": [{"fulfillmentLineItemId": fulfillment_line, "quantity": 1}]}}),
    );
    let return_id = requested["data"]["returnRequest"]["return"]["id"].clone();
    let declined = run(
        &mut shop,
        "mutation($id: ID!) { returnDeclineRequest(input: {id: $id, declineReason: FINAL_SALE, declineNote: \"Sale item\"}) { return { status decline { reason note } } userErrors { message } } }",
        json!({"id": return_id}),
    );
    assert_eq!(
        declined["data"]["returnDeclineRequest"]["return"],
        json!({"status": "DECLINED", "decline": {"reason": "FINAL_SALE", "note": "Sale item"}})
    );
    let created = run(
        &mut shop,
        "mutation($input: ReturnInput!) { returnCreate(returnInput: $input) { return { id status name } userErrors { message } } }",
        json!({"input": {"orderId": id, "returnLineItems": [{"fulfillmentLineItemId": fulfillment_line, "quantity": 1, "returnReasonNote": "Changed my mind"}]}}),
    );
    let open = &created["data"]["returnCreate"]["return"];
    assert_eq!(open["status"], json!("OPEN"), "{created}");
    assert_eq!(open["name"], json!("#1001-R2"));
    let cancelled = run(
        &mut shop,
        "mutation($id: ID!) { returnCancel(id: $id) { return { status } userErrors { message } } }",
        json!({"id": open["id"]}),
    );
    assert_eq!(
        cancelled["data"]["returnCancel"]["return"]["status"],
        json!("CANCELED")
    );
    assert_eq!(
        topics(&mut shop),
        [
            "returns/request",
            "returns/decline",
            "returns/approve",
            "returns/cancel"
        ]
    );
}

#[test]
fn an_order_edit_adds_removes_and_discounts_line_items() {
    let mut shop = demo();
    let id = order_named(&shop, "#1002");
    let before = order(&mut shop, &id);
    let existing = before["lineItems"]["nodes"][0].clone();
    let existing_quantity = existing["quantity"].as_i64().unwrap();
    let begin = run(
        &mut shop,
        "mutation($id: ID!) { orderEditBegin(id: $id) { calculatedOrder { id originalOrder { id }
            lineItems(first: 5) { nodes { id quantity editableQuantity } } } orderEditSession { id } userErrors { field message } } }",
        json!({"id": id}),
    );
    let calculated = begin["data"]["orderEditBegin"]["calculatedOrder"].clone();
    assert_eq!(calculated["originalOrder"]["id"], json!(id));
    let session = begin["data"]["orderEditBegin"]["orderEditSession"]["id"].clone();
    let calculated_line = calculated["lineItems"]["nodes"][0].clone();
    assert_eq!(
        calculated_line["editableQuantity"],
        json!(existing_quantity)
    );

    // Another variant than the line's, by the session's id.
    let line_variant = shop.get(existing["id"].as_str().unwrap()).unwrap()["variant"].clone();
    let variant = shop
        .keys("ProductVariant")
        .find(|variant| json!(variant) != line_variant)
        .unwrap()
        .to_string();
    let price = crate::state::value::exact_cents(shop.get(&variant).unwrap().get("price")).unwrap();
    let added = run(
        &mut shop,
        "mutation($id: ID!, $variant: ID!) { orderEditAddVariant(id: $id, variantId: $variant, quantity: 2) {
            calculatedLineItem { id quantity stagedChanges { __typename ... on OrderStagedChangeAddVariant { quantity } } }
            calculatedOrder { subtotalLineItemsQuantity } userErrors { field message } } }",
        json!({"id": session, "variant": variant}),
    );
    let payload = &added["data"]["orderEditAddVariant"];
    assert_eq!(payload["userErrors"], json!([]), "{added}");
    assert_eq!(
        payload["calculatedLineItem"]["stagedChanges"],
        json!([{"__typename": "OrderStagedChangeAddVariant", "quantity": 2}])
    );
    let added_line = payload["calculatedLineItem"]["id"].clone();
    let duplicate = run(
        &mut shop,
        "mutation($id: ID!, $variant: ID!) { orderEditAddVariant(id: $id, variantId: $variant, quantity: 1) { userErrors { field message } } }",
        json!({"id": calculated["id"], "variant": variant}),
    );
    assert_eq!(
        duplicate["data"]["orderEditAddVariant"]["userErrors"][0]["field"],
        json!(["variantId"])
    );
    let discounted = run(
        &mut shop,
        "mutation($id: ID!, $line: ID!) { orderEditAddLineItemDiscount(id: $id, lineItemId: $line, discount: {percentValue: 50, description: \"Goodwill\"}) {
            addedDiscountStagedChange { description value { ... on PricingPercentageValue { percentage } } }
            calculatedLineItem { hasStagedLineItemDiscount } userErrors { field message } } }",
        json!({"id": calculated["id"], "line": added_line}),
    );
    assert_eq!(
        discounted["data"]["orderEditAddLineItemDiscount"]["addedDiscountStagedChange"],
        json!({"description": "Goodwill", "value": {"percentage": 50.0}})
    );
    let removed = run(
        &mut shop,
        "mutation($id: ID!, $line: ID!) { orderEditSetQuantity(id: $id, lineItemId: $line, quantity: 0, restock: true) {
            calculatedLineItem { quantity stagedChanges { __typename ... on OrderStagedChangeDecrementItem { delta restock } } } userErrors { field message } } }",
        json!({"id": calculated["id"], "line": calculated_line["id"]}),
    );
    assert_eq!(
        removed["data"]["orderEditSetQuantity"]["calculatedLineItem"],
        json!({"quantity": 0, "stagedChanges": [{"__typename": "OrderStagedChangeDecrementItem", "delta": existing_quantity, "restock": true}]})
    );
    // Staging changes nothing of the order, and fires nothing.
    assert_eq!(order(&mut shop, &id), before);
    assert!(shop.take_events().is_empty());

    let committed = run(
        &mut shop,
        "mutation($id: ID!) { orderEditCommit(id: $id, staffNote: \"Swapped\") { order { id edited } userErrors { field message } } }",
        json!({"id": calculated["id"]}),
    );
    assert_eq!(
        committed["data"]["orderEditCommit"],
        json!({"order": {"id": id, "edited": true}, "userErrors": []})
    );
    let after = order(&mut shop, &id);
    let lines = after["lineItems"]["nodes"].as_array().unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["currentQuantity"], json!(0));
    assert_eq!(lines[1]["quantity"], json!(2));
    assert_eq!(lines[1]["unfulfilledQuantity"], json!(2));
    assert_eq!(
        amount(&after["currentSubtotalPriceSet"]),
        crate::state::value::decimal(price)
    );
    let fired = topics(&mut shop);
    assert!(fired.contains(&"refunds/create".to_string()), "{fired:?}");
    assert_eq!(
        &fired[fired.len() - 2..],
        ["orders/edited", "orders/updated"]
    );
    let again = run(
        &mut shop,
        "mutation($id: ID!) { orderEditCommit(id: $id) { order { id } userErrors { field message } } }",
        json!({"id": calculated["id"]}),
    );
    assert_eq!(
        again["data"]["orderEditCommit"]["userErrors"],
        json!([{"field": ["id"], "message": "The order edit was already committed."}])
    );
    // A unit that shipped cannot be removed.
    let fulfilled = order_named(&shop, "#1001");
    let begun = run(
        &mut shop,
        "mutation($id: ID!) { orderEditBegin(id: $id) { calculatedOrder { id lineItems(first: 1) { nodes { id editableQuantity } } } } }",
        json!({"id": fulfilled}),
    );
    let calculated = &begun["data"]["orderEditBegin"]["calculatedOrder"];
    assert_eq!(
        calculated["lineItems"]["nodes"][0]["editableQuantity"],
        json!(0)
    );
    let refused = run(
        &mut shop,
        "mutation($id: ID!, $line: ID!) { orderEditSetQuantity(id: $id, lineItemId: $line, quantity: 0) { userErrors { field message } } }",
        json!({"id": calculated["id"], "line": calculated["lineItems"]["nodes"][0]["id"]}),
    );
    assert_eq!(
        refused["data"]["orderEditSetQuantity"]["userErrors"],
        json!([{"field": ["quantity"], "message": "Quantity cannot be less than the quantity that was fulfilled"}])
    );
}

const DRAFT_FIELDS: &str = "id name status email note2 tags invoiceSentAt completedAt order { id name displayFinancialStatus }
    lineItems(first: 5) { nodes { title quantity custom sku originalUnitPriceSet { shopMoney { amount } }
      discountedTotalSet { shopMoney { amount } } appliedDiscount { valueType value amountSet { shopMoney { amount } } } } }
    lineItemsSubtotalPrice { shopMoney { amount } } subtotalPriceSet { shopMoney { amount } }
    totalDiscountsSet { shopMoney { amount } } totalShippingPriceSet { shopMoney { amount } }
    totalTaxSet { shopMoney { amount } } totalPriceSet { shopMoney { amount } } totalQuantityOfLineItems
    taxLines { title rate priceSet { shopMoney { amount } } } shippingLine { title custom }";

#[test]
fn draft_orders_are_created_calculated_updated_and_completed() {
    let mut shop = demo();
    shop.set_setting(super::TAX_RATE_SETTING, json!(2000));
    let (variant, item) = first_variant(&shop);
    let price = crate::state::value::exact_cents(shop.get(&variant).unwrap().get("price")).unwrap();
    let customer = shop.keys("Customer").next().unwrap().to_string();
    let input = json!({
        "lineItems": [
            {"variantId": variant, "quantity": 2},
            {"title": "Gift wrap", "quantity": 1, "originalUnitPriceWithCurrency": {"amount": "10.00", "currencyCode": "EUR"},
             "appliedDiscount": {"valueType": "PERCENTAGE", "value": 50.0, "title": "Half"}, "taxable": false},
        ],
        "purchasingEntity": {"customerId": customer},
        "appliedDiscount": {"valueType": "FIXED_AMOUNT", "value": 1.0, "amountWithCurrency": {"amount": "1.00", "currencyCode": "EUR"}},
        "shippingLine": {"title": "Express", "priceWithCurrency": {"amount": "7.50", "currencyCode": "EUR"}},
        "note": "Call first",
        "tags": ["wholesale"],
    });
    let calculated = run(
        &mut shop,
        "mutation($input: DraftOrderInput!) { draftOrderCalculate(input: $input) { calculatedDraftOrder {
            subtotalPriceSet { shopMoney { amount } } totalTaxSet { shopMoney { amount } } totalPriceSet { shopMoney { amount } }
            lineItems { title quantity } } userErrors { field message } } }",
        json!({"input": input}),
    );
    let calculation = &calculated["data"]["draftOrderCalculate"]["calculatedDraftOrder"];
    assert_eq!(
        calculated["data"]["draftOrderCalculate"]["userErrors"],
        json!([]),
        "{calculated}"
    );
    assert!(shop.take_events().is_empty());
    let created = run(
        &mut shop,
        &format!(
            "mutation($input: DraftOrderInput!) {{ draftOrderCreate(input: $input) {{ draftOrder {{ {DRAFT_FIELDS} }} userErrors {{ field message }} }} }}"
        ),
        json!({"input": input}),
    );
    let draft = created["data"]["draftOrderCreate"]["draftOrder"].clone();
    assert_eq!(
        created["data"]["draftOrderCreate"]["userErrors"],
        json!([]),
        "{created}"
    );
    assert_eq!(draft["name"], json!("#D1"));
    assert_eq!(draft["status"], json!("OPEN"));
    assert_eq!(draft["note2"], json!("Call first"));
    assert_eq!(draft["email"], shop.get(&customer).unwrap()["email"]);
    assert_eq!(draft["lineItems"]["nodes"][1]["custom"], json!(true));
    assert_eq!(
        draft["lineItems"]["nodes"][1]["discountedTotalSet"]["shopMoney"]["amount"],
        json!("5.0")
    );
    // Lines 2 × price + 5.00, less 1.00; tax 20 % of the taxable part; shipping 7.50.
    let subtotal = 2 * price + 500 - 100;
    let taxable = 2 * price - 100 * (2 * price) / (2 * price + 500);
    let tax = (taxable as f64 * 0.2).round() as i64;
    let decimal = crate::state::value::decimal;
    assert_eq!(amount(&draft["subtotalPriceSet"]), decimal(subtotal));
    assert_eq!(amount(&draft["totalTaxSet"]), decimal(tax));
    assert_eq!(
        amount(&draft["totalPriceSet"]),
        decimal(subtotal + 750 + tax)
    );
    assert_eq!(draft["taxLines"][0]["rate"], json!(0.2));
    assert_eq!(calculation["totalPriceSet"], draft["totalPriceSet"]);
    assert_eq!(calculation["lineItems"][0]["quantity"], json!(2));
    assert_eq!(
        draft["shippingLine"],
        json!({"title": "Express", "custom": true})
    );
    let id = draft["id"].as_str().unwrap().to_string();
    assert_eq!(
        events(&mut shop),
        [("draft_orders/create".to_string(), id.clone())]
    );

    let updated = run(
        &mut shop,
        "mutation($id: ID!) { draftOrderUpdate(id: $id, input: {note: \"Leave at the door\", email: \"buyer@example.com\"}) { draftOrder { note2 email totalPriceSet { shopMoney { amount } } } userErrors { field message } } }",
        json!({"id": id}),
    );
    assert_eq!(
        updated["data"]["draftOrderUpdate"]["draftOrder"],
        json!({"note2": "Leave at the door", "email": "buyer@example.com", "totalPriceSet": draft["totalPriceSet"]})
    );
    let sent = run(
        &mut shop,
        "mutation($id: ID!) { draftOrderInvoiceSend(id: $id) { draftOrder { status invoiceSentAt } userErrors { field message } } }",
        json!({"id": id}),
    );
    assert_eq!(
        sent["data"]["draftOrderInvoiceSend"]["draftOrder"]["status"],
        json!("INVOICE_SENT")
    );
    let copy = run(
        &mut shop,
        "mutation($id: ID!) { draftOrderDuplicate(id: $id) { draftOrder { id name status totalPriceSet { shopMoney { amount } } } userErrors { message } } }",
        json!({"id": id}),
    );
    assert_eq!(
        copy["data"]["draftOrderDuplicate"]["draftOrder"]["name"],
        json!("#D2")
    );
    assert_eq!(
        copy["data"]["draftOrderDuplicate"]["draftOrder"]["status"],
        json!("OPEN")
    );
    assert_eq!(
        copy["data"]["draftOrderDuplicate"]["draftOrder"]["totalPriceSet"],
        draft["totalPriceSet"]
    );
    assert_eq!(
        topics(&mut shop),
        [
            "draft_orders/update",
            "draft_orders/update",
            "draft_orders/create"
        ]
    );

    let available_before = available(&shop, &item);
    let completed = run(
        &mut shop,
        "mutation($id: ID!) { draftOrderComplete(id: $id) { draftOrder { status completedAt order { id name displayFinancialStatus sourceName
            totalPriceSet { shopMoney { amount } } tags } } userErrors { field message } } }",
        json!({"id": id}),
    );
    let done = &completed["data"]["draftOrderComplete"]["draftOrder"];
    assert_eq!(
        completed["data"]["draftOrderComplete"]["userErrors"],
        json!([]),
        "{completed}"
    );
    assert_eq!(done["status"], json!("COMPLETED"));
    assert_eq!(done["order"]["name"], json!("#1004"));
    assert_eq!(done["order"]["displayFinancialStatus"], json!("PAID"));
    assert_eq!(done["order"]["sourceName"], json!("shopify_draft_order"));
    assert_eq!(done["order"]["totalPriceSet"], draft["totalPriceSet"]);
    assert_eq!(done["order"]["tags"], json!(["wholesale"]));
    if line_is_tracked(&shop, &item) {
        assert_eq!(available(&shop, &item), available_before - 2);
    }
    let fired = topics(&mut shop);
    assert_eq!(fired[0], "orders/create");
    assert_eq!(fired[1], "order_transactions/create");
    assert_eq!(
        &fired[fired.len() - 2..],
        ["orders/paid", "draft_orders/update"]
    );
    let again = run(
        &mut shop,
        "mutation($id: ID!) { draftOrderComplete(id: $id) { draftOrder { id } userErrors { field message } } }",
        json!({"id": id}),
    );
    assert_eq!(
        again["data"]["draftOrderComplete"]["userErrors"],
        json!([{"field": null, "message": "This draft order has already been completed"}])
    );
}

#[test]
fn draft_orders_answer_shopifys_errors_and_are_deleted() {
    let mut shop = demo();
    let empty = run(
        &mut shop,
        "mutation { draftOrderCreate(input: {note: \"x\"}) { draftOrder { id } userErrors { field message } } }",
        Value::Null,
    );
    assert_eq!(
        empty["data"]["draftOrderCreate"],
        json!({"draftOrder": null, "userErrors": [{"field": null, "message": "Add at least 1 product"}]})
    );
    let missing = run(
        &mut shop,
        "mutation { draftOrderUpdate(id: \"gid://shopify/DraftOrder/1\", input: {note: \"x\"}) { draftOrder { id } userErrors { field message } } }",
        Value::Null,
    );
    assert_eq!(
        missing["data"]["draftOrderUpdate"],
        json!({"draftOrder": null, "userErrors": [{"field": null, "message": "Draft order not found"}]})
    );
    let custom = run(
        &mut shop,
        "mutation { draftOrderCreate(input: {lineItems: [{title: \"Service\", quantity: 1, originalUnitPriceWithCurrency: {amount: \"30\", currencyCode: EUR}}]}) { draftOrder { id } userErrors { message } } }",
        Value::Null,
    );
    let id = custom["data"]["draftOrderCreate"]["draftOrder"]["id"].clone();
    let no_email = run(
        &mut shop,
        "mutation($id: ID!) { draftOrderInvoiceSend(id: $id) { draftOrder { id status } userErrors { field message } } }",
        json!({"id": id}),
    );
    assert_eq!(
        no_email["data"]["draftOrderInvoiceSend"],
        json!({"draftOrder": {"id": id, "status": "OPEN"}, "userErrors": [{"field": null, "message": "To is invalid"}]})
    );
    events(&mut shop);
    let deleted = run(
        &mut shop,
        "mutation($id: ID!) { draftOrderDelete(input: {id: $id}) { deletedId userErrors { message } } }",
        json!({"id": id}),
    );
    assert_eq!(
        deleted["data"]["draftOrderDelete"],
        json!({"deletedId": id, "userErrors": []})
    );
    let fired = shop.take_events();
    assert_eq!(fired.len(), 1);
    assert_eq!(fired[0].topic, "draft_orders/delete");
    assert!(fired[0].snapshot.is_some());
    assert_eq!(shop.count("DraftOrderLineItem"), 0);
}

/// The payload of an event of the shop, as the webhooks build it.
fn payload_of(shop: &ShopState, topic: &str, gid: &str) -> Value {
    let record = shop.get(gid).unwrap_or_else(|| panic!("no record {gid}"));
    crate::webhooks::payload::payload(topic, record, &crate::api::samples::ShopLookup(shop))
        .unwrap_or_else(|| panic!("no payload for {topic}"))
}

#[test]
fn the_events_of_the_family_have_their_payloads() {
    let mut shop = demo();
    let id = order_named(&shop, "#1002");
    let fo = order(&mut shop, &id)["fulfillmentOrders"]["nodes"][0].clone();
    let fo_id = fo["id"].as_str().unwrap().to_string();
    run(
        &mut shop,
        "mutation($id: ID!) { fulfillmentOrderHold(id: $id, fulfillmentHold: {reason: INCORRECT_ADDRESS, handle: \"address\"}) { userErrors { message } } }",
        json!({"id": fo_id}),
    );
    let fired = events(&mut shop);
    let built: Vec<Value> = fired
        .iter()
        .map(|(topic, gid)| payload_of(&shop, topic, gid))
        .collect();
    assert_eq!(fired[0].0, "fulfillment_holds/added");
    assert_eq!(built[0]["fulfillment_order"]["id"], json!(fo_id));
    assert_eq!(
        built[0]["fulfillment_hold"]["reason"],
        json!("incorrect_address")
    );
    assert_eq!(built[0]["fulfillment_hold"]["handle"], json!("address"));
    assert_eq!(fired[1].0, "fulfillment_orders/placed_on_hold");
    assert_eq!(built[1]["fulfillment_order"]["status"], json!("on_hold"));
    assert_eq!(built[1]["remaining_fulfillment_order"], Value::Null);
    assert_eq!(
        built[1]["created_fulfillment_hold"],
        built[0]["fulfillment_hold"]
    );
    run(
        &mut shop,
        "mutation($id: ID!) { fulfillmentOrderReleaseHold(id: $id) { userErrors { message } } }",
        json!({"id": fo_id}),
    );
    events(&mut shop);
    let other = shop
        .keys("Location")
        .find(|location| json!(location) != fo["assignedLocation"]["location"]["id"])
        .unwrap()
        .to_string();
    run(
        &mut shop,
        "mutation($id: ID!, $to: ID!) { fulfillmentOrderMove(id: $id, newLocationId: $to) { userErrors { message } } }",
        json!({"id": fo_id, "to": other}),
    );
    let (topic, gid) = events(&mut shop).remove(0);
    let moved = payload_of(&shop, &topic, &gid);
    assert_eq!(moved["destination_location_id"], json!(other));
    assert_eq!(
        moved["source_location"]["id"],
        fo["assignedLocation"]["location"]["id"]
    );
    assert_eq!(moved["original_fulfillment_order"]["id"], json!(fo_id));

    let (variant, _) = first_variant(&shop);
    let created = run(
        &mut shop,
        ORDER_CREATE,
        json!({"order": {"lineItems": [{"variantId": variant, "quantity": 1}], "transactions": [{"kind": "AUTHORIZATION", "gateway": "bogus", "amountSet": {"shopMoney": {"amount": "3.00", "currencyCode": "EUR"}}}]}}),
    );
    assert_eq!(
        created["data"]["orderCreate"]["userErrors"],
        json!([]),
        "{created}"
    );
    let transaction = events(&mut shop)
        .into_iter()
        .find(|(topic, _)| topic == "order_transactions/create")
        .unwrap();
    let payload = payload_of(&shop, &transaction.0, &transaction.1);
    assert_eq!(payload["kind"], json!("authorization"));
    assert_eq!(payload["gateway"], json!("bogus"));
    assert_eq!(payload["amount"], json!("3.00"));
    assert_eq!(payload["payment_details"], Value::Null);
}

#[test]
fn a_fulfillment_service_receives_accepts_and_cancels_requests() {
    let mut shop = demo();
    let created = run(
        &mut shop,
        "mutation { fulfillmentServiceCreate(name: \"Acme 3PL\", callbackUrl: \"https://3pl.example/callback\", inventoryManagement: true, trackingSupport: true) {
            fulfillmentService { id serviceName handle location { id name isFulfillmentService } } userErrors { field message } } }",
        Value::Null,
    );
    let service = &created["data"]["fulfillmentServiceCreate"]["fulfillmentService"];
    assert_eq!(service["handle"], json!("acme-3pl"), "{created}");
    assert_eq!(service["location"]["isFulfillmentService"], json!(true));
    let location = service["location"]["id"].clone();
    assert_eq!(topics(&mut shop), ["locations/create"]);
    let taken = run(
        &mut shop,
        "mutation { fulfillmentServiceCreate(name: \"acme 3pl\") { userErrors { field message } } }",
        Value::Null,
    );
    assert_eq!(
        taken["data"]["fulfillmentServiceCreate"]["userErrors"],
        json!([{"field": ["name"], "message": "Name has already been taken"}])
    );

    // The merchant moves an order to the service and asks it to fulfill it.
    let id = order_named(&shop, "#1002");
    let fo = order(&mut shop, &id)["fulfillmentOrders"]["nodes"][0]["id"].clone();
    run(
        &mut shop,
        "mutation($id: ID!, $to: ID!) { fulfillmentOrderMove(id: $id, newLocationId: $to) { userErrors { message } } }",
        json!({"id": fo, "to": location}),
    );
    let actions = run(
        &mut shop,
        "query($id: ID!) { fulfillmentOrder(id: $id) { supportedActions { action } } }",
        json!({"id": fo}),
    );
    assert_eq!(
        actions["data"]["fulfillmentOrder"]["supportedActions"][0]["action"],
        json!("REQUEST_FULFILLMENT")
    );
    let submitted = run(
        &mut shop,
        "mutation($id: ID!) { fulfillmentOrderSubmitFulfillmentRequest(id: $id, message: \"Fragile\") {
            submittedFulfillmentOrder { id requestStatus merchantRequests(first: 1) { nodes { kind message } } } unsubmittedFulfillmentOrder { id } userErrors { field message } } }",
        json!({"id": fo}),
    );
    let payload = &submitted["data"]["fulfillmentOrderSubmitFulfillmentRequest"];
    assert_eq!(payload["userErrors"], json!([]), "{submitted}");
    assert_eq!(
        payload["submittedFulfillmentOrder"]["requestStatus"],
        json!("SUBMITTED")
    );
    assert_eq!(
        payload["submittedFulfillmentOrder"]["merchantRequests"]["nodes"],
        json!([{"kind": "FULFILLMENT_REQUEST", "message": "Fragile"}])
    );
    events(&mut shop);
    let requested = run(
        &mut shop,
        "{ assignedFulfillmentOrders(first: 5, assignmentStatus: FULFILLMENT_REQUESTED) { nodes { id } } }",
        Value::Null,
    );
    assert_eq!(
        requested["data"]["assignedFulfillmentOrders"]["nodes"],
        json!([{"id": fo}])
    );

    // The service accepts, then the merchant asks to cancel, and the service agrees.
    let accept = run(
        &mut shop,
        "mutation($id: ID!) { fulfillmentOrderAcceptFulfillmentRequest(id: $id, message: \"Ships tomorrow\") { fulfillmentOrder { status requestStatus } userErrors { message } } }",
        json!({"id": fo}),
    );
    assert_eq!(
        accept["data"]["fulfillmentOrderAcceptFulfillmentRequest"]["fulfillmentOrder"],
        json!({"status": "IN_PROGRESS", "requestStatus": "ACCEPTED"})
    );
    let (topic, gid) = events(&mut shop).remove(0);
    assert_eq!(topic, "fulfillment_orders/fulfillment_request_accepted");
    assert_eq!(
        payload_of(&shop, &topic, &gid)["message"],
        json!("Ships tomorrow")
    );
    let twice = run(
        &mut shop,
        "mutation($id: ID!) { fulfillmentOrderAcceptFulfillmentRequest(id: $id) { userErrors { field message } } }",
        json!({"id": fo}),
    );
    assert_eq!(
        twice["data"]["fulfillmentOrderAcceptFulfillmentRequest"]["userErrors"][0]["field"],
        json!(["id"])
    );
    run(
        &mut shop,
        "mutation($id: ID!) { fulfillmentOrderSubmitCancellationRequest(id: $id, message: \"Customer cancelled\") { userErrors { message } } }",
        json!({"id": fo}),
    );
    let (topic, gid) = events(&mut shop).remove(0);
    let built = payload_of(&shop, &topic, &gid);
    assert_eq!(
        built["fulfillment_order"]["request_status"],
        json!("cancellation_request")
    );
    assert_eq!(
        built["fulfillment_order_merchant_request"]["message"],
        json!("Customer cancelled")
    );
    let accepted = run(
        &mut shop,
        "mutation($id: ID!) { fulfillmentOrderAcceptCancellationRequest(id: $id) { fulfillmentOrder { status requestStatus } userErrors { message } } }",
        json!({"id": fo}),
    );
    assert_eq!(
        accepted["data"]["fulfillmentOrderAcceptCancellationRequest"]["fulfillmentOrder"],
        json!({"status": "CLOSED", "requestStatus": "CANCELLATION_ACCEPTED"})
    );
    let after = order(&mut shop, &id);
    let fos = after["fulfillmentOrders"]["nodes"].as_array().unwrap();
    assert_eq!(fos.len(), 2);
    assert_eq!(fos[1]["status"], json!("OPEN"));
    assert_eq!(after["displayFulfillmentStatus"], json!("UNFULFILLED"));

    let deleted = run(
        &mut shop,
        "mutation($id: ID!) { fulfillmentServiceDelete(id: $id, inventoryAction: DELETE) { deletedId userErrors { message } } }",
        json!({"id": service["id"]}),
    );
    assert_eq!(
        deleted["data"]["fulfillmentServiceDelete"]["deletedId"],
        service["id"]
    );
    assert!(shop.get(location.as_str().unwrap()).is_none());
    // Its open fulfillment order went back to a location of the shop.
    let moved_back = order(&mut shop, &id)["fulfillmentOrders"]["nodes"][1]["assignedLocation"]["location"]["id"].clone();
    assert!(shop.keys("Location").any(|key| json!(key) == moved_back));
}

/// The rich fixture's store and the shop seeded from it.
fn rich_with_store() -> (lsf_core::Store, ShopState) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rich");
    let (store, _) = lsf_core::store::load::load(
        &lsf_core::store::load::DataSource::Directory(path),
        &Default::default(),
    );
    let shop = ShopState::seeded(&store, None);
    (store, shop)
}

/// Sets the `available` and `on_hand` quantities of an inventory level, as
/// `inventorySetQuantities` would, and refreshes its product.
fn set_stock(shop: &mut ShopState, level: &str, product: &str, quantity: i64) {
    for entry in shop.get_mut(level).unwrap()["quantities"]
        .as_array_mut()
        .unwrap()
    {
        if entry["name"] == json!("available") || entry["name"] == json!("on_hand") {
            entry["quantity"] = json!(quantity);
        }
    }
    crate::domains::catalog::refresh_product(shop, product);
}

/// The linen dress of the rich fixture (tracked, `DENY`), its first variant and its inventory
/// item, with 4 units at Paris and none at Berlin.
fn dress_with_four_at_paris(store: &lsf_core::Store, shop: &mut ShopState) -> (String, String) {
    use crate::domains::catalog;
    let dress = store.product_by_handle("linen-dress").unwrap();
    let variant = &dress.variants[0];
    let product = catalog::product_gid(dress);
    for (location, quantity) in [(&store.locations[0], 4), (&store.locations[1], 0)] {
        let level = catalog::inventory_level_gid(variant.id, location.id);
        set_stock(shop, &level, &product, quantity);
    }
    let variant = catalog::variant_gid(variant);
    let item = shop.get(&variant).unwrap()["inventoryItem"]
        .as_str()
        .unwrap()
        .to_string();
    (variant, item)
}

/// An order that takes a variant's last units sells it out as an inventory mutation does: its
/// `availableForSale` turns false, `variants/out_of_stock` is emitted, and the storefront shows
/// it out of stock.
#[test]
fn an_order_that_takes_the_last_units_sells_the_variant_out() {
    let (store, mut shop) = rich_with_store();
    let (variant, _) = dress_with_four_at_paris(&store, &mut shop);
    shop.take_events();
    let answer = run(
        &mut shop,
        ORDER_CREATE,
        json!({
            "order": {"lineItems": [{"variantId": variant, "quantity": 4}]},
            "options": {"inventoryBehaviour": "DECREMENT_OBEYING_POLICY"},
        }),
    );
    assert_eq!(
        answer["data"]["orderCreate"]["userErrors"],
        json!([]),
        "{answer}"
    );
    let read = run(
        &mut shop,
        "query($id: ID!) { productVariant(id: $id) { inventoryQuantity availableForSale } }",
        json!({"id": variant}),
    );
    assert_eq!(
        read["data"]["productVariant"],
        json!({"inventoryQuantity": 0, "availableForSale": false})
    );
    let fired = events(&mut shop);
    assert!(
        fired.contains(&("variants/out_of_stock".to_string(), variant.clone())),
        "{fired:?}"
    );
    let overlay = crate::sync::overlay(&shop, &store).expect("the stock changed");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rich");
    let (synced, _) = lsf_core::store::load::load_with_overlays(
        &lsf_core::store::load::DataSource::Directory(path),
        &Default::default(),
        &[overlay],
    );
    let after = &synced.product_by_handle("linen-dress").unwrap().variants[0];
    assert_eq!(after.inventory_quantity, 0);
    assert!(!after.available);
}

/// The quantities of an inventory item at each of its levels, under a name.
fn quantities(shop: &ShopState, item: &str, name: &str) -> Vec<i64> {
    crate::domains::catalog::gids(shop.get(item).unwrap().get("inventoryLevels"))
        .iter()
        .map(|level| {
            shop.get(level).unwrap()["quantities"]
                .as_array()
                .unwrap()
                .iter()
                .find(|quantity| quantity["name"] == json!(name))
                .and_then(|quantity| quantity["quantity"].as_i64())
                .unwrap_or(0)
        })
        .collect()
}

/// Ships every unit of an order's first fulfillment order.
fn ship_all(shop: &mut ShopState, order_id: &str) -> Value {
    let fo = order(shop, order_id)["fulfillmentOrders"]["nodes"][0].clone();
    let lines: Vec<Value> = fo["lineItems"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| json!({"id": line["id"], "quantity": line["remainingQuantity"]}))
        .collect();
    run(
        shop,
        FULFILLMENT_CREATE,
        json!({"input": {"lineItemsByFulfillmentOrder": [{"fulfillmentOrderId": fo["id"], "fulfillmentOrderLineItems": lines}]}}),
    )
}

/// Shipping or restocking an order moves the units it committed, never those another order
/// committed: an order of the store data committed none, so its fulfillment and its restock
/// leave the stock as it is.
#[test]
fn an_order_ships_and_restocks_only_what_it_committed() {
    let mut shop = demo();
    let seeded = order_named(&shop, "#1002");
    let line = gids_of(shop.get(&seeded).unwrap(), "lineItems")[0].clone();
    let variant = shop.get(&line).unwrap()["variant"]
        .as_str()
        .unwrap()
        .to_string();
    let item = shop.get(&variant).unwrap()["inventoryItem"]
        .as_str()
        .unwrap()
        .to_string();
    let location = shop
        .get(&item)
        .map(|record| gids_of(record, "inventoryLevels"))
        .unwrap()[0]
        .clone();
    let location = shop.get(&location).unwrap()["location"].clone();
    assert_eq!(quantities(&shop, &item, "available"), [15, 15]);
    assert_eq!(quantities(&shop, &item, "on_hand"), [15, 15]);

    // An order of the admin commits 2 units.
    let answer = run(
        &mut shop,
        ORDER_CREATE,
        json!({
            "order": {"lineItems": [{"variantId": variant, "quantity": 2}]},
            "options": {"inventoryBehaviour": "DECREMENT_IGNORING_POLICY"},
        }),
    );
    let created = answer["data"]["orderCreate"]["order"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(quantities(&shop, &item, "committed"), [2, 0]);

    // #1002 of the store data never committed its 4 units: shipping them does not take the
    // other order's, and restocking them on a return puts nothing back.
    let shipped = ship_all(&mut shop, &seeded);
    assert_eq!(
        shipped["data"]["fulfillmentCreate"]["userErrors"],
        json!([]),
        "{shipped}"
    );
    assert_eq!(quantities(&shop, &item, "committed"), [2, 0]);
    assert_eq!(quantities(&shop, &item, "on_hand"), [15, 15]);
    let quantity = shop.get(&line).unwrap()["quantity"].clone();
    let refund = run(
        &mut shop,
        REFUND_CREATE,
        json!({"input": {"orderId": seeded, "refundLineItems": [{"lineItemId": line, "quantity": quantity, "restockType": "RETURN", "locationId": location}]}}),
    );
    assert_eq!(
        refund["data"]["refundCreate"]["userErrors"],
        json!([]),
        "{refund}"
    );
    assert_eq!(quantities(&shop, &item, "available"), [13, 15]);
    assert_eq!(quantities(&shop, &item, "on_hand"), [15, 15]);

    // The order that committed ships its own units, then gets them back on a return.
    ship_all(&mut shop, &created);
    assert_eq!(quantities(&shop, &item, "committed"), [0, 0]);
    assert_eq!(quantities(&shop, &item, "on_hand"), [13, 15]);
    let line = gids_of(shop.get(&created).unwrap(), "lineItems")[0].clone();
    let refund = run(
        &mut shop,
        REFUND_CREATE,
        json!({"input": {"orderId": created, "refundLineItems": [{"lineItemId": line, "quantity": 2, "restockType": "RETURN", "locationId": location}]}}),
    );
    assert_eq!(
        refund["data"]["refundCreate"]["userErrors"],
        json!([]),
        "{refund}"
    );
    assert_eq!(quantities(&shop, &item, "available"), [15, 15]);
    assert_eq!(quantities(&shop, &item, "on_hand"), [15, 15]);

    // Cancelled before it ships, an order that committed gives its units back to `available`;
    // a second restock of the same order finds nothing left to give.
    let answer = run(
        &mut shop,
        ORDER_CREATE,
        json!({
            "order": {"lineItems": [{"variantId": variant, "quantity": 3}]},
            "options": {"inventoryBehaviour": "DECREMENT_IGNORING_POLICY"},
        }),
    );
    let third = answer["data"]["orderCreate"]["order"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(quantities(&shop, &item, "available"), [12, 15]);
    let cancelled = run(
        &mut shop,
        ORDER_CANCEL,
        json!({"id": third, "restock": true, "refund": false}),
    );
    assert_eq!(
        cancelled["data"]["orderCancel"]["orderCancelUserErrors"],
        json!([]),
        "{cancelled}"
    );
    assert_eq!(quantities(&shop, &item, "available"), [15, 15]);
    assert_eq!(quantities(&shop, &item, "committed"), [0, 0]);
    assert_eq!(quantities(&shop, &item, "on_hand"), [15, 15]);
}

fn gids_of(record: &crate::state::Record, field: &str) -> Vec<String> {
    crate::domains::catalog::gids(record.get(field))
}

/// No amount of an order or a draft overflows: a money input past lsf's bound, or a line whose
/// total would be, answers a user error, never a panic or a wrapped total.
#[test]
fn amounts_past_the_bound_answer_user_errors() {
    let mut shop = demo();
    let (variant, _) = first_variant(&shop);
    let money = |amount: &str| json!({"shopMoney": {"amount": amount, "currencyCode": "EUR"}});
    let answer = run(
        &mut shop,
        ORDER_CREATE,
        json!({"order": {"lineItems": [{"title": "Huge", "quantity": 2, "priceSet": money("92233720368547758.07")}]}}),
    );
    assert_eq!(
        answer["data"]["orderCreate"],
        json!({"order": null, "userErrors": [{
            "field": ["order", "lineItems", "0", "priceSet", "shopMoney", "amount"],
            "message": "Amount must be less than 10000000000000",
            "code": "INVALID",
        }]}),
        "{answer}"
    );
    // Within the bound, but not twice over.
    let answer = run(
        &mut shop,
        ORDER_CREATE,
        json!({"order": {"lineItems": [{"title": "Large", "quantity": 2, "priceSet": money("9999999999999.99")}]}}),
    );
    assert_eq!(
        answer["data"]["orderCreate"]["userErrors"],
        json!([{"field": ["order", "lineItems", "0", "quantity"], "message": "Total price must be less than 10000000000000", "code": "INVALID"}]),
        "{answer}"
    );
    // Two lines within the bound whose sum is not; a tax rate past it.
    let answer = run(
        &mut shop,
        ORDER_CREATE,
        json!({"order": {"lineItems": [
            {"title": "A", "quantity": 1, "priceSet": money("6000000000000")},
            {"title": "B", "quantity": 1, "priceSet": money("6000000000000")},
        ]}}),
    );
    assert_eq!(
        answer["data"]["orderCreate"]["userErrors"][0]["message"],
        json!("Total price must be less than 10000000000000"),
        "{answer}"
    );
    let answer = run(
        &mut shop,
        ORDER_CREATE,
        json!({"order": {"lineItems": [{"variantId": variant, "quantity": 1}], "taxLines": [{"title": "VAT", "rate": "100000000000000000000"}]}}),
    );
    assert_eq!(
        answer["data"]["orderCreate"]["userErrors"][0]["message"],
        json!("Total price must be less than 10000000000000"),
        "{answer}"
    );
    let draft = |shop: &mut ShopState, line: Value| {
        run(
            shop,
            "mutation($input: DraftOrderInput!) { draftOrderCreate(input: $input) { draftOrder { id } userErrors { field message } } }",
            json!({"input": {"lineItems": [line]}}),
        )["data"]["draftOrderCreate"]
            .clone()
    };
    assert_eq!(
        draft(
            &mut shop,
            json!({"title": "Huge", "quantity": 2, "originalUnitPrice": "92233720368547758.07"})
        ),
        json!({"draftOrder": null, "userErrors": [{"field": ["input", "lineItems", "0", "originalUnitPrice"], "message": "Original unit price must be less than 10000000000000"}]})
    );
    assert_eq!(
        draft(
            &mut shop,
            json!({"title": "Large", "quantity": 2000000000, "originalUnitPrice": "9999.99"})
        ),
        json!({"draftOrder": null, "userErrors": [{"field": ["lineItems", "0", "quantity"], "message": "Total price must be less than 10000000000000"}]})
    );
    assert!(shop.take_events().is_empty());
}

/// A fulfillment service's location is the service's, not one of the store: the storefront
/// does not get it.
#[test]
fn a_fulfillment_service_location_does_not_reach_the_storefront() {
    let (store, mut shop) = rich_with_store();
    let created = run(
        &mut shop,
        "mutation { fulfillmentServiceCreate(name: \"Acme 3PL\", callbackUrl: \"https://3pl.example/callback\", inventoryManagement: true) {
            fulfillmentService { location { id isFulfillmentService } } userErrors { field message } } }",
        Value::Null,
    );
    assert_eq!(
        created["data"]["fulfillmentServiceCreate"]["fulfillmentService"]["location"]["isFulfillmentService"],
        json!(true),
        "{created}"
    );
    let names = |store: &lsf_core::Store| -> Vec<String> {
        store
            .locations
            .iter()
            .map(|location| location.name.clone())
            .collect()
    };
    match crate::sync::overlay(&shop, &store) {
        None => {}
        Some(overlay) => {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/rich");
            let (synced, _) = lsf_core::store::load::load_with_overlays(
                &lsf_core::store::load::DataSource::Directory(path),
                &Default::default(),
                &[overlay],
            );
            assert_eq!(names(&synced), names(&store));
        }
    }
}

/// A checkout order of a variant the Admin API created names that variant's inventory item,
/// whose id is of the counter, not the one lsf-core would derive from the variant.
#[test]
fn a_checkout_order_names_the_inventory_item_of_its_variant() {
    let mut shop = demo();
    let created = run(
        &mut shop,
        "mutation { productCreate(product: {title: \"Admin hat\"}) { product { id variants(first: 1) { nodes { id inventoryItem { id } } } } userErrors { message } } }",
        Value::Null,
    );
    let product = created["data"]["productCreate"]["product"].clone();
    let variant = product["variants"]["nodes"][0].clone();
    let item = variant["inventoryItem"]["id"].as_str().unwrap().to_string();
    let id = |gid: &Value| crate::state::parse_gid(gid.as_str().unwrap()).unwrap().1;
    assert_ne!(
        item,
        crate::domains::catalog::inventory_item_gid(id(&variant["id"]))
    );
    let result = lsf_checkout::CheckoutResult {
        lines: vec![lsf_checkout::ResultLine {
            id: "gid://shopify/CartLine/1".into(),
            variant_id: id(&variant["id"]),
            product_id: id(&product["id"]),
            title: "Admin hat".into(),
            quantity: 1,
            unit_price: 1000,
            subtotal: 1000,
            total: 1000,
            ..Default::default()
        }],
        ..Default::default()
    };
    let recorded = super::record_placed_order(
        &mut shop,
        &super::PlacedOrder {
            id: None,
            number: None,
            processed_at: "2026-05-01T09:30:00Z".parse().unwrap(),
            result: &result,
            customer_id: None,
            company_location_id: None,
            email: None,
            phone: None,
            shipping_address: None,
            note: None,
            attributes: &[],
            payment: None,
        },
    );
    let answer = run(
        &mut shop,
        "query($id: ID!) { order(id: $id) { fulfillmentOrders(first: 1) { nodes { lineItems(first: 1) { nodes { inventoryItemId } } } } } }",
        json!({"id": recorded.id}),
    );
    assert_eq!(
        answer["data"]["order"]["fulfillmentOrders"]["nodes"][0]["lineItems"]["nodes"][0]["inventoryItemId"],
        json!(item),
        "{answer}"
    );
}

/// One mutation sends one webhook per topic and record: an order created fulfilled that commits
/// then ships the units of one level reports that level once, and a product given metafields
/// with its variant is updated once.
#[test]
fn a_mutation_sends_one_event_per_topic_and_record() {
    let mut shop = demo();
    let id = order_named(&shop, "#1002");
    let line = gids_of(shop.get(&id).unwrap(), "lineItems")[0].clone();
    let variant = shop.get(&line).unwrap()["variant"]
        .as_str()
        .unwrap()
        .to_string();
    let product = shop.get(&variant).unwrap()["product"]
        .as_str()
        .unwrap()
        .to_string();
    shop.take_events();
    let answer = run(
        &mut shop,
        ORDER_CREATE,
        json!({
            "order": {"lineItems": [{"variantId": variant, "quantity": 2}], "fulfillmentStatus": "FULFILLED"},
            "options": {"inventoryBehaviour": "DECREMENT_IGNORING_POLICY"},
        }),
    );
    assert_eq!(
        answer["data"]["orderCreate"]["userErrors"],
        json!([]),
        "{answer}"
    );
    let fired = topics(&mut shop);
    assert_eq!(
        fired
            .iter()
            .filter(|topic| *topic == "inventory_levels/update")
            .count(),
        1,
        "{fired:?}"
    );

    let set = run(
        &mut shop,
        "mutation($metafields: [MetafieldsSetInput!]!) { metafieldsSet(metafields: $metafields) { metafields { id } userErrors { field message } } }",
        json!({"metafields": [
            {"ownerId": product, "namespace": "custom", "key": "zz_a", "type": "single_line_text_field", "value": "a"},
            {"ownerId": variant, "namespace": "custom", "key": "zz_b", "type": "single_line_text_field", "value": "b"},
        ]}),
    );
    assert_eq!(
        set["data"]["metafieldsSet"]["userErrors"],
        json!([]),
        "{set}"
    );
    assert_eq!(
        events(&mut shop),
        [("products/update".to_string(), product)]
    );

    // Two mutations of one request are two changes: each sends its own.
    let twice = run(
        &mut shop,
        "mutation($id: ID!) { a: tagsAdd(id: $id, tags: [\"zz-1\"]) { userErrors { message } } b: tagsAdd(id: $id, tags: [\"zz-2\"]) { userErrors { message } } }",
        json!({"id": id}),
    );
    assert!(twice.get("errors").is_none(), "{twice}");
    assert_eq!(topics(&mut shop), ["orders/updated", "orders/updated"]);
}

/// `DECREMENT_OBEYING_POLICY` checks the stock of the level the order commits from, at the
/// order's location, not the item's first level.
#[test]
fn an_order_obeying_the_policy_checks_the_stock_where_it_commits() {
    let (store, mut shop) = rich_with_store();
    let dress = store.product_by_handle("linen-dress").unwrap();
    let variant = crate::domains::catalog::variant_gid(&dress.variants[0]);
    let item = shop.get(&variant).unwrap()["inventoryItem"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(quantities(&shop, &item, "available"), [4, 3]);
    let berlin = crate::state::gid("Location", store.locations[1].id);
    let order = |shop: &mut ShopState, quantity: i64| {
        run(
            shop,
            ORDER_CREATE,
            json!({
                "order": {"lineItems": [{"variantId": variant, "quantity": quantity}], "fulfillment": {"locationId": berlin}},
                "options": {"inventoryBehaviour": "DECREMENT_OBEYING_POLICY"},
            }),
        )["data"]["orderCreate"]["userErrors"]
            .clone()
    };
    // Paris has 4, Berlin 3: 4 from Berlin is too many, though the first level has them.
    assert_eq!(
        order(&mut shop, 4),
        json!([{"field": ["order", "lineItems", "0", "quantity"], "message": "Linen Dress is out of stock", "code": "INVENTORY_CLAIM_FAILED"}])
    );
    assert_eq!(quantities(&shop, &item, "available"), [4, 3]);
    assert_eq!(order(&mut shop, 3), json!([]));
    assert_eq!(quantities(&shop, &item, "available"), [4, 0]);
}

/// `orderCreate`'s `discountCode` makes a `DiscountCodeApplication` and the allocations it made
/// on the lines or the shipping line, as the reference's examples read them back, so that a
/// backend that splits discounts by line finds them, in the API and in `orders/create`.
#[test]
fn an_order_created_with_a_discount_code_records_its_application_and_allocations() {
    let mut shop = demo();
    let money = |amount: &str| json!({"shopMoney": {"amount": amount, "currencyCode": "EUR"}});
    let lines = json!([
        {"title": "A", "quantity": 1, "priceSet": money("30.00")},
        {"title": "B", "quantity": 2, "priceSet": money("10.00")},
    ]);
    let query = "query($id: ID!) { order(id: $id) {
        discountCodes totalDiscountsSet { shopMoney { amount } }
        discountApplications(first: 5) { nodes { __typename allocationMethod targetSelection targetType
            value { ... on MoneyV2 { amount } ... on PricingPercentageValue { percentage } }
            ... on DiscountCodeApplication { code } } }
        lineItems(first: 5) { nodes { discountAllocations { allocatedAmountSet { shopMoney { amount } } discountApplication { index } } } }
        shippingLine { discountAllocations { allocatedAmountSet { shopMoney { amount } } } } } }";
    let create = |shop: &mut ShopState, code: Value| -> (String, Value) {
        let answer = run(
            shop,
            ORDER_CREATE,
            json!({"order": {"lineItems": lines, "discountCode": code,
                "shippingLines": [{"title": "Express", "priceSet": money("7.00")}]}}),
        );
        assert_eq!(
            answer["data"]["orderCreate"]["userErrors"],
            json!([]),
            "{answer}"
        );
        let id = answer["data"]["orderCreate"]["order"]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let read = run(shop, query, json!({"id": id}));
        (id, read["data"]["order"].clone())
    };
    let allocated = |order: &Value| -> Vec<Value> {
        order["lineItems"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|line| {
                line["discountAllocations"][0]["allocatedAmountSet"]["shopMoney"]["amount"].clone()
            })
            .collect()
    };

    // 10% of each line: the reference's example answers `value { percentage: 10 }`.
    let (id, order) = create(
        &mut shop,
        json!({"itemPercentageDiscountCode": {"code": "SUMMER SALE", "percentage": 10}}),
    );
    assert_eq!(order["discountCodes"], json!(["SUMMER SALE"]));
    assert_eq!(
        order["discountApplications"]["nodes"],
        json!([{"__typename": "DiscountCodeApplication", "allocationMethod": "EACH", "targetSelection": "ALL",
            "targetType": "LINE_ITEM", "value": {"percentage": 10.0}, "code": "SUMMER SALE"}])
    );
    assert_eq!(allocated(&order), [json!("3.0"), json!("2.0")]);
    assert_eq!(
        order["lineItems"]["nodes"][0]["discountAllocations"][0]["discountApplication"]["index"],
        json!(0)
    );
    let payload = payload_of(&shop, "orders/create", &id);
    assert_eq!(
        payload["discount_applications"][0]["code"],
        json!("SUMMER SALE")
    );
    assert_eq!(
        payload["line_items"][0]["discount_allocations"][0]["amount"],
        json!("3.00")
    );

    // 5.00 off the order, spread over the lines by their totals (30 and 20).
    let (_, order) = create(
        &mut shop,
        json!({"itemFixedDiscountCode": {"code": "FIVE", "amountSet": money("5.00")}}),
    );
    assert_eq!(
        order["discountApplications"]["nodes"][0]["value"],
        json!({"amount": "5.0"})
    );
    assert_eq!(
        order["discountApplications"]["nodes"][0]["allocationMethod"],
        json!("ACROSS")
    );
    assert_eq!(allocated(&order), [json!("3.0"), json!("2.0")]);
    assert_eq!(
        order["totalDiscountsSet"]["shopMoney"]["amount"],
        json!("5.0")
    );

    // Free shipping: the shipping line's price.
    let (_, order) = create(
        &mut shop,
        json!({"freeShippingDiscountCode": {"code": "SHIPFREE"}}),
    );
    assert_eq!(
        order["discountApplications"]["nodes"][0]["targetType"],
        json!("SHIPPING_LINE")
    );
    assert_eq!(
        order["shippingLine"]["discountAllocations"][0]["allocatedAmountSet"]["shopMoney"]["amount"],
        json!("7.0")
    );
    assert_eq!(
        order["lineItems"]["nodes"][0]["discountAllocations"],
        json!([])
    );
}

/// The sum of a quantity of an item over its levels.
fn quantity_of(shop: &ShopState, item: &str, name: &str) -> i64 {
    crate::domains::catalog::gids(shop.get(item).unwrap().get("inventoryLevels"))
        .iter()
        .filter_map(|level| shop.get(level).unwrap()["quantities"].as_array())
        .flatten()
        .filter(|quantity| quantity["name"] == json!(name))
        .map(|quantity| quantity["quantity"].as_i64().unwrap())
        .sum()
}

/// A bundle of the demo store's tote (×1) and socks (×2): the parent variant, and the
/// components' variants and inventory items.
fn gift_set(shop: &mut ShopState) -> (String, [(String, String); 2]) {
    let created = run(
        shop,
        r#"mutation { productCreate(product: {title: "Gift set"}) { product { id variants(first: 1) { nodes { id } } } } }"#,
        json!({}),
    );
    let product = created["data"]["productCreate"]["product"].clone();
    let parent = product["variants"]["nodes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let component = |shop: &ShopState, handle: &str| {
        let product = shop
            .all("Product")
            .find(|product| product["handle"] == json!(handle))
            .unwrap();
        let variant = product["variants"][0].as_str().unwrap().to_string();
        let item = shop.get(&variant).unwrap()["inventoryItem"]
            .as_str()
            .unwrap()
            .to_string();
        (variant, item)
    };
    let tote = component(shop, "canvas-tote-bag");
    let socks = component(shop, "merino-crew-socks");
    let bundled = run(
        shop,
        r#"mutation($input: [ProductVariantRelationshipUpdateInput!]!) { productVariantRelationshipBulkUpdate(input: $input) { userErrors { message } } }"#,
        json!({"input": [{"parentProductId": product["id"],
            "productVariantRelationshipsToCreate": [{"id": tote.0, "quantity": 1}, {"id": socks.0, "quantity": 2}]}]}),
    );
    assert_eq!(
        bundled["data"]["productVariantRelationshipBulkUpdate"]["userErrors"],
        json!([]),
        "{bundled}"
    );
    (parent, [tote, socks])
}

fn bundle_quantity(shop: &mut ShopState, parent: &str) -> Value {
    run(
        shop,
        "query($id: ID!) { productVariant(id: $id) { inventoryQuantity availableForSale } }",
        json!({"id": parent}),
    )["data"]["productVariant"]
        .clone()
}

/// A bundle sold takes its components' units: committed when the order claims them, checked by
/// `DECREMENT_OBEYING_POLICY`, shipped by a fulfillment and put back by a restock, and the
/// bundle's quantity follows them.
#[test]
fn a_bundle_sold_takes_its_components_stock() {
    let mut shop = demo();
    let (parent, [(_, tote), (socks_variant, socks)]) = gift_set(&mut shop);
    let before = |shop: &ShopState, item: &str| {
        (
            quantity_of(shop, item, "available"),
            quantity_of(shop, item, "committed"),
            quantity_of(shop, item, "on_hand"),
        )
    };
    let (tote_available, tote_committed, tote_on_hand) = before(&shop, &tote);
    let (socks_available, socks_committed, socks_on_hand) = before(&shop, &socks);
    assert_eq!(
        bundle_quantity(&mut shop, &parent),
        json!({"inventoryQuantity": 30, "availableForSale": true})
    );
    shop.take_events();

    let answer = run(
        &mut shop,
        ORDER_CREATE,
        json!({
            "order": {"lineItems": [{"variantId": parent, "quantity": 3}]},
            "options": {"inventoryBehaviour": "DECREMENT_OBEYING_POLICY"},
        }),
    );
    assert_eq!(
        answer["data"]["orderCreate"]["userErrors"],
        json!([]),
        "{answer}"
    );
    let order_id = answer["data"]["orderCreate"]["order"]["id"].clone();
    assert_eq!(quantity_of(&shop, &tote, "committed"), tote_committed + 3);
    assert_eq!(quantity_of(&shop, &tote, "available"), tote_available - 3);
    assert_eq!(quantity_of(&shop, &socks, "committed"), socks_committed + 6);
    assert_eq!(quantity_of(&shop, &socks, "available"), socks_available - 6);
    // min(37 totes, 54 / 2 socks).
    assert_eq!(
        bundle_quantity(&mut shop, &parent),
        json!({"inventoryQuantity": 27, "availableForSale": true})
    );
    let fired = events(&mut shop);
    assert!(
        fired
            .iter()
            .filter(|(topic, _)| topic == "inventory_levels/update")
            .count()
            >= 2,
        "{fired:?}"
    );

    // 28 bundles take 56 socks, of 54: refused for the socks, though the bundle's own item is
    // not tracked.
    let refused = run(
        &mut shop,
        ORDER_CREATE,
        json!({
            "order": {"lineItems": [{"variantId": parent, "quantity": 28}]},
            "options": {"inventoryBehaviour": "DECREMENT_OBEYING_POLICY"},
        }),
    );
    assert_eq!(
        refused["data"]["orderCreate"]["userErrors"][0]["code"],
        json!("INVENTORY_CLAIM_FAILED"),
        "{refused}"
    );

    // Shipped: the components' committed units leave the stock.
    let fulfillment_order = run(
        &mut shop,
        "query($id: ID!) { order(id: $id) { fulfillmentOrders(first: 1) { nodes { id lineItems(first: 1) { nodes { id } } } } } }",
        json!({"id": order_id}),
    )["data"]["order"]["fulfillmentOrders"]["nodes"][0]
        .clone();
    let shipped = run(
        &mut shop,
        FULFILLMENT_CREATE,
        json!({"input": {"lineItemsByFulfillmentOrder": [{
            "fulfillmentOrderId": fulfillment_order["id"],
            "fulfillmentOrderLineItems": [{"id": fulfillment_order["lineItems"]["nodes"][0]["id"], "quantity": 1}],
        }]}}),
    );
    assert_eq!(
        shipped["data"]["fulfillmentCreate"]["userErrors"],
        json!([]),
        "{shipped}"
    );
    assert_eq!(quantity_of(&shop, &tote, "committed"), tote_committed + 2);
    assert_eq!(quantity_of(&shop, &tote, "on_hand"), tote_on_hand - 1);
    assert_eq!(quantity_of(&shop, &socks, "committed"), socks_committed + 4);
    assert_eq!(quantity_of(&shop, &socks, "on_hand"), socks_on_hand - 2);

    // Cancelled with a restock: what had not shipped goes back to `available`.
    let cancelled = run(
        &mut shop,
        ORDER_CANCEL,
        json!({"id": order_id, "restock": true, "refund": false}),
    );
    assert_eq!(
        cancelled["data"]["orderCancel"]["orderCancelUserErrors"],
        json!([]),
        "{cancelled}"
    );
    assert_eq!(quantity_of(&shop, &tote, "committed"), tote_committed);
    assert_eq!(quantity_of(&shop, &tote, "available"), tote_available - 1);
    assert_eq!(quantity_of(&shop, &socks, "committed"), socks_committed);
    assert_eq!(quantity_of(&shop, &socks, "available"), socks_available - 2);
    assert_eq!(
        bundle_quantity(&mut shop, &parent),
        json!({"inventoryQuantity": 29, "availableForSale": true})
    );
    // The socks' own variant reads the same stock.
    let socks_read = run(
        &mut shop,
        "query($id: ID!) { productVariant(id: $id) { inventoryQuantity } }",
        json!({"id": socks_variant}),
    );
    assert_eq!(
        socks_read["data"]["productVariant"]["inventoryQuantity"],
        json!(socks_available - 2)
    );
}

/// `orderCreate` without a location fulfills from where every line can come from: the primary
/// location when it can, else another active location.
#[test]
fn an_order_without_a_location_takes_the_stock_where_it_is() {
    let mut shop = demo();
    let primary = crate::domains::catalog::default_location(&shop).unwrap();
    // A variant stocked at the primary location and at another one.
    let (variant, item, other) = shop
        .keys("ProductVariant")
        .find_map(|variant| {
            let item = shop.get(variant)?["inventoryItem"].as_str()?.to_string();
            let levels = crate::domains::catalog::gids(shop.get(&item)?.get("inventoryLevels"));
            let other = levels.iter().find_map(|level| {
                let location = shop.get(level)?["location"].as_str()?;
                (location != primary).then(|| location.to_string())
            })?;
            (levels.len() > 1).then(|| (variant.to_string(), item, other))
        })
        .expect("a variant at two locations");
    shop.get_mut(&item)
        .unwrap()
        .insert("tracked".into(), json!(true));
    shop.get_mut(&variant)
        .unwrap()
        .insert("inventoryPolicy".into(), json!("DENY"));
    let set = |shop: &mut ShopState, location: &str, units: i64| {
        let level = crate::domains::catalog::level_at(shop, &item, location).unwrap();
        let record = shop.get_mut(&level).unwrap();
        for quantity in record["quantities"].as_array_mut().unwrap() {
            if ["available", "on_hand"].contains(&quantity["name"].as_str().unwrap()) {
                quantity["quantity"] = json!(units);
            }
        }
    };
    set(&mut shop, &primary, 0);
    set(&mut shop, &other, 5);
    let answer = run(
        &mut shop,
        ORDER_CREATE,
        json!({
            "order": {"lineItems": [{"variantId": variant, "quantity": 2}]},
            "options": {"inventoryBehaviour": "DECREMENT_OBEYING_POLICY"},
        }),
    );
    assert_eq!(
        answer["data"]["orderCreate"]["userErrors"],
        json!([]),
        "{answer}"
    );
    assert_eq!(
        model::inventory_quantity(&shop, &item, Some(&other), "committed"),
        2
    );
    assert_eq!(
        model::inventory_quantity(&shop, &item, Some(&primary), "available"),
        0
    );
    // Short everywhere: the primary location, and the claim fails.
    let refused = run(
        &mut shop,
        ORDER_CREATE,
        json!({
            "order": {"lineItems": [{"variantId": variant, "quantity": 4}]},
            "options": {"inventoryBehaviour": "DECREMENT_OBEYING_POLICY"},
        }),
    );
    assert_eq!(
        refused["data"]["orderCreate"]["userErrors"][0]["code"],
        json!("INVENTORY_CLAIM_FAILED"),
        "{refused}"
    );
}

/// `orderCreate`, then a failure: a mutation that created an order before it failed.
fn creates_an_order_then_fails(
    mutation: &mut crate::resolve::Mutation<'_>,
    ctx: &crate::graphql::FieldContext<'_>,
) -> Result<Value, crate::graphql::FieldError> {
    let create = Registry::standard().mutation_fn("orderCreate").unwrap();
    create(mutation, ctx)?;
    Ok(crate::resolve::payload(
        json!({"order": null}),
        &[crate::resolve::UserError::new(&["order"], "No")],
    ))
}

/// The next order number is one above the highest order's: a mutation that fails after it
/// created an order gives its number back, deleting the last order makes its number the next
/// one again, and a reset (or a shop made from the seed) starts from the seed's.
#[test]
fn order_numbers_follow_a_rollback_a_deletion_and_a_reset() {
    let (store, _) = lsf_core::store::load::load(
        &lsf_core::store::load::DataSource::Demo,
        &Default::default(),
    );
    let mut shops = crate::state::Shops::new(&store, None);
    let shop = shops.default_shop_mut().unwrap();
    let seeded = model::next_order_number(shop);
    assert!(seeded > 1001);
    let (variant, _) = first_variant(shop);
    let order = json!({"order": {"lineItems": [{"variantId": variant, "quantity": 1}]}});

    let mut failing = Registry::new();
    failing.mutation("orderCreate", creates_an_order_then_fails);
    let before = shop.picture();
    let mut resolver = ShopResolver::new(shop, &failing);
    let answer = graphql::run(
        crate::schema::admin(),
        &mut resolver,
        &Request::new(ORDER_CREATE).with_variables(order.clone()),
        None,
    )
    .to_json();
    assert_eq!(
        answer["data"]["orderCreate"]["userErrors"][0]["message"],
        "No"
    );
    assert_eq!(shop.picture(), before);
    assert!(shop.index_in_step());
    assert_eq!(model::next_order_number(shop), seeded);

    let created = run(shop, ORDER_CREATE, order.clone());
    let id = created["data"]["orderCreate"]["order"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(shop.get(&id).unwrap()["number"], json!(seeded));
    assert_eq!(model::next_order_number(shop), seeded + 1);
    run(
        shop,
        "mutation($id: ID!) { orderClose(input: {id: $id}) { userErrors { message } } }",
        json!({"id": id}),
    );
    let deleted = run(
        shop,
        "mutation($id: ID!) { orderDelete(orderId: $id) { deletedId } }",
        json!({"id": id}),
    );
    assert_eq!(deleted["data"]["orderDelete"]["deletedId"], json!(id));
    assert_eq!(model::next_order_number(shop), seeded);

    run(shop, ORDER_CREATE, order.clone());
    run(shop, ORDER_CREATE, order);
    assert_eq!(model::next_order_number(shop), seeded + 2);
    shops.reset();
    assert_eq!(
        model::next_order_number(shops.default_shop().unwrap()),
        seeded
    );
    let other = shops.create("test-1").unwrap();
    assert_eq!(model::next_order_number(other), seeded);
    assert!(other.index_in_step());
}
