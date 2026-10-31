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
    let emails = outbox::entries(&shop, Some(outbox::Channel::Email));
    assert_eq!(emails.len(), 1);
    assert_eq!(emails[0]["what"], "payment_reminder");
    assert_eq!(emails[0]["to"], "buyer@example.com");
    assert_eq!(emails[0]["about"], json!(schedule));

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
    assert_eq!(
        outbox::entries(&shop, Some(outbox::Channel::Email)).len(),
        1
    );
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
    let emails = outbox::entries(&shop, Some(outbox::Channel::Email));
    assert_eq!(emails[0]["what"], "order_invoice");
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
    assert_eq!(
        outbox::entries(&shop, Some(outbox::Channel::Email)).len(),
        1
    );
}

const CREDIT: &str = "mutation($id: ID!, $input: StoreCreditAccountCreditInput!) {
    storeCreditAccountCredit(id: $id, creditInput: $input) {
      storeCreditAccountTransaction { id amount { amount currencyCode } balanceAfterTransaction { amount }
        expiresAt remainingAmount { amount } event account { id balance { amount currencyCode } owner { ... on Customer { id } } } }
      userErrors { field message code } } }";

const DEBIT: &str = "mutation($id: ID!, $input: StoreCreditAccountDebitInput!) {
    storeCreditAccountDebit(id: $id, debitInput: $input) {
      storeCreditAccountTransaction { id amount { amount } balanceAfterTransaction { amount } event origin { ... on OrderTransaction { id } } account { balance { amount } } }
      userErrors { field message code } } }";

fn usd(amount: &str) -> Value {
    json!({"amount": amount, "currencyCode": "USD"})
}

#[test]
fn store_credit_is_credited_debited_by_expiry_and_expires() {
    let mut shop = demo();
    let customer = shop.keys("Customer").next().unwrap().to_string();
    let first = run(
        &mut shop,
        CREDIT,
        json!({"id": customer, "input": {"creditAmount": usd("49.99")}}),
    )["data"]["storeCreditAccountCredit"]
        .clone();
    assert_eq!(first["userErrors"], json!([]), "{first}");
    let transaction = &first["storeCreditAccountTransaction"];
    assert_eq!(transaction["account"]["owner"]["id"], json!(customer));
    assert_eq!(transaction["account"]["balance"]["amount"], "49.99");
    assert_eq!(transaction["event"], "ADJUSTMENT");
    let account = transaction["account"]["id"].as_str().unwrap().to_string();
    // By the account's id, a credit that expires.
    let expiring = run(
        &mut shop,
        CREDIT,
        json!({"id": account, "input": {"creditAmount": usd("20.00"), "expiresAt": "2099-01-01T00:00:00Z", "notify": true}}),
    )["data"]["storeCreditAccountCredit"]
        .clone();
    assert_eq!(
        expiring["storeCreditAccountTransaction"]["balanceAfterTransaction"]["amount"],
        "69.99"
    );
    assert_eq!(
        outbox::entries(&shop, Some(outbox::Channel::Email))[0]["what"],
        "store_credit_issued"
    );
    // A debit takes from the credit that expires first.
    let debited = run(
        &mut shop,
        DEBIT,
        json!({"id": customer, "input": {"debitAmount": usd("25.00")}}),
    )["data"]["storeCreditAccountDebit"]
        .clone();
    assert_eq!(debited["userErrors"], json!([]), "{debited}");
    assert_eq!(
        debited["storeCreditAccountTransaction"]["amount"]["amount"],
        "-25.0"
    );
    assert_eq!(
        debited["storeCreditAccountTransaction"]["account"]["balance"]["amount"],
        "44.99"
    );
    let read = run(
        &mut shop,
        "query($id: ID!) { storeCreditAccount(id: $id) { balance { amount }
            expiring: transactions(first: 5, query: \"type:credit AND expires_at:*\") { nodes { ... on StoreCreditAccountCreditTransaction { remainingAmount { amount } } } }
            recent: transactions(first: 5, sortKey: CREATED_AT, reverse: true) { nodes { __typename amount { amount } } } } }",
        json!({"id": account}),
    );
    let read = &read["data"]["storeCreditAccount"];
    assert_eq!(
        read["expiring"]["nodes"],
        json!([{"remainingAmount": {"amount": "0.0"}}])
    );
    assert_eq!(
        read["recent"]["nodes"][0]["__typename"],
        "StoreCreditAccountDebitTransaction"
    );
    let accounts = run(
        &mut shop,
        "query($id: ID!) { customer(id: $id) { usd: storeCreditAccounts(first: 5, query: \"currency_code:USD\") { nodes { id } }
            eur: storeCreditAccounts(first: 5, query: \"currency_code:EUR\") { nodes { id } } } }",
        json!({"id": customer}),
    );
    assert_eq!(
        accounts["data"]["customer"]["usd"]["nodes"][0]["id"],
        json!(account)
    );
    assert_eq!(accounts["data"]["customer"]["eur"]["nodes"], json!([]));

    // What is left of a credit leaves the account when it expires.
    let credit = run(
        &mut shop,
        CREDIT,
        json!({"id": account, "input": {"creditAmount": usd("10.00"), "expiresAt": "2099-06-01"}}),
    );
    assert_eq!(
        credit["data"]["storeCreditAccountCredit"]["userErrors"],
        json!([])
    );
    shop.set_clock(Clock::Frozen(
        chrono::DateTime::parse_from_rfc3339("2099-07-01T00:00:00Z")
            .unwrap()
            .into(),
    ));
    super::tick(&mut shop);
    let after = run(
        &mut shop,
        "query($id: ID!) { storeCreditAccount(id: $id) { balance { amount }
            transactions(first: 1, sortKey: CREATED_AT, reverse: true) { nodes { __typename amount { amount }
              ... on StoreCreditAccountExpirationTransaction { creditTransaction { id } } } } } }",
        json!({"id": account}),
    );
    let after = &after["data"]["storeCreditAccount"];
    assert_eq!(after["balance"]["amount"], "44.99");
    let expiration = &after["transactions"]["nodes"][0];
    assert_eq!(
        expiration["__typename"],
        "StoreCreditAccountExpirationTransaction"
    );
    assert_eq!(expiration["amount"]["amount"], "-10.0");
}

#[test]
fn store_credit_is_refused_as_shopify_refuses_it_and_nothing_changes() {
    let mut shop = demo();
    let customer = shop.keys("Customer").next().unwrap().to_string();
    let before = shop.get(&customer).cloned();
    for (amount, message) in [
        (
            "-100.00",
            "A positive amount must be used to credit a store credit account",
        ),
        (
            "100000.00",
            "The operation would cause the account's credit limit to be exceeded",
        ),
    ] {
        let answer = run(
            &mut shop,
            CREDIT,
            json!({"id": customer, "input": {"creditAmount": usd(amount)}}),
        );
        let payload = &answer["data"]["storeCreditAccountCredit"];
        assert_eq!(payload["storeCreditAccountTransaction"], Value::Null);
        assert_eq!(payload["userErrors"][0]["message"], message);
        assert_eq!(
            payload["userErrors"][0]["field"],
            json!(["creditInput", "creditAmount", "amount"])
        );
    }
    let past = run(
        &mut shop,
        CREDIT,
        json!({"id": customer, "input": {"creditAmount": usd("1.00"), "expiresAt": "2001-01-01T00:00:00Z"}}),
    );
    assert_eq!(
        past["data"]["storeCreditAccountCredit"]["userErrors"][0]["code"],
        "EXPIRES_AT_IN_PAST"
    );
    let unknown = run(
        &mut shop,
        CREDIT,
        json!({"id": "gid://shopify/Customer/1", "input": {"creditAmount": usd("1.00")}}),
    );
    assert_eq!(
        unknown["data"]["storeCreditAccountCredit"]["userErrors"][0]["code"],
        "OWNER_NOT_FOUND"
    );
    // A refused credit makes no account.
    assert_eq!(shop.count("StoreCreditAccount"), 0);
    assert_eq!(shop.get(&customer).cloned(), before);
    let none = run(
        &mut shop,
        DEBIT,
        json!({"id": customer, "input": {"debitAmount": usd("1.00")}}),
    );
    assert_eq!(
        none["data"]["storeCreditAccountDebit"]["userErrors"][0]["code"],
        "ACCOUNT_NOT_FOUND"
    );
    run(
        &mut shop,
        CREDIT,
        json!({"id": customer, "input": {"creditAmount": usd("5.00")}}),
    );
    let account = shop.keys("StoreCreditAccount").next().unwrap().to_string();
    let account_before = shop.get(&account).cloned();
    let too_much = run(
        &mut shop,
        DEBIT,
        json!({"id": account, "input": {"debitAmount": usd("9.99")}}),
    );
    let payload = &too_much["data"]["storeCreditAccountDebit"];
    assert_eq!(payload["userErrors"][0]["code"], "INSUFFICIENT_FUNDS");
    assert_eq!(
        payload["userErrors"][0]["message"],
        "The store credit account does not have sufficient funds to satisfy the request"
    );
    assert_eq!(shop.get(&account).cloned(), account_before);
}

#[test]
fn an_order_is_paid_and_refunded_with_store_credit() {
    let mut shop = demo();
    let customer = shop.keys("Customer").next().unwrap().to_string();
    run(
        &mut shop,
        CREDIT,
        json!({"id": customer, "input": {"creditAmount": usd("100.00")}}),
    );
    let account = shop.keys("StoreCreditAccount").next().unwrap().to_string();
    topics(&mut shop);
    let variant = shop.keys("ProductVariant").next().unwrap().to_string();
    let price = shop.get(&variant).unwrap()["price"]
        .as_str()
        .unwrap()
        .to_string();
    let create = "mutation($order: OrderCreateOrderInput!) { orderCreate(order: $order) {
        order { id displayFinancialStatus transactions { id kind gateway } } userErrors { field message code } } }";
    let order_input = |amount: &str| {
        json!({"order": {
            "lineItems": [{"variantId": variant, "quantity": 1}],
            "customer": {"toAssociate": {"id": customer}},
            "transactions": [{"kind": "SALE", "gateway": "shopify_store_credit", "amountSet": {"shopMoney": usd(amount)}}],
        }})
    };
    // More than the balance is refused, and debits nothing.
    let refused = run(&mut shop, create, order_input("150.00"));
    assert_eq!(refused["data"]["orderCreate"]["order"], Value::Null);
    assert_eq!(
        refused["data"]["orderCreate"]["userErrors"][0]["field"],
        json!(["order", "transactions"])
    );
    assert_eq!(super::store_credit::balance(&shop, &account), 10_000);
    let answer = run(&mut shop, create, order_input(&price));
    let order = &answer["data"]["orderCreate"]["order"];
    assert_eq!(order["displayFinancialStatus"], "PAID", "{answer}");
    let sale = order["transactions"][0]["id"].as_str().unwrap().to_string();
    let paid = super::model::parse_cents(&price).unwrap();
    assert_eq!(super::store_credit::balance(&shop, &account), 10_000 - paid);
    let debit = shop
        .all("StoreCreditAccountDebitTransaction")
        .next()
        .unwrap()
        .clone();
    assert_eq!(debit["event"], "ORDER_PAYMENT");
    assert_eq!(debit["origin"], json!(sale));
    // A refund to store credit gives it back.
    let order_id = order["id"].as_str().unwrap().to_string();
    let refund = run(
        &mut shop,
        "mutation($input: RefundInput!) { refundCreate(input: $input) { refund { totalRefundedSet { shopMoney { amount } } transactions(first: 5) { nodes { gateway kind } } } userErrors { field message } } }",
        json!({"input": {"orderId": order_id, "refundMethods": [{"storeCreditRefund": {"amount": usd(&price)}}]}}),
    );
    let refund = &refund["data"]["refundCreate"];
    assert_eq!(refund["userErrors"], json!([]), "{refund}");
    assert_eq!(
        refund["refund"]["transactions"]["nodes"][0],
        json!({"gateway": "shopify_store_credit", "kind": "REFUND"})
    );
    assert_eq!(super::store_credit::balance(&shop, &account), 10_000);
    let credit = shop
        .all("StoreCreditAccountCreditTransaction")
        .last()
        .unwrap()
        .clone();
    assert_eq!(credit["event"], "ORDER_REFUND");
    // Beyond what was paid, a refund to store credit is refused.
    let over = run(
        &mut shop,
        "mutation($input: RefundInput!) { refundCreate(input: $input) { refund { id } userErrors { field message } } }",
        json!({"input": {"orderId": order_id, "refundMethods": [{"storeCreditRefund": {"amount": usd("1.00")}}]}}),
    );
    assert_eq!(
        over["data"]["refundCreate"]["userErrors"][0]["field"],
        json!(["refundMethods"])
    );
    assert_eq!(super::store_credit::balance(&shop, &account), 10_000);
}

#[test]
fn a_cancelled_order_is_refunded_to_store_credit() {
    let mut shop = demo();
    let order = order_named(&shop, "#1002");
    let cancel = "mutation($id: ID!, $method: OrderCancelRefundMethodInput) {
        orderCancel(orderId: $id, reason: CUSTOMER, restock: false, refundMethod: $method) { job { done } orderCancelUserErrors { field message code } } }";
    let answer = run(
        &mut shop,
        cancel,
        json!({"id": order, "method": {"storeCreditRefund": {"expiresAt": "2099-01-01T00:00:00Z"}}}),
    );
    assert_eq!(
        answer["data"]["orderCancel"]["orderCancelUserErrors"],
        json!([]),
        "{answer}"
    );
    let record = shop.get(&order).unwrap().clone();
    let account = shop.get(record["customer"].as_str().unwrap()).unwrap()["storeCreditAccounts"][0]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        super::store_credit::balance(&shop, &account),
        super::model::field_cents(&record, "totalRefundedSet")
    );
    let credit = shop
        .all("StoreCreditAccountCreditTransaction")
        .next()
        .unwrap();
    assert_eq!(credit["event"], "ORDER_CANCELLATION");
    assert_eq!(credit["expiresAt"], "2099-01-01T00:00:00Z");
}

const ASSESS: &str = "mutation($input: OrderRiskAssessmentCreateInput!) {
    orderRiskAssessmentCreate(orderRiskAssessmentInput: $input) {
      orderRiskAssessment { riskLevel facts { description sentiment } provider { title } }
      userErrors { field message code } } }";

#[test]
fn a_risk_assessment_sets_the_orders_risk_and_recommendation() {
    let mut shop = demo();
    let order = order_named(&shop, "#1002");
    let before = run(
        &mut shop,
        "query($id: ID!) { order(id: $id) { risk { recommendation assessments { riskLevel } } riskLevel } }",
        json!({"id": order}),
    );
    assert_eq!(
        before["data"]["order"],
        json!({"risk": {"recommendation": "NONE", "assessments": []}, "riskLevel": "LOW"})
    );
    let answer = run(
        &mut shop,
        ASSESS,
        json!({"input": {"orderId": order, "riskLevel": "MEDIUM", "facts": [
            {"description": "Payment verification successful.", "sentiment": "POSITIVE"},
            {"description": "Buyer verification inconclusive.", "sentiment": "NEUTRAL"},
        ]}}),
    );
    let created = &answer["data"]["orderRiskAssessmentCreate"];
    assert_eq!(created["userErrors"], json!([]), "{answer}");
    assert_eq!(created["orderRiskAssessment"]["riskLevel"], "MEDIUM");
    assert!(created["orderRiskAssessment"]["provider"]["title"].is_string());
    assert_eq!(topics(&mut shop), ["orders/risk_assessment_changed"]);
    let after = run(
        &mut shop,
        "query($id: ID!) { order(id: $id) { risk { recommendation assessments { riskLevel facts { description } } } riskLevel risks { level message display } } }",
        json!({"id": order}),
    );
    let after = &after["data"]["order"];
    assert_eq!(after["risk"]["recommendation"], "INVESTIGATE");
    assert_eq!(after["riskLevel"], "MEDIUM");
    assert_eq!(
        after["risks"][1]["message"],
        "Buyer verification inconclusive."
    );
    run(
        &mut shop,
        ASSESS,
        json!({"input": {"orderId": order, "riskLevel": "HIGH", "facts": []}}),
    );
    let high = run(
        &mut shop,
        "query($id: ID!) { order(id: $id) { risk { recommendation } } }",
        json!({"id": order}),
    );
    assert_eq!(high["data"]["order"]["risk"]["recommendation"], "CANCEL");
}

#[test]
fn a_risk_assessment_is_refused_for_a_fulfilled_order_or_too_many_facts() {
    let mut shop = demo();
    let fulfilled = order_named(&shop, "#1001");
    let open = order_named(&shop, "#1002");
    let before = (shop.get(&fulfilled).cloned(), shop.get(&open).cloned());
    let refused = run(
        &mut shop,
        ASSESS,
        json!({"input": {"orderId": fulfilled, "riskLevel": "LOW", "facts": []}}),
    );
    assert_eq!(
        refused["data"]["orderRiskAssessmentCreate"]["userErrors"][0]["code"],
        "ORDER_ALREADY_FULFILLED"
    );
    let facts: Vec<Value> = (0..21)
        .map(|n| json!({"description": format!("Fact {n}"), "sentiment": "NEUTRAL"}))
        .collect();
    let too_many = run(
        &mut shop,
        ASSESS,
        json!({"input": {"orderId": open, "riskLevel": "LOW", "facts": facts}}),
    );
    assert_eq!(
        too_many["data"]["orderRiskAssessmentCreate"]["userErrors"][0]["code"],
        "TOO_MANY_FACTS"
    );
    assert_eq!(
        too_many["data"]["orderRiskAssessmentCreate"]["orderRiskAssessment"],
        Value::Null
    );
    assert_eq!(
        (shop.get(&fulfilled).cloned(), shop.get(&open).cloned()),
        before
    );
    assert!(topics(&mut shop).is_empty());
}

const UPSERT: &str = "mutation($input: OrderAttributionDefinitionUpsertInput!) {
    orderAttributionDefinitionUpsert(input: $input) { orderAttributionDefinition { id handle displayName icon } userErrors { field message code } } }";

#[test]
fn attribution_definitions_are_upserted_by_handle_and_attribute_orders() {
    let mut shop = demo();
    let icon = "<svg viewBox=\"0 0 20 20\" xmlns=\"http://www.w3.org/2000/svg\"><path d=\"M10 2 2 18h16L10 2Z\" /></svg>";
    let created = run(
        &mut shop,
        UPSERT,
        json!({"input": {"handle": "amazon-us-a1b2c3", "displayName": "Amazon US", "icon": icon}}),
    )["data"]["orderAttributionDefinitionUpsert"]
        .clone();
    assert_eq!(created["userErrors"], json!([]), "{created}");
    let id = created["orderAttributionDefinition"]["id"].clone();
    // The same handle, in another case: the definition is updated, its icon kept.
    let updated = run(
        &mut shop,
        UPSERT,
        json!({"input": {"handle": "AMAZON-US-A1B2C3", "displayName": "Amazon (US)"}}),
    )["data"]["orderAttributionDefinitionUpsert"]
        .clone();
    assert_eq!(updated["orderAttributionDefinition"]["id"], id);
    assert_eq!(updated["orderAttributionDefinition"]["icon"], icon);
    assert_eq!(
        updated["orderAttributionDefinition"]["displayName"],
        "Amazon (US)"
    );
    let cleared = run(
        &mut shop,
        UPSERT,
        json!({"input": {"handle": "amazon-us-a1b2c3", "displayName": "Amazon (US)", "icon": null}}),
    );
    assert_eq!(
        cleared["data"]["orderAttributionDefinitionUpsert"]["orderAttributionDefinition"]["icon"],
        Value::Null
    );
    let listed = run(
        &mut shop,
        "{ orderAttributionDefinitions(first: 5) { nodes { id handle } } }",
        json!({}),
    );
    assert_eq!(
        listed["data"]["orderAttributionDefinitions"]["nodes"][0]["id"],
        id
    );
    // An order created with the handle as its source is attributed to it.
    let variant = shop.keys("ProductVariant").next().unwrap().to_string();
    let order = run(
        &mut shop,
        "mutation($order: OrderCreateOrderInput!) { orderCreate(order: $order) { order { attribution { handle displayName } } } }",
        json!({"order": {"sourceName": "amazon-us-a1b2c3", "lineItems": [{"variantId": variant, "quantity": 1}]}}),
    );
    assert_eq!(
        order["data"]["orderCreate"]["order"]["attribution"],
        json!({"handle": "amazon-us-a1b2c3", "displayName": "Amazon (US)"})
    );
    let delete = "mutation($id: ID!) { orderAttributionDefinitionDelete(id: $id) { deletedId userErrors { field message code } } }";
    let deleted = run(&mut shop, delete, json!({"id": id}));
    assert_eq!(
        deleted["data"]["orderAttributionDefinitionDelete"]["deletedId"],
        id
    );
    let again = run(&mut shop, delete, json!({"id": id}));
    assert_eq!(
        again["data"]["orderAttributionDefinitionDelete"]["userErrors"][0]["code"],
        "NOT_FOUND"
    );
}

#[test]
fn an_attribution_definition_is_refused_without_a_name_or_with_another_icon() {
    let mut shop = demo();
    for input in [
        json!({"handle": "x", "displayName": " "}),
        json!({"handle": "", "displayName": "X"}),
        json!({"handle": "x", "displayName": "X", "icon": "<svg viewBox=\"0 0 24 24\"></svg>"}),
    ] {
        let answer = run(&mut shop, UPSERT, json!({"input": input}));
        let payload = &answer["data"]["orderAttributionDefinitionUpsert"];
        assert_eq!(payload["orderAttributionDefinition"], Value::Null);
        assert_eq!(payload["userErrors"][0]["code"], "INVALID");
    }
    assert_eq!(shop.count("OrderAttributionDefinition"), 0);
    assert!(topics(&mut shop).is_empty());
}

#[test]
fn shopify_protect_and_a_link_request_are_what_a_test_makes_happen() {
    let mut shop = demo();
    let order = order_named(&shop, "#1002");
    let read = |shop: &mut ShopState| {
        run(
            shop,
            "query($id: ID!) { order(id: $id) { shopifyProtect { status eligibility { status } } } }",
            json!({"id": order}),
        )["data"]["order"]["shopifyProtect"]
            .clone()
    };
    assert_eq!(read(&mut shop), Value::Null);
    super::risk::set_protect(&mut shop, &order, "protected", "eligible").unwrap();
    assert_eq!(
        read(&mut shop),
        json!({"status": "PROTECTED", "eligibility": {"status": "ELIGIBLE"}})
    );
    assert!(super::risk::set_protect(&mut shop, &order, "SAFE", "ELIGIBLE").is_err());
    super::risk::request_link(&mut shop, &order).unwrap();
    assert_eq!(
        topics(&mut shop),
        [
            "orders/shopify_protect_eligibility_changed",
            "orders/link_requested"
        ]
    );
    assert_eq!(
        outbox::entries(&shop, Some(outbox::Channel::Email))[0]["what"],
        "order_link_requested"
    );
}

const ACCOUNT: &str = "{ shopifyPaymentsAccount { id activated defaultCurrency country accountOpenerName payoutStatementDescriptor
    payoutSchedule { interval } balance { amount currencyCode }
    bankAccounts(first: 5) { nodes { bankName currency country status } }
    balanceTransactions(first: 50) { nodes { id type amount { amount } fee { amount } net { amount } associatedOrder { name } associatedPayout { id status } } }
    payouts(first: 50) { nodes { id issuedAt status transactionType net { amount } summary { chargesGross { amount } chargesFee { amount } } } } } }";

#[test]
fn the_shopify_payments_account_is_what_its_orders_paid_and_its_payouts() {
    let mut shop = demo();
    let answer = run(&mut shop, ACCOUNT, json!({}));
    assert!(answer.get("extensions").is_none(), "{answer}");
    let account = &answer["data"]["shopifyPaymentsAccount"];
    assert_eq!(account["activated"], true);
    assert_eq!(account["defaultCurrency"], "USD");
    assert_eq!(account["accountOpenerName"], "Local Supply Co.");
    assert_eq!(account["payoutStatementDescriptor"], "LOCAL SUPP");
    assert_eq!(account["bankAccounts"]["nodes"][0]["status"], "VALIDATED");
    // The orders of the store data were paid by card: charges, less their fee.
    let charges: Vec<&Value> = account["balanceTransactions"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|transaction| transaction["type"] == "CHARGE")
        .collect();
    assert!(!charges.is_empty());
    for charge in &charges {
        let amount = cents(&charge["amount"]["amount"]);
        assert_eq!(
            cents(&charge["fee"]["amount"]),
            (amount * 29 + 500) / 1000 + 30
        );
        assert_eq!(
            cents(&charge["net"]["amount"]),
            amount - cents(&charge["fee"]["amount"])
        );
    }
    // Their days are over: the clock pays them out.
    super::tick(&mut shop);
    let paid = run(&mut shop, ACCOUNT, json!({}));
    let account = &paid["data"]["shopifyPaymentsAccount"];
    assert_eq!(account["balance"][0]["amount"], "0.0");
    let payouts = account["payouts"]["nodes"].as_array().unwrap();
    assert!(!payouts.is_empty());
    assert!(payouts.iter().all(|payout| payout["status"] == "PAID"));
    assert!(
        account["balanceTransactions"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|transaction| transaction["associatedPayout"]["status"] == "PAID")
    );
    let hidden = run(
        &mut shop,
        "{ shopifyPaymentsAccount { balanceTransactions(first: 50, hideTransfers: true) { nodes { type } } } }",
        json!({}),
    );
    assert!(
        hidden["data"]["shopifyPaymentsAccount"]["balanceTransactions"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|transaction| transaction["type"] != "TRANSFER")
    );
}

fn cents(text: &Value) -> i64 {
    super::model::parse_cents(text.as_str().unwrap()).unwrap()
}

#[test]
fn a_payment_today_is_paid_out_at_once_in_its_currency_only() {
    let mut shop = demo();
    super::tick(&mut shop);
    let order = pending_order(&mut shop);
    let mandate = vaulted_card(&mut shop, &order);
    run(
        &mut shop,
        MANDATE,
        json!({"id": order, "mandate": mandate, "key": "k"}),
    );
    let balance = run(
        &mut shop,
        "{ shopifyPaymentsAccount { balance { amount } } }",
        json!({}),
    );
    let pending = cents(&balance["data"]["shopifyPaymentsAccount"]["balance"][0]["amount"]);
    assert!(pending > 0);
    let payout = "mutation($currency: CurrencyCode!) { shopifyPaymentsPayoutAlternateCurrencyCreate(currency: $currency) {
        success payout { amount { amount } currency remoteId } userErrors { field message code } } }";
    let refused = run(&mut shop, payout, json!({"currency": "CAD"}));
    let refused = &refused["data"]["shopifyPaymentsPayoutAlternateCurrencyCreate"];
    assert_eq!(refused["success"], false);
    assert_eq!(
        refused["userErrors"][0]["code"],
        "ALTERNATE_CURRENCY_PAYOUT_FAILED_NO_ELIGIBLE_BALANCE"
    );
    assert_eq!(
        cents(
            &run(
                &mut shop,
                "{ shopifyPaymentsAccount { balance { amount } } }",
                json!({})
            )["data"]["shopifyPaymentsAccount"]["balance"][0]["amount"]
        ),
        pending
    );
    let done = run(&mut shop, payout, json!({"currency": "USD"}));
    let done = &done["data"]["shopifyPaymentsPayoutAlternateCurrencyCreate"];
    assert_eq!(done["success"], true, "{done}");
    assert_eq!(cents(&done["payout"]["amount"]["amount"]), pending);
    assert!(
        done["payout"]["remoteId"]
            .as_str()
            .unwrap()
            .starts_with("po_")
    );
    let after = run(
        &mut shop,
        "{ shopifyPaymentsAccount { balance { amount } } }",
        json!({}),
    );
    assert_eq!(
        after["data"]["shopifyPaymentsAccount"]["balance"][0]["amount"],
        "0.0"
    );
}

const EVIDENCE: &str = "mutation($id: ID!, $input: ShopifyPaymentsDisputeEvidenceUpdateInput!) {
    disputeEvidenceUpdate(id: $id, input: $input) {
      disputeEvidence { id submitted uncategorizedText customerFirstName shippingAddress { city countryCodeV2 }
        disputeFileUploads { id disputeEvidenceType } dispute { status evidenceSentOn } }
      userErrors { field message code } } }";

#[test]
fn a_dispute_takes_money_back_and_takes_evidence_until_submitted() {
    let mut shop = demo();
    let order = order_named(&shop, "#1001");
    let dispute = super::shopify_payments::open_dispute(
        &mut shop,
        &super::shopify_payments::NewDispute {
            order: &order,
            amount: Some(1150),
            kind: "CHARGEBACK",
            reason: "FRAUDULENT",
        },
    )
    .unwrap();
    assert_eq!(topics(&mut shop), ["disputes/create"]);
    let read = run(
        &mut shop,
        "query($id: ID!, $order: ID!) { dispute(id: $id) { amount { amount } status type reasonDetails { reason networkReasonCode }
            evidenceDueBy initiatedAt order { id } disputeEvidence { id customerEmailAddress fulfillments { id } } }
          order(id: $order) { disputes { id initiatedAs status } }
          disputes(first: 5, query: \"status:needs_response\") { nodes { id } } }",
        json!({"id": dispute, "order": order}),
    );
    assert!(read.get("extensions").is_none(), "{read}");
    let found = &read["data"]["dispute"];
    assert_eq!(found["amount"]["amount"], "11.5");
    assert_eq!(
        found["reasonDetails"],
        json!({"reason": "FRAUDULENT", "networkReasonCode": "4837"})
    );
    assert_eq!(
        read["data"]["order"]["disputes"],
        json!([{"id": dispute, "initiatedAs": "CHARGEBACK", "status": "NEEDS_RESPONSE"}])
    );
    assert_eq!(read["data"]["disputes"]["nodes"][0]["id"], json!(dispute));
    let evidence = found["disputeEvidence"]["id"].clone();
    // The chargeback and its fee left the balance.
    let adjustment = shop
        .all("ShopifyPaymentsBalanceTransaction")
        .find(|transaction| transaction["type"] == "ADJUSTMENT")
        .unwrap();
    assert_eq!(adjustment["amount"]["amount"], "-11.5");
    assert_eq!(adjustment["fee"]["amount"], "15.0");

    // Evidence, with a file of the shop, then submitted.
    let file = "gid://shopify/GenericFile/77";
    shop.insert(value::object(json!({"__typename": "GenericFile", "id": file, "url": "https://example.com/receipt.pdf", "mimeType": "application/pdf", "originalFileSize": 1024})));
    let updated = run(
        &mut shop,
        EVIDENCE,
        json!({"id": evidence, "input": {
            "customerFirstName": "Kermit", "uncategorizedText": "Test uncategorized text",
            "shippingAddress": {"address1": "1 Infinite Loop", "city": "Cupertino", "zip": "95014", "country": "US", "province": "California"},
            "uncategorizedFile": {"id": file},
        }}),
    );
    let payload = &updated["data"]["disputeEvidenceUpdate"];
    assert_eq!(payload["userErrors"], json!([]), "{updated}");
    let evidence_read = &payload["disputeEvidence"];
    assert_eq!(
        evidence_read["uncategorizedText"],
        "Test uncategorized text"
    );
    assert_eq!(
        evidence_read["shippingAddress"],
        json!({"city": "Cupertino", "countryCodeV2": "US"})
    );
    assert_eq!(
        evidence_read["disputeFileUploads"][0]["disputeEvidenceType"],
        "UNCATEGORIZED_FILE"
    );
    assert_eq!(evidence_read["dispute"]["status"], "NEEDS_RESPONSE");
    assert_eq!(topics(&mut shop), ["disputes/update"]);
    let submitted = run(
        &mut shop,
        EVIDENCE,
        json!({"id": evidence, "input": {"submitEvidence": true}}),
    );
    let submitted = &submitted["data"]["disputeEvidenceUpdate"]["disputeEvidence"];
    assert_eq!(submitted["submitted"], true);
    assert_eq!(submitted["dispute"]["status"], "UNDER_REVIEW");
    assert!(submitted["dispute"]["evidenceSentOn"].is_string());
    topics(&mut shop);
    // Once submitted, the evidence is refused, and nothing changes.
    let before = shop.get(evidence.as_str().unwrap()).cloned();
    let refused = run(
        &mut shop,
        EVIDENCE,
        json!({"id": evidence, "input": {"uncategorizedText": "More"}}),
    );
    assert_eq!(
        refused["data"]["disputeEvidenceUpdate"]["userErrors"][0]["code"],
        "EVIDENCE_ALREADY_ACCEPTED"
    );
    assert_eq!(shop.get(evidence.as_str().unwrap()).cloned(), before);
    assert!(topics(&mut shop).is_empty());
    // The bank decides for the merchant: the amount comes back.
    let number = crate::state::parse_gid(&dispute).unwrap().1;
    super::shopify_payments::decide_dispute(&mut shop, number, "won").unwrap();
    assert_eq!(shop.get(&dispute).unwrap()["status"], "WON");
    assert!(
        shop.all("ShopifyPaymentsBalanceTransaction")
            .any(|transaction| transaction["type"] == "DISPUTE_REVERSAL")
    );
    assert_eq!(topics(&mut shop), ["disputes/update"]);
    assert!(super::shopify_payments::decide_dispute(&mut shop, number, "lost").is_err());
}

#[test]
fn dispute_evidence_is_refused_past_its_due_date_or_with_an_unknown_file() {
    let mut shop = demo();
    let order = order_named(&shop, "#1001");
    let dispute = super::shopify_payments::open_dispute(
        &mut shop,
        &super::shopify_payments::NewDispute {
            order: &order,
            amount: None,
            kind: "INQUIRY",
            reason: "GENERAL",
        },
    )
    .unwrap();
    let evidence = shop.get(&dispute).unwrap()["disputeEvidence"].clone();
    // An inquiry moves no money.
    assert!(
        !shop
            .all("ShopifyPaymentsBalanceTransaction")
            .any(|transaction| transaction["type"] == "ADJUSTMENT")
    );
    let unknown = run(
        &mut shop,
        EVIDENCE,
        json!({"id": evidence, "input": {"uncategorizedFile": {"id": "gid://shopify/GenericFile/1"}}}),
    );
    assert_eq!(
        unknown["data"]["disputeEvidenceUpdate"]["userErrors"][0]["code"],
        "FILE_NOT_FOUND"
    );
    let missing = run(
        &mut shop,
        EVIDENCE,
        json!({"id": "gid://shopify/ShopifyPaymentsDisputeEvidence/0", "input": {}}),
    );
    assert_eq!(
        missing["data"]["disputeEvidenceUpdate"]["userErrors"][0]["code"],
        "DISPUTE_EVIDENCE_NOT_FOUND"
    );
    later(&mut shop, 8);
    let late = run(
        &mut shop,
        EVIDENCE,
        json!({"id": evidence, "input": {"uncategorizedText": "Late"}}),
    );
    assert_eq!(
        late["data"]["disputeEvidenceUpdate"]["userErrors"][0]["code"],
        "EVIDENCE_PAST_DUE_DATE"
    );
    assert_eq!(
        shop.get(evidence.as_str().unwrap()).unwrap()["uncategorizedText"],
        Value::Null
    );
}

#[test]
fn the_tax_app_is_configured_then_activated_and_then_locked() {
    let mut shop = demo();
    let configure = "mutation($ready: Boolean!) { taxAppConfigure(ready: $ready) { taxAppConfiguration { state } userErrors { field message code } } }";
    let ready = run(&mut shop, configure, json!({"ready": true}));
    assert_eq!(
        ready["data"]["taxAppConfigure"],
        json!({"taxAppConfiguration": {"state": "READY"}, "userErrors": []})
    );
    assert_eq!(topics(&mut shop), ["tax_services/create"]);
    let pending = run(&mut shop, configure, json!({"ready": false}));
    assert_eq!(
        pending["data"]["taxAppConfigure"]["taxAppConfiguration"]["state"],
        "PENDING"
    );
    assert_eq!(topics(&mut shop), ["tax_services/update"]);
    assert!(
        super::tax::activate(&mut shop).is_err(),
        "a pending app cannot be turned on"
    );
    run(&mut shop, configure, json!({"ready": true}));
    super::tax::activate(&mut shop).unwrap();
    let events = shop.take_events();
    let last = events.last().unwrap();
    let payload = crate::webhooks::payload::payload(
        &last.topic,
        shop.get(&last.gid).unwrap(),
        &crate::api::samples::ShopLookup(&shop),
    )
    .unwrap();
    assert_eq!(payload["active"], true);
    assert_eq!(payload["name"], "lsf");
    let locked = run(&mut shop, configure, json!({"ready": false}));
    assert_eq!(
        locked["data"]["taxAppConfigure"]["userErrors"][0]["code"],
        "TAX_PARTNER_ALREADY_ACTIVE"
    );
    assert_eq!(super::tax::tax_state(&shop), "ACTIVE");
    assert!(topics(&mut shop).is_empty());
}

#[test]
fn tax_summaries_are_asked_for_an_order_or_a_period() {
    let mut shop = demo();
    let order = order_named(&shop, "#1001");
    let create = "mutation($order: ID, $start: DateTime, $end: DateTime) { taxSummaryCreate(orderId: $order, startTime: $start, endTime: $end) {
        enqueuedOrders { id } userErrors { field message code } } }";
    let one = run(&mut shop, create, json!({"order": order}));
    assert_eq!(
        one["data"]["taxSummaryCreate"],
        json!({"enqueuedOrders": [{"id": order}], "userErrors": []})
    );
    let all = run(
        &mut shop,
        create,
        json!({"start": "2000-01-01T00:00:00Z", "end": "2099-01-01T00:00:00Z"}),
    );
    assert_eq!(
        all["data"]["taxSummaryCreate"]["enqueuedOrders"]
            .as_array()
            .unwrap()
            .len(),
        shop.count("Order")
    );
    let none = run(
        &mut shop,
        create,
        json!({"start": "2000-01-01T00:00:00Z", "end": "2000-01-02T00:00:00Z"}),
    );
    assert_eq!(
        none["data"]["taxSummaryCreate"]["userErrors"][0]["code"],
        "ORDER_NOT_FOUND"
    );
    let unknown = run(&mut shop, create, json!({"order": "gid://shopify/Order/1"}));
    assert_eq!(
        unknown["data"]["taxSummaryCreate"]["enqueuedOrders"],
        Value::Null
    );
    let neither = run(&mut shop, create, json!({}));
    assert_eq!(
        neither["data"]["taxSummaryCreate"]["userErrors"][0]["code"],
        "GENERAL_ERROR"
    );
}

/// The fields of the area answer real values on the demo store: none is a default.
#[test]
fn nothing_of_the_area_is_defaulted_on_the_demo_store() {
    let mut shop = demo();
    for order in ["#1001", "#1002"] {
        let id = order_named(&shop, order);
        let answer = run(
            &mut shop,
            "query($id: ID!) { order(id: $id) {
                paymentTerms { id } paymentCollectionDetails { additionalPaymentCollectionUrl vaultedPaymentMethods { id } }
                risk { recommendation assessments { riskLevel } } riskLevel risks { level } shopifyProtect { status }
                disputes { id } displayRequestedEditStatus requestedOrderEdits(first: 5) { nodes { id } } attribution { handle }
                currentTotalDutiesSet { shopMoney { amount } } originalTotalDutiesSet { shopMoney { amount } }
                refundDiscrepancySet { shopMoney { amount } } totalCashRoundingAdjustment { paymentSet { shopMoney { amount } } refundSet { shopMoney { amount } } }
                suggestedRefund { amountSet { shopMoney { amount } } }
                lineItems(first: 5) { nodes { duties { id } } }
                transactions { id paymentId shopifyPaymentsSet { refundSet { acquirerReferenceNumber } } maximumRefundableV2 { amount } totalUnsettledSet { shopMoney { amount } } }
                returns(first: 5) { nodes { id exchangeLineItems(first: 5) { nodes { id } } reverseFulfillmentOrders(first: 5) { nodes { id status } } returnShippingFees { id } } }
              }
              returnableFulfillments(orderId: $id, first: 5) { nodes { id } }
              tenderTransactions(first: 5) { nodes { id amount { amount } paymentMethod processedAt remoteReference test transactionDetails { __typename } user { id } order { id } } }
              paymentTermsTemplates { id }
              orderAttributionDefinitions(first: 5) { nodes { id } }
              disputes(first: 5) { nodes { id } }
              shopifyPaymentsAccount { id balance { amount } chargeStatementDescriptors { default prefix } payoutSchedule { interval monthlyAnchor weeklyAnchor } } }",
            json!({"id": id}),
        );
        assert!(answer.get("errors").is_none(), "{answer}");
        assert!(
            answer.get("extensions").is_none(),
            "{order}: {}",
            answer["extensions"]
        );
    }
}

/// A credit's `expiresAt` given as a day expires at the start of that day in the shop's time
/// zone, not in UTC's.
#[test]
fn a_credit_expiring_on_a_day_expires_at_the_shops_midnight() {
    let mut shop = demo();
    let customer = shop.keys("Customer").next().unwrap().to_string();
    let credited = run(
        &mut shop,
        CREDIT,
        json!({"id": customer, "input": {"creditAmount": usd("5.00"), "expiresAt": "2099-01-01"}}),
    )["data"]["storeCreditAccountCredit"]
        .clone();
    assert_eq!(credited["userErrors"], json!([]), "{credited}");
    let zone = crate::search::Options::of(&shop);
    let midnight = zone.day_start(chrono::NaiveDate::from_ymd_opt(2099, 1, 1).unwrap());
    assert_eq!(
        credited["storeCreditAccountTransaction"]["expiresAt"],
        json!(crate::state::value::date_time(midnight))
    );
}
