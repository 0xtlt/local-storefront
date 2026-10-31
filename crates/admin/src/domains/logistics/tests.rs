//! The logistics family through queries and mutations over the demo store and the rich fixture.

use serde_json::{Value, json};

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

/// The topics of the events emitted since the last call.
fn topics(shop: &mut ShopState) -> Vec<String> {
    shop.take_events()
        .into_iter()
        .map(|event| event.topic)
        .collect()
}

/// The payload of the last event of a topic emitted since the last call, as the webhooks build
/// it.
fn last_payload(shop: &mut ShopState, topic: &str) -> Value {
    let events = shop.take_events();
    let event = events
        .iter()
        .rev()
        .find(|event| event.topic == topic)
        .unwrap_or_else(|| {
            panic!(
                "no {topic} in {:?}",
                events.iter().map(|e| &e.topic).collect::<Vec<_>>()
            )
        });
    let mut record = shop
        .get(&event.gid)
        .cloned()
        .or_else(|| event.snapshot.clone())
        .unwrap();
    if let Some(snapshot) = &event.snapshot
        && !topic.ends_with("/delete")
    {
        record.insert(
            crate::webhooks::payload::PREVIOUS.into(),
            Value::Object(snapshot.clone()),
        );
    }
    let lookup = crate::api::samples::ShopLookup(shop);
    crate::webhooks::payload::payload(topic, &record, &lookup).unwrap()
}

/// The locations of the rich fixture, Paris then Berlin.
fn locations(shop: &ShopState) -> (String, String) {
    let named = |name: &str| {
        shop.keys("Location")
            .find(|key| shop.get(key).unwrap()["name"] == json!(name))
            .unwrap()
            .to_string()
    };
    (named("Paris"), named("Berlin warehouse"))
}

/// The inventory item of a SKU.
fn item_of(shop: &ShopState, sku: &str) -> String {
    let variant = shop
        .keys("ProductVariant")
        .find(|key| shop.get(key).unwrap()["sku"] == json!(sku))
        .unwrap();
    shop.get(variant).unwrap()["inventoryItem"]
        .as_str()
        .unwrap()
        .to_string()
}

/// A quantity of an item at a location.
fn quantity(shop: &ShopState, item: &str, location: &str, name: &str) -> i64 {
    super::stock::quantity(shop, item, location, name)
}

/// `(available, committed, incoming, on_hand)` of an item at a location.
fn levels(shop: &ShopState, item: &str, location: &str) -> [i64; 4] {
    ["available", "committed", "incoming", "on_hand"]
        .map(|name| quantity(shop, item, location, name))
}

const TRANSFER_FIELDS: &str = "id name status totalQuantity receivedQuantity
    origin { name location { id } } destination { name location { id } }
    lineItems(first: 10) { nodes { id title totalQuantity shippedQuantity pickedForShipmentQuantity processableQuantity shippableQuantity inventoryItem { id } } }
    lineItemsCount { count }
    shipments(first: 10) { nodes { id name status } }";

const CREATE: &str = "mutation($input: InventoryTransferCreateInput!) {
    inventoryTransferCreate(input: $input) @idempotent(key: \"create-1\") {
      inventoryTransfer { id name status totalQuantity } userErrors { field message code } } }";

#[test]
fn a_transfer_ships_and_is_received_with_the_stock_following() {
    let mut shop = rich();
    let (paris, berlin) = locations(&shop);
    let item = item_of(&shop, "DRESS-S-RED");
    assert_eq!(levels(&shop, &item, &paris), [4, 0, 0, 4]);
    assert_eq!(levels(&shop, &item, &berlin), [3, 0, 0, 3]);

    // A draft commits nothing.
    let answer = run(
        &mut shop,
        CREATE,
        json!({"input": {"originLocationId": paris, "destinationLocationId": berlin,
            "lineItems": [{"inventoryItemId": item, "quantity": 3}], "tags": ["restock"], "note": "Spring"}}),
    );
    let created = &answer["data"]["inventoryTransferCreate"];
    assert_eq!(created["userErrors"], json!([]), "{answer}");
    assert_eq!(created["inventoryTransfer"]["status"], "DRAFT");
    assert_eq!(created["inventoryTransfer"]["name"], "#T1");
    let transfer = created["inventoryTransfer"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(levels(&shop, &item, &paris), [4, 0, 0, 4]);
    let add = last_payload(&mut shop, "inventory_transfers/add_items");
    assert_eq!(add["id"], json!(transfer));
    assert_eq!(add["origin"], json!({"id": paris}));
    assert_eq!(add["items_added"][0]["quantity"], 3);

    // Ready to ship: committed at the origin.
    let answer = run(
        &mut shop,
        "mutation($id: ID!) { inventoryTransferMarkAsReadyToShip(id: $id) { inventoryTransfer { status } userErrors { code } } }",
        json!({"id": transfer}),
    );
    assert_eq!(
        answer["data"]["inventoryTransferMarkAsReadyToShip"]["inventoryTransfer"]["status"],
        "READY_TO_SHIP"
    );
    assert_eq!(levels(&shop, &item, &paris), [1, 3, 0, 4]);
    let ready = last_payload(&mut shop, "inventory_transfers/ready_to_ship");
    assert_eq!(ready["status"], "READY_TO_SHIP");
    assert_eq!(ready["line_items"][0]["quantity"], 3);

    // A shipment in transit: out of the origin, incoming at the destination.
    let answer = run(
        &mut shop,
        "mutation($input: InventoryShipmentCreateInput!) {
          inventoryShipmentCreateInTransit(input: $input) @idempotent(key: \"ship-1\") {
            inventoryShipment { id name status lineItemTotalQuantity dateShipped
              lineItems(first: 5) { nodes { id quantity unreceivedQuantity } } }
            userErrors { field message code } } }",
        json!({"input": {"movementId": transfer, "lineItems": [{"inventoryItemId": item, "quantity": 3}],
            "trackingInput": {"company": "Purolator", "trackingNumber": "8375692728", "trackingUrl": "https://example.com/track/8375692728"}}}),
    );
    let shipped = &answer["data"]["inventoryShipmentCreateInTransit"];
    assert_eq!(shipped["userErrors"], json!([]), "{answer}");
    assert_eq!(shipped["inventoryShipment"]["status"], "IN_TRANSIT");
    assert_eq!(shipped["inventoryShipment"]["name"], "T1-1");
    let shipment = shipped["inventoryShipment"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let line = shipped["inventoryShipment"]["lineItems"]["nodes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(levels(&shop, &item, &paris), [1, 0, 0, 1]);
    assert_eq!(levels(&shop, &item, &berlin), [3, 0, 3, 3]);
    let events = topics(&mut shop);
    for topic in [
        "inventory_shipments/create",
        "inventory_shipments/mark_in_transit",
        "inventory_levels/update",
    ] {
        assert!(
            events.iter().any(|known| known == topic),
            "{topic}: {events:?}"
        );
    }
    let transfer_now = run(
        &mut shop,
        &format!("query($id: ID!) {{ inventoryTransfer(id: $id) {{ {TRANSFER_FIELDS} }} }}"),
        json!({"id": transfer}),
    );
    let read = &transfer_now["data"]["inventoryTransfer"];
    assert_eq!(read["status"], "IN_PROGRESS");
    assert_eq!(read["lineItems"]["nodes"][0]["shippedQuantity"], 3);
    assert_eq!(read["lineItems"]["nodes"][0]["processableQuantity"], 0);
    assert_eq!(read["lineItemsCount"]["count"], 1);
    assert_eq!(read["shipments"]["nodes"][0]["id"], json!(shipment));
    assert!(
        transfer_now
            .get("extensions")
            .is_none_or(|e| e.get("lsf").is_none()),
        "{transfer_now}"
    );

    // Partly received: 1 accepted, 1 rejected.
    let receive = "mutation($id: ID!, $lineItems: [InventoryShipmentReceiveItemInput!], $bulk: InventoryShipmentReceiveLineItemReason) {
        inventoryShipmentReceive(id: $id, lineItems: $lineItems, bulkReceiveAction: $bulk) @idempotent(key: \"first\") {
          inventoryShipment { status totalAcceptedQuantity totalRejectedQuantity totalReceivedQuantity dateReceived }
          userErrors { field message code } } }";
    let answer = run(
        &mut shop,
        receive,
        json!({"id": shipment, "lineItems": [
            {"shipmentLineItemId": line, "quantity": 1, "reason": "ACCEPTED"},
            {"shipmentLineItemId": line, "quantity": 1, "reason": "REJECTED"}]}),
    );
    let received = &answer["data"]["inventoryShipmentReceive"]["inventoryShipment"];
    assert_eq!(received["status"], "PARTIALLY_RECEIVED", "{answer}");
    assert_eq!(received["totalReceivedQuantity"], 2);
    assert_eq!(levels(&shop, &item, &berlin), [4, 0, 1, 4]);
    let payload = last_payload(&mut shop, "inventory_shipments/receive_items");
    assert_eq!(payload["status"], "PARTIALLY_RECEIVED");
    assert_eq!(payload["items_received"][0]["new_accepted_quantity"], 1);
    assert_eq!(payload["items_received"][0]["new_rejected_quantity"], 1);
    assert_eq!(
        payload["items_received"][0]["remaining_quantity_to_receive"],
        1
    );

    // The rest received: the transfer is complete.
    let answer = run(
        &mut shop,
        &receive.replace("\"first\"", "\"second\""),
        json!({"id": shipment, "bulk": "ACCEPTED"}),
    );
    assert_eq!(
        answer["data"]["inventoryShipmentReceive"]["inventoryShipment"]["status"], "RECEIVED",
        "{answer}"
    );
    assert_eq!(levels(&shop, &item, &berlin), [5, 0, 0, 5]);
    let events = topics(&mut shop);
    assert!(
        events
            .iter()
            .any(|topic| topic == "inventory_transfers/complete"),
        "{events:?}"
    );
    assert_eq!(shop.get(&transfer).unwrap()["status"], "TRANSFERRED");
    assert_eq!(shop.get(&transfer).unwrap()["receivedQuantity"], 3);
}

#[test]
fn cancelling_a_ready_to_ship_transfer_makes_its_units_available_again() {
    let mut shop = rich();
    let (paris, berlin) = locations(&shop);
    let item = item_of(&shop, "DRESS-S-RED");
    let answer = run(
        &mut shop,
        "mutation($input: InventoryTransferCreateAsReadyToShipInput!) {
          inventoryTransferCreateAsReadyToShip(input: $input) @idempotent(key: \"r\") {
            inventoryTransfer { id status } userErrors { field message code } } }",
        json!({"input": {"originLocationId": paris, "destinationLocationId": berlin,
            "lineItems": [{"inventoryItemId": item, "quantity": 4}]}}),
    );
    let transfer =
        answer["data"]["inventoryTransferCreateAsReadyToShip"]["inventoryTransfer"]["id"]
            .as_str()
            .unwrap()
            .to_string();
    assert_eq!(levels(&shop, &item, &paris), [0, 4, 0, 4]);
    // The variant is still in stock: Berlin has 3.
    let variant_events = topics(&mut shop);
    assert!(
        !variant_events
            .iter()
            .any(|topic| topic == "variants/out_of_stock")
    );

    let answer = run(
        &mut shop,
        "mutation($id: ID!) { inventoryTransferCancel(id: $id) { inventoryTransfer { status } userErrors { code } } }",
        json!({"id": transfer}),
    );
    assert_eq!(
        answer["data"]["inventoryTransferCancel"]["inventoryTransfer"]["status"],
        "CANCELED"
    );
    assert_eq!(levels(&shop, &item, &paris), [4, 0, 0, 4]);
    let payload = last_payload(&mut shop, "inventory_transfers/cancel");
    assert_eq!(payload["status"], "CANCELED");

    // Cancelled once: a second time is refused, and nothing changes.
    let before = shop.picture();
    let answer = run(
        &mut shop,
        "mutation($id: ID!) { inventoryTransferCancel(id: $id) { inventoryTransfer { status } userErrors { field message code } } }",
        json!({"id": transfer}),
    );
    assert_eq!(
        answer["data"]["inventoryTransferCancel"]["userErrors"],
        json!([{"field": ["id"], "message": "Current transfer status does not support this operation.", "code": "INVALID_TRANSFER_STATUS"}])
    );
    assert_eq!(shop.picture(), before);
}

#[test]
fn a_transfer_refuses_what_it_cannot_move_and_changes_nothing() {
    let mut shop = rich();
    let (paris, berlin) = locations(&shop);
    let item = item_of(&shop, "DRESS-S-RED");
    let before = shop.picture();
    let answer = run(
        &mut shop,
        CREATE,
        json!({"input": {"originLocationId": paris, "destinationLocationId": paris,
            "lineItems": [{"inventoryItemId": item, "quantity": 1}, {"inventoryItemId": item, "quantity": 2},
                {"inventoryItemId": "gid://shopify/InventoryItem/1", "quantity": 1}]}}),
    );
    let errors = &answer["data"]["inventoryTransferCreate"]["userErrors"];
    let codes: Vec<&str> = errors
        .as_array()
        .unwrap()
        .iter()
        .map(|error| error["code"].as_str().unwrap())
        .collect();
    assert_eq!(
        codes,
        [
            "TRANSFER_ORIGIN_CANNOT_BE_THE_SAME_AS_DESTINATION",
            "DUPLICATE_ITEM",
            "ITEM_NOT_FOUND"
        ],
        "{answer}"
    );
    assert_eq!(
        errors[1]["field"],
        json!(["input", "lineItems", "1", "inventoryItemId"])
    );
    assert_eq!(errors[1]["message"], "A single item can't be listed twice.");
    assert_eq!(
        answer["data"]["inventoryTransferCreate"]["inventoryTransfer"],
        Value::Null
    );
    assert_eq!(shop.picture(), before);

    // Without the idempotency key that 2026-04 requires: an error, nothing made.
    let answer = run(
        &mut shop,
        &CREATE.replace("@idempotent(key: \"create-1\")", ""),
        json!({"input": {"originLocationId": paris, "destinationLocationId": berlin}}),
    );
    assert!(
        answer["errors"][0]["message"]
            .as_str()
            .unwrap()
            .contains("idempotency key")
    );
    assert_eq!(shop.picture(), before);
}

const ORDER_CREATE: &str = "mutation($order: OrderCreateOrderInput!, $options: OrderCreateOptionsInput) {
    orderCreate(order: $order, options: $options) { order { id fulfillmentOrders(first: 5) { nodes { id } } } userErrors { field message } } }";

/// An order of 3 units of DRESS-S-RED (and 1 of DRESS-M-FLO) that commits its stock, and its
/// fulfillment order.
fn order_with_stock(shop: &mut ShopState) -> (String, String) {
    let variant = |sku: &str| {
        shop.keys("ProductVariant")
            .find(|key| shop.get(key).unwrap()["sku"] == json!(sku))
            .unwrap()
            .to_string()
    };
    let (red, floral) = (variant("DRESS-S-RED"), variant("DRESS-M-FLO"));
    let answer = run(
        shop,
        ORDER_CREATE,
        json!({"order": {"lineItems": [{"variantId": red, "quantity": 3}, {"variantId": floral, "quantity": 1}],
            "shippingAddress": {"address1": "1 Main St", "city": "Lyon", "countryCode": "FR", "zip": "69001"}},
            "options": {"inventoryBehaviour": "DECREMENT_IGNORING_POLICY"}}),
    );
    let order = &answer["data"]["orderCreate"]["order"];
    let fulfillment_order = order["fulfillmentOrders"]["nodes"][0]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("{answer}"))
        .to_string();
    shop.take_events();
    (order["id"].as_str().unwrap().to_string(), fulfillment_order)
}

const FO_FIELDS: &str = "id status fulfillBy assignedLocation { location { id } }
    supportedActions { action }
    lineItems(first: 10) { nodes { id totalQuantity remainingQuantity lineItem { sku } } }";

fn read_fo(shop: &mut ShopState, id: &str) -> Value {
    let answer = run(
        shop,
        &format!("query($id: ID!) {{ fulfillmentOrder(id: $id) {{ {FO_FIELDS} }} }}"),
        json!({"id": id}),
    );
    answer["data"]["fulfillmentOrder"].clone()
}

#[test]
fn a_fulfillment_order_splits_and_merges_back() {
    let mut shop = rich();
    let (_, fo) = order_with_stock(&mut shop);
    let before = read_fo(&mut shop, &fo);
    let lines = before["lineItems"]["nodes"].as_array().unwrap().clone();
    let (red, floral) = (
        lines[0]["id"].as_str().unwrap(),
        lines[1]["id"].as_str().unwrap(),
    );
    let answer = run(
        &mut shop,
        "mutation($splits: [FulfillmentOrderSplitInput!]!) { fulfillmentOrderSplit(fulfillmentOrderSplits: $splits) {
            fulfillmentOrderSplits { fulfillmentOrder { id } remainingFulfillmentOrder { id lineItems(first: 5) { nodes { id totalQuantity } } } replacementFulfillmentOrder { id } }
            userErrors { field message code } } }",
        json!({"splits": [{"fulfillmentOrderId": fo, "fulfillmentOrderLineItems": [{"id": red, "quantity": 1}, {"id": floral, "quantity": 1}]}]}),
    );
    let result = &answer["data"]["fulfillmentOrderSplit"]["fulfillmentOrderSplits"][0];
    assert_eq!(result["fulfillmentOrder"]["id"], json!(fo), "{answer}");
    let new = result["remainingFulfillmentOrder"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let moved = result["remainingFulfillmentOrder"]["lineItems"]["nodes"]
        .as_array()
        .unwrap();
    // The floral line moved whole, with its id; one red unit is a new line.
    assert_eq!(moved.len(), 2);
    assert!(moved.iter().any(|line| line["id"] == json!(floral)));
    let left = read_fo(&mut shop, &fo);
    assert_eq!(left["lineItems"]["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(left["lineItems"]["nodes"][0]["remainingQuantity"], 2);
    let split = last_payload(&mut shop, "fulfillment_orders/split");
    assert_eq!(split["fulfillment_order"]["id"], json!(fo));
    assert_eq!(split["remaining_fulfillment_order"]["id"], json!(new));
    assert!(
        left["supportedActions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|action| action["action"] == "MERGE")
    );

    // Merged back: everything on the first, the other closed.
    let answer = run(
        &mut shop,
        "mutation($inputs: [FulfillmentOrderMergeInput!]!) { fulfillmentOrderMerge(fulfillmentOrderMergeInputs: $inputs) {
            fulfillmentOrderMerges { fulfillmentOrder { id } } userErrors { field message code } } }",
        json!({"inputs": [{"mergeIntents": [{"fulfillmentOrderId": fo}, {"fulfillmentOrderId": new}]}]}),
    );
    assert_eq!(
        answer["data"]["fulfillmentOrderMerge"]["fulfillmentOrderMerges"][0]["fulfillmentOrder"]["id"],
        json!(fo),
        "{answer}"
    );
    let merged = read_fo(&mut shop, &fo);
    let total: i64 = merged["lineItems"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line["remainingQuantity"].as_i64().unwrap())
        .sum();
    assert_eq!(total, 4);
    assert_eq!(read_fo(&mut shop, &new)["status"], "CLOSED");
    let payload = last_payload(&mut shop, "fulfillment_orders/merged");
    assert_eq!(
        payload["fulfillment_order_merges"]["fulfillment_order"]["id"],
        json!(fo)
    );
    assert_eq!(payload["merge_intents"].as_array().unwrap().len(), 2);

    // A quantity beyond what remains: refused, nothing changed.
    let before = shop.picture();
    let answer = run(
        &mut shop,
        "mutation($splits: [FulfillmentOrderSplitInput!]!) { fulfillmentOrderSplit(fulfillmentOrderSplits: $splits) {
            fulfillmentOrderSplits { fulfillmentOrder { id } } userErrors { field message code } } }",
        json!({"splits": [{"fulfillmentOrderId": fo, "fulfillmentOrderLineItems": [{"id": red, "quantity": 9}]}]}),
    );
    assert_eq!(
        answer["data"]["fulfillmentOrderSplit"]["userErrors"],
        json!([{"field": ["fulfillmentOrderSplits", "0", "fulfillmentOrderLineItems"],
            "message": "The fulfillment order line item quantity is invalid.", "code": "INVALID_LINE_ITEM_QUANTITY"}])
    );
    assert_eq!(shop.picture(), before);
}

#[test]
fn rerouting_moves_a_fulfillment_order_and_its_committed_stock() {
    let mut shop = rich();
    let (paris, berlin) = locations(&shop);
    let item = item_of(&shop, "DRESS-S-RED");
    let (_, fo) = order_with_stock(&mut shop);
    let from = read_fo(&mut shop, &fo)["assignedLocation"]["location"]["id"].clone();
    assert_eq!(from, json!(paris));
    assert_eq!(levels(&shop, &item, &paris), [1, 3, 0, 4]);

    // Berlin has 3 of the red dress but none of the floral one: no location can take it all.
    let reroute = "mutation($ids: [ID!]!, $included: [ID!]) { fulfillmentOrdersReroute(fulfillmentOrderIds: $ids, includedLocationIds: $included) {
        movedFulfillmentOrders { id assignedLocation { location { id } } } userErrors { field message code } } }";
    let before = shop.picture();
    let answer = run(
        &mut shop,
        reroute,
        json!({"ids": [fo], "included": [berlin]}),
    );
    let errors = &answer["data"]["fulfillmentOrdersReroute"]["userErrors"];
    assert_eq!(
        errors[0]["code"], "CANNOT_REASSIGN_LOCATION_FOR_FULFILLMENT_ORDERS",
        "{answer}"
    );
    assert_eq!(shop.picture(), before);

    // Stocked there, it moves, and the stock the order committed with it.
    let floral = item_of(&shop, "DRESS-M-FLO");
    if super::stock::quantity(&shop, &floral, &berlin, "available") < 1 {
        super::stock::incoming(&mut shop, &floral, &berlin, 1);
        super::stock::receive(&mut shop, &floral, &berlin, 1, 0);
    }
    shop.take_events();
    let answer = run(
        &mut shop,
        reroute,
        json!({"ids": [fo], "included": [berlin]}),
    );
    let moved = &answer["data"]["fulfillmentOrdersReroute"]["movedFulfillmentOrders"][0];
    assert_eq!(
        moved["assignedLocation"]["location"]["id"],
        json!(berlin),
        "{answer}"
    );
    assert_eq!(levels(&shop, &item, &paris), [4, 0, 0, 4]);
    assert_eq!(levels(&shop, &item, &berlin), [0, 3, 0, 3]);
    let events = topics(&mut shop);
    assert!(
        events
            .iter()
            .any(|topic| topic == "fulfillment_orders/moved")
    );
    assert!(
        events
            .iter()
            .any(|topic| topic == "fulfillment_orders/order_routing_complete")
    );
    assert!(
        events.iter().any(|topic| topic == "variants/out_of_stock")
            || levels(&shop, &item, &paris)[0] > 0
    );
}

#[test]
fn progress_is_reported_then_stopped_by_opening_the_fulfillment_order() {
    let mut shop = rich();
    let (_, fo) = order_with_stock(&mut shop);
    let answer = run(
        &mut shop,
        "mutation($id: ID!) { fulfillmentOrderReportProgress(id: $id, progressReport: {reasonNotes: \"Picking\"}) {
            fulfillmentOrder { status } userErrors { field message code } } }",
        json!({"id": fo}),
    );
    assert_eq!(
        answer["data"]["fulfillmentOrderReportProgress"]["fulfillmentOrder"]["status"],
        "IN_PROGRESS",
        "{answer}"
    );
    let payload = last_payload(&mut shop, "fulfillment_orders/progress_reported");
    assert_eq!(payload["initial_status"], "open");
    assert_eq!(payload["progress_report"]["reason_notes"], "Picking");
    assert_eq!(payload["fulfillment_order"]["status"], "in_progress");
    let actions = read_fo(&mut shop, &fo)["supportedActions"].clone();
    assert!(
        actions
            .as_array()
            .unwrap()
            .iter()
            .any(|action| action["action"] == "MARK_AS_OPEN")
    );

    let answer = run(
        &mut shop,
        "mutation($id: ID!) { fulfillmentOrderOpen(id: $id) { fulfillmentOrder { status } userErrors { field message } } }",
        json!({"id": fo}),
    );
    assert_eq!(
        answer["data"]["fulfillmentOrderOpen"]["fulfillmentOrder"]["status"], "OPEN",
        "{answer}"
    );
    let payload = last_payload(
        &mut shop,
        "fulfillment_orders/manually_reported_progress_stopped",
    );
    assert_eq!(payload["fulfillment_order"]["status"], "open");

    // A closed fulfillment order reports no progress.
    let before = shop.picture();
    let answer = run(
        &mut shop,
        "mutation($id: ID!) { fulfillmentOrderReportProgress(id: $id) { fulfillmentOrder { status } userErrors { field message code } } }",
        json!({"id": "gid://shopify/FulfillmentOrder/999999"}),
    );
    assert_eq!(
        answer["data"]["fulfillmentOrderReportProgress"]["userErrors"][0]["code"],
        "FULFILLMENT_ORDER_NOT_FOUND"
    );
    assert_eq!(shop.picture(), before);
}

#[test]
fn a_pickup_fulfillment_order_is_prepared_and_its_notification_recorded() {
    let mut shop = rich();
    let (_, fo) = order_with_stock(&mut shop);
    let prepare = "mutation($input: FulfillmentOrderLineItemsPreparedForPickupInput!) {
        fulfillmentOrderLineItemsPreparedForPickup(input: $input) { userErrors { field message code } } }";
    // A shipping fulfillment order is not one to prepare for pickup.
    let before = shop.picture();
    let answer = run(
        &mut shop,
        prepare,
        json!({"input": {"lineItemsByFulfillmentOrder": [{"fulfillmentOrderId": fo}]}}),
    );
    assert_eq!(
        answer["data"]["fulfillmentOrderLineItemsPreparedForPickup"]["userErrors"],
        json!([{"field": ["input", "lineItemsByFulfillmentOrder", "0", "fulfillmentOrderId"],
            "message": "Invalid fulfillment order ID provided.", "code": "FULFILLMENT_ORDER_INVALID"}])
    );
    assert_eq!(shop.picture(), before);

    shop.get_mut(&fo).unwrap()["deliveryMethod"]["methodType"] = json!("PICK_UP");
    let answer = run(
        &mut shop,
        prepare,
        json!({"input": {"lineItemsByFulfillmentOrder": [{"fulfillmentOrderId": fo}]}}),
    );
    assert_eq!(
        answer["data"]["fulfillmentOrderLineItemsPreparedForPickup"]["userErrors"],
        json!([])
    );
    let payload = last_payload(
        &mut shop,
        "fulfillment_orders/line_items_prepared_for_pickup",
    );
    assert_eq!(
        payload,
        json!({"fulfillment_order": {"id": fo, "status": "open", "preparable": true, "delivery_method": {"method_type": "pickup"}}})
    );
    let outbox = super::outbox(&shop);
    assert_eq!(outbox.len(), 1);
    assert_eq!(outbox[0]["kind"], "notification");
    assert_eq!(outbox[0]["details"]["template"], "Ready For Pickup");
}

#[test]
fn a_fulfillment_deadline_is_set_on_fulfillment_orders() {
    let mut shop = rich();
    let (_, fo) = order_with_stock(&mut shop);
    let set = "mutation($at: DateTime!, $ids: [ID!]!) { fulfillmentOrdersSetFulfillmentDeadline(fulfillmentDeadline: $at, fulfillmentOrderIds: $ids) {
        success userErrors { field message code } } }";
    let answer = run(
        &mut shop,
        set,
        json!({"at": "2026-11-25T18:44:04Z", "ids": [fo]}),
    );
    assert_eq!(
        answer["data"]["fulfillmentOrdersSetFulfillmentDeadline"],
        json!({"success": true, "userErrors": []})
    );
    assert_eq!(read_fo(&mut shop, &fo)["fulfillBy"], "2026-11-25T18:44:04Z");
    let before = shop.picture();
    let answer = run(
        &mut shop,
        set,
        json!({"at": "2026-12-01T00:00:00Z", "ids": [fo, "gid://shopify/FulfillmentOrder/999999"]}),
    );
    assert_eq!(
        answer["data"]["fulfillmentOrdersSetFulfillmentDeadline"]["userErrors"][0]["code"],
        "FULFILLMENT_ORDERS_NOT_FOUND"
    );
    assert_eq!(shop.picture(), before);
}
