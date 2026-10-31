//! The payments of orders through queries and mutations over the demo store: payment terms and
//! their schedules, manual and mandate payments, reminders, tender transactions, B2B drafts.

use chrono::Duration;
use serde_json::{Value, json};

use super::tests::{amount, demo, order_named, run, topics};
use crate::outbox;
use crate::state::{Clock, ShopState, value};

/// A pending order of one unit of the demo store's first variant, made with `orderCreate`.
fn pending_order(shop: &mut ShopState) -> String {
    let variant = shop.keys("ProductVariant").next().unwrap().to_string();
    let answer = run(
        shop,
        "mutation($order: OrderCreateOrderInput!) { orderCreate(order: $order) { order { id } userErrors { message } } }",
        json!({"order": {
            "lineItems": [{"variantId": variant, "quantity": 2}],
            "financialStatus": "PENDING",
            "email": "buyer@example.com",
        }}),
    );
    topics(shop);
    answer["data"]["orderCreate"]["order"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

const TERMS: &str = "id paymentTermsName paymentTermsType dueInDays due overdue
    order { id } draftOrder { id }
    paymentSchedules(first: 5) { nodes { id issuedAt dueAt completedAt due
      balanceDue { amount } totalBalance { amount } amount { amount } } }";

fn create_terms(shop: &mut ShopState, order: &str, attributes: Value) -> Value {
    run(
        shop,
        &format!(
            "mutation($id: ID!, $attributes: PaymentTermsCreateInput!) {{
              paymentTermsCreate(referenceId: $id, paymentTermsAttributes: $attributes) {{
                paymentTerms {{ {TERMS} }} userErrors {{ field message code }} }} }}"
        ),
        json!({"id": order, "attributes": attributes}),
    )["data"]["paymentTermsCreate"]
        .clone()
}

/// Freezes the shop's clock at `days` after its time now.
fn later(shop: &mut ShopState, days: i64) {
    let now = shop.now() + Duration::days(days);
    shop.set_clock(Clock::Frozen(now));
}

#[test]
fn the_templates_are_shopifys() {
    let mut shop = demo();
    let answer = run(
        &mut shop,
        "{ all: paymentTermsTemplates { id name paymentTermsType dueInDays description translatedName }
           net: paymentTermsTemplates(paymentTermsType: NET) { name } }",
        json!({}),
    );
    let all = answer["data"]["all"].as_array().unwrap();
    assert_eq!(all.len(), 9);
    assert_eq!(
        all[4],
        json!({"id": "gid://shopify/PaymentTermsTemplate/4", "name": "Net 30", "paymentTermsType": "NET",
               "dueInDays": 30, "description": "Within 30 days", "translatedName": "Net 30"})
    );
    let net: Vec<&str> = answer["data"]["net"]
        .as_array()
        .unwrap()
        .iter()
        .map(|template| template["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        net,
        ["Net 7", "Net 15", "Net 30", "Net 45", "Net 60", "Net 90"]
    );
    assert!(answer.get("extensions").is_none(), "{answer}");
}

#[test]
fn net_terms_come_due_when_the_clock_passes_them() {
    let mut shop = demo();
    shop.set_clock(Clock::Frozen(
        chrono::DateTime::parse_from_rfc3339("2026-10-01T12:00:00Z")
            .unwrap()
            .into(),
    ));
    let order = pending_order(&mut shop);
    let created = create_terms(
        &mut shop,
        &order,
        json!({"paymentTermsTemplateId": "gid://shopify/PaymentTermsTemplate/4"}),
    );
    assert_eq!(created["userErrors"], json!([]), "{created}");
    let terms = &created["paymentTerms"];
    assert_eq!(terms["paymentTermsName"], "Net 30");
    assert_eq!(terms["order"]["id"], json!(order));
    assert_eq!(
        (terms["due"].clone(), terms["overdue"].clone()),
        (json!(false), json!(false))
    );
    let schedule = &terms["paymentSchedules"]["nodes"][0];
    assert_eq!(schedule["issuedAt"], "2026-10-01T12:00:00Z");
    assert_eq!(schedule["dueAt"], "2026-10-31T12:00:00Z");
    let total = run(
        &mut shop,
        "query($id: ID!) { order(id: $id) { totalOutstandingSet { shopMoney { amount } } paymentTerms { id } } }",
        json!({"id": order}),
    );
    assert_eq!(
        schedule["balanceDue"]["amount"],
        total["data"]["order"]["totalOutstandingSet"]["shopMoney"]["amount"]
    );
    assert_eq!(total["data"]["order"]["paymentTerms"]["id"], terms["id"]);
    assert_eq!(
        topics(&mut shop),
        ["payment_terms/create", "orders/updated"]
    );

    // Not due yet: nothing fires.
    super::tick(&mut shop);
    assert!(topics(&mut shop).is_empty());
    // The clock passes the due date: `payment_schedules/due`, once.
    later(&mut shop, 30);
    super::tick(&mut shop);
    let events: Vec<(String, String)> = shop
        .take_events()
        .into_iter()
        .map(|event| (event.topic, event.gid))
        .collect();
    assert_eq!(
        events,
        [(
            "payment_schedules/due".to_string(),
            schedule["id"].as_str().unwrap().to_string()
        )]
    );
    super::tick(&mut shop);
    assert!(topics(&mut shop).is_empty());
    let read = |shop: &mut ShopState| {
        run(
            shop,
            &format!("query($id: ID!) {{ order(id: $id) {{ paymentTerms {{ {TERMS} }} }} }}"),
            json!({"id": order}),
        )["data"]["order"]["paymentTerms"]
            .clone()
    };
    let due = read(&mut shop);
    assert_eq!(
        (due["due"].clone(), due["overdue"].clone()),
        (json!(true), json!(false))
    );
    later(&mut shop, 1);
    assert_eq!(read(&mut shop)["overdue"], json!(true));
}

#[test]
fn payment_terms_are_refused_where_shopify_refuses_them_and_nothing_changes() {
    let mut shop = demo();
    let paid = order_named(&shop, "#1001");
    let before = shop.clone();
    let refused = create_terms(
        &mut shop,
        &paid,
        json!({"paymentTermsTemplateId": "gid://shopify/PaymentTermsTemplate/4"}),
    );
    assert_eq!(refused["paymentTerms"], Value::Null);
    assert_eq!(
        refused["userErrors"][0]["code"],
        "PAYMENT_TERMS_CREATION_UNSUCCESSFUL"
    );
    let order = pending_order(&mut shop);
    let before_order = shop.get(&order).cloned();
    for attributes in [
        json!({"paymentTermsTemplateId": "gid://shopify/PaymentTermsTemplate/99"}),
        json!({"paymentTermsTemplateId": "gid://shopify/PaymentTermsTemplate/7"}),
        json!({"paymentTermsTemplateId": "gid://shopify/PaymentTermsTemplate/2", "paymentSchedules": [{}, {}]}),
    ] {
        let refused = create_terms(&mut shop, &order, attributes);
        assert_eq!(refused["paymentTerms"], Value::Null, "{refused}");
        assert_eq!(refused["userErrors"].as_array().unwrap().len(), 1);
    }
    assert_eq!(shop.get(&order).cloned(), before_order);
    assert_eq!(shop.count("PaymentTerms"), 0);
    assert_eq!(before.get(&paid), shop.get(&paid));
    assert!(topics(&mut shop).is_empty());
}

#[test]
fn payment_terms_are_updated_and_deleted() {
    let mut shop = demo();
    let order = pending_order(&mut shop);
    let created = create_terms(
        &mut shop,
        &order,
        json!({"paymentTermsTemplateId": "gid://shopify/PaymentTermsTemplate/2"}),
    );
    let id = created["paymentTerms"]["id"].as_str().unwrap().to_string();
    topics(&mut shop);
    let update = format!(
        "mutation($input: PaymentTermsUpdateInput!) {{ paymentTermsUpdate(input: $input) {{
            paymentTerms {{ {TERMS} }} userErrors {{ field message code }} }} }}"
    );
    let updated = run(
        &mut shop,
        &update,
        json!({"input": {"paymentTermsId": id, "paymentTermsAttributes": {
            "paymentTermsTemplateId": "gid://shopify/PaymentTermsTemplate/7",
            "paymentSchedules": [{"dueAt": "2026-12-15T10:00:00.000Z"}],
        }}}),
    )["data"]["paymentTermsUpdate"]
        .clone();
    assert_eq!(updated["userErrors"], json!([]));
    assert_eq!(updated["paymentTerms"]["paymentTermsType"], "FIXED");
    assert_eq!(
        updated["paymentTerms"]["paymentSchedules"]["nodes"][0]["dueAt"],
        "2026-12-15T10:00:00Z"
    );
    assert_eq!(
        topics(&mut shop),
        ["payment_terms/update", "orders/updated"]
    );
    // Fixed terms without a due date are refused, and change nothing.
    let refused = run(
        &mut shop,
        &update,
        json!({"input": {"paymentTermsId": id, "paymentTermsAttributes": {
            "paymentTermsTemplateId": "gid://shopify/PaymentTermsTemplate/7",
            "paymentSchedules": [{"issuedAt": "2026-12-15T10:00:00Z"}],
        }}}),
    )["data"]["paymentTermsUpdate"]
        .clone();
    assert_eq!(
        refused["userErrors"][0]["code"],
        "PAYMENT_TERMS_UPDATE_UNSUCCESSFUL"
    );
    assert!(topics(&mut shop).is_empty());

    let delete = "mutation($id: ID!) { paymentTermsDelete(input: {paymentTermsId: $id}) { deletedId userErrors { field message code } } }";
    let deleted = run(&mut shop, delete, json!({"id": id}));
    assert_eq!(
        deleted["data"]["paymentTermsDelete"],
        json!({"deletedId": id, "userErrors": []})
    );
    assert_eq!(shop.get(&order).unwrap()["paymentTerms"], Value::Null);
    assert_eq!(shop.count("PaymentSchedule"), 0);
    assert_eq!(
        topics(&mut shop),
        ["payment_terms/delete", "orders/updated"]
    );
    let again = run(&mut shop, delete, json!({"id": id}));
    assert_eq!(
        again["data"]["paymentTermsDelete"]["userErrors"][0]["code"],
        "PAYMENT_TERMS_DELETE_UNSUCCESSFUL"
    );
}

const MANUAL: &str = "mutation($id: ID!, $amount: MoneyInput, $name: String, $at: DateTime) {
    orderCreateManualPayment(id: $id, amount: $amount, paymentMethodName: $name, processedAt: $at) {
      order { id displayFinancialStatus totalOutstandingSet { shopMoney { amount } }
        transactions { kind gateway manualPaymentGateway amountSet { shopMoney { amount } } processedAt } }
      userErrors { field message code } } }";

#[test]
fn a_manual_payment_pays_the_order_and_its_schedule() {
    let mut shop = demo();
    let order = pending_order(&mut shop);
    create_terms(
        &mut shop,
        &order,
        json!({"paymentTermsTemplateId": "gid://shopify/PaymentTermsTemplate/4"}),
    );
    topics(&mut shop);
    let total = value::decimal(super::model::field_cents(
        shop.get(&order).unwrap(),
        "totalOutstandingSet",
    ));
    let part = run(
        &mut shop,
        MANUAL,
        json!({"id": order, "amount": {"amount": "10.00", "currencyCode": "USD"}, "name": "Bank Deposit", "at": "2026-01-02T03:04:05Z"}),
    )["data"]["orderCreateManualPayment"]
        .clone();
    assert_eq!(part["userErrors"], json!([]), "{part}");
    assert_eq!(part["order"]["displayFinancialStatus"], "PARTIALLY_PAID");
    let sale = &part["order"]["transactions"][0];
    assert_eq!(
        (
            sale["kind"].clone(),
            sale["gateway"].clone(),
            sale["manualPaymentGateway"].clone()
        ),
        (json!("SALE"), json!("Bank Deposit"), json!(true))
    );
    assert_eq!(sale["processedAt"], "2026-01-02T03:04:05Z");
    assert_eq!(
        topics(&mut shop),
        [
            "order_transactions/create",
            "tender_transactions/create",
            "orders/updated"
        ]
    );
    // The rest, by default the outstanding amount, through the default method.
    let rest =
        run(&mut shop, MANUAL, json!({"id": order}))["data"]["orderCreateManualPayment"].clone();
    assert_eq!(rest["order"]["displayFinancialStatus"], "PAID");
    assert_eq!(amount(&rest["order"]["totalOutstandingSet"]), "0.0");
    assert_eq!(rest["order"]["transactions"][1]["gateway"], "Other");
    assert_eq!(
        topics(&mut shop),
        [
            "order_transactions/create",
            "tender_transactions/create",
            "orders/paid",
            "orders/updated"
        ]
    );
    let schedule = shop.keys("PaymentSchedule").next().unwrap().to_string();
    let schedule = shop.get(&schedule).unwrap();
    assert_eq!(schedule["balanceDue"]["amount"], "0.0");
    assert!(schedule["completedAt"].is_string());
    // Two tender transactions add up to what was paid.
    let tenders = run(
        &mut shop,
        "{ tenderTransactions(first: 50, reverse: true) { nodes { amount { amount } paymentMethod order { id } } } }",
        json!({}),
    );
    let paid: Vec<&str> = tenders["data"]["tenderTransactions"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|tender| tender["order"]["id"] == json!(order))
        .map(|tender| tender["amount"]["amount"].as_str().unwrap())
        .collect();
    assert_eq!(paid.len(), 2);
    let cents: i64 = paid
        .iter()
        .map(|amount| super::model::parse_cents(amount).unwrap())
        .sum();
    assert_eq!(value::decimal(cents), total);
}

#[test]
fn a_manual_payment_is_refused_where_the_order_cannot_take_it() {
    let mut shop = demo();
    let order = pending_order(&mut shop);
    let before = shop.get(&order).cloned();
    for (variables, code) in [
        (json!({"id": "gid://shopify/Order/1"}), "ORDER_NOT_FOUND"),
        (
            json!({"id": order, "amount": {"amount": "100000.00", "currencyCode": "USD"}}),
            "AMOUNT_EXCEEDS_BALANCE",
        ),
        (
            json!({"id": order, "amount": {"amount": "0", "currencyCode": "USD"}}),
            "AMOUNT_NOT_POSITIVE",
        ),
        (
            json!({"id": order, "amount": {"amount": "1.00", "currencyCode": "EUR"}}),
            "CURRENCY_MISMATCH",
        ),
        (
            json!({"id": order, "at": "2099-01-01T00:00:00Z"}),
            "PROCESSED_AT_INVALID",
        ),
    ] {
        let answer = run(&mut shop, MANUAL, variables);
        let payload = &answer["data"]["orderCreateManualPayment"];
        assert_eq!(payload["order"], Value::Null, "{answer}");
        assert_eq!(payload["userErrors"][0]["code"], code, "{answer}");
    }
    let paid = order_named(&shop, "#1001");
    let answer = run(&mut shop, MANUAL, json!({"id": paid}));
    assert_eq!(
        answer["data"]["orderCreateManualPayment"]["userErrors"][0]["code"],
        "AMOUNT_EXCEEDS_BALANCE"
    );
    assert_eq!(shop.get(&order).cloned(), before);
    assert!(topics(&mut shop).is_empty());
}

/// A card of the order's customer, as `Customer.paymentMethods` holds it.
fn vaulted_card(shop: &mut ShopState, order: &str) -> String {
    let customer = shop.get(order).unwrap()["customer"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| {
            let customer = shop.keys("Customer").next().unwrap().to_string();
            shop.get_mut(order)
                .unwrap()
                .insert("customer".into(), json!(customer));
            customer
        });
    let method = "gid://shopify/CustomerPaymentMethod/1a2b3c";
    shop.insert(value::object(json!({
        "__typename": "CustomerPaymentMethod",
        "id": method,
        "customer": customer,
        "revokedAt": null,
        "instrument": {"__typename": "CustomerCreditCard", "brand": "visa", "lastDigits": "4242",
                       "expiryMonth": 12, "expiryYear": 2030, "name": "Jane Doe"},
    })));
    shop.get_mut(&customer)
        .unwrap()
        .insert("paymentMethods".into(), json!([method]));
    let answer = run(
        shop,
        "query($id: ID!) { order(id: $id) { paymentCollectionDetails { additionalPaymentCollectionUrl
            vaultedPaymentMethods { id paymentInstrument { __typename ... on VaultCreditCard { brand lastDigits } } } } } }",
        json!({"id": order}),
    );
    let details = &answer["data"]["order"]["paymentCollectionDetails"];
    assert!(details["additionalPaymentCollectionUrl"].is_string());
    let mandate = &details["vaultedPaymentMethods"][0];
    assert_eq!(
        mandate["paymentInstrument"],
        json!({"__typename": "VaultCreditCard", "brand": "visa", "lastDigits": "4242"})
    );
    mandate["id"].as_str().unwrap().to_string()
}

const MANDATE: &str = "mutation($id: ID!, $mandate: ID!, $key: String!, $capture: Boolean, $amount: MoneyInput) {
    orderCreateMandatePayment(id: $id, mandateId: $mandate, idempotencyKey: $key, autoCapture: $capture, amount: $amount) {
      job { id done } jobResult { id done status order { id displayFinancialStatus } } paymentReferenceId
      userErrors { field message code } } }";

#[test]
fn a_mandate_payment_charges_a_vaulted_card_once_per_key() {
    let mut shop = demo();
    let order = pending_order(&mut shop);
    let mandate = vaulted_card(&mut shop, &order);
    let variables = json!({"id": order, "mandate": mandate, "key": "key-1"});
    let answer = run(&mut shop, MANDATE, variables.clone());
    let payload = answer["data"]["orderCreateMandatePayment"].clone();
    assert_eq!(payload["userErrors"], json!([]), "{answer}");
    assert_eq!(payload["job"]["done"], json!(true));
    assert_eq!(payload["jobResult"]["status"], "SUCCEEDED");
    assert_eq!(
        payload["jobResult"]["order"]["displayFinancialStatus"],
        "PAID"
    );
    assert!(payload["paymentReferenceId"].is_string());
    let result = payload["jobResult"]["id"].as_str().unwrap();
    let node = run(
        &mut shop,
        "query($id: ID!) { node(id: $id) { ... on OrderCreateMandatePaymentJobResult { done status } } }",
        json!({"id": result}),
    );
    assert_eq!(
        node["data"]["node"],
        json!({"done": true, "status": "SUCCEEDED"})
    );
    let sale = shop
        .get(&order)
        .unwrap()
        .get("transactions")
        .and_then(Value::as_array)
        .unwrap()
        .last()
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(shop.get(&sale).unwrap()["gateway"], "shopify_payments");
    assert_eq!(
        topics(&mut shop),
        [
            "order_transactions/create",
            "tender_transactions/create",
            "orders/paid",
            "orders/updated"
        ]
    );
    // The same key with the same arguments answers the first answer, and charges nothing.
    let again = run(&mut shop, MANDATE, variables);
    assert_eq!(again["data"]["orderCreateMandatePayment"], payload);
    assert!(topics(&mut shop).is_empty());
    // With other arguments, the key is refused.
    let other = run(
        &mut shop,
        MANDATE,
        json!({"id": order, "mandate": mandate, "key": "key-1", "capture": false}),
    );
    assert_eq!(
        other["data"]["orderCreateMandatePayment"]["userErrors"][0]["code"],
        "ORDER_MANDATE_PAYMENT_ERROR_CODE"
    );
}

#[test]
fn a_mandate_payment_is_refused_without_a_mandate_of_the_order() {
    let mut shop = demo();
    let order = pending_order(&mut shop);
    let before = shop.get(&order).cloned();
    let answer = run(
        &mut shop,
        MANDATE,
        json!({"id": order, "mandate": "gid://shopify/PaymentMandate/1", "key": "k"}),
    );
    let payload = &answer["data"]["orderCreateMandatePayment"];
    assert_eq!(payload["job"], Value::Null);
    assert_eq!(payload["userErrors"][0]["field"], json!(["mandateId"]));
    assert_eq!(shop.get(&order).cloned(), before);
    assert!(topics(&mut shop).is_empty());
}

#[test]
fn a_due_schedule_is_captured_and_a_reminder_recorded() {
    let mut shop = demo();
    let order = pending_order(&mut shop);
    vaulted_card(&mut shop, &order);
    let created = create_terms(
        &mut shop,
        &order,
        json!({"paymentTermsTemplateId": "gid://shopify/PaymentTermsTemplate/2"}),
    );
    let schedule = created["paymentTerms"]["paymentSchedules"]["nodes"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    topics(&mut shop);
    let capture = "mutation($id: ID!) { paymentScheduleCapture(paymentScheduleId: $id) {
        job { done } jobResult { status order { displayFinancialStatus } } userErrors { field message code } } }";
    let early = run(&mut shop, capture, json!({"id": schedule}));
    assert_eq!(
        early["data"]["paymentScheduleCapture"]["userErrors"][0]["code"],
        "PAYMENT_SCHEDULE_CAPTURE_ERROR_CODE"
    );
    assert!(topics(&mut shop).is_empty());

    let remind = "mutation($id: ID!) { paymentReminderSend(paymentScheduleId: $id) { success userErrors { field message code } } }";
    let reminded = run(&mut shop, remind, json!({"id": schedule}));
    assert_eq!(
        reminded["data"]["paymentReminderSend"],
        json!({"success": true, "userErrors": []})
    );
    let emails = outbox::emails(&shop);
    assert_eq!(emails.len(), 1);
    assert_eq!(emails[0]["template"], "payment_reminder");
    assert_eq!(emails[0]["to"], "buyer@example.com");
    assert_eq!(emails[0]["resource"], json!(schedule));

    later(&mut shop, 7);
    let captured = run(&mut shop, capture, json!({"id": schedule}));
    let payload = &captured["data"]["paymentScheduleCapture"];
    assert_eq!(payload["userErrors"], json!([]), "{captured}");
    assert_eq!(
        payload["jobResult"]["order"]["displayFinancialStatus"],
        "PAID"
    );
    assert!(shop.get(&schedule).unwrap()["completedAt"].is_string());
    // Paid: a reminder is refused, and records nothing.
    let refused = run(&mut shop, remind, json!({"id": schedule}));
    assert_eq!(
        refused["data"]["paymentReminderSend"]["userErrors"][0]["code"],
        "PAYMENT_REMINDER_SEND_UNSUCCESSFUL"
    );
    assert_eq!(outbox::emails(&shop).len(), 1);
}

#[test]
fn a_b2b_draft_with_net_terms_becomes_an_order_with_a_schedule() {
    let mut shop = demo();
    let company = run(
        &mut shop,
        "{ companies(first: 1) { nodes { id locations(first: 1) { nodes { id } } contacts(first: 1) { nodes { id customer { id email } } } } } }",
        json!({}),
    );
    let company = &company["data"]["companies"]["nodes"][0];
    let location = company["locations"]["nodes"][0]["id"].clone();
    let contact = &company["contacts"]["nodes"][0];
    let variant = shop.keys("ProductVariant").next().unwrap().to_string();
    let create = run(
        &mut shop,
        "mutation($input: DraftOrderInput!) { draftOrderCreate(input: $input) {
            draftOrder { id customer { id } purchasingEntity { __typename ... on PurchasingCompany { location { id } } }
              paymentTerms { paymentTermsName paymentSchedules(first: 1) { nodes { issuedAt dueAt } } } }
            userErrors { field message } } }",
        json!({"input": {
            "lineItems": [{"variantId": variant, "quantity": 3}],
            "purchasingEntity": {"purchasingCompany": {"companyId": company["id"], "companyLocationId": location, "companyContactId": contact["id"]}},
            "paymentTerms": {"paymentTermsTemplateId": "gid://shopify/PaymentTermsTemplate/4"},
        }}),
    );
    let draft = &create["data"]["draftOrderCreate"]["draftOrder"];
    assert_eq!(
        create["data"]["draftOrderCreate"]["userErrors"],
        json!([]),
        "{create}"
    );
    assert_eq!(draft["customer"]["id"], contact["customer"]["id"]);
    assert_eq!(draft["purchasingEntity"]["location"]["id"], location);
    assert_eq!(draft["paymentTerms"]["paymentTermsName"], "Net 30");
    // A draft's net terms are issued when the order is made (or the invoice sent).
    assert_eq!(
        draft["paymentTerms"]["paymentSchedules"]["nodes"][0]["issuedAt"],
        Value::Null
    );
    assert_eq!(
        topics(&mut shop),
        ["draft_orders/create", "payment_terms/create"]
    );
    let draft_id = draft["id"].as_str().unwrap().to_string();
    let completed = run(
        &mut shop,
        "mutation($id: ID!) { draftOrderComplete(id: $id) { draftOrder { order { id displayFinancialStatus
            purchasingEntity { __typename } paymentTerms { order { id } paymentSchedules(first: 1) { nodes { issuedAt dueAt balanceDue { amount } } } }
            totalOutstandingSet { shopMoney { amount } } } } userErrors { message } } }",
        json!({"id": draft_id}),
    );
    let order = &completed["data"]["draftOrderComplete"]["draftOrder"]["order"];
    assert_eq!(order["displayFinancialStatus"], "PENDING", "{completed}");
    assert_eq!(order["purchasingEntity"]["__typename"], "PurchasingCompany");
    assert_eq!(order["paymentTerms"]["order"]["id"], order["id"]);
    let schedule = &order["paymentTerms"]["paymentSchedules"]["nodes"][0];
    let issued =
        chrono::DateTime::parse_from_rfc3339(schedule["issuedAt"].as_str().unwrap()).unwrap();
    let due = chrono::DateTime::parse_from_rfc3339(schedule["dueAt"].as_str().unwrap()).unwrap();
    assert_eq!(due - issued, Duration::days(30));
    assert_eq!(
        schedule["balanceDue"]["amount"],
        order["totalOutstandingSet"]["shopMoney"]["amount"]
    );
    let topics = topics(&mut shop);
    assert!(
        topics.contains(&"payment_terms/update".to_string()),
        "{topics:?}"
    );
    assert!(!topics.contains(&"orders/paid".to_string()));
}

#[test]
fn a_b2b_draft_is_refused_for_a_location_of_another_company() {
    let mut shop = demo();
    let variant = shop.keys("ProductVariant").next().unwrap().to_string();
    let company = shop.keys("Company").next().unwrap().to_string();
    let contact = shop.keys("CompanyContact").next().unwrap().to_string();
    let drafts = shop.count("DraftOrder");
    let answer = run(
        &mut shop,
        "mutation($input: DraftOrderInput!) { draftOrderCreate(input: $input) { draftOrder { id } userErrors { field message } } }",
        json!({"input": {
            "lineItems": [{"variantId": variant, "quantity": 1}],
            "purchasingEntity": {"purchasingCompany": {"companyId": company, "companyLocationId": "gid://shopify/CompanyLocation/1", "companyContactId": contact}},
        }}),
    );
    assert_eq!(
        answer["data"]["draftOrderCreate"]["draftOrder"],
        Value::Null
    );
    assert_eq!(
        answer["data"]["draftOrderCreate"]["userErrors"][0]["field"],
        json!(["purchasingEntity", "purchasingCompany", "companyLocationId"])
    );
    assert_eq!(shop.count("DraftOrder"), drafts);
    assert!(topics(&mut shop).is_empty());
}

#[test]
fn the_tender_transactions_of_the_store_data_are_its_payments_and_refunds() {
    let mut shop = demo();
    let answer = run(
        &mut shop,
        "{ tenderTransactions(first: 50) { nodes { id amount { amount currencyCode } paymentMethod processedAt remoteReference test order { name } } } }",
        json!({}),
    );
    let nodes = answer["data"]["tenderTransactions"]["nodes"]
        .as_array()
        .unwrap();
    let sales = shop
        .all("OrderTransaction")
        .filter(|transaction| {
            matches!(
                transaction["kind"].as_str(),
                Some("SALE" | "CAPTURE" | "REFUND")
            )
        })
        .count();
    assert_eq!(nodes.len(), sales);
    assert!(nodes.iter().all(|node| node["order"]["name"].is_string()));
    assert!(answer.get("extensions").is_none(), "{answer}");
}

#[test]
fn invoices_are_recorded_in_the_outbox() {
    let mut shop = demo();
    let order = pending_order(&mut shop);
    let sent = run(
        &mut shop,
        "mutation($id: ID!) { orderInvoiceSend(id: $id, email: {customMessage: \"Thanks\"}) { order { id } userErrors { field message code } } }",
        json!({"id": order}),
    );
    assert_eq!(sent["data"]["orderInvoiceSend"]["userErrors"], json!([]));
    let emails = outbox::emails(&shop);
    assert_eq!(emails[0]["template"], "order_invoice");
    assert_eq!(emails[0]["to"], "buyer@example.com");
    assert!(emails[0]["body"].as_str().unwrap().contains("Thanks"));
    // Without an address to send to, nothing is recorded.
    shop.get_mut(&order)
        .unwrap()
        .insert("email".into(), Value::Null);
    let refused = run(
        &mut shop,
        "mutation($id: ID!) { orderInvoiceSend(id: $id) { order { id } userErrors { code } } }",
        json!({"id": order}),
    );
    assert_eq!(
        refused["data"]["orderInvoiceSend"]["userErrors"][0]["code"],
        "ORDER_INVOICE_SEND_UNSUCCESSFUL"
    );
    assert_eq!(outbox::emails(&shop).len(), 1);
}
