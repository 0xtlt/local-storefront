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
        json!({"order": {"lineItems": [{"variantId": red, "quantity": 3, "requiresShipping": true}, {"variantId": floral, "quantity": 1, "requiresShipping": true}],
            "shippingAddress": {"address1": "1 Main St", "city": "Lyon", "countryCode": "FR", "zip": "69001"},
            "shippingLines": [{"title": "Standard", "priceSet": {"shopMoney": {"amount": "5.00", "currencyCode": "EUR"}}}]},
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

/// The extensions an answer reports about defaults (`defaulted`, `defaultedFields`).
fn defaulted(answer: &Value) -> Value {
    answer
        .pointer("/extensions/lsf/defaulted")
        .cloned()
        .unwrap_or(Value::Null)
}

#[test]
fn the_demo_store_answers_the_logistics_queries_with_real_values() {
    let mut shop = demo();
    let location = shop.keys("Location").next().unwrap().to_string();
    let variant = shop.keys("ProductVariant").next().unwrap().to_string();
    let answer = run(
        &mut shop,
        "query($location: ID!, $variant: ID!) {
          inventoryTransfers(first: 5) { nodes { id name status } }
          inventoryShipments(first: 5) { nodes { id name status } }
          carrierServices(first: 5) { nodes { id name active } }
          availableCarrierServices { carrierService { id } locations { id } }
          fulfillmentConstraintRules { id deliveryMethodTypes }
          deliveryPromiseSettings { deliveryDatesEnabled processingTime }
          deliveryPromiseProvider(locationId: $location) { id }
          deliveryPromiseParticipants(brandedPromiseHandle: \"shop_promise\", first: 5) { nodes { id } }
          locationsAvailableForDeliveryProfiles { id name }
          locationsAvailableForDeliveryProfilesConnection(first: 5) { nodes { id } }
          deliveryProfiles(first: 5) { nodes { id name default version activeMethodDefinitionsCount
            originLocationCount locationsWithoutRatesCount zoneCountryCount createdAt
            productVariantsCount { count precision }
            unassignedLocations { id }
            profileItems(first: 3) { nodes { id product { id } variants(first: 2) { nodes { id } } } }
            profileLocationGroups { locationGroup { id locationsCount { count } locations(first: 5) { nodes { id } } }
              countriesInAnyZone { zone country { name code { restOfWorld countryCode } } }
              locationGroupZones(first: 5) { nodes { zone { id name countries { id name code { restOfWorld } provinces { code } } }
                methodDefinitionCounts { rateDefinitionsCount participantDefinitionsCount }
                methodDefinitions(first: 5) { nodes { id name description active
                  rateProvider { ... on DeliveryRateDefinition { id price { amount currencyCode } } } methodConditions { id } } } } } } } }
          productVariant(id: $variant) { deliveryProfile { id name default } }
        }",
        json!({"location": location, "variant": variant}),
    );
    assert!(answer.get("errors").is_none(), "{answer}");
    assert_eq!(defaulted(&answer), Value::Null, "{answer}");
    let data = &answer["data"];
    assert_eq!(
        data["deliveryPromiseSettings"],
        json!({"deliveryDatesEnabled": false, "processingTime": null})
    );
    let profile = &data["deliveryProfiles"]["nodes"][0];
    assert_eq!(profile["name"], "General profile");
    assert_eq!(profile["default"], true);
    assert_eq!(
        profile["productVariantsCount"]["count"],
        json!(shop.count("ProductVariant"))
    );
    assert_eq!(profile["unassignedLocations"], json!([]));
    let zone = &profile["profileLocationGroups"][0]["locationGroupZones"]["nodes"][0];
    assert_eq!(zone["zone"]["countries"][0]["code"]["restOfWorld"], true);
    // The store data's shipping rates are the general profile's.
    let (store, _) = lsf_core::store::load::load(
        &lsf_core::store::load::DataSource::Demo,
        &Default::default(),
    );
    let shipping: Vec<&str> = store
        .checkout
        .shipping_rates
        .iter()
        .filter(|rate| rate.kind == lsf_core::store::model::CheckoutDeliveryKind::Shipping)
        .map(|rate| rate.title.as_str())
        .collect();
    let names: Vec<&str> = zone["methodDefinitions"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|method| method["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, shipping);
    assert_eq!(
        data["productVariant"]["deliveryProfile"]["id"],
        profile["id"]
    );
}

const PROFILE_CREATE: &str = "mutation($profile: DeliveryProfileInput!) { deliveryProfileCreate(profile: $profile) {
    profile { id name version productVariantsCount { count }
      profileLocationGroups { locationGroup { id } locationGroupZones(first: 5) { nodes { zone { id name countries { code { countryCode } provinces { code } } }
        methodDefinitions(first: 5) { nodes { id name active methodConditions { field operator conditionCriteria { ... on Weight { unit value } } }
          rateProvider { ... on DeliveryRateDefinition { price { amount currencyCode } } ... on DeliveryParticipant { carrierService { id } } } } } } } } }
    userErrors { field message } } }";

#[test]
fn a_delivery_profile_is_created_updated_and_removed_with_its_variants() {
    let mut shop = demo();
    let location = shop.keys("Location").next().unwrap().to_string();
    let variant = shop.keys("ProductVariant").next().unwrap().to_string();
    let answer = run(
        &mut shop,
        PROFILE_CREATE,
        json!({"profile": {"name": "Heavy goods", "variantsToAssociate": [variant],
            "locationGroupsToCreate": [{"locationsToAdd": [location], "zonesToCreate": [{
                "name": "Canada Zone", "countries": {"code": "CA", "provinces": [{"code": "ON"}]},
                "methodDefinitionsToCreate": [{"name": "Standard", "rateDefinition": {"price": {"amount": 1, "currencyCode": "USD"}},
                    "weightConditionsToCreate": [{"operator": "LESS_THAN_OR_EQUAL_TO", "criteria": {"value": 15, "unit": "KILOGRAMS"}}]}]}]}]}}),
    );
    let created = &answer["data"]["deliveryProfileCreate"];
    assert_eq!(created["userErrors"], json!([]), "{answer}");
    let profile = created["profile"]["id"].as_str().unwrap().to_string();
    assert_eq!(created["profile"]["productVariantsCount"]["count"], 1);
    let zone = &created["profile"]["profileLocationGroups"][0]["locationGroupZones"]["nodes"][0];
    assert_eq!(zone["zone"]["name"], "Canada Zone");
    assert_eq!(zone["zone"]["countries"][0]["provinces"][0]["code"], "ON");
    let method = &zone["methodDefinitions"]["nodes"][0];
    assert_eq!(method["rateProvider"]["price"]["amount"], "1.0");
    assert_eq!(
        method["methodConditions"][0]["conditionCriteria"]["value"],
        15.0
    );
    let payload = last_payload(&mut shop, "profiles/create");
    assert_eq!(payload["name"], "Heavy goods");
    assert_eq!(payload["default"], false);
    let read = |shop: &mut ShopState| {
        run(
            shop,
            "query($id: ID!) { productVariant(id: $id) { deliveryProfile { id } } }",
            json!({"id": variant}),
        )["data"]["productVariant"]["deliveryProfile"]["id"]
            .clone()
    };
    assert_eq!(read(&mut shop), json!(profile));

    // Renamed, a rate added: its version moves on.
    let group = created["profile"]["profileLocationGroups"][0]["locationGroup"]["id"].clone();
    let zone_id = zone["zone"]["id"].clone();
    let answer = run(
        &mut shop,
        "mutation($id: ID!, $profile: DeliveryProfileInput!) { deliveryProfileUpdate(id: $id, profile: $profile) {
            profile { name version activeMethodDefinitionsCount } userErrors { field message } } }",
        json!({"id": profile, "profile": {"name": "Bulky", "locationGroupsToUpdate": [{"id": group,
            "zonesToUpdate": [{"id": zone_id, "methodDefinitionsToCreate": [{"name": "Express", "rateDefinition": {"price": {"amount": "9.50", "currencyCode": "USD"}}}]}]}]}}),
    );
    assert_eq!(
        answer["data"]["deliveryProfileUpdate"]["profile"],
        json!({"name": "Bulky", "version": 2, "activeMethodDefinitionsCount": 2}),
        "{answer}"
    );
    assert_eq!(last_payload(&mut shop, "profiles/update")["version"], 2);

    // A blank name: refused, nothing changed.
    let before = shop.picture();
    let answer = run(
        &mut shop,
        PROFILE_CREATE,
        json!({"profile": {"name": " ", "variantsToAssociate": [variant]}}),
    );
    assert_eq!(
        answer["data"]["deliveryProfileCreate"]["userErrors"],
        json!([{"field": ["profile", "name"], "message": "Name can't be blank"}])
    );
    assert_eq!(shop.picture(), before);

    // The general profile cannot go; this one can, and its variant goes back to the general one.
    let general = super::profiles::general_profile_gid();
    let remove = "mutation($id: ID!) { deliveryProfileRemove(id: $id) { job { id done } userErrors { field message } } }";
    let answer = run(&mut shop, remove, json!({"id": general}));
    assert_eq!(
        answer["data"]["deliveryProfileRemove"]["userErrors"][0]["message"],
        "The general profile can't be removed"
    );
    let answer = run(&mut shop, remove, json!({"id": profile}));
    assert_eq!(
        answer["data"]["deliveryProfileRemove"]["job"]["done"], true,
        "{answer}"
    );
    assert_eq!(read(&mut shop), json!(general));
    assert_eq!(
        last_payload(&mut shop, "profiles/delete")["admin_graphql_api_id"],
        json!(profile)
    );
    assert!(shop.get(&profile).is_none());
}

#[test]
fn a_carrier_service_is_created_listed_updated_and_deleted_with_its_rates() {
    let mut shop = demo();
    let create = "mutation($input: DeliveryCarrierServiceCreateInput!) { carrierServiceCreate(input: $input) {
        carrierService { id name formattedName callbackUrl active supportsServiceDiscovery } userErrors { field message code } } }";
    let answer = run(
        &mut shop,
        create,
        json!({"input": {"name": "test carrier service", "callbackUrl": "https://example.com/", "supportsServiceDiscovery": true, "active": true}}),
    );
    let service = &answer["data"]["carrierServiceCreate"]["carrierService"];
    assert_eq!(service["name"], "test carrier service", "{answer}");
    assert_eq!(service["callbackUrl"], "https://example.com/");
    let id = service["id"].as_str().unwrap().to_string();

    // The same name again, and a callback that is no URL: refused, nothing changed.
    let before = shop.picture();
    let answer = run(
        &mut shop,
        create,
        json!({"input": {"name": "Test carrier service", "callbackUrl": "ftp://x", "supportsServiceDiscovery": false, "active": true}}),
    );
    assert_eq!(
        answer["data"]["carrierServiceCreate"]["userErrors"],
        json!([
            {"field": ["input", "name"], "message": "Name has already been taken", "code": "CARRIER_SERVICE_CREATE_FAILED"},
            {"field": ["input", "callbackUrl"], "message": "Callback url is invalid", "code": "CARRIER_SERVICE_CREATE_FAILED"},
        ])
    );
    assert_eq!(shop.picture(), before);

    let answer = run(
        &mut shop,
        "query { carrierServices(first: 5, query: \"active:true\") { nodes { id } } availableCarrierServices { carrierService { id } locations { id } } }",
        json!({}),
    );
    assert_eq!(
        answer["data"]["carrierServices"]["nodes"][0]["id"],
        json!(id)
    );
    assert_eq!(
        answer["data"]["availableCarrierServices"][0]["locations"]
            .as_array()
            .unwrap()
            .len(),
        shop.count("Location")
    );
    let answer = run(
        &mut shop,
        "mutation($input: DeliveryCarrierServiceUpdateInput!) { carrierServiceUpdate(input: $input) { carrierService { name active } userErrors { message } } }",
        json!({"input": {"id": id, "name": "new test carrier service", "active": false}}),
    );
    assert_eq!(
        answer["data"]["carrierServiceUpdate"]["carrierService"],
        json!({"name": "new test carrier service", "active": false})
    );

    // A rate of the general profile from the carrier service goes with it.
    let general = super::profiles::general_profile_gid();
    let group = shop.get(&general).unwrap()["profileLocationGroups"][0]["locationGroup"].clone();
    let zone =
        shop.get(&general).unwrap()["profileLocationGroups"][0]["locationGroupZones"][0]["zone"]
            .clone();
    let answer = run(
        &mut shop,
        "mutation($id: ID!, $profile: DeliveryProfileInput!) { deliveryProfileUpdate(id: $id, profile: $profile) {
            profile { activeMethodDefinitionsCount } userErrors { field message } } }",
        json!({"id": general, "profile": {"locationGroupsToUpdate": [{"id": group, "zonesToUpdate": [{"id": zone,
            "methodDefinitionsToCreate": [{"name": "Live rates", "participant": {"carrierServiceId": id, "adaptToNewServices": true}}]}]}]}}),
    );
    let with_carrier =
        answer["data"]["deliveryProfileUpdate"]["profile"]["activeMethodDefinitionsCount"]
            .as_i64()
            .unwrap_or_else(|| panic!("{answer}"));
    let answer = run(
        &mut shop,
        "mutation($id: ID!) { carrierServiceDelete(id: $id) { deletedId userErrors { message } } }",
        json!({"id": id}),
    );
    assert_eq!(
        answer["data"]["carrierServiceDelete"]["deletedId"],
        json!(id)
    );
    assert_eq!(
        super::profiles::method_count(&shop, &general),
        usize::try_from(with_carrier - 1).unwrap()
    );
}

#[test]
fn a_fulfillment_constraint_rule_runs_a_function_of_the_api() {
    let mut shop = demo();
    let create = "mutation($handle: String, $types: [DeliveryMethodType!]!) {
        fulfillmentConstraintRuleCreate(functionHandle: $handle, deliveryMethodTypes: $types) {
          fulfillmentConstraintRule { id deliveryMethodTypes function { id handle } } userErrors { field message code } } }";
    let before = shop.picture();
    let answer = run(
        &mut shop,
        create,
        json!({"handle": "keep-together", "types": ["SHIPPING"]}),
    );
    assert_eq!(
        answer["data"]["fulfillmentConstraintRuleCreate"]["userErrors"],
        json!([{"field": ["functionHandle"], "message": "No Shopify Function found for provided function_id.", "code": "FUNCTION_NOT_FOUND"}])
    );
    assert_eq!(shop.picture(), before);

    let function = "4b8d6d2e-1f6a-4c3e-9d1c-000000000001";
    shop.insert_as(
        function,
        crate::state::value::object(json!({
            "__typename": "ShopifyFunction", "id": function, "handle": "keep-together",
            "title": "Keep together", "apiType": "fulfillment_constraints",
        })),
    );
    let answer = run(
        &mut shop,
        create,
        json!({"handle": "keep-together", "types": ["SHIPPING", "LOCAL"]}),
    );
    let rule = &answer["data"]["fulfillmentConstraintRuleCreate"]["fulfillmentConstraintRule"];
    assert_eq!(rule["function"]["handle"], "keep-together", "{answer}");
    let id = rule["id"].as_str().unwrap().to_string();
    let again = run(
        &mut shop,
        create,
        json!({"handle": "keep-together", "types": ["SHIPPING"]}),
    );
    assert_eq!(
        again["data"]["fulfillmentConstraintRuleCreate"]["userErrors"][0]["code"],
        "FUNCTION_ALREADY_REGISTERED"
    );
    let answer = run(
        &mut shop,
        "mutation($id: ID!) { fulfillmentConstraintRuleUpdate(id: $id, deliveryMethodTypes: [PICK_UP]) { fulfillmentConstraintRule { deliveryMethodTypes } userErrors { code } } }",
        json!({"id": id}),
    );
    assert_eq!(
        answer["data"]["fulfillmentConstraintRuleUpdate"]["fulfillmentConstraintRule"]["deliveryMethodTypes"],
        json!(["PICK_UP"])
    );
    let listed = run(
        &mut shop,
        "{ fulfillmentConstraintRules { id } }",
        json!({}),
    );
    assert_eq!(
        listed["data"]["fulfillmentConstraintRules"][0]["id"],
        json!(id)
    );
    let answer = run(
        &mut shop,
        "mutation($id: ID!) { fulfillmentConstraintRuleDelete(id: $id) { success userErrors { code } } }",
        json!({"id": id}),
    );
    assert_eq!(
        answer["data"]["fulfillmentConstraintRuleDelete"]["success"],
        true
    );
    assert!(shop.get(&id).is_none());
}

#[test]
fn a_shipping_label_is_purchased_at_once_and_fulfills_the_fulfillment_order() {
    let mut shop = rich();
    let (order, fo) = order_with_stock(&mut shop);
    let purchase = "mutation($input: ShippingLabelPurchaseInput!) { shippingLabelPurchase(shippingLabelPurchase: $input) {
        shippingLabelPurchaseResult { id done status errors { message } shippingLabels { id printed cancellable location { id }
          trackingInfo { company number } shippingDocuments { documentType format } } }
        userErrors { field message code } } }";
    let today = shop.timestamp();
    let before = shop.picture();
    let answer = run(
        &mut shop,
        purchase,
        json!({"input": {"fulfillmentOrderId": fo, "shippingDatetime": "2001-01-01T00:00:00Z"}}),
    );
    assert_eq!(
        answer["data"]["shippingLabelPurchase"]["userErrors"],
        json!([{"field": ["shippingLabelPurchase", "shippingDatetime"], "message": "Shipping date has already passed.", "code": "SHIPPING_DATE_IN_THE_PAST"}])
    );
    assert_eq!(shop.picture(), before);

    let answer = run(
        &mut shop,
        purchase,
        json!({"input": {"fulfillmentOrderId": fo, "shippingDatetime": today, "notifyCustomer": true,
            "preferredRateSelection": {"carrierCode": "ups", "serviceCode": "ground"}}}),
    );
    let result = &answer["data"]["shippingLabelPurchase"]["shippingLabelPurchaseResult"];
    assert_eq!(result["status"], "PURCHASED", "{answer}");
    assert_eq!(result["done"], true);
    let label = &result["shippingLabels"][0];
    assert_eq!(label["trackingInfo"]["company"], "UPS");
    assert_eq!(label["shippingDocuments"][0]["documentType"], "LABEL");
    let number = label["trackingInfo"]["number"]
        .as_str()
        .unwrap()
        .to_string();
    // The units are fulfilled with the label's tracking.
    assert_eq!(read_fo(&mut shop, &fo)["status"], "CLOSED");
    let fulfillments = run(
        &mut shop,
        "query($id: ID!) { order(id: $id) { displayFulfillmentStatus fulfillments { trackingInfo { number } } } }",
        json!({"id": order}),
    );
    assert_eq!(
        fulfillments["data"]["order"]["displayFulfillmentStatus"],
        "FULFILLED"
    );
    assert_eq!(
        fulfillments["data"]["order"]["fulfillments"][0]["trackingInfo"][0]["number"],
        json!(number)
    );
    let outbox = super::outbox(&shop);
    let kinds: Vec<&str> = outbox
        .iter()
        .map(|entry| entry["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["shipping_label", "notification"]);
    assert_eq!(outbox[0]["details"]["serviceCode"], "ground");
    let read = run(
        &mut shop,
        "query($id: ID!) { node(id: $id) { ... on ShippingLabelPurchaseResult { status } } }",
        json!({"id": result["id"]}),
    );
    assert_eq!(read["data"]["node"]["status"], "PURCHASED");
}

#[test]
fn delivery_promises_settings_and_packages() {
    let mut shop = demo();
    let location = shop.keys("Location").next().unwrap().to_string();
    let upsert = "mutation($location: ID!, $zone: String) { deliveryPromiseProviderUpsert(locationId: $location, timeZone: $zone, fulfillmentDelay: 3600) {
        deliveryPromiseProvider { id active timeZone fulfillmentDelay location { id } } userErrors { field message code } } }";
    let before = shop.picture();
    let answer = run(&mut shop, upsert, json!({"location": location}));
    assert_eq!(
        answer["data"]["deliveryPromiseProviderUpsert"]["userErrors"][0]["code"],
        "MUST_BELONG_TO_APP"
    );
    assert_eq!(shop.picture(), before);
    // A fulfillment service's location is the app's.
    let service = run(
        &mut shop,
        "mutation { fulfillmentServiceCreate(name: \"Warehouse partner\", callbackUrl: \"https://example.com/fulfillment\") {
            fulfillmentService { location { id } } userErrors { message } } }",
        json!({}),
    );
    let partner =
        service["data"]["fulfillmentServiceCreate"]["fulfillmentService"]["location"]["id"]
            .as_str()
            .unwrap_or_else(|| panic!("{service}"))
            .to_string();
    let answer = run(
        &mut shop,
        upsert,
        json!({"location": partner, "zone": "Mars/Olympus"}),
    );
    assert_eq!(
        answer["data"]["deliveryPromiseProviderUpsert"]["userErrors"][0]["code"],
        "INVALID_TIME_ZONE"
    );
    let answer = run(
        &mut shop,
        upsert,
        json!({"location": partner, "zone": "America/Toronto"}),
    );
    let provider = &answer["data"]["deliveryPromiseProviderUpsert"]["deliveryPromiseProvider"];
    assert_eq!(provider["timeZone"], "America/Toronto", "{answer}");
    assert_eq!(provider["active"], true);
    let read = run(
        &mut shop,
        "query($location: ID!) { deliveryPromiseProvider(locationId: $location) { id fulfillmentDelay } }",
        json!({"location": partner}),
    );
    assert_eq!(
        read["data"]["deliveryPromiseProvider"]["fulfillmentDelay"],
        3600
    );

    // Participants of a branded promise.
    let variant = shop.keys("ProductVariant").next().unwrap().to_string();
    let update = "mutation($add: [ID!], $remove: [ID!]) { deliveryPromiseParticipantsUpdate(brandedPromiseHandle: \"express\", ownersToAdd: $add, ownersToRemove: $remove) {
        promiseParticipants { id owner { ... on ProductVariant { id } } ownerType } userErrors { field message } } }";
    let answer = run(&mut shop, update, json!({"add": [variant]}));
    assert_eq!(
        answer["data"]["deliveryPromiseParticipantsUpdate"]["promiseParticipants"][0]["owner"]["id"],
        json!(variant),
        "{answer}"
    );
    let listed = run(
        &mut shop,
        "{ deliveryPromiseParticipants(brandedPromiseHandle: \"express\", first: 5) { nodes { ownerType } } }",
        json!({}),
    );
    assert_eq!(
        listed["data"]["deliveryPromiseParticipants"]["nodes"][0]["ownerType"],
        "PRODUCTVARIANT"
    );
    run(&mut shop, update, json!({"remove": [variant]}));
    assert_eq!(shop.count("DeliveryPromiseParticipant"), 0);

    // Settings and the legacy shipping origin.
    let answer = run(
        &mut shop,
        "mutation { deliverySettingUpdate { userErrors { message } } }",
        json!({}),
    );
    assert_eq!(
        answer["data"]["deliverySettingUpdate"]["userErrors"],
        json!([])
    );
    let answer = run(
        &mut shop,
        "mutation($id: ID!) { deliveryShippingOriginAssign(locationId: $id) { userErrors { field message } } }",
        json!({"id": location}),
    );
    assert_eq!(
        answer["data"]["deliveryShippingOriginAssign"]["userErrors"][0]["message"],
        "Single origin shipping mode is no longer supported."
    );

    // The sample box: updated, made default, deleted.
    let package = "gid://shopify/ShippingPackage/1";
    let answer = run(
        &mut shop,
        "mutation($id: ID!) { shippingPackageUpdate(id: $id, shippingPackage: {name: \"Small box\", dimensions: {length: 20, width: 10, height: 5, unit: CENTIMETERS}}) { userErrors { message } } }",
        json!({"id": package}),
    );
    assert_eq!(
        answer["data"]["shippingPackageUpdate"]["userErrors"],
        json!([])
    );
    assert_eq!(super::delivery::packages(&shop)[0]["name"], "Small box");
    let answer = run(
        &mut shop,
        "mutation($id: ID!) { shippingPackageMakeDefault(id: $id) { userErrors { field message } } }",
        json!({"id": "gid://shopify/ShippingPackage/9"}),
    );
    assert_eq!(
        answer["data"]["shippingPackageMakeDefault"]["userErrors"],
        json!([{"field": ["id"], "message": "Shipping package not found."}])
    );
    let answer = run(
        &mut shop,
        "mutation($id: ID!) { shippingPackageMakeDefault(id: $id) { userErrors { message } } }",
        json!({"id": package}),
    );
    assert_eq!(
        answer["data"]["shippingPackageMakeDefault"]["userErrors"],
        json!([])
    );
    let answer = run(
        &mut shop,
        "mutation($id: ID!) { shippingPackageDelete(id: $id) { deletedId userErrors { message } } }",
        json!({"id": package}),
    );
    assert_eq!(
        answer["data"]["shippingPackageDelete"]["deletedId"],
        json!(package)
    );
    assert!(super::delivery::packages(&shop).is_empty());
}

/// Runs a mutation expected to refuse its input: its user errors' codes, the shop unchanged.
fn refused(shop: &mut ShopState, query: &str, variables: Value, name: &str) -> Vec<String> {
    let before = shop.picture();
    let answer = run(shop, query, variables);
    assert_eq!(shop.picture(), before, "{name} changed the shop: {answer}");
    let errors = answer["data"][name]["userErrors"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(!errors.is_empty(), "{name} refused nothing: {answer}");
    errors
        .iter()
        .map(|error| {
            error["code"]
                .as_str()
                .or(error["message"].as_str())
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Runs a mutation expected to succeed: its payload.
fn done(shop: &mut ShopState, query: &str, variables: Value, name: &str) -> Value {
    let answer = run(shop, query, variables);
    let payload = answer["data"][name].clone();
    assert_eq!(payload["userErrors"], json!([]), "{name}: {answer}");
    payload
}

#[test]
fn a_transfer_is_edited_given_items_duplicated_and_deleted() {
    let mut shop = rich();
    let (paris, berlin) = locations(&shop);
    let red = item_of(&shop, "DRESS-S-RED");
    let floral = item_of(&shop, "DRESS-M-FLO");
    let created = done(
        &mut shop,
        CREATE,
        json!({"input": {"originLocationId": paris, "destinationLocationId": berlin,
            "lineItems": [{"inventoryItemId": red, "quantity": 2}]}}),
        "inventoryTransferCreate",
    );
    let transfer = created["inventoryTransfer"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    shop.take_events();

    let edit = "mutation($id: ID!, $input: InventoryTransferEditInput!) { inventoryTransferEdit(id: $id, input: $input) {
        inventoryTransfer { referenceName note tags dateCreated destination { location { id } } } userErrors { field message code } } }";
    let edited = done(
        &mut shop,
        edit,
        json!({"id": transfer, "input": {"referenceName": "PO-7", "note": "Fragile", "tags": ["spring"], "dateCreated": "2026-03-01"}}),
        "inventoryTransferEdit",
    );
    assert_eq!(edited["inventoryTransfer"]["referenceName"], "PO-7");
    assert_eq!(
        edited["inventoryTransfer"]["dateCreated"],
        "2026-03-01T00:00:00Z"
    );
    assert_eq!(
        last_payload(&mut shop, "inventory_transfers/updated")["id"],
        json!(transfer)
    );
    assert_eq!(
        refused(
            &mut shop,
            edit,
            json!({"id": transfer, "input": {"originId": berlin, "destinationId": berlin}}),
            "inventoryTransferEdit"
        ),
        ["TRANSFER_ORIGIN_CANNOT_BE_THE_SAME_AS_DESTINATION"]
    );

    let set = "mutation($input: InventoryTransferSetItemsInput!) { inventoryTransferSetItems(input: $input) @idempotent(key: $key) {
        updatedLineItems { inventoryItemId newQuantity deltaQuantity } userErrors { field message code } } }";
    let set = |key: &str| set.replace("$key", &format!("\"{key}\""));
    let answer = done(
        &mut shop,
        &set("a"),
        json!({"input": {"id": transfer, "lineItems": [{"inventoryItemId": red, "quantity": 3}, {"inventoryItemId": floral, "quantity": 0}]}}),
        "inventoryTransferSetItems",
    );
    assert_eq!(
        answer["updatedLineItems"],
        json!([{"inventoryItemId": red, "newQuantity": 3, "deltaQuantity": 1}, {"inventoryItemId": floral, "newQuantity": 0, "deltaQuantity": 0}])
    );
    let events = shop.take_events();
    assert!(
        events
            .iter()
            .any(|event| event.topic == "inventory_transfers/update_item_quantities")
    );
    assert!(
        events
            .iter()
            .any(|event| event.topic == "inventory_transfers/add_items")
    );
    assert_eq!(
        refused(
            &mut shop,
            &set("b"),
            json!({"input": {"id": transfer, "lineItems": [{"inventoryItemId": red, "quantity": 1}, {"inventoryItemId": red, "quantity": 2}]}}),
            "inventoryTransferSetItems"
        ),
        ["DUPLICATE_ITEM"]
    );

    // Ready to ship: 3 committed; 0 is no longer a quantity, the origin no longer moves.
    done(
        &mut shop,
        "mutation($id: ID!) { inventoryTransferMarkAsReadyToShip(id: $id) { inventoryTransfer { id } userErrors { code } } }",
        json!({"id": transfer}),
        "inventoryTransferMarkAsReadyToShip",
    );
    assert_eq!(levels(&shop, &red, &paris), [1, 3, 0, 4]);
    assert_eq!(
        refused(
            &mut shop,
            &set("c"),
            json!({"input": {"id": transfer, "lineItems": [{"inventoryItemId": red, "quantity": 0}]}}),
            "inventoryTransferSetItems"
        ),
        ["INVALID_QUANTITY"]
    );
    assert_eq!(
        refused(
            &mut shop,
            edit,
            json!({"id": transfer, "input": {"originId": berlin}}),
            "inventoryTransferEdit"
        ),
        ["TRANSFER_LOCATION_IMMUTABLE"]
    );
    // Reduced to 2 on a ready-to-ship transfer: one unit back to available.
    done(
        &mut shop,
        &set("d"),
        json!({"input": {"id": transfer, "lineItems": [{"inventoryItemId": red, "quantity": 2}]}}),
        "inventoryTransferSetItems",
    );
    assert_eq!(levels(&shop, &red, &paris), [2, 2, 0, 4]);

    let lines: Vec<String> =
        crate::domains::catalog::gids(shop.get(&transfer).unwrap().get("lineItems"));
    let floral_line = lines
        .iter()
        .find(|line| shop.get(line).unwrap()["inventoryItem"] == json!(floral))
        .unwrap()
        .clone();
    let red_line = lines
        .iter()
        .find(|line| **line != floral_line)
        .unwrap()
        .clone();
    let remove = "mutation($input: InventoryTransferRemoveItemsInput!) { inventoryTransferRemoveItems(input: $input) {
        removedQuantities { inventoryItemId deltaQuantity newQuantity } userErrors { field message code } } }";
    let answer = done(
        &mut shop,
        remove,
        json!({"input": {"id": transfer, "transferLineItemIds": [floral_line]}}),
        "inventoryTransferRemoveItems",
    );
    assert_eq!(
        answer["removedQuantities"][0]["inventoryItemId"],
        json!(floral)
    );
    assert_eq!(
        last_payload(&mut shop, "inventory_transfers/remove_items")["items_removed"],
        json!([{"line_item_id": floral_line}])
    );
    assert_eq!(
        refused(
            &mut shop,
            remove,
            json!({"input": {"id": transfer, "transferLineItemIds": [red_line]}}),
            "inventoryTransferRemoveItems"
        ),
        ["CANT_REMOVE_ALL_ITEMS_FROM_READY_TO_SHIP_TRANSFER"]
    );

    // Duplicated: a draft of the same items, nothing committed.
    let duplicate =
        "mutation($id: ID!) { inventoryTransferDuplicate(id: $id) @idempotent(key: $id) {
        inventoryTransfer { id status totalQuantity } userErrors { field message code } } }";
    let duplicate = duplicate.replace("@idempotent(key: $id)", "@idempotent(key: \"dup\")");
    let copy = done(
        &mut shop,
        &duplicate,
        json!({"id": transfer}),
        "inventoryTransferDuplicate",
    );
    assert_eq!(copy["inventoryTransfer"]["status"], "DRAFT");
    assert_eq!(copy["inventoryTransfer"]["totalQuantity"], 2);
    assert_eq!(levels(&shop, &red, &paris), [2, 2, 0, 4]);
    assert_eq!(
        refused(
            &mut shop,
            &duplicate.replace("\"dup\"", "\"dup-2\""),
            json!({"id": "gid://shopify/InventoryTransfer/999"}),
            "inventoryTransferDuplicate"
        ),
        ["TRANSFER_NOT_FOUND"]
    );
    // The same key with the same arguments answers the first answer again, and makes nothing.
    let count = shop.count("InventoryTransfer");
    let again = done(
        &mut shop,
        &duplicate,
        json!({"id": transfer}),
        "inventoryTransferDuplicate",
    );
    assert_eq!(
        again["inventoryTransfer"]["id"],
        copy["inventoryTransfer"]["id"]
    );
    assert_eq!(shop.count("InventoryTransfer"), count);

    let delete = "mutation($id: ID!) { inventoryTransferDelete(id: $id) { deletedId userErrors { field message code } } }";
    assert_eq!(
        refused(
            &mut shop,
            delete,
            json!({"id": transfer}),
            "inventoryTransferDelete"
        ),
        ["INVALID_TRANSFER_STATUS"]
    );
    let copy_id = copy["inventoryTransfer"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let deleted = done(
        &mut shop,
        delete,
        json!({"id": copy_id}),
        "inventoryTransferDelete",
    );
    assert_eq!(deleted["deletedId"], json!(copy_id));
    assert!(shop.get(&copy_id).is_none());
}

#[test]
fn a_shipment_is_drafted_changed_tracked_shipped_and_deleted() {
    let mut shop = rich();
    let (paris, berlin) = locations(&shop);
    let red = item_of(&shop, "DRESS-S-RED");
    let answer = run(
        &mut shop,
        "mutation($input: InventoryTransferCreateAsReadyToShipInput!) {
          inventoryTransferCreateAsReadyToShip(input: $input) @idempotent(key: \"r\") { inventoryTransfer { id } userErrors { code } } }",
        json!({"input": {"originLocationId": paris, "destinationLocationId": berlin, "lineItems": [{"inventoryItemId": red, "quantity": 3}]}}),
    );
    let transfer =
        answer["data"]["inventoryTransferCreateAsReadyToShip"]["inventoryTransfer"]["id"]
            .as_str()
            .unwrap()
            .to_string();
    let create = "mutation($input: InventoryShipmentCreateInput!) { inventoryShipmentCreate(input: $input) @idempotent(key: $key) {
        inventoryShipment { id status lineItems(first: 5) { nodes { id quantity } } } userErrors { field message code } } }";
    let create = |key: &str| create.replace("$key", &format!("\"{key}\""));
    let shipment = done(
        &mut shop,
        &create("a"),
        json!({"input": {"movementId": transfer, "lineItems": [{"inventoryItemId": red, "quantity": 1}]}}),
        "inventoryShipmentCreate",
    );
    let id = shipment["inventoryShipment"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(shipment["inventoryShipment"]["status"], "DRAFT");
    // A draft moves nothing: the units are picked.
    assert_eq!(levels(&shop, &red, &paris), [1, 3, 0, 4]);
    let line_of_transfer =
        crate::domains::catalog::gids(shop.get(&transfer).unwrap().get("lineItems"))[0].clone();
    assert_eq!(
        shop.get(&line_of_transfer).unwrap()["pickedForShipmentQuantity"],
        1
    );
    assert_eq!(
        last_payload(&mut shop, "inventory_shipments/create")["status"],
        "DRAFT"
    );
    assert_eq!(
        refused(
            &mut shop,
            &create("b"),
            json!({"input": {"movementId": "gid://shopify/InventoryTransfer/9", "lineItems": []}}),
            "inventoryShipmentCreate"
        ),
        ["TRANSFER_NOT_FOUND"]
    );

    let add = "mutation($id: ID!, $lines: [InventoryShipmentLineItemInput!]!) { inventoryShipmentAddItems(id: $id, lineItems: $lines) @idempotent(key: $key) {
        addedItems { id quantity } userErrors { field message code } } }";
    let add = |key: &str| add.replace("$key", &format!("\"{key}\""));
    let added = done(
        &mut shop,
        &add("a"),
        json!({"id": id, "lines": [{"inventoryItemId": red, "quantity": 1}]}),
        "inventoryShipmentAddItems",
    );
    assert_eq!(added["addedItems"][0]["quantity"], 2);
    let line = added["addedItems"][0]["id"].as_str().unwrap().to_string();
    assert_eq!(
        last_payload(&mut shop, "inventory_shipments/add_items")["items_added"],
        json!([{"id": line, "quantity": 1}])
    );
    assert_eq!(
        refused(
            &mut shop,
            &add("b"),
            json!({"id": id, "lines": [{"inventoryItemId": red, "quantity": 5}]}),
            "inventoryShipmentAddItems"
        ),
        ["INVALID_QUANTITY"]
    );

    let update = "mutation($id: ID!, $items: [InventoryShipmentUpdateItemQuantitiesInput!]) { inventoryShipmentUpdateItemQuantities(id: $id, items: $items) {
        updatedLineItems { id quantity } userErrors { field message code } } }";
    let updated = done(
        &mut shop,
        update,
        json!({"id": id, "items": [{"shipmentLineItemId": line, "quantity": 1}]}),
        "inventoryShipmentUpdateItemQuantities",
    );
    assert_eq!(updated["updatedLineItems"][0]["quantity"], 1);
    assert_eq!(
        last_payload(&mut shop, "inventory_shipments/update_item_quantities")["items_updated"],
        json!([{"id": line, "old_quantity": 2, "new_quantity": 1}])
    );
    assert_eq!(
        refused(
            &mut shop,
            update,
            json!({"id": id, "items": [{"shipmentLineItemId": line, "quantity": 0}]}),
            "inventoryShipmentUpdateItemQuantities"
        ),
        ["INVALID_QUANTITY"]
    );

    let barcode = "mutation($id: ID!, $barcode: String!) { inventoryShipmentSetBarcode(id: $id, barcode: $barcode) { inventoryShipment { barcode } userErrors { field message code } } }";
    assert_eq!(
        done(
            &mut shop,
            barcode,
            json!({"id": id, "barcode": "BOX-1"}),
            "inventoryShipmentSetBarcode"
        )["inventoryShipment"]["barcode"],
        "BOX-1"
    );
    let other = done(
        &mut shop,
        &create("c"),
        json!({"input": {"movementId": transfer, "lineItems": [{"inventoryItemId": red, "quantity": 1}]}}),
        "inventoryShipmentCreate",
    )["inventoryShipment"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        refused(
            &mut shop,
            barcode,
            json!({"id": other, "barcode": "BOX-1"}),
            "inventoryShipmentSetBarcode"
        ),
        ["BARCODE_DUPLICATE"]
    );

    let tracking = "mutation($id: ID!, $tracking: InventoryShipmentTrackingInput!) { inventoryShipmentSetTracking(id: $id, tracking: $tracking) {
        inventoryShipment { tracking { company trackingNumber trackingUrl arrivesAt } } userErrors { field message code } } }";
    done(
        &mut shop,
        tracking,
        json!({"id": id, "tracking": {"company": "DHL", "trackingNumber": "42", "trackingUrl": "https://example.com/42", "arrivesAt": "2026-04-02T09:00:00Z"}}),
        "inventoryShipmentSetTracking",
    );
    let payload = last_payload(&mut shop, "inventory_shipments/update_tracking");
    assert_eq!(payload["shipment_id"], json!(id));
    assert_eq!(payload["updated_tracking"]["tracking_number"], "42");
    assert_eq!(
        refused(
            &mut shop,
            tracking,
            json!({"id": id, "tracking": {"trackingUrl": "ftp://example.com/42"}}),
            "inventoryShipmentSetTracking"
        ),
        ["INVALID_URL"]
    );

    // The other draft: its line removed, then itself deleted.
    let other_line =
        crate::domains::catalog::gids(shop.get(&other).unwrap().get("lineItems"))[0].clone();
    let remove = "mutation($id: ID!, $lines: [ID!]!) { inventoryShipmentRemoveItems(id: $id, lineItems: $lines) { inventoryShipment { lineItemTotalQuantity } userErrors { field message code } } }";
    assert_eq!(
        refused(
            &mut shop,
            remove,
            json!({"id": other, "lines": ["gid://shopify/InventoryShipmentLineItem/9"]}),
            "inventoryShipmentRemoveItems"
        ),
        ["ITEM_NOT_FOUND"]
    );
    let removed = done(
        &mut shop,
        remove,
        json!({"id": other, "lines": [other_line]}),
        "inventoryShipmentRemoveItems",
    );
    assert_eq!(removed["inventoryShipment"]["lineItemTotalQuantity"], 0);
    let delete = "mutation($id: ID!) { inventoryShipmentDelete(id: $id) { id userErrors { field message code } } }";
    assert_eq!(
        done(
            &mut shop,
            delete,
            json!({"id": other}),
            "inventoryShipmentDelete"
        )["id"],
        json!(other)
    );
    assert_eq!(
        last_payload(&mut shop, "inventory_shipments/delete")["id"],
        json!(other)
    );

    // In transit: one unit out of Paris, incoming in Berlin; then it cannot be shipped again or
    // deleted, and reducing it to nothing is refused.
    let mark = "mutation($id: ID!) { inventoryShipmentMarkInTransit(id: $id, dateShipped: \"2026-03-20T08:00:00Z\") { inventoryShipment { status dateShipped } userErrors { field message code } } }";
    let marked = done(
        &mut shop,
        mark,
        json!({"id": id}),
        "inventoryShipmentMarkInTransit",
    );
    assert_eq!(
        marked["inventoryShipment"]["dateShipped"],
        "2026-03-20T08:00:00Z"
    );
    assert_eq!(levels(&shop, &red, &paris), [1, 2, 0, 3]);
    assert_eq!(levels(&shop, &red, &berlin), [3, 0, 1, 3]);
    assert_eq!(
        refused(
            &mut shop,
            mark,
            json!({"id": id}),
            "inventoryShipmentMarkInTransit"
        ),
        ["INVALID_SHIPMENT_STATUS"]
    );
    assert_eq!(
        refused(
            &mut shop,
            delete,
            json!({"id": id}),
            "inventoryShipmentDelete"
        ),
        ["INVALID_SHIPMENT_STATUS"]
    );
    // Two units more on the shipment in transit; then back to one: the unit comes back,
    // committed again for the transfer.
    done(
        &mut shop,
        &add("c"),
        json!({"id": id, "lines": [{"inventoryItemId": red, "quantity": 2}]}),
        "inventoryShipmentAddItems",
    );
    assert_eq!(levels(&shop, &red, &paris), [1, 0, 0, 1]);
    assert_eq!(levels(&shop, &red, &berlin), [3, 0, 3, 3]);
    done(
        &mut shop,
        update,
        json!({"id": id, "items": [{"shipmentLineItemId": line, "quantity": 2}]}),
        "inventoryShipmentUpdateItemQuantities",
    );
    assert_eq!(levels(&shop, &red, &paris), [1, 1, 0, 2]);
    assert_eq!(levels(&shop, &red, &berlin), [3, 0, 2, 3]);

    let receive = "mutation($id: ID!, $line: ID!, $quantity: Int!) { inventoryShipmentReceive(id: $id, lineItems: [{shipmentLineItemId: $line, quantity: $quantity, reason: ACCEPTED}]) @idempotent(key: \"x\") {
        inventoryShipment { status } userErrors { field message code } } }";
    assert_eq!(
        refused(
            &mut shop,
            receive,
            json!({"id": id, "line": line, "quantity": 3}),
            "inventoryShipmentReceive"
        ),
        ["INVALID_QUANTITY"]
    );
}

#[test]
fn the_other_mutations_refuse_what_they_cannot_do_and_change_nothing() {
    let mut shop = rich();
    let (_, fo) = order_with_stock(&mut shop);
    let (_, other) = order_with_stock(&mut shop);
    assert_eq!(
        refused(
            &mut shop,
            "mutation($inputs: [FulfillmentOrderMergeInput!]!) { fulfillmentOrderMerge(fulfillmentOrderMergeInputs: $inputs) { fulfillmentOrderMerges { fulfillmentOrder { id } } userErrors { field message code } } }",
            json!({"inputs": [{"mergeIntents": [{"fulfillmentOrderId": fo}, {"fulfillmentOrderId": other}]}]}),
            "fulfillmentOrderMerge",
        ),
        [
            "The fulfillment orders to merge must be at least two, of the same order and location, with the same delivery method."
        ]
    );
    assert_eq!(
        refused(
            &mut shop,
            "mutation { carrierServiceDelete(id: \"gid://shopify/DeliveryCarrierService/9\") { deletedId userErrors { field message code } } }",
            json!({}),
            "carrierServiceDelete",
        ),
        ["CARRIER_SERVICE_DELETE_FAILED"]
    );
    assert_eq!(
        refused(
            &mut shop,
            "mutation { carrierServiceUpdate(input: {id: \"gid://shopify/DeliveryCarrierService/9\", name: \"x\"}) { carrierService { id } userErrors { field message code } } }",
            json!({}),
            "carrierServiceUpdate",
        ),
        ["CARRIER_SERVICE_UPDATE_FAILED"]
    );
    for mutation in [
        "fulfillmentConstraintRuleDelete(id: \"gid://shopify/FulfillmentConstraintRule/9\") { success",
        "fulfillmentConstraintRuleUpdate(id: \"gid://shopify/FulfillmentConstraintRule/9\", deliveryMethodTypes: [SHIPPING]) { fulfillmentConstraintRule { id }",
    ] {
        let name = &mutation[..mutation.find('(').unwrap()];
        assert_eq!(
            refused(
                &mut shop,
                &format!("mutation {{ {mutation} userErrors {{ field message code }} }} }}"),
                json!({}),
                name
            ),
            ["NOT_FOUND"]
        );
    }
    let general = super::profiles::general_profile_gid();
    assert_eq!(
        refused(
            &mut shop,
            "mutation($id: ID!) { deliveryProfileUpdate(id: $id, profile: {locationGroupsToCreate: [{locationsToAdd: [\"gid://shopify/Location/9\"]}]}) { profile { id } userErrors { field message } } }",
            json!({"id": general}),
            "deliveryProfileUpdate",
        ),
        ["Location not found"]
    );
    assert_eq!(
        refused(
            &mut shop,
            "mutation { deliveryPromiseParticipantsUpdate(brandedPromiseHandle: \"express\", ownersToAdd: [\"gid://shopify/Product/1\"]) { promiseParticipants { id } userErrors { field message } } }",
            json!({}),
            "deliveryPromiseParticipantsUpdate",
        ),
        ["Owner gid://shopify/Product/1 is not a product variant of the shop"]
    );
    assert_eq!(
        refused(
            &mut shop,
            "mutation { shippingPackageUpdate(id: \"gid://shopify/ShippingPackage/1\", shippingPackage: {name: \" \"}) { userErrors { field message } } }",
            json!({}),
            "shippingPackageUpdate",
        ),
        ["Name can't be blank"]
    );
    assert_eq!(
        refused(
            &mut shop,
            "mutation { shippingPackageDelete(id: \"gid://shopify/ShippingPackage/2\") { deletedId userErrors { field message } } }",
            json!({}),
            "shippingPackageDelete",
        ),
        ["Shipping package not found."]
    );
    assert_eq!(
        refused(
            &mut shop,
            "mutation { deliveryProfileRemove(id: \"gid://shopify/DeliveryProfile/9\") { job { id } userErrors { field message } } }",
            json!({}),
            "deliveryProfileRemove",
        ),
        ["Profile could not be found"]
    );
}
