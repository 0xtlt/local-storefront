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

const REQUEST: &str =
    "mutation($input: RequestedOrderEditCreateInput!) { requestedOrderEditCreate(input: $input) {
    requestedOrderEdit { id status requestedAt order { id displayRequestedEditStatus }
      lineItems { removals(first: 5) { nodes { id quantity resolvedQuantity lineItem { id } } } } }
    userErrors { field message code } } }";

#[test]
fn a_requested_edit_is_resolved_with_what_the_merchant_took_off() {
    let mut shop = demo();
    let order = order_named(&shop, "#1002");
    let line = super::line_items(&shop, &order)[0].clone();
    let created = run(
        &mut shop,
        REQUEST,
        json!({"input": {"orderId": order, "lineItems": {"removals": [{"lineItemId": line, "quantity": 1}]}}}),
    );
    let edit = &created["data"]["requestedOrderEditCreate"]["requestedOrderEdit"];
    assert_eq!(
        created["data"]["requestedOrderEditCreate"]["userErrors"],
        json!([]),
        "{created}"
    );
    assert_eq!(edit["status"], "REQUESTED");
    assert_eq!(edit["order"]["displayRequestedEditStatus"], "REQUESTED");
    assert_eq!(edit["lineItems"]["removals"]["nodes"][0]["quantity"], 1);
    assert_eq!(topics(&mut shop), ["requested_order_edit/created"]);
    let id = edit["id"].clone();
    // The merchant removes the unit with an order edit, then resolves the request.
    let calculated = begin(&mut shop, &order);
    let calculated_line = shop.get(&calculated).unwrap()["lineItems"][0]["id"].clone();
    let current = shop.get(&line).unwrap()["currentQuantity"]
        .as_i64()
        .unwrap();
    run(
        &mut shop,
        "mutation($id: ID!, $line: ID!, $quantity: Int!) { orderEditSetQuantity(id: $id, lineItemId: $line, quantity: $quantity) { userErrors { message } } }",
        json!({"id": calculated, "line": calculated_line, "quantity": current - 1}),
    );
    run(
        &mut shop,
        "mutation($id: ID!) { orderEditCommit(id: $id) { userErrors { message } } }",
        json!({"id": calculated}),
    );
    topics(&mut shop);
    let resolve = "mutation($id: ID!) { requestedOrderEditResolve(input: {id: $id}) {
        requestedOrderEdit { status requestResolvedAt order { displayRequestedEditStatus } lineItems { removals(first: 5) { nodes { resolvedQuantity } } } }
        userErrors { field message code } } }";
    let resolved = run(&mut shop, resolve, json!({"id": id}));
    let edit = &resolved["data"]["requestedOrderEditResolve"]["requestedOrderEdit"];
    assert_eq!(edit["status"], "RESOLVED");
    assert!(edit["requestResolvedAt"].is_string());
    assert_eq!(edit["order"]["displayRequestedEditStatus"], "RESOLVED");
    assert_eq!(
        edit["lineItems"]["removals"]["nodes"][0]["resolvedQuantity"],
        1
    );
    assert_eq!(topics(&mut shop), ["requested_order_edit/resolved"]);
    // Closed, it cannot be declined.
    let again = run(
        &mut shop,
        "mutation($id: ID!) { requestedOrderEditDecline(input: {id: $id}) { requestedOrderEdit { id } userErrors { code } } }",
        json!({"id": id}),
    );
    assert_eq!(
        again["data"]["requestedOrderEditDecline"]["userErrors"][0]["code"],
        "INVALID"
    );
}

#[test]
fn a_requested_edit_is_declined_with_a_note_to_the_buyer() {
    let mut shop = demo();
    let order = order_named(&shop, "#1002");
    let line = super::line_items(&shop, &order)[0].clone();
    let created = run(
        &mut shop,
        REQUEST,
        json!({"input": {"orderId": order, "lineItems": {"removals": [{"lineItemId": line, "quantity": 1}]}}}),
    );
    let id = created["data"]["requestedOrderEditCreate"]["requestedOrderEdit"]["id"].clone();
    topics(&mut shop);
    let declined = run(
        &mut shop,
        "mutation($id: ID!) { requestedOrderEditDecline(input: {id: $id, declineNote: \"Already packed\"}) {
            requestedOrderEdit { status requestDeclinedAt order { displayRequestedEditStatus } } userErrors { code } } }",
        json!({"id": id}),
    );
    let edit = &declined["data"]["requestedOrderEditDecline"]["requestedOrderEdit"];
    assert_eq!(edit["status"], "DECLINED");
    assert_eq!(edit["order"]["displayRequestedEditStatus"], "DECLINED");
    assert_eq!(topics(&mut shop), ["requested_order_edit/declined"]);
    let emails = crate::outbox::emails(&shop);
    assert_eq!(emails[0]["template"], "requested_order_edit_declined");
    assert!(
        emails[0]["body"]
            .as_str()
            .unwrap()
            .contains("Already packed")
    );
}

#[test]
fn a_requested_edit_is_refused_beyond_what_is_left_to_fulfill() {
    let mut shop = demo();
    let fulfilled = order_named(&shop, "#1001");
    let shipped = super::line_items(&shop, &fulfilled)[0].clone();
    let open = order_named(&shop, "#1002");
    let before = shop.get(&fulfilled).cloned();
    for (input, code) in [
        (
            json!({"orderId": fulfilled, "lineItems": {"removals": [{"lineItemId": shipped, "quantity": 1}]}}),
            "INVALID",
        ),
        (
            json!({"orderId": open, "lineItems": {"removals": [{"lineItemId": shipped, "quantity": 1}]}}),
            "NOT_FOUND",
        ),
        (
            json!({"orderId": open, "lineItems": {"removals": []}}),
            "BLANK",
        ),
        (
            json!({"orderId": "gid://shopify/Order/1", "lineItems": {"removals": []}}),
            "NOT_FOUND",
        ),
    ] {
        let answer = run(&mut shop, REQUEST, json!({"input": input}));
        let payload = &answer["data"]["requestedOrderEditCreate"];
        assert_eq!(payload["requestedOrderEdit"], Value::Null);
        assert_eq!(payload["userErrors"][0]["code"], code, "{answer}");
    }
    assert_eq!(shop.get(&fulfilled).cloned(), before);
    assert_eq!(shop.count("RequestedOrderEdit"), 0);
    assert!(topics(&mut shop).is_empty());
}

/// A draft order of the demo store's first variant, tagged.
fn draft(shop: &mut ShopState, tags: &[&str]) -> String {
    let variant = shop.keys("ProductVariant").next().unwrap().to_string();
    let answer = run(
        shop,
        "mutation($input: DraftOrderInput!) { draftOrderCreate(input: $input) { draftOrder { id } userErrors { message } } }",
        json!({"input": {"lineItems": [{"variantId": variant, "quantity": 1}], "tags": tags, "email": "buyer@example.com"}}),
    );
    topics(shop);
    answer["data"]["draftOrderCreate"]["draftOrder"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn drafts_are_tagged_untagged_and_deleted_in_bulk() {
    let mut shop = demo();
    let first = draft(&mut shop, &["wholesale"]);
    let second = draft(&mut shop, &["retail"]);
    let add = "mutation($ids: [ID!], $search: String, $tags: [String!]!) { draftOrderBulkAddTags(ids: $ids, search: $search, tags: $tags) { job { id done } userErrors { field message } } }";
    let added = run(
        &mut shop,
        add,
        json!({"ids": [first, second], "tags": ["priority"]}),
    );
    assert_eq!(added["data"]["draftOrderBulkAddTags"]["job"]["done"], true);
    assert_eq!(
        topics(&mut shop),
        ["draft_orders/update", "draft_orders/update"]
    );
    assert_eq!(
        shop.get(&first).unwrap()["tags"],
        json!(["wholesale", "priority"])
    );
    // By a search: only the drafts it matches.
    let remove = "mutation($search: String, $tags: [String!]!) { draftOrderBulkRemoveTags(search: $search, tags: $tags) { job { done } userErrors { field message } } }";
    let removed = run(
        &mut shop,
        remove,
        json!({"search": "tag:retail", "tags": ["priority"]}),
    );
    assert_eq!(
        removed["data"]["draftOrderBulkRemoveTags"]["userErrors"],
        json!([])
    );
    assert_eq!(shop.get(&second).unwrap()["tags"], json!(["retail"]));
    assert_eq!(
        shop.get(&first).unwrap()["tags"],
        json!(["wholesale", "priority"])
    );
    assert_eq!(topics(&mut shop), ["draft_orders/update"]);
    let deleted = run(
        &mut shop,
        "mutation($ids: [ID!]) { draftOrderBulkDelete(ids: $ids) { job { done } userErrors { message } } }",
        json!({"ids": [first]}),
    );
    assert_eq!(deleted["data"]["draftOrderBulkDelete"]["job"]["done"], true);
    assert!(!shop.contains(&first));
    assert!(shop.contains(&second));
    assert_eq!(topics(&mut shop), ["draft_orders/delete"]);
}

#[test]
fn a_bulk_draft_mutation_is_refused_without_one_target_and_changes_nothing() {
    let mut shop = demo();
    let first = draft(&mut shop, &["wholesale"]);
    let before = shop.get(&first).cloned();
    let add = "mutation($ids: [ID!], $search: String, $tags: [String!]!) { draftOrderBulkAddTags(ids: $ids, search: $search, tags: $tags) { job { id } userErrors { field message } } }";
    for variables in [
        json!({"tags": ["x"]}),
        json!({"ids": [first], "search": "tag:x", "tags": ["x"]}),
        json!({"ids": [first], "tags": []}),
        json!({"ids": [first, "gid://shopify/DraftOrder/999999"], "tags": ["x"]}),
    ] {
        let variables_text = variables.to_string();
        let answer = run(&mut shop, add, variables);
        assert_eq!(
            answer["data"]["draftOrderBulkAddTags"]["job"],
            Value::Null,
            "{variables_text} {answer}"
        );
        assert_eq!(
            answer["data"]["draftOrderBulkAddTags"]["userErrors"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
    let deleted = run(
        &mut shop,
        "mutation { draftOrderBulkDelete(savedSearchId: \"gid://shopify/SavedSearch/1\") { job { id } userErrors { field message } } }",
        json!({}),
    );
    assert_eq!(deleted["data"]["draftOrderBulkDelete"]["job"], Value::Null);
    assert_eq!(shop.get(&first).cloned(), before);
    assert!(topics(&mut shop).is_empty());
}

#[test]
fn a_draft_is_made_from_an_order_and_its_invoice_previewed() {
    let mut shop = demo();
    let order = order_named(&shop, "#1002");
    let record = shop.get(&order).unwrap().clone();
    let made = run(
        &mut shop,
        "mutation($id: ID!) { draftOrderCreateFromOrder(orderId: $id) { draftOrder { id name status customer { id } email
            lineItems(first: 10) { nodes { quantity variant { id } } } } userErrors { field message } } }",
        json!({"id": order}),
    );
    let draft = &made["data"]["draftOrderCreateFromOrder"]["draftOrder"];
    assert_eq!(
        made["data"]["draftOrderCreateFromOrder"]["userErrors"],
        json!([]),
        "{made}"
    );
    assert_eq!(draft["status"], "OPEN");
    assert_eq!(draft["customer"]["id"], record["customer"]);
    let lines = super::line_items(&shop, &order);
    let first = shop.get(&lines[0]).unwrap();
    assert_eq!(
        draft["lineItems"]["nodes"][0]["variant"]["id"],
        first["variant"]
    );
    assert_eq!(
        draft["lineItems"]["nodes"][0]["quantity"],
        first["currentQuantity"]
    );
    assert_eq!(topics(&mut shop), ["draft_orders/create"]);

    let id = draft["id"].clone();
    let name = draft["name"].as_str().unwrap();
    let preview = run(
        &mut shop,
        "mutation($id: ID!, $email: EmailInput) { draftOrderInvoicePreview(id: $id, email: $email) { previewHtml previewSubject userErrors { field message } } }",
        json!({"id": id, "email": {"subject": "Custom subject", "customMessage": "Hi,\nThis is a custom message for the customer."}}),
    );
    let preview = &preview["data"]["draftOrderInvoicePreview"];
    assert_eq!(preview["previewSubject"], "Custom subject");
    assert_eq!(
        preview["previewHtml"],
        format!(
            "<!DOCTYPE html PUBLIC \"-//W3C//DTD HTML 4.0 Transitional//EN\" \"http://www.w3.org/TR/REC-html40/loose.dtd\">\n<html><body>\n<p>Invoice {name} from Local Supply Co.</p>\n<p>Hi,<br>This is a custom message for the customer.</p>\n</body></html>\n"
        )
    );
    assert!(topics(&mut shop).is_empty());
    assert!(
        crate::outbox::emails(&shop).is_empty(),
        "a preview sends nothing"
    );
    // A completed draft cannot be previewed: the reference's message.
    run(
        &mut shop,
        "mutation($id: ID!) { draftOrderComplete(id: $id) { draftOrder { id } } }",
        json!({"id": id}),
    );
    let refused = run(
        &mut shop,
        "mutation($id: ID!) { draftOrderInvoicePreview(id: $id) { previewHtml userErrors { field message } } }",
        json!({"id": id}),
    );
    assert_eq!(
        refused["data"]["draftOrderInvoicePreview"],
        json!({"previewHtml": null, "userErrors": [{"field": null, "message": "Draft order Invoice can't be sent. This draft order is already paid."}]})
    );
    let unknown = run(
        &mut shop,
        "mutation { draftOrderCreateFromOrder(orderId: \"gid://shopify/Order/1\") { draftOrder { id } userErrors { field message } } }",
        json!({}),
    );
    assert_eq!(
        unknown["data"]["draftOrderCreateFromOrder"]["userErrors"][0]["field"],
        json!(["orderId"])
    );
}
