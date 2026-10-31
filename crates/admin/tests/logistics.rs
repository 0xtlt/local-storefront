//! The logistics family from the outside: the records its mutations make checked against the
//! schema, the general delivery profile's rates projected into the checkout's shipping rates,
//! and `GET /__lsf/admin` listing its mutations and the shop's outbox.

mod support;

use std::time::Duration;

use lsf_admin::api::{AdminApi, AdminOptions, HttpRequest, ThrottleSetting};
use lsf_admin::domains::logistics;
use lsf_admin::graphql::{self, Request};
use lsf_admin::resolve::{Registry, ShopResolver};
use lsf_admin::state::ShopState;
use lsf_admin::sync;
use lsf_admin::webhooks::DeliveryOptions;
use lsf_core::store::load::{DataSource, LoadOptions, load_with_overlays};
use serde_json::{Value, json};
use support::shape::Schema;

fn run(shop: &mut ShopState, query: &str, variables: Value) -> Value {
    let mut resolver = ShopResolver::new(shop, Registry::standard());
    graphql::run(
        lsf_admin::schema::admin(),
        &mut resolver,
        &Request::new(query).with_variables(variables),
        None,
    )
    .to_json()
}

/// The data of an answer whose mutation `name` has no errors.
fn ok(answer: &Value, name: &str) -> Value {
    assert!(answer.get("errors").is_none(), "{name}: {answer}");
    let payload = &answer["data"][name];
    assert_eq!(payload["userErrors"], json!([]), "{name}: {answer}");
    payload.clone()
}

fn id(value: &Value) -> String {
    value
        .as_str()
        .unwrap_or_else(|| panic!("no id: {value}"))
        .to_string()
}

fn sku(shop: &ShopState, sku: &str) -> (String, String) {
    let variant = shop
        .keys("ProductVariant")
        .find(|key| shop.get(key).unwrap()["sku"] == json!(sku))
        .unwrap()
        .to_string();
    let item = id(&shop.get(&variant).unwrap()["inventoryItem"]);
    (variant, item)
}

fn location(shop: &ShopState, name: &str) -> String {
    shop.keys("Location")
        .find(|key| shop.get(key).unwrap()["name"] == json!(name))
        .unwrap()
        .to_string()
}

/// Every mutation family of the module, run over the rich fixture.
fn exercise(shop: &mut ShopState) {
    let (paris, berlin) = (location(shop, "Paris"), location(shop, "Berlin warehouse"));
    let (variant, item) = sku(shop, "DRESS-S-RED");
    let (floral, _) = sku(shop, "DRESS-M-FLO");

    let created = ok(
        &run(
            shop,
            "mutation($input: InventoryTransferCreateAsReadyToShipInput!) { inventoryTransferCreateAsReadyToShip(input: $input) @idempotent(key: \"t\") { inventoryTransfer { id } userErrors { message } } }",
            json!({"input": {"originLocationId": paris, "destinationLocationId": berlin, "tags": ["a"],
                "lineItems": [{"inventoryItemId": item, "quantity": 2}]}}),
        ),
        "inventoryTransferCreateAsReadyToShip",
    );
    let transfer = id(&created["inventoryTransfer"]["id"]);
    let shipment = ok(
        &run(
            shop,
            "mutation($input: InventoryShipmentCreateInput!) { inventoryShipmentCreateInTransit(input: $input) @idempotent(key: \"s\") { inventoryShipment { id lineItems(first: 1) { nodes { id } } } userErrors { message } } }",
            json!({"input": {"movementId": transfer, "barcode": "B-1", "lineItems": [{"inventoryItemId": item, "quantity": 2}],
                "trackingInput": {"company": "UPS", "trackingNumber": "1Z", "trackingUrl": "https://example.com/1Z", "arrivesAt": "2026-04-01T10:00:00Z"}}}),
        ),
        "inventoryShipmentCreateInTransit",
    );
    let shipment_id = id(&shipment["inventoryShipment"]["id"]);
    let line = id(&shipment["inventoryShipment"]["lineItems"]["nodes"][0]["id"]);
    ok(
        &run(
            shop,
            "mutation($id: ID!, $line: ID!) { inventoryShipmentReceive(id: $id, lineItems: [{shipmentLineItemId: $line, quantity: 1, reason: ACCEPTED}]) @idempotent(key: \"r\") { inventoryShipment { id } userErrors { message } } }",
            json!({"id": shipment_id, "line": line}),
        ),
        "inventoryShipmentReceive",
    );

    let order = ok(
        &run(
            shop,
            "mutation($order: OrderCreateOrderInput!) { orderCreate(order: $order, options: {inventoryBehaviour: DECREMENT_IGNORING_POLICY}) { order { fulfillmentOrders(first: 1) { nodes { id lineItems(first: 5) { nodes { id } } } } } userErrors { message } } }",
            json!({"order": {"lineItems": [{"variantId": variant, "quantity": 2, "requiresShipping": true}, {"variantId": floral, "quantity": 1, "requiresShipping": true}],
                "shippingAddress": {"address1": "1 Main St", "city": "Lyon", "countryCode": "FR", "zip": "69001"},
                "shippingLines": [{"title": "Standard", "priceSet": {"shopMoney": {"amount": "5.00", "currencyCode": "EUR"}}}]}}),
        ),
        "orderCreate",
    );
    let fo = &order["order"]["fulfillmentOrders"]["nodes"][0];
    let fo_id = id(&fo["id"]);
    ok(
        &run(
            shop,
            "mutation($splits: [FulfillmentOrderSplitInput!]!) { fulfillmentOrderSplit(fulfillmentOrderSplits: $splits) { fulfillmentOrderSplits { remainingFulfillmentOrder { id } } userErrors { message } } }",
            json!({"splits": [{"fulfillmentOrderId": fo_id, "fulfillmentOrderLineItems": [{"id": fo["lineItems"]["nodes"][0]["id"], "quantity": 1}]}]}),
        ),
        "fulfillmentOrderSplit",
    );
    ok(
        &run(
            shop,
            "mutation($id: ID!) { fulfillmentOrderReportProgress(id: $id, progressReport: {reasonNotes: \"Picking\"}) { fulfillmentOrder { id } userErrors { message } } }",
            json!({"id": fo_id}),
        ),
        "fulfillmentOrderReportProgress",
    );
    ok(
        &run(
            shop,
            "mutation($id: ID!, $at: DateTime!) { shippingLabelPurchase(shippingLabelPurchase: {fulfillmentOrderId: $id, shippingDatetime: $at}) { shippingLabelPurchaseResult { id } userErrors { message } } }",
            json!({"id": fo_id, "at": shop.timestamp()}),
        ),
        "shippingLabelPurchase",
    );

    let carrier = ok(
        &run(
            shop,
            "mutation { carrierServiceCreate(input: {name: \"Live\", callbackUrl: \"https://example.com/rates\", active: true, supportsServiceDiscovery: true}) { carrierService { id } userErrors { message } } }",
            json!({}),
        ),
        "carrierServiceCreate",
    );
    ok(
        &run(
            shop,
            "mutation($profile: DeliveryProfileInput!) { deliveryProfileCreate(profile: $profile) { profile { id } userErrors { message } } }",
            json!({"profile": {"name": "Heavy", "variantsToAssociate": [variant], "locationGroupsToCreate": [{"locationsToAdd": [paris], "zonesToCreate": [
                {"name": "France", "countries": [{"code": "FR", "provinces": [{"code": "75"}]}], "methodDefinitionsToCreate": [
                    {"name": "Flat", "rateDefinition": {"price": {"amount": "7.5", "currencyCode": "EUR"}},
                     "priceConditionsToCreate": [{"operator": "GREATER_THAN_OR_EQUAL_TO", "criteria": {"amount": "10", "currencyCode": "EUR"}}],
                     "weightConditionsToCreate": [{"operator": "LESS_THAN_OR_EQUAL_TO", "criteria": {"value": 5, "unit": "KILOGRAMS"}}]},
                    {"name": "Live", "participant": {"carrierServiceId": carrier["carrierService"]["id"], "fixedFee": {"amount": "1", "currencyCode": "EUR"},
                     "participantServices": [{"name": "Ground", "active": true}]}}]}]}]}}),
        ),
        "deliveryProfileCreate",
    );
    let service = ok(
        &run(
            shop,
            "mutation { fulfillmentServiceCreate(name: \"Partner\", callbackUrl: \"https://example.com/f\") { fulfillmentService { location { id } } userErrors { message } } }",
            json!({}),
        ),
        "fulfillmentServiceCreate",
    );
    ok(
        &run(
            shop,
            "mutation($id: ID!) { deliveryPromiseProviderUpsert(locationId: $id, timeZone: \"Europe/Paris\") { deliveryPromiseProvider { id } userErrors { message } } }",
            json!({"id": service["fulfillmentService"]["location"]["id"]}),
        ),
        "deliveryPromiseProviderUpsert",
    );
    ok(
        &run(
            shop,
            "mutation($id: ID!) { deliveryPromiseParticipantsUpdate(brandedPromiseHandle: \"express\", ownersToAdd: [$id]) { promiseParticipants { id } userErrors { message } } }",
            json!({"id": variant}),
        ),
        "deliveryPromiseParticipantsUpdate",
    );
}

#[test]
fn the_records_the_logistics_mutations_make_are_in_the_shape_of_their_types() {
    let store = support::fixture_store("rich");
    let mut shop = ShopState::seeded(&store, None);
    exercise(&mut shop);
    let Some(schema) = Schema::load() else {
        return;
    };
    let problems = schema.check(&shop);
    assert!(
        problems.is_empty(),
        "{} problems:\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn the_general_profiles_rates_are_the_checkouts_shipping_rates() {
    let store = support::fixture_store("rich");
    let mut shop = ShopState::seeded(&store, None);
    // Another profile's rates, a transfer, a carrier service: nothing for the checkout.
    exercise(&mut shop);
    let overlay = sync::overlay(&shop, &store).expect("the stock moved");
    assert!(overlay.data.get("checkout").is_none(), "{overlay:#?}");
    // The storefront's stock is what the transfer, the shipment and the order left available.
    let (moved, diagnostics) = load_with_overlays(
        &DataSource::Directory(support::fixture("rich")),
        &LoadOptions::default(),
        std::slice::from_ref(&overlay),
    );
    assert!(!diagnostics.has_errors(), "{diagnostics}");
    let (_, item) = sku(&shop, "DRESS-S-RED");
    let available: i64 =
        lsf_admin::domains::catalog::gids(shop.get(&item).unwrap().get("inventoryLevels"))
            .iter()
            .map(|level| lsf_admin::domains::catalog::available_of(shop.get(level).unwrap()))
            .sum();
    let variant = moved
        .products
        .iter()
        .flat_map(|product| &product.variants)
        .find(|variant| variant.sku == "DRESS-S-RED")
        .unwrap();
    assert_eq!(variant.inventory_quantity, available);
    assert_ne!(available, 7, "the stock did not move");

    let general = logistics::profiles::general_profile_gid();
    let profile = shop.get(&general).unwrap().clone();
    let group = profile["profileLocationGroups"][0]["locationGroup"].clone();
    let zone = profile["profileLocationGroups"][0]["locationGroupZones"][0]["zone"].clone();
    let method = profile["profileLocationGroups"][0]["locationGroupZones"][0]["methodDefinitions"]
        [0]
    .clone();
    let update = "mutation($id: ID!, $profile: DeliveryProfileInput!) { deliveryProfileUpdate(id: $id, profile: $profile) { profile { id } userErrors { field message } } }";
    ok(
        &run(
            &mut shop,
            update,
            json!({"id": general, "profile": {"locationGroupsToUpdate": [{"id": group, "zonesToUpdate": [{"id": zone,
                "methodDefinitionsToUpdate": [{"id": method, "rateDefinition": {"price": {"amount": "6.90", "currencyCode": "EUR"}}}],
                "methodDefinitionsToCreate": [{"name": "Express delivery", "description": "Next day", "rateDefinition": {"price": {"amount": "12", "currencyCode": "EUR"}}},
                    {"name": "Heavy", "rateDefinition": {"price": {"amount": "20", "currencyCode": "EUR"}},
                     "weightConditionsToCreate": [{"operator": "GREATER_THAN_OR_EQUAL_TO", "criteria": {"value": 10, "unit": "KILOGRAMS"}}]}]}]}]}}),
        ),
        "deliveryProfileUpdate",
    );
    let overlay = sync::overlay(&shop, &store).expect("the rates changed");
    let (synced, diagnostics) = load_with_overlays(
        &DataSource::Directory(support::fixture("rich")),
        &LoadOptions::default(),
        std::slice::from_ref(&overlay),
    );
    assert!(!diagnostics.has_errors(), "{diagnostics}");
    let rates: Vec<(String, String, i64)> = synced
        .checkout
        .shipping_rates
        .iter()
        .map(|rate| (rate.handle.clone(), rate.title.clone(), rate.price))
        .collect();
    // The conditional rate has no place in the store data.
    assert_eq!(
        rates,
        [
            ("colissimo".to_string(), "Colissimo".to_string(), 690),
            (
                "express-delivery".to_string(),
                "Express delivery".to_string(),
                1200
            ),
        ]
    );
    assert_eq!(
        synced.checkout.shipping_rates[1].description.as_deref(),
        Some("Next day")
    );

    // Deactivated, the data's rate leaves the checkout.
    ok(
        &run(
            &mut shop,
            update,
            json!({"id": general, "profile": {"locationGroupsToUpdate": [{"id": group, "zonesToUpdate": [{"id": zone,
                "methodDefinitionsToUpdate": [{"id": method, "active": false}]}]}]}}),
        ),
        "deliveryProfileUpdate",
    );
    let overlay = sync::overlay(&shop, &store).expect("the rates changed");
    let (synced, _) = load_with_overlays(
        &DataSource::Directory(support::fixture("rich")),
        &LoadOptions::default(),
        std::slice::from_ref(&overlay),
    );
    let handles: Vec<&str> = synced
        .checkout
        .shipping_rates
        .iter()
        .map(|rate| rate.handle.as_str())
        .collect();
    assert_eq!(handles, ["express-delivery"]);
}

#[test]
fn the_status_lists_the_logistics_mutations_and_the_outbox() {
    let api = AdminApi::new(
        &support::demo_store(),
        AdminOptions {
            app: None,
            client_secret: "test-secret".to_string(),
            app_url: None,
            throttle: ThrottleSetting::Store,
            delivery: DeliveryOptions {
                timeout: Duration::from_secs(3),
                retry_delays: Vec::new(),
                ..DeliveryOptions::default()
            },
            token_seed: Some(7),
        },
    );
    let response = api
        .handle(&HttpRequest {
            method: "GET".to_string(),
            path: "/__lsf/admin".to_string(),
            query: Default::default(),
            headers: Default::default(),
            body: Vec::new(),
        })
        .expect("routed");
    let status: Value = serde_json::from_slice(&response.body).unwrap();
    let listed: Vec<&str> = status["mutations"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for mutation in [
        "carrierServiceCreate",
        "carrierServiceDelete",
        "carrierServiceUpdate",
        "deliveryProfileCreate",
        "deliveryProfileRemove",
        "deliveryProfileUpdate",
        "deliveryPromiseParticipantsUpdate",
        "deliveryPromiseProviderUpsert",
        "deliverySettingUpdate",
        "deliveryShippingOriginAssign",
        "fulfillmentConstraintRuleCreate",
        "fulfillmentConstraintRuleDelete",
        "fulfillmentConstraintRuleUpdate",
        "fulfillmentOrderLineItemsPreparedForPickup",
        "fulfillmentOrderMerge",
        "fulfillmentOrderReportProgress",
        "fulfillmentOrderSplit",
        "fulfillmentOrdersReroute",
        "fulfillmentOrdersSetFulfillmentDeadline",
        "inventoryShipmentAddItems",
        "inventoryShipmentCreate",
        "inventoryShipmentCreateInTransit",
        "inventoryShipmentDelete",
        "inventoryShipmentMarkInTransit",
        "inventoryShipmentReceive",
        "inventoryShipmentRemoveItems",
        "inventoryShipmentSetBarcode",
        "inventoryShipmentSetTracking",
        "inventoryShipmentUpdateItemQuantities",
        "inventoryTransferCancel",
        "inventoryTransferCreate",
        "inventoryTransferCreateAsReadyToShip",
        "inventoryTransferDelete",
        "inventoryTransferDuplicate",
        "inventoryTransferEdit",
        "inventoryTransferMarkAsReadyToShip",
        "inventoryTransferRemoveItems",
        "inventoryTransferSetItems",
        "shippingLabelPurchase",
        "shippingPackageDelete",
        "shippingPackageMakeDefault",
        "shippingPackageUpdate",
    ] {
        assert!(listed.contains(&mutation), "{mutation} is not listed");
    }
    let shop = &status["shops"][0];
    assert_eq!(shop["outbox"], json!([]));
    assert_eq!(shop["shippingPackages"][0]["name"], "Sample box");
}
