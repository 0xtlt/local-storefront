//! The money of an order end to end, through `AdminApi::handle`: a B2B draft order with net 30
//! payment terms becomes an order with a payment schedule, which comes due when the shop's
//! clock passes it and is paid by a manual payment; a store credit account pays an order. Each
//! change is delivered as Shopify delivers it, signed, to a listener of the test.

mod support;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::time::Duration;

use lsf_admin::api::{AdminApi, AdminOptions, HttpRequest, ThrottleSetting};
use lsf_admin::auth::signing::verify_webhook_hmac;
use lsf_admin::webhooks::DeliveryOptions;
use serde_json::{Value, json};

const SECRET: &str = "test-secret";

/// The GraphQL path of a version lsf serves.
fn graphql_path() -> String {
    format!("/admin/api/{}/graphql.json", lsf_admin::version::SERVED[0])
}

fn api() -> AdminApi {
    AdminApi::new(
        &support::demo_store(),
        AdminOptions {
            app: None,
            client_secret: SECRET.to_string(),
            app_url: None,
            throttle: ThrottleSetting::Store,
            delivery: DeliveryOptions {
                timeout: Duration::from_secs(3),
                retry_delays: Vec::new(),
                ..DeliveryOptions::default()
            },
            token_seed: Some(7),
        },
    )
}

/// A request to the API.
fn request(api: &AdminApi, method: &str, path: &str, body: Value) -> (u16, Value) {
    let token = api.token(&api.default_shop());
    let response = api
        .handle(&HttpRequest {
            method: method.into(),
            path: path.into(),
            query: Vec::new(),
            headers: HashMap::from([
                ("content-type".to_string(), "application/json".to_string()),
                ("x-shopify-access-token".to_string(), token),
            ]),
            body: body.to_string().into_bytes(),
        })
        .expect("the path is routed");
    (
        response.status,
        serde_json::from_slice(&response.body).unwrap_or(Value::Null),
    )
}

/// The answer to a GraphQL request with the default token.
fn graphql(api: &AdminApi, query: &str, variables: Value) -> Value {
    let (status, answer) = request(
        api,
        "POST",
        &graphql_path(),
        json!({"query": query, "variables": variables}),
    );
    assert_eq!(status, 200);
    answer
}

/// A webhook receiver: an HTTP listener that answers `200` and hands over each request.
struct Receiver {
    url: String,
    requests: mpsc::Receiver<(HashMap<String, String>, Vec<u8>)>,
}

impl Receiver {
    fn start() -> Receiver {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (sender, requests) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut headers = HashMap::new();
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    let trimmed = line.trim_end();
                    if trimmed.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = trimmed.split_once(':') {
                        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
                    }
                }
                let length: usize = headers
                    .get("content-length")
                    .and_then(|length| length.parse().ok())
                    .unwrap_or(0);
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
                if sender.send((headers, body)).is_err() {
                    break;
                }
            }
        });
        Receiver { url, requests }
    }

    /// The next delivery: its topic and its payload, its signature checked.
    fn next(&self) -> (String, Value) {
        let (headers, body) = self
            .requests
            .recv_timeout(Duration::from_secs(10))
            .expect("a webhook was delivered");
        assert!(verify_webhook_hmac(
            SECRET,
            &body,
            &headers["x-shopify-hmac-sha256"]
        ));
        (
            headers["x-shopify-topic"].clone(),
            serde_json::from_slice(&body).unwrap(),
        )
    }

    /// The next `count` deliveries.
    fn take(&self, count: usize) -> Vec<(String, Value)> {
        (0..count).map(|_| self.next()).collect()
    }
}

fn subscribe(api: &AdminApi, receiver: &Receiver, topics: &[&str]) {
    for topic in topics {
        let answer = graphql(
            api,
            "mutation($topic: WebhookSubscriptionTopic!, $uri: String!) { webhookSubscriptionCreate(topic: $topic, webhookSubscription: {uri: $uri}) { userErrors { message } } }",
            json!({"topic": topic, "uri": format!("{}/webhooks", receiver.url)}),
        );
        assert_eq!(
            answer["data"]["webhookSubscriptionCreate"]["userErrors"],
            json!([]),
            "{answer}"
        );
    }
}

/// The cents of an amount (`"12.5"`, `"12.50"`).
fn cents(amount: &str) -> i64 {
    lsf_admin::domains::orders::model::parse_cents(amount).expect("an amount")
}

fn topics(deliveries: &[(String, Value)]) -> Vec<&str> {
    deliveries.iter().map(|(topic, _)| topic.as_str()).collect()
}

#[test]
fn a_b2b_order_on_net_terms_comes_due_and_is_paid() {
    let api = api();
    let receiver = Receiver::start();
    subscribe(
        &api,
        &receiver,
        &[
            "DRAFT_ORDERS_CREATE",
            "PAYMENT_TERMS_CREATE",
            "PAYMENT_TERMS_UPDATE",
            "ORDERS_CREATE",
            "PAYMENT_SCHEDULES_DUE",
            "ORDER_TRANSACTIONS_CREATE",
            "TENDER_TRANSACTIONS_CREATE",
            "ORDERS_PAID",
        ],
    );
    // The company of the demo store, its location and its contact.
    let company = graphql(
        &api,
        "{ companies(first: 1) { nodes { id locations(first: 1) { nodes { id } } contacts(first: 1) { nodes { id } } } } productVariants(first: 1) { nodes { id } } }",
        json!({}),
    );
    let data = &company["data"];
    let company = &data["companies"]["nodes"][0];
    let draft = graphql(
        &api,
        "mutation($input: DraftOrderInput!) { draftOrderCreate(input: $input) { draftOrder { id } userErrors { field message } } }",
        json!({"input": {
            "lineItems": [{"variantId": data["productVariants"]["nodes"][0]["id"], "quantity": 4}],
            "email": "buyer@northwind.example",
            "purchasingEntity": {"purchasingCompany": {
                "companyId": company["id"],
                "companyLocationId": company["locations"]["nodes"][0]["id"],
                "companyContactId": company["contacts"]["nodes"][0]["id"],
            }},
            "paymentTerms": {"paymentTermsTemplateId": "gid://shopify/PaymentTermsTemplate/4"},
        }}),
    );
    assert_eq!(
        draft["data"]["draftOrderCreate"]["userErrors"],
        json!([]),
        "{draft}"
    );
    let draft = draft["data"]["draftOrderCreate"]["draftOrder"]["id"].clone();
    let created = receiver.take(2);
    assert_eq!(
        topics(&created),
        ["draft_orders/create", "payment_terms/create"]
    );
    assert_eq!(
        created[0].1["payment_terms"]["payment_terms_name"],
        "Net 30"
    );
    assert_eq!(created[1].1["payment_terms_type"], "net");
    assert_eq!(created[1].1["due_in_days"], 30);

    let completed = graphql(
        &api,
        "mutation($id: ID!) { draftOrderComplete(id: $id) { draftOrder { order { id name displayFinancialStatus
            totalOutstandingSet { shopMoney { amount } }
            paymentTerms { paymentSchedules(first: 1) { nodes { id dueAt } } } } } userErrors { message } } }",
        json!({"id": draft}),
    );
    let order = completed["data"]["draftOrderComplete"]["draftOrder"]["order"].clone();
    assert_eq!(order["displayFinancialStatus"], "PENDING", "{completed}");
    let schedule = order["paymentTerms"]["paymentSchedules"]["nodes"][0].clone();
    let total = order["totalOutstandingSet"]["shopMoney"]["amount"]
        .as_str()
        .unwrap()
        .to_string();
    let made = receiver.take(2);
    assert_eq!(topics(&made), ["orders/create", "payment_terms/update"]);
    assert_eq!(
        made[0].1["payment_terms"]["payment_terms_name"], "Net 30",
        "the order's payload has its terms"
    );
    assert_eq!(
        made[1].1["payment_schedules"][0]["due_at"],
        schedule["dueAt"]
    );

    // A reminder before it is due: recorded in the outbox, nothing sent.
    let reminded = graphql(
        &api,
        "mutation($id: ID!) { paymentReminderSend(paymentScheduleId: $id) { success userErrors { message } } }",
        json!({"id": schedule["id"]}),
    );
    assert_eq!(reminded["data"]["paymentReminderSend"]["success"], true);
    let (status, emails) = request(&api, "GET", "/__lsf/admin/emails", Value::Null);
    assert_eq!(status, 200);
    assert_eq!(emails["count"], 1);
    assert_eq!(emails["emails"][0]["to"], "buyer@northwind.example");
    assert_eq!(emails["emails"][0]["what"], "payment_reminder");
    assert_eq!(emails["emails"][0]["channel"], "email");
    // The same entry in the outbox, and in the shop's status.
    let (_, outbox) = request(&api, "GET", "/__lsf/admin/outbox", Value::Null);
    assert_eq!(outbox["outbox"], emails["emails"]);
    let shop = api.default_shop();
    let status = api.status();
    let listed = status["shops"]
        .as_array()
        .unwrap()
        .iter()
        .find(|listed| listed["shop"] == json!(shop))
        .unwrap();
    assert_eq!(listed["outbox"], emails["emails"]);

    // Thirty-one days later, the schedule is due.
    let (status, clock) = request(
        &api,
        "POST",
        "/__lsf/admin/clock",
        json!({"advance": "31d"}),
    );
    assert_eq!(status, 200, "{clock}");
    assert_eq!(clock["frozen"], true);
    let (topic, due) = receiver.next();
    assert_eq!(topic, "payment_schedules/due");
    assert_eq!(due["admin_graphql_api_id"], schedule["id"]);
    assert_eq!(
        cents(due["balance_due"].as_str().unwrap()),
        cents(&total),
        "{due}"
    );
    let terms = graphql(
        &api,
        "query($id: ID!) { order(id: $id) { paymentTerms { due overdue } } }",
        json!({"id": order["id"]}),
    );
    assert_eq!(
        terms["data"]["order"]["paymentTerms"],
        json!({"due": true, "overdue": true})
    );

    // The buyer pays by bank transfer: a manual payment of the balance.
    let paid = graphql(
        &api,
        "mutation($id: ID!) { orderCreateManualPayment(id: $id, paymentMethodName: \"Bank Deposit\") {
            order { displayFinancialStatus totalOutstandingSet { shopMoney { amount } }
              paymentTerms { due paymentSchedules(first: 1) { nodes { completedAt balanceDue { amount } } } } }
            userErrors { field message code } } }",
        json!({"id": order["id"]}),
    );
    let paid = &paid["data"]["orderCreateManualPayment"];
    assert_eq!(paid["userErrors"], json!([]), "{paid}");
    assert_eq!(paid["order"]["displayFinancialStatus"], "PAID");
    assert_eq!(
        paid["order"]["totalOutstandingSet"]["shopMoney"]["amount"],
        "0.0"
    );
    assert_eq!(paid["order"]["paymentTerms"]["due"], false);
    let schedule = &paid["order"]["paymentTerms"]["paymentSchedules"]["nodes"][0];
    assert!(schedule["completedAt"].is_string());
    assert_eq!(schedule["balanceDue"]["amount"], "0.0");
    let payment = receiver.take(3);
    assert_eq!(
        topics(&payment),
        [
            "order_transactions/create",
            "tender_transactions/create",
            "orders/paid"
        ]
    );
    assert_eq!(
        cents(payment[1].1["amount"].as_str().unwrap()),
        cents(&total)
    );
    assert_eq!(payment[1].1["payment_method"], "Bank Deposit");
    assert_eq!(payment[2].1["financial_status"], "paid");
}

#[test]
fn an_order_is_paid_with_the_customers_store_credit() {
    let api = api();
    let receiver = Receiver::start();
    subscribe(
        &api,
        &receiver,
        &[
            "ORDERS_CREATE",
            "ORDER_TRANSACTIONS_CREATE",
            "TENDER_TRANSACTIONS_CREATE",
            "ORDERS_PAID",
        ],
    );
    let data = graphql(
        &api,
        "{ customers(first: 1) { nodes { id } } productVariants(first: 1) { nodes { id price } } }",
        json!({}),
    );
    let customer = data["data"]["customers"]["nodes"][0]["id"].clone();
    let variant = &data["data"]["productVariants"]["nodes"][0];
    let credited = graphql(
        &api,
        "mutation($id: ID!) { storeCreditAccountCredit(id: $id, creditInput: {creditAmount: {amount: \"200.00\", currencyCode: USD}}) {
            storeCreditAccountTransaction { account { id } } userErrors { message } } }",
        json!({"id": customer}),
    );
    let account = credited["data"]["storeCreditAccountCredit"]["storeCreditAccountTransaction"]
        ["account"]["id"]
        .clone();
    assert!(account.is_string(), "{credited}");
    let price = variant["price"].as_str().unwrap().to_string();
    let created = graphql(
        &api,
        "mutation($order: OrderCreateOrderInput!) { orderCreate(order: $order) { order { id displayFinancialStatus } userErrors { field message } } }",
        json!({"order": {
            "lineItems": [{"variantId": variant["id"], "quantity": 1}],
            "customer": {"toAssociate": {"id": customer}},
            "transactions": [{"kind": "SALE", "gateway": "shopify_store_credit",
                              "amountSet": {"shopMoney": {"amount": price, "currencyCode": "USD"}}}],
        }}),
    );
    let order = &created["data"]["orderCreate"]["order"];
    assert_eq!(order["displayFinancialStatus"], "PAID", "{created}");
    let deliveries = receiver.take(4);
    assert_eq!(
        topics(&deliveries),
        [
            "orders/create",
            "order_transactions/create",
            "tender_transactions/create",
            "orders/paid"
        ]
    );
    assert_eq!(deliveries[1].1["gateway"], "shopify_store_credit");
    assert_eq!(deliveries[2].1["payment_method"], "shopify_store_credit");
    assert_eq!(
        cents(deliveries[2].1["amount"].as_str().unwrap()),
        cents(&price)
    );
    let read = graphql(
        &api,
        "query($id: ID!) { storeCreditAccount(id: $id) { balance { amount }
            transactions(first: 5, query: \"type:debit\") { nodes { amount { amount } event origin { ... on OrderTransaction { order { id } } } } } } }",
        json!({"id": account}),
    );
    let read = &read["data"]["storeCreditAccount"];
    assert_eq!(
        cents(read["balance"]["amount"].as_str().unwrap()),
        20_000 - cents(&price)
    );
    let debit = &read["transactions"]["nodes"][0];
    assert_eq!(debit["event"], "ORDER_PAYMENT");
    assert_eq!(debit["origin"]["order"]["id"], order["id"]);
}

#[test]
fn what_buyers_and_shopify_do_to_an_order_is_delivered() {
    let api = api();
    let receiver = Receiver::start();
    subscribe(
        &api,
        &receiver,
        &[
            "ORDERS_RISK_ASSESSMENT_CHANGED",
            "ORDERS_SHOPIFY_PROTECT_ELIGIBILITY_CHANGED",
            "ORDERS_LINK_REQUESTED",
        ],
    );
    let orders = graphql(
        &api,
        "{ orders(first: 5, sortKey: ORDER_NUMBER) { nodes { id name displayFulfillmentStatus } } }",
        json!({}),
    );
    let order = orders["data"]["orders"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|order| order["displayFulfillmentStatus"] != "FULFILLED")
        .unwrap()["id"]
        .clone();
    let assessed = graphql(
        &api,
        "mutation($id: ID!) { orderRiskAssessmentCreate(orderRiskAssessmentInput: {orderId: $id, riskLevel: HIGH, facts: [{description: \"Card declined twice\", sentiment: NEGATIVE}]}) { userErrors { message } } }",
        json!({"id": order}),
    );
    assert_eq!(
        assessed["data"]["orderRiskAssessmentCreate"]["userErrors"],
        json!([])
    );
    let (topic, risk) = receiver.next();
    assert_eq!(topic, "orders/risk_assessment_changed");
    assert_eq!(risk["risk_level"], "high");
    assert_eq!(risk["admin_graphql_api_order_id"], order);
    assert_eq!(risk["provider_title"], "lsf");

    let (status, _) = request(
        &api,
        "POST",
        "/__lsf/admin/orders/protect",
        json!({"order": order, "status": "PROTECTED", "eligibility": "ELIGIBLE"}),
    );
    assert_eq!(status, 200);
    let (topic, protect) = receiver.next();
    assert_eq!(topic, "orders/shopify_protect_eligibility_changed");
    assert_eq!(protect["status"], "protected");
    assert_eq!(protect["eligibility"]["status"], "eligible");

    let (status, _) = request(
        &api,
        "POST",
        "/__lsf/admin/orders/link-request",
        json!({"order": order}),
    );
    assert_eq!(status, 200);
    let (topic, link) = receiver.next();
    assert_eq!(topic, "orders/link_requested");
    assert_eq!(link["admin_graphql_api_id"], order);
    let (_, emails) = request(&api, "GET", "/__lsf/admin/emails", Value::Null);
    assert_eq!(emails["emails"][0]["what"], "order_link_requested");
    let (status, refused) = request(
        &api,
        "POST",
        "/__lsf/admin/orders/protect",
        json!({"order": "gid://shopify/Order/1"}),
    );
    assert_eq!(status, 422, "{refused}");
}

#[test]
fn a_chargeback_is_opened_answered_and_won() {
    let api = api();
    let receiver = Receiver::start();
    subscribe(&api, &receiver, &["DISPUTES_CREATE", "DISPUTES_UPDATE"]);
    let orders = graphql(
        &api,
        "{ orders(first: 1, sortKey: ORDER_NUMBER) { nodes { id } } }",
        json!({}),
    );
    let order = orders["data"]["orders"]["nodes"][0]["id"].clone();
    let (status, opened) = request(
        &api,
        "POST",
        "/__lsf/admin/disputes",
        json!({"order": order, "amount": "11.50", "reason": "fraudulent"}),
    );
    assert_eq!(status, 201, "{opened}");
    let (topic, created) = receiver.next();
    assert_eq!(topic, "disputes/create");
    assert_eq!(created["type"], "chargeback");
    assert_eq!(created["amount"], "11.50");
    assert_eq!(created["reason"], "fraudulent");
    assert_eq!(created["status"], "needs_response");
    let dispute = graphql(
        &api,
        "query($id: ID!) { dispute(id: $id) { disputeEvidence { id } } }",
        json!({"id": opened["dispute"]}),
    );
    let evidence = dispute["data"]["dispute"]["disputeEvidence"]["id"].clone();
    let submitted = graphql(
        &api,
        "mutation($id: ID!) { disputeEvidenceUpdate(id: $id, input: {uncategorizedText: \"Delivered\", submitEvidence: true}) { disputeEvidence { submitted } userErrors { message } } }",
        json!({"id": evidence}),
    );
    assert_eq!(
        submitted["data"]["disputeEvidenceUpdate"]["disputeEvidence"]["submitted"],
        true
    );
    let (topic, update) = receiver.next();
    assert_eq!(topic, "disputes/update");
    assert_eq!(update["status"], "under_review");
    assert!(update["evidence_sent_on"].is_string());
    let number = opened["dispute"]
        .as_str()
        .unwrap()
        .rsplit('/')
        .next()
        .unwrap()
        .to_string();
    let (status, _) = request(
        &api,
        "POST",
        &format!("/__lsf/admin/disputes/{number}/won"),
        Value::Null,
    );
    assert_eq!(status, 200);
    let (topic, won) = receiver.next();
    assert_eq!(topic, "disputes/update");
    assert_eq!(won["status"], "won");
    assert!(won["finalized_on"].is_string());
    let (status, _) = request(
        &api,
        "POST",
        &format!("/__lsf/admin/disputes/{number}/lost"),
        Value::Null,
    );
    assert_eq!(status, 409);
}

/// Every mutation of the area is simulated: `GET /__lsf/admin` lists it.
#[test]
fn the_status_lists_every_mutation_of_payments_returns_and_drafts() {
    let api = api();
    let (status, answer) = request(&api, "GET", "/__lsf/admin", Value::Null);
    assert_eq!(status, 200);
    let listed: Vec<&str> = answer["mutations"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    for mutation in [
        "orderCreateMandatePayment",
        "orderCreateManualPayment",
        "orderRiskAssessmentCreate",
        "orderAttributionDefinitionDelete",
        "orderAttributionDefinitionUpsert",
        "paymentReminderSend",
        "paymentScheduleCapture",
        "paymentTermsCreate",
        "paymentTermsDelete",
        "paymentTermsUpdate",
        "orderEditAddShippingLine",
        "orderEditRemoveShippingLine",
        "orderEditUpdateDiscount",
        "orderEditUpdateShippingLine",
        "orderEditRemoveLineItemDiscount",
        "requestedOrderEditCreate",
        "requestedOrderEditDecline",
        "requestedOrderEditResolve",
        "removeFromReturn",
        "returnLineItemRemoveFromReturn",
        "returnRefund",
        "reverseDeliveryCreateWithShipping",
        "reverseDeliveryShippingUpdate",
        "reverseFulfillmentOrderDispose",
        "draftOrderBulkAddTags",
        "draftOrderBulkDelete",
        "draftOrderBulkRemoveTags",
        "draftOrderCreateFromOrder",
        "draftOrderInvoicePreview",
        "disputeEvidenceUpdate",
        "storeCreditAccountCredit",
        "storeCreditAccountDebit",
        "giftCardProductSet",
        "giftCardSendNotificationToCustomer",
        "giftCardSendNotificationToRecipient",
        "taxAppConfigure",
        "taxSummaryCreate",
        "shopifyPaymentsPayoutAlternateCurrencyCreate",
    ] {
        assert!(listed.contains(&mutation), "{mutation} is not simulated");
    }
}
