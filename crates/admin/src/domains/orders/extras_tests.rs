//! The rest of the orders family through queries and mutations over the demo store: shipping
//! lines and discounts in order edits, requested edits, draft orders in bulk, returns.

use serde_json::{Value, json};

use super::tests::{amount, demo, order_named, run, topics};
use crate::state::ShopState;

const CALCULATED: &str = "id totalPriceSet { shopMoney { amount } } totalOutstandingSet { shopMoney { amount } }
    shippingLines { id title stagedStatus price { shopMoney { amount } } }
    stagedChanges(first: 10) { nodes { __typename ... on OrderStagedChangeAddShippingLine { title price { amount } } } }";

/// Begins an edit of an order: the calculated order's id.
fn begin(shop: &mut ShopState, order: &str) -> String {
    let answer = run(
        shop,
        "mutation($id: ID!) { orderEditBegin(id: $id) { calculatedOrder { id } userErrors { message } } }",
        json!({"id": order}),
    );
    answer["data"]["orderEditBegin"]["calculatedOrder"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

fn cents(text: &str) -> i64 {
    super::model::parse_cents(text).unwrap()
}

/// An order of the demo store's first variant with a shipping line, made with `orderCreate`.
fn shipped_order(shop: &mut ShopState) -> String {
    let variant = shop.keys("ProductVariant").next().unwrap().to_string();
    let answer = run(
        shop,
        "mutation($order: OrderCreateOrderInput!) { orderCreate(order: $order) { order { id } userErrors { message } } }",
        json!({"order": {
            "lineItems": [{"variantId": variant, "quantity": 1}],
            "shippingLines": [{"title": "Standard", "priceSet": {"shopMoney": {"amount": "6.00", "currencyCode": "USD"}}}],
            "financialStatus": "PENDING",
        }}),
    );
    topics(shop);
    answer["data"]["orderCreate"]["order"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn an_edit_adds_updates_and_removes_shipping_lines() {
    let mut shop = demo();
    let order = shipped_order(&mut shop);
    let before = shop.get(&order).unwrap().clone();
    let total = super::model::field_cents(&before, "totalPriceSet");
    let calculated = begin(&mut shop, &order);
    let added = run(
        &mut shop,
        &format!("mutation($id: ID!, $line: OrderEditAddShippingLineInput!) {{ orderEditAddShippingLine(id: $id, shippingLine: $line) {{
            calculatedOrder {{ {CALCULATED} }} calculatedShippingLine {{ id title stagedStatus price {{ shopMoney {{ amount }} }} }} userErrors {{ field message code }} }} }}"),
        json!({"id": calculated, "line": {"title": "2-Day Shipping", "price": {"amount": 19.99, "currencyCode": "USD"}}}),
    );
    let payload = &added["data"]["orderEditAddShippingLine"];
    assert_eq!(payload["userErrors"], json!([]), "{added}");
    let line = &payload["calculatedShippingLine"];
    assert_eq!(line["stagedStatus"], "ADDED");
    assert!(
        line["id"]
            .as_str()
            .unwrap()
            .starts_with("gid://shopify/CalculatedShippingLine/")
    );
    assert_eq!(
        cents(amount(&payload["calculatedOrder"]["totalPriceSet"])),
        total + 1999
    );
    assert_eq!(
        payload["calculatedOrder"]["stagedChanges"]["nodes"],
        json!([{"__typename": "OrderStagedChangeAddShippingLine", "title": "2-Day Shipping", "price": {"amount": "19.99"}}])
    );
    let line_id = line["id"].clone();
    let update = "mutation($id: ID!, $line: ID!, $input: OrderEditUpdateShippingLineInput!) {
        orderEditUpdateShippingLine(id: $id, shippingLineId: $line, shippingLine: $input) {
          calculatedOrder { totalPriceSet { shopMoney { amount } } } userErrors { field message code } } }";
    let updated = run(
        &mut shop,
        update,
        json!({"id": calculated, "line": line_id, "input": {"price": {"amount": "5.00", "currencyCode": "USD"}}}),
    );
    assert_eq!(
        cents(amount(
            &updated["data"]["orderEditUpdateShippingLine"]["calculatedOrder"]["totalPriceSet"]
        )),
        total + 500
    );
    // The order's own shipping line can only be removed.
    let own = shop.get(&calculated).unwrap()["shippingLines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|line| line["stagedStatus"] == "NONE")
        .unwrap()["id"]
        .clone();
    let refused = run(
        &mut shop,
        update,
        json!({"id": calculated, "line": own, "input": {"title": "Express"}}),
    );
    assert_eq!(
        refused["data"]["orderEditUpdateShippingLine"]["userErrors"][0]["code"],
        "INVALID"
    );
    let own_price = super::model::cents(
        shop.get(&calculated).unwrap()["shippingLines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|line| line["id"] == own)
            .unwrap()
            .get("price"),
    );
    let removed = run(
        &mut shop,
        "mutation($id: ID!, $line: ID!) { orderEditRemoveShippingLine(id: $id, shippingLineId: $line) {
            calculatedOrder { totalPriceSet { shopMoney { amount } } shippingLines { stagedStatus } } userErrors { field message code } } }",
        json!({"id": calculated, "line": own}),
    );
    let removed = &removed["data"]["orderEditRemoveShippingLine"];
    assert_eq!(removed["userErrors"], json!([]));
    assert_eq!(
        cents(amount(&removed["calculatedOrder"]["totalPriceSet"])),
        total + 500 - own_price
    );
    assert!(
        topics(&mut shop).is_empty(),
        "nothing fires before the commit"
    );

    let committed = run(
        &mut shop,
        "mutation($id: ID!) { orderEditCommit(id: $id) { order { totalPriceSet { shopMoney { amount } }
            totalShippingPriceSet { shopMoney { amount } } shippingLine { title }
            shippingLines(first: 5, includeRemovals: true) { nodes { title isRemoved custom originalPriceSet { shopMoney { amount } } } } } userErrors { message } } }",
        json!({"id": calculated}),
    );
    let order_after = &committed["data"]["orderEditCommit"]["order"];
    assert_eq!(cents(amount(&order_after["totalShippingPriceSet"])), 500);
    assert_eq!(order_after["shippingLine"]["title"], "2-Day Shipping");
    let lines = order_after["shippingLines"]["nodes"].as_array().unwrap();
    assert_eq!(lines.len(), 2, "{committed}");
    assert_eq!(lines[0]["isRemoved"], true);
    assert_eq!(lines[1]["custom"], true);
    assert_eq!(
        cents(amount(&order_after["totalPriceSet"])),
        total + 500 - own_price
    );
    let topics = topics(&mut shop);
    assert!(topics.contains(&"orders/edited".to_string()), "{topics:?}");
}

#[test]
fn an_edits_shipping_line_is_refused_without_a_title_and_changes_nothing() {
    let mut shop = demo();
    let order = order_named(&shop, "#1002");
    let calculated = begin(&mut shop, &order);
    let before = shop.get(&calculated).cloned();
    let answer = run(
        &mut shop,
        "mutation($id: ID!, $line: OrderEditAddShippingLineInput!) { orderEditAddShippingLine(id: $id, shippingLine: $line) {
            calculatedShippingLine { id } userErrors { field message code } } }",
        json!({"id": calculated, "line": {"title": " ", "price": {"amount": "1.00", "currencyCode": "USD"}}}),
    );
    let payload = &answer["data"]["orderEditAddShippingLine"];
    assert_eq!(payload["calculatedShippingLine"], Value::Null);
    assert_eq!(
        payload["userErrors"][0]["field"],
        json!(["shippingLine", "title"])
    );
    let unknown = run(
        &mut shop,
        "mutation($id: ID!) { orderEditRemoveShippingLine(id: $id, shippingLineId: \"gid://shopify/CalculatedShippingLine/x\") { userErrors { code } } }",
        json!({"id": calculated}),
    );
    assert_eq!(
        unknown["data"]["orderEditRemoveShippingLine"]["userErrors"][0]["code"],
        "INVALID"
    );
    assert_eq!(shop.get(&calculated).cloned(), before);
}

#[test]
fn an_edits_discount_is_updated_and_removed_from_its_line() {
    let mut shop = demo();
    let order = order_named(&shop, "#1002");
    let calculated = begin(&mut shop, &order);
    let variant = shop.keys("ProductVariant").last().unwrap().to_string();
    let added = run(
        &mut shop,
        "mutation($id: ID!, $variant: ID!) { orderEditAddVariant(id: $id, variantId: $variant, quantity: 2, allowDuplicates: true) { calculatedLineItem { id originalUnitPriceSet { shopMoney { amount } } } } }",
        json!({"id": calculated, "variant": variant}),
    );
    let line = added["data"]["orderEditAddVariant"]["calculatedLineItem"].clone();
    let unit = cents(amount(&line["originalUnitPriceSet"]));
    let discounted = run(
        &mut shop,
        "mutation($id: ID!, $line: ID!) { orderEditAddLineItemDiscount(id: $id, lineItemId: $line, discount: {percentValue: 10, description: \"Ten\"}) { addedDiscountStagedChange { id } } }",
        json!({"id": calculated, "line": line["id"]}),
    );
    let application =
        discounted["data"]["orderEditAddLineItemDiscount"]["addedDiscountStagedChange"]["id"]
            .clone();
    let update = "mutation($id: ID!, $application: ID!, $discount: OrderEditAppliedDiscountInput!) {
        orderEditUpdateDiscount(id: $id, discountApplicationId: $application, discount: $discount) {
          calculatedOrder { addedDiscountApplications(first: 5) { nodes { description value { ... on MoneyV2 { amount } } } }
            lineItems(first: 20) { nodes { id calculatedDiscountAllocations { allocatedAmountSet { shopMoney { amount } } } } } }
          userErrors { field message code } } }";
    let updated = run(
        &mut shop,
        update,
        json!({"id": calculated, "application": application, "discount": {"fixedValue": {"amount": "1.00", "currencyCode": "USD"}, "description": "One off"}}),
    );
    let payload = &updated["data"]["orderEditUpdateDiscount"];
    assert_eq!(payload["userErrors"], json!([]), "{updated}");
    assert_eq!(
        payload["calculatedOrder"]["addedDiscountApplications"]["nodes"][0],
        json!({"description": "One off", "value": {"amount": "1.0"}})
    );
    let allocation = payload["calculatedOrder"]["lineItems"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == line["id"])
        .unwrap()["calculatedDiscountAllocations"][0]["allocatedAmountSet"]
        .clone();
    assert_eq!(cents(amount(&allocation)), 200.min(unit * 2));
    let refused = run(
        &mut shop,
        update,
        json!({"id": calculated, "application": "gid://shopify/CalculatedManualDiscountApplication/0", "discount": {"percentValue": 5}}),
    );
    assert_eq!(
        refused["data"]["orderEditUpdateDiscount"]["userErrors"][0]["code"],
        "INVALID"
    );
    let removed = run(
        &mut shop,
        "mutation($id: ID!, $application: ID!) { orderEditRemoveLineItemDiscount(id: $id, discountApplicationId: $application) {
            calculatedLineItem { id hasStagedLineItemDiscount } userErrors { field message } } }",
        json!({"id": calculated, "application": application}),
    );
    assert_eq!(
        removed["data"]["orderEditRemoveLineItemDiscount"],
        json!({"calculatedLineItem": {"id": line["id"], "hasStagedLineItemDiscount": false}, "userErrors": []})
    );
}
