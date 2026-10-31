//! Point of sale over the demo store: drawers, devices, reason codes, payment sessions, and the
//! totals and summaries they derive. Each mutation has a success and a refused input that changes
//! nothing.

use serde_json::json;

use crate::domains::catalog::mutation_tests::{demo, ok, run, run_with, topics, unchanged};
use crate::state::ShopState;

/// The first location and its device.
fn location_and_device(shop: &ShopState) -> (String, String) {
    let location = shop.keys("Location").next().unwrap().to_string();
    (location.clone(), super::device_gid(&location))
}

#[test]
fn the_seed_gives_each_location_a_device_and_the_shop_its_reason_codes_and_owner() {
    let mut shop = demo();
    let (_, device) = location_and_device(&shop);
    let read = run_with(
        &mut shop,
        r#"query($device: ID!, $owner: ID!) { pointOfSaleDevice(id: $device) { id cashDrawer { id } activePaymentSession { id } }
            staffMember(id: $owner) { name isShopOwner }
            cashManagementReasonCodes(first: 20) { nodes { __typename ... on CashManagementDefaultReasonCode { code } } } }"#,
        json!({"device": device, "owner": crate::domains::staff::owner_gid()}),
    );
    assert!(read.get("errors").is_none(), "{read}");
    assert_eq!(
        read["data"]["pointOfSaleDevice"],
        json!({"id": device, "cashDrawer": null, "activePaymentSession": null})
    );
    assert_eq!(read["data"]["staffMember"]["isShopOwner"], json!(true));
    let codes = read["data"]["cashManagementReasonCodes"]["nodes"]
        .as_array()
        .unwrap();
    assert_eq!(codes.len(), 15);
    assert!(
        codes.contains(
            &json!({"__typename": "CashManagementDefaultReasonCode", "code": "CASH_COUNT"})
        )
    );
    assert_eq!(shop.count("PointOfSaleDevice"), 2);
}

#[test]
fn drawers_are_created_found_renamed_and_assigned() {
    let mut shop = demo();
    let (location, device) = location_and_device(&shop);
    let created = run_with(
        &mut shop,
        r#"mutation($location: ID!) { cashDrawerCreate(locationId: $location, name: "Front") {
            cashDrawer { id name balance { amount currencyCode } location { id } } userErrors { field message code } } }"#,
        json!({"location": location}),
    );
    let drawer = &ok(&created, "cashDrawerCreate")["cashDrawer"];
    assert_eq!(
        drawer["balance"],
        json!({"amount": "0.0", "currencyCode": "USD"})
    );
    let drawer = drawer["id"].as_str().unwrap().to_string();
    // Found by its name, and the device assigned to it.
    let found = run_with(
        &mut shop,
        r#"mutation($location: ID!, $device: ID!) { cashDrawerFindOrCreate(locationId: $location, name: "front", pointOfSaleDeviceId: $device) {
            cashDrawer { id pointOfSaleDevices(first: 5) { nodes { id } } } userErrors { field message code } } }"#,
        json!({"location": location, "device": device}),
    );
    let payload = ok(&found, "cashDrawerFindOrCreate");
    assert_eq!(payload["cashDrawer"]["id"], json!(drawer));
    assert_eq!(
        payload["cashDrawer"]["pointOfSaleDevices"]["nodes"],
        json!([{"id": device}])
    );
    assert!(
        found["extensions"]["lsf"]["notes"][0]
            .as_str()
            .unwrap()
            .contains("outbox")
    );
    let told = crate::outbox::entries(&shop, Some(crate::outbox::Channel::Pos));
    assert_eq!(told.len(), 1);
    assert_eq!(told[0]["what"], "cash_drawer_assigned");
    assert_eq!(told[0]["to"], json!(device));
    assert_eq!(told[0]["about"], json!(drawer));
    let renamed = run_with(
        &mut shop,
        r#"mutation($id: ID!) { cashDrawerUpdate(id: $id, input: {name: "Counter"}) { cashDrawer { name } userErrors { message } } }"#,
        json!({"id": drawer}),
    );
    assert_eq!(
        ok(&renamed, "cashDrawerUpdate")["cashDrawer"]["name"],
        json!("Counter")
    );
    let listed = run(&mut shop, r#"{ cashDrawers(first: 5) { nodes { name } } }"#);
    assert_eq!(
        listed["data"]["cashDrawers"]["nodes"],
        json!([{"name": "Counter"}])
    );

    // A device of another location is refused, as a name taken; nothing changes.
    let other_location = shop.keys("Location").nth(1).unwrap().to_string();
    let other_device = super::device_gid(&other_location);
    shop.take_events();
    let before = shop.clone();
    let mismatch = run_with(
        &mut shop,
        r#"mutation($drawer: ID!, $device: ID!) { pointOfSaleDeviceAssignToCashDrawer(cashDrawerId: $drawer, pointOfSaleDeviceId: $device) {
            pointOfSaleDevice { id } userErrors { field message code } } }"#,
        json!({"drawer": drawer, "device": other_device}),
    );
    assert_eq!(
        mismatch["data"]["pointOfSaleDeviceAssignToCashDrawer"]["userErrors"],
        json!([{"field": ["cashDrawerId"], "message": "The cash drawer and point of sale device must be in the same location.", "code": "LOCATION_MISMATCH"}])
    );
    let taken = run_with(
        &mut shop,
        r#"mutation($location: ID!) { cashDrawerCreate(locationId: $location, name: "counter") { cashDrawer { id } userErrors { field code } } }"#,
        json!({"location": location}),
    );
    assert_eq!(
        taken["data"]["cashDrawerCreate"]["userErrors"],
        json!([{"field": ["name"], "code": "CASH_DRAWER_ALREADY_EXISTS"}])
    );
    let missing = run(
        &mut shop,
        r#"mutation { cashDrawerUpdate(id: "gid://shopify/CashDrawer/999999", input: {name: "x"}) { userErrors { field code } } }"#,
    );
    assert_eq!(
        missing["data"]["cashDrawerUpdate"]["userErrors"],
        json!([{"field": ["id"], "code": "CASH_DRAWER_NOT_FOUND"}])
    );
    unchanged(&mut shop, &before, &[&drawer, &device, &other_device]);

    // Assigned to another drawer of its location: the prior assignment goes.
    let second = run_with(
        &mut shop,
        r#"mutation($location: ID!) { cashDrawerCreate(locationId: $location, name: "Back") { cashDrawer { id } } }"#,
        json!({"location": location}),
    )["data"]["cashDrawerCreate"]["cashDrawer"]["id"]
        .clone();
    let assigned = run_with(
        &mut shop,
        r#"mutation($drawer: ID!, $device: ID!) { pointOfSaleDeviceAssignToCashDrawer(cashDrawerId: $drawer, pointOfSaleDeviceId: $device) {
            pointOfSaleDevice { cashDrawer { name } } userErrors { message } } }"#,
        json!({"drawer": second, "device": device}),
    );
    assert_eq!(
        ok(&assigned, "pointOfSaleDeviceAssignToCashDrawer")["pointOfSaleDevice"]["cashDrawer"]["name"],
        json!("Back")
    );
}

#[test]
fn reason_codes_are_created_and_only_custom_ones_deleted() {
    let mut shop = demo();
    let created = run(
        &mut shop,
        r#"mutation { cashManagementReasonCodeCreate(code: "Float top-up") { reasonCode { id code } userErrors { field message code } } }"#,
    );
    let code = ok(&created, "cashManagementReasonCodeCreate")["reasonCode"]["id"].clone();
    shop.take_events();
    let before = shop.clone();
    let taken = run(
        &mut shop,
        r#"mutation { cashManagementReasonCodeCreate(code: "petty_cash") { reasonCode { id } userErrors { field message code } } }"#,
    );
    assert_eq!(
        taken["data"]["cashManagementReasonCodeCreate"]["userErrors"],
        json!([{"field": ["code"], "message": "The reason code already exists.", "code": "CODE_ALREADY_EXISTS"}])
    );
    let system = shop
        .keys("CashManagementSystemReasonCode")
        .next()
        .unwrap()
        .to_string();
    let not_deletable = run_with(
        &mut shop,
        r#"mutation($id: ID!) { cashManagementReasonCodeDelete(id: $id) { deletedId userErrors { field code } } }"#,
        json!({"id": system}),
    );
    assert_eq!(
        not_deletable["data"]["cashManagementReasonCodeDelete"]["userErrors"],
        json!([{"field": ["id"], "code": "NOT_DELETABLE"}])
    );
    unchanged(&mut shop, &before, &[&system]);
    let deleted = run_with(
        &mut shop,
        r#"mutation($id: ID!) { cashManagementReasonCodeDelete(id: $id) { deletedId userErrors { message } } }"#,
        json!({"id": code}),
    );
    assert_eq!(
        ok(&deleted, "cashManagementReasonCodeDelete")["deletedId"],
        code
    );
}

#[test]
fn a_payment_session_counts_adjusts_and_closes_its_drawer() {
    let mut shop = demo();
    let (location, device) = location_and_device(&shop);
    let owner = crate::domains::staff::owner_gid();
    // Without a drawer, a session cannot open; nothing changes.
    let before = shop.clone();
    let refused = run_with(
        &mut shop,
        r#"mutation($device: ID!, $owner: ID!) { pointOfSaleDevicePaymentSessionOpen(pointOfSaleDeviceId: $device, staffMemberId: $owner) {
            pointOfSaleDevicePaymentSession { id } userErrors { field message code } } }"#,
        json!({"device": device, "owner": owner}),
    );
    assert_eq!(
        refused["data"]["pointOfSaleDevicePaymentSessionOpen"]["userErrors"],
        json!([{"field": ["pointOfSaleDeviceId"], "message": "Point of sale device has no cash drawer associated with it.", "code": "NO_CASH_DRAWER_ASSOCIATED"}])
    );
    unchanged(&mut shop, &before, &[&device]);

    let drawer = run_with(
        &mut shop,
        r#"mutation($location: ID!, $device: ID!) { cashDrawerFindOrCreate(locationId: $location, name: "Front", pointOfSaleDeviceId: $device) { cashDrawer { id } } }"#,
        json!({"location": location, "device": device}),
    )["data"]["cashDrawerFindOrCreate"]["cashDrawer"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let opened = run_with(
        &mut shop,
        r#"mutation($device: ID!, $owner: ID!) { pointOfSaleDevicePaymentSessionOpen(pointOfSaleDeviceId: $device, staffMemberId: $owner,
            balance: {amount: "100.00", currencyCode: USD}, note: "Float", time: "2026-10-31T09:00:00Z") {
            pointOfSaleDevicePaymentSession { id status cashCountedAtOpen { amount } expectedCashAtOpen { amount } openingStaffMember { name }
                pointOfSaleDevice { activePaymentSession { id } } cashDrawer { balance { amount } } } userErrors { field message code } } }"#,
        json!({"device": device, "owner": owner}),
    );
    let session =
        &ok(&opened, "pointOfSaleDevicePaymentSessionOpen")["pointOfSaleDevicePaymentSession"];
    assert_eq!(session["status"], json!("OPEN"));
    assert_eq!(session["cashCountedAtOpen"]["amount"], json!("100.0"));
    assert_eq!(session["expectedCashAtOpen"]["amount"], json!("0.0"));
    assert_eq!(session["cashDrawer"]["balance"]["amount"], json!("100.0"));
    assert_eq!(
        session["pointOfSaleDevice"]["activePaymentSession"]["id"],
        session["id"]
    );
    let id = session["id"].as_str().unwrap().to_string();

    // Opened twice: refused.
    let twice = run_with(
        &mut shop,
        r#"mutation($device: ID!, $owner: ID!) { pointOfSaleDevicePaymentSessionOpen(pointOfSaleDeviceId: $device, staffMemberId: $owner) { userErrors { code } } }"#,
        json!({"device": device, "owner": owner}),
    );
    assert_eq!(
        twice["data"]["pointOfSaleDevicePaymentSessionOpen"]["userErrors"],
        json!([{"code": "ALREADY_OPEN"}])
    );
    let adjusted = run_with(
        &mut shop,
        r#"mutation($id: ID!, $owner: ID!) { pointOfSaleDevicePaymentSessionAdjust(pointOfSaleDevicePaymentSessionId: $id, staffMemberId: $owner,
            cash: {amount: "-20.00", currencyCode: USD}, note: "Pickup", time: "2026-10-31T12:00:00Z") {
            pointOfSaleDevicePaymentSession { totalAdjustments { amount } cashDrawer { balance { amount } } } userErrors { field message } } }"#,
        json!({"id": id, "owner": owner}),
    );
    let session =
        &ok(&adjusted, "pointOfSaleDevicePaymentSessionAdjust")["pointOfSaleDevicePaymentSession"];
    assert_eq!(session["totalAdjustments"]["amount"], json!("-20.0"));
    assert_eq!(session["cashDrawer"]["balance"]["amount"], json!("80.0"));
    let counted = run_with(
        &mut shop,
        r#"mutation($id: ID!, $owner: ID!) { pointOfSaleDevicePaymentSessionCount(pointOfSaleDevicePaymentSessionId: $id, staffMemberId: $owner,
            balance: {amount: "78.00", currencyCode: USD}, time: "2026-10-31T15:00:00Z") {
            pointOfSaleDevicePaymentSession { totalDiscrepancy { amount } } userErrors { field message code } } }"#,
        json!({"id": id, "owner": owner}),
    );
    assert_eq!(
        ok(&counted, "pointOfSaleDevicePaymentSessionCount")["pointOfSaleDevicePaymentSession"]["totalDiscrepancy"]
            ["amount"],
        json!("98.0")
    );
    // Too early, in another currency: refused.
    shop.take_events();
    let before = shop.clone();
    let wrong = run_with(
        &mut shop,
        r#"mutation($id: ID!, $owner: ID!) { pointOfSaleDevicePaymentSessionClose(pointOfSaleDevicePaymentSessionId: $id, staffMemberId: $owner,
            balance: {amount: "78.00", currencyCode: EUR}, time: "2026-10-31T08:00:00Z") { userErrors { field code } } }"#,
        json!({"id": id, "owner": owner}),
    );
    assert_eq!(
        wrong["data"]["pointOfSaleDevicePaymentSessionClose"]["userErrors"],
        json!([{"field": ["time"], "code": "TIME_BEFORE_SESSION_OPEN"}])
    );
    let wrong = run_with(
        &mut shop,
        r#"mutation($id: ID!, $owner: ID!) { pointOfSaleDevicePaymentSessionClose(pointOfSaleDevicePaymentSessionId: $id, staffMemberId: $owner,
            balance: {amount: "78.00", currencyCode: EUR}) { userErrors { field code } } }"#,
        json!({"id": id, "owner": owner}),
    );
    assert_eq!(
        wrong["data"]["pointOfSaleDevicePaymentSessionClose"]["userErrors"],
        json!([{"field": ["balance"], "code": "CURRENCY_MISMATCH"}])
    );
    unchanged(&mut shop, &before, &[&id, &drawer, &device]);

    let closed = run_with(
        &mut shop,
        r#"mutation($id: ID!, $owner: ID!) { pointOfSaleDevicePaymentSessionClose(pointOfSaleDevicePaymentSessionId: $id, staffMemberId: $owner,
            balance: {amount: "80.00", currencyCode: USD}, note: "End of day", time: "2026-10-31T18:00:00Z") {
            pointOfSaleDevicePaymentSession { status closingTime closingBalance { amount } closingAdjustment { amount } expectedCashAtClose { amount }
                cashActivities(first: 10) { nodes { __typename ... on CashCountActivity { countType: type cashDiscrepancy { amount } } ... on CashAdjustmentActivity { adjustmentType: type cash { amount } } } } }
            userErrors { field message code } } }"#,
        json!({"id": id, "owner": owner}),
    );
    let session =
        &ok(&closed, "pointOfSaleDevicePaymentSessionClose")["pointOfSaleDevicePaymentSession"];
    assert_eq!(session["status"], json!("CLOSED"));
    assert_eq!(session["closingAdjustment"]["amount"], json!("2.0"));
    assert_eq!(session["expectedCashAtClose"]["amount"], json!("78.0"));
    assert_eq!(
        session["cashActivities"]["nodes"],
        json!([
            {"__typename": "CashCountActivity", "countType": "OPENING", "cashDiscrepancy": {"amount": "100.0"}},
            {"__typename": "CashAdjustmentActivity", "adjustmentType": "ADJUSTMENT", "cash": {"amount": "-20.0"}},
            {"__typename": "CashCountActivity", "countType": "MID_SESSION", "cashDiscrepancy": {"amount": "-2.0"}},
            {"__typename": "CashCountActivity", "countType": "CLOSING", "cashDiscrepancy": {"amount": "2.0"}},
        ])
    );
    assert_eq!(
        shop.get(&device).unwrap()["activePaymentSession"],
        json!(null)
    );
    assert!(
        topics(&mut shop).contains(&"point_of_sale_device_payment_sessions/update".to_string())
    );

    // What it derives: the drawer's totals, the summaries, the deprecated tracking sessions.
    let read = run_with(
        &mut shop,
        r#"query($drawer: ID!, $location: ID!) {
            cashDrawer(id: $drawer) { balance { amount } totalAdjustments(dateRange: {from: "2026-10-31T00:00:00Z", to: "2026-10-31T23:59:59Z"}) { amount }
                totalDiscrepancies(dateRange: {from: "2026-10-31T00:00:00Z", to: "2026-10-31T23:59:59Z"}) { amount } }
            cashManagementLocationSummary(locationId: $location, startDate: "2026-10-31", endDate: "2026-10-31") {
                sessionsOpened sessionsClosed cashBalanceAtStart { amount } cashBalanceAtEnd { amount } netCash { amount } totalDiscrepancies { amount } }
            pointOfSaleDevicePaymentSessions(first: 5, query: "is_open:false") { nodes { status } }
            cashTrackingSessions(first: 5) { nodes { openingBalance { amount } closingBalance { amount } registerName adjustments(first: 5) { nodes { cash { amount } } } } } }"#,
        json!({"drawer": drawer, "location": location}),
    );
    assert!(read.get("errors").is_none(), "{read}");
    let data = &read["data"];
    assert_eq!(data["cashDrawer"]["balance"]["amount"], json!("80.0"));
    assert_eq!(
        data["cashDrawer"]["totalAdjustments"]["amount"],
        json!("-20.0")
    );
    assert_eq!(
        data["cashDrawer"]["totalDiscrepancies"]["amount"],
        json!("100.0")
    );
    assert_eq!(
        data["cashManagementLocationSummary"],
        json!({"sessionsOpened": 1, "sessionsClosed": 1, "cashBalanceAtStart": {"amount": "0.0"},
            "cashBalanceAtEnd": {"amount": "80.0"}, "netCash": {"amount": "-20.0"}, "totalDiscrepancies": {"amount": "100.0"}})
    );
    assert_eq!(
        data["pointOfSaleDevicePaymentSessions"]["nodes"],
        json!([{"status": "CLOSED"}])
    );
    assert_eq!(
        data["cashTrackingSessions"]["nodes"],
        json!([{"openingBalance": {"amount": "100.0"}, "closingBalance": {"amount": "80.0"}, "registerName": "Front",
            "adjustments": {"nodes": [{"cash": {"amount": "-20.0"}}]}}])
    );
}
