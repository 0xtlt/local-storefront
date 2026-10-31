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
