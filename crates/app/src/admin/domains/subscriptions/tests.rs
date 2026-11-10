//! The Subscriptions API through queries and mutations over the demo store, its clock frozen at
//! noon (UTC) on 2026-01-15 in the shop's New York time.
//!
//! Each mutation is tested once as it succeeds (its answer, its records, its events) and once as
//! it refuses its input, the shop then left exactly as it was ([`refused`]).

use chrono::{TimeZone, Utc};
use serde_json::{Value, json};

use super::payment_methods::vault_session;
use crate::admin::graphql::{self, Request};
use crate::admin::resolve::{Registry, ShopResolver};
use crate::admin::state::{Clock, ShopState, gid_number, value};

fn shop() -> ShopState {
    let (store, _) = lsf_core::store::load::load(
        &lsf_core::store::load::DataSource::Demo,
        &Default::default(),
    );
    let mut state = ShopState::seeded(&store, None);
    state.set_clock(Clock::Frozen(
        Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap(),
    ));
    state.take_events();
    state
}

fn at(shop: &mut ShopState, date: &str) {
    let date = chrono::DateTime::parse_from_rfc3339(date)
        .unwrap()
        .with_timezone(&Utc);
    shop.set_clock(Clock::Frozen(date));
}

/// Runs a request: its answer (`extensions` without `cost`).
fn run(shop: &mut ShopState, query: &str, variables: Value) -> Value {
    let mut resolver =
        ShopResolver::new(shop, Registry::standard()).with_origin("http://127.0.0.1:9292");
    let mut answer = graphql::run(
        crate::admin::schema::admin(),
        &mut resolver,
        &Request::new(query).with_variables(variables),
        None,
    );
    answer.extensions.extend(resolver.extensions());
    answer.extensions.remove("cost");
    let answer = answer.to_json();
    assert!(answer.get("errors").is_none(), "{answer:#}");
    answer
}

/// The `data` field of a mutation's answer.
fn data(answer: &Value, field: &str) -> Value {
    answer["data"][field].clone()
}

/// The topics emitted since the last call.
fn topics(shop: &mut ShopState) -> Vec<String> {
    shop.take_events()
        .into_iter()
        .map(|event| event.topic)
        .filter(|topic| topic != "lsf/changed")
        .collect()
}

/// Runs a mutation that must refuse its input: answers its user errors, after checking the shop
/// is exactly as it was (records, counters, events).
fn refused(shop: &mut ShopState, query: &str, variables: Value, field: &str) -> Value {
    shop.take_events();
    let before = shop.picture();
    let answer = run(shop, query, variables);
    let errors = answer["data"][field]["userErrors"].clone();
    assert!(
        errors.as_array().is_some_and(|errors| !errors.is_empty()),
        "{field} should refuse: {answer:#}"
    );
    assert_eq!(
        shop.picture(),
        before,
        "{field} changed the shop when it refused"
    );
    errors
}

fn customer(shop: &ShopState) -> String {
    shop.keys("Customer")
        .find(|key| shop.get(key).unwrap()["email"] == json!("jane.doe@example.com"))
        .unwrap()
        .to_string()
}

fn product(shop: &ShopState, handle: &str) -> String {
    shop.keys("Product")
        .find(|key| shop.get(key).unwrap()["handle"] == json!(handle))
        .unwrap()
        .to_string()
}

/// The first variant of the socks the demo store sells on subscription.
fn variant(shop: &ShopState) -> String {
    let product = product(shop, "merino-crew-socks");
    crate::admin::domains::catalog::gids(shop.get(&product).unwrap().get("variants"))[0].clone()
}

/// The demo store's monthly plan ("Deliver every month, 10% off").
fn monthly_plan(shop: &ShopState) -> String {
    shop.keys("SellingPlan")
        .find(|key| shop.get(key).unwrap()["name"] == json!("Deliver every month, 10% off"))
        .unwrap()
        .to_string()
}

fn session(number: &str) -> String {
    vault_session(&value::object(json!({
        "number": number, "first_name": "Jane", "last_name": "Doe", "month": 8, "year": 2030,
    })))
    .unwrap()
}

const CARD_CREATE: &str = r#"mutation($customer: ID!, $session: String!) {
  customerPaymentMethodCreditCardCreate(customerId: $customer, sessionId: $session, billingAddress: {address1: "12 Maple Street", city: "Portland", countryCode: US, provinceCode: "OR", zip: "97201"}) {
    customerPaymentMethod { id customer { id } revokedAt instrument { __typename ... on CustomerCreditCard { brand lastDigits firstDigits expiryMonth expiryYear maskedNumber name expiresSoon isRevocable billingAddress { city countryCode } } } }
    processing
    userErrors { field message }
  }
}"#;

/// A card of the customer, from a vault session of `number`.
fn card(shop: &mut ShopState, number: &str) -> String {
    let customer = customer(shop);
    let answer = run(
        shop,
        CARD_CREATE,
        json!({"customer": customer, "session": session(number)}),
    );
    data(&answer, "customerPaymentMethodCreditCardCreate")["customerPaymentMethod"]["id"]
        .as_str()
        .unwrap()
        .to_string()
}

const ATOMIC: &str = r#"mutation($input: SubscriptionContractAtomicCreateInput!) {
  subscriptionContractAtomicCreate(input: $input) {
    contract { id status revisionId nextBillingDate createdAt currencyCode customer { id } customerPaymentMethod { id }
      billingPolicy { interval intervalCount minCycles maxCycles cadence { unit count } }
      deliveryPolicy { interval intervalCount }
      deliveryPrice { amount }
      deliveryMethod { __typename ... on SubscriptionDeliveryMethodShipping { address { city } shippingOption { title code } } }
      lines(first: 5) { nodes { id title variantId productId quantity currentPrice { amount } lineDiscountedPrice { amount } sellingPlanId sellingPlanName discountAllocations { amount { amount } } } }
      linesCount { count } lineCount
      discounts(first: 5) { nodes { id title type targetType value { __typename ... on SubscriptionDiscountPercentageValue { percentage } } } }
      latestCommittedProjectedOrderTotals { subtotal { amount } totalDelivery { amount } total { amount } }
    }
    userErrors { field message code }
  }
}"#;

/// The input of an atomic creation: two pairs of socks, monthly, the card given.
fn atomic_input(shop: &ShopState, method: Option<&str>) -> Value {
    json!({
        "customerId": customer(shop),
        "currencyCode": "USD",
        "nextBillingDate": "2026-02-15T12:00:00Z",
        "contract": {
            "status": "ACTIVE",
            "paymentMethodId": method,
            "billingPolicy": {"interval": "MONTH", "intervalCount": 1, "minCycles": 1, "maxCycles": 12},
            "deliveryPolicy": {"interval": "MONTH", "intervalCount": 1},
            "deliveryPrice": "5.00",
            "deliveryMethod": {"shipping": {
                "address": {"firstName": "Jane", "lastName": "Doe", "address1": "12 Maple Street", "city": "Portland", "provinceCode": "OR", "countryCode": "US", "zip": "97201"},
                "shippingOption": {"title": "Standard", "code": "standard"},
            }},
        },
        "lines": [{
            "line": {"productVariantId": variant(shop), "quantity": 2, "currentPrice": "10.00", "sellingPlanId": monthly_plan(shop)},
            "discounts": [],
        }],
    })
}

/// A contract of the customer, made with `subscriptionContractAtomicCreate`.
fn contract(shop: &mut ShopState, method: Option<&str>) -> String {
    let input = atomic_input(shop, method);
    let answer = run(shop, ATOMIC, json!({"input": input}));
    let created = data(&answer, "subscriptionContractAtomicCreate");
    assert_eq!(created["userErrors"], json!([]), "{created:#}");
    created["contract"]["id"].as_str().unwrap().to_string()
}

// ---- payment methods ------------------------------------------------------------------------

#[test]
fn a_card_from_a_vault_session_is_a_payment_method_of_the_customer() {
    let mut shop = shop();
    let customer = customer(&shop);
    let answer = run(
        &mut shop,
        CARD_CREATE,
        json!({"customer": customer, "session": session("4242 4242 4242 4242")}),
    );
    let created = data(&answer, "customerPaymentMethodCreditCardCreate");
    assert_eq!(created["processing"], json!(false));
    let method = &created["customerPaymentMethod"];
    assert!(
        method["id"]
            .as_str()
            .unwrap()
            .starts_with("gid://shopify/CustomerPaymentMethod/")
    );
    assert_eq!(
        method["id"]
            .as_str()
            .unwrap()
            .rsplit('/')
            .next()
            .unwrap()
            .len(),
        32
    );
    assert_eq!(method["customer"]["id"], json!(customer));
    assert_eq!(
        method["instrument"],
        json!({"__typename": "CustomerCreditCard", "brand": "Visa", "lastDigits": "4242", "firstDigits": "424242", "expiryMonth": 8, "expiryYear": 2030, "maskedNumber": "•••• •••• •••• 4242", "name": "Jane Doe", "expiresSoon": false, "isRevocable": true, "billingAddress": {"city": "Portland", "countryCode": "US"}})
    );
    assert_eq!(topics(&mut shop), ["customer_payment_methods/create"]);

    // A session lsf's vault did not make, a customer that is not one: refused.
    let errors = refused(
        &mut shop,
        CARD_CREATE,
        json!({"customer": customer, "session": "east-123"}),
        "customerPaymentMethodCreditCardCreate",
    );
    assert_eq!(errors[0]["field"], json!(["sessionId"]));
    let errors = refused(
        &mut shop,
        CARD_CREATE,
        json!({"customer": "gid://shopify/Customer/1", "session": session("4242424242424242")}),
        "customerPaymentMethodCreditCardCreate",
    );
    assert_eq!(
        errors,
        json!([{"field": ["customerId"], "message": "Customer does not exist"}])
    );
}

#[test]
fn the_vault_reads_the_brand_and_what_a_charge_does_from_the_number() {
    assert_eq!(
        super::payment_methods::brand("5555555555554444"),
        "Mastercard"
    );
    assert_eq!(
        super::payment_methods::brand("378282246310005"),
        "American Express"
    );
    assert_eq!(
        super::payment_methods::brand("6011111111111117"),
        "Discover"
    );
    assert_eq!(super::payment_methods::brand("1"), "Bogus");
    assert_eq!(
        super::payment_methods::card_outcome("4000000000000002"),
        "CARD_DECLINED"
    );
    assert_eq!(
        super::payment_methods::card_outcome("4000000000003220"),
        "challenge"
    );
    assert_eq!(super::payment_methods::card_outcome("2"), "CARD_DECLINED");
    assert_eq!(
        super::payment_methods::card_outcome("4242424242424242"),
        "success"
    );
    assert_eq!(
        super::payment_methods::remote_outcome("pm_card_chargeDeclinedInsufficientFunds"),
        "INSUFFICIENT_FUNDS"
    );
    assert_eq!(
        super::payment_methods::remote_outcome("pm_card_authenticationRequired"),
        "challenge"
    );
    assert_eq!(
        super::payment_methods::remote_outcome("pm_card_visa"),
        "success"
    );
    assert!(
        vault_session(&value::object(
            json!({"number": "4242", "month": 13, "year": 2030})
        ))
        .is_err()
    );
}

const CARD_UPDATE: &str = r#"mutation($id: ID!, $session: String!) {
  customerPaymentMethodCreditCardUpdate(id: $id, sessionId: $session, billingAddress: {city: "Ottawa", countryCode: CA}) {
    customerPaymentMethod { id instrument { ... on CustomerCreditCard { brand lastDigits billingAddress { city } } } }
    userErrors { field message }
  }
}"#;

#[test]
fn a_card_is_replaced_by_the_card_of_a_new_session() {
    let mut shop = shop();
    let method = card(&mut shop, "4242424242424242");
    shop.take_events();
    let answer = run(
        &mut shop,
        CARD_UPDATE,
        json!({"id": method, "session": session("5555555555554444")}),
    );
    let updated = data(&answer, "customerPaymentMethodCreditCardUpdate");
    assert_eq!(
        updated["customerPaymentMethod"]["instrument"],
        json!({"brand": "Mastercard", "lastDigits": "4444", "billingAddress": {"city": "Ottawa"}})
    );
    assert_eq!(topics(&mut shop), ["customer_payment_methods/update"]);
    let errors = refused(
        &mut shop,
        CARD_UPDATE,
        json!({"id": "gid://shopify/CustomerPaymentMethod/0eccccc666aac73efcd31094ddc4ebf0", "session": session("4242424242424242")}),
        "customerPaymentMethodCreditCardUpdate",
    );
    assert_eq!(
        errors[0]["message"],
        json!("Customer payment method does not exist")
    );
}

const PAYPAL_CREATE: &str = r#"mutation($customer: ID!, $agreement: String!) {
  customerPaymentMethodPaypalBillingAgreementCreate(customerId: $customer, billingAgreementId: $agreement, billingAddress: {firstName: "Jane", lastName: "Doe", city: "Portland", countryCode: US}) {
    customerPaymentMethod { id instrument { __typename ... on CustomerPaypalBillingAgreement { paypalAccountEmail inactive isRevocable billingAddress { name city } } } }
    userErrors { field message code }
  }
}"#;

const PAYPAL_UPDATE: &str = r#"mutation($id: ID!) {
  customerPaymentMethodPaypalBillingAgreementUpdate(id: $id, billingAddress: {firstName: "Jane", lastName: "Doe", city: "Seattle", countryCode: US}) {
    customerPaymentMethod { id instrument { ... on CustomerPaypalBillingAgreement { billingAddress { city } } } }
    userErrors { field message code }
  }
}"#;

#[test]
fn a_paypal_billing_agreement_is_vaulted_and_its_address_updated() {
    let mut shop = shop();
    let customer = customer(&shop);
    let answer = run(
        &mut shop,
        PAYPAL_CREATE,
        json!({"customer": customer, "agreement": "B-12345678901234567"}),
    );
    let created = data(&answer, "customerPaymentMethodPaypalBillingAgreementCreate");
    assert_eq!(
        created["customerPaymentMethod"]["instrument"],
        json!({"__typename": "CustomerPaypalBillingAgreement", "paypalAccountEmail": "jane.doe@example.com", "inactive": false, "isRevocable": true, "billingAddress": {"name": "Jane Doe", "city": "Portland"}})
    );
    assert_eq!(topics(&mut shop), ["customer_payment_methods/create"]);
    let errors = refused(
        &mut shop,
        PAYPAL_CREATE,
        json!({"customer": customer, "agreement": "12345"}),
        "customerPaymentMethodPaypalBillingAgreementCreate",
    );
    assert_eq!(errors[0]["code"], json!("INVALID"));
    let errors = refused(
        &mut shop,
        PAYPAL_CREATE,
        json!({"customer": customer, "agreement": "B-12345678901234567"}),
        "customerPaymentMethodPaypalBillingAgreementCreate",
    );
    assert_eq!(errors[0]["code"], json!("TAKEN"));

    let method = created["customerPaymentMethod"]["id"].clone();
    let answer = run(&mut shop, PAYPAL_UPDATE, json!({"id": method}));
    assert_eq!(
        data(&answer, "customerPaymentMethodPaypalBillingAgreementUpdate")["customerPaymentMethod"]
            ["instrument"]["billingAddress"]["city"],
        json!("Seattle")
    );
    assert_eq!(topics(&mut shop), ["customer_payment_methods/update"]);
    let card = card(&mut shop, "4242424242424242");
    let errors = refused(
        &mut shop,
        PAYPAL_UPDATE,
        json!({"id": card}),
        "customerPaymentMethodPaypalBillingAgreementUpdate",
    );
    assert_eq!(errors[0]["field"], json!(["id"]));
}

const REMOTE: &str = r#"mutation($customer: ID!, $remote: CustomerPaymentMethodRemoteInput!) {
  customerPaymentMethodRemoteCreate(customerId: $customer, remoteReference: $remote) {
    customerPaymentMethod { id instrument { ... on CustomerCreditCard { brand lastDigits } } }
    userErrors { field message code }
  }
}"#;

#[test]
fn a_remote_method_is_imported_at_once_with_its_test_outcome() {
    let mut shop = shop();
    let customer = customer(&shop);
    let answer = run(
        &mut shop,
        REMOTE,
        json!({"customer": customer, "remote": {"stripePaymentMethod": {"customerId": "cus_1", "paymentMethodId": "pm_card_chargeDeclined"}}}),
    );
    let created = data(&answer, "customerPaymentMethodRemoteCreate");
    assert_eq!(
        created["customerPaymentMethod"]["instrument"],
        json!({"brand": "Visa", "lastDigits": "4242"})
    );
    let id = created["customerPaymentMethod"]["id"].as_str().unwrap();
    assert_eq!(shop.get(id).unwrap()["lsfOutcome"], json!("CARD_DECLINED"));
    assert_eq!(topics(&mut shop), ["customer_payment_methods/create"]);
    let errors = refused(
        &mut shop,
        REMOTE,
        json!({"customer": customer, "remote": {"stripePaymentMethod": {"customerId": "cus_1", "paymentMethodId": "pm_1"}, "braintreePaymentMethod": {"customerId": "1", "paymentMethodToken": "t"}}}),
        "customerPaymentMethodRemoteCreate",
    );
    assert_eq!(
        errors,
        json!([{"field": ["remoteReference"], "message": "Exactly one remote reference is required.", "code": "EXACTLY_ONE_REMOTE_REFERENCE_REQUIRED"}])
    );
}

const REVOKE: &str = r#"mutation($id: ID!) { customerPaymentMethodRevoke(customerPaymentMethodId: $id) { revokedCustomerPaymentMethodId userErrors { field message } } }"#;

#[test]
fn a_method_is_revoked_unless_an_active_contract_uses_it() {
    let mut shop = shop();
    let method = card(&mut shop, "4242424242424242");
    let used = contract(&mut shop, Some(&method));
    let errors = refused(
        &mut shop,
        REVOKE,
        json!({"id": method}),
        "customerPaymentMethodRevoke",
    );
    assert_eq!(errors[0]["field"], json!(["customerPaymentMethodId"]));

    run(
        &mut shop,
        "mutation($id: ID!) { subscriptionContractCancel(subscriptionContractId: $id) { userErrors { message } } }",
        json!({"id": used}),
    );
    shop.take_events();
    let answer = run(&mut shop, REVOKE, json!({"id": method}));
    assert_eq!(
        data(&answer, "customerPaymentMethodRevoke")["revokedCustomerPaymentMethodId"],
        json!(method)
    );
    assert_eq!(topics(&mut shop), ["customer_payment_methods/revoke"]);
    let customer = customer(&shop);
    let read = run(
        &mut shop,
        r#"query($id: ID!, $customer: ID!) {
          hidden: customerPaymentMethod(id: $id) { id }
          shown: customerPaymentMethod(id: $id, showRevoked: true) { revokedAt revokedReason }
          customer(id: $customer) { paymentMethods(first: 5) { nodes { id } } all: paymentMethods(first: 5, showRevoked: true) { nodes { id } } }
        }"#,
        json!({"id": method, "customer": customer}),
    );
    assert_eq!(read["data"]["hidden"], Value::Null);
    assert_eq!(
        read["data"]["shown"],
        json!({"revokedAt": "2026-01-15T12:00:00Z", "revokedReason": "MANUALLY_REVOKED"})
    );
    assert_eq!(
        read["data"]["customer"]["paymentMethods"]["nodes"],
        json!([])
    );
    assert_eq!(
        read["data"]["customer"]["all"]["nodes"],
        json!([{"id": method}])
    );
}

#[test]
fn the_update_url_is_for_shop_pay_and_the_emails_are_logged() {
    let mut shop = shop();
    let customer = customer(&shop);
    let card = card(&mut shop, "4242424242424242");
    let url = r#"mutation($id: ID!) { customerPaymentMethodGetUpdateUrl(customerPaymentMethodId: $id) { updatePaymentMethodUrl userErrors { field message code } } }"#;
    let errors = refused(
        &mut shop,
        url,
        json!({"id": card}),
        "customerPaymentMethodGetUpdateUrl",
    );
    assert_eq!(errors[0]["code"], json!("INVALID_INSTRUMENT"));
    let shop_pay = super::payment_methods::create_method(
        &mut shop,
        &customer,
        |method| json!({"__typename": "CustomerShopPayAgreement", "lastDigits": "4242", "expiryMonth": 1, "expiryYear": 2026, "maskedNumber": "•••• •••• •••• 4242", "name": "Jane Doe", "inactive": false, "lsfMethod": method}),
        "success",
    );
    shop.take_events();
    let answer = run(&mut shop, url, json!({"id": shop_pay}));
    let token = shop_pay.rsplit('/').next().unwrap();
    assert_eq!(
        data(&answer, "customerPaymentMethodGetUpdateUrl")["updatePaymentMethodUrl"],
        json!(format!(
            "http://127.0.0.1:9292/__lsf/admin/payment-methods/{token}/update?shop=local-supply-co.myshopify.com"
        ))
    );
    let expires = run(
        &mut shop,
        "query($id: ID!) { customerPaymentMethod(id: $id) { instrument { ... on CustomerShopPayAgreement { expiresSoon } } } }",
        json!({"id": shop_pay}),
    );
    assert_eq!(
        expires["data"]["customerPaymentMethod"]["instrument"]["expiresSoon"],
        json!(true)
    );

    let send = r#"mutation($id: ID!) { customerPaymentMethodSendUpdateEmail(customerPaymentMethodId: $id, email: {subject: "Card expiring"}) { customer { id } userErrors { field message } } }"#;
    let answer = run(&mut shop, send, json!({"id": card}));
    assert_eq!(
        data(&answer, "customerPaymentMethodSendUpdateEmail")["customer"]["id"],
        json!(customer)
    );
    let emails = super::payment_methods::emails(&shop);
    assert_eq!(emails[0]["to"], json!("jane.doe@example.com"));
    assert_eq!(emails[0]["subject"], json!("Card expiring"));
    assert_eq!(emails[0]["what"], json!("customer_payment_method_update"));
    assert_eq!(emails[0]["about"], json!(card));
    let errors = refused(
        &mut shop,
        send,
        json!({"id": "gid://shopify/CustomerPaymentMethod/x"}),
        "customerPaymentMethodSendUpdateEmail",
    );
    assert_eq!(errors[0]["field"], json!(["customerPaymentMethodId"]));

    let used = contract(&mut shop, None);
    let add = r#"mutation($id: String!, $type: PaymentInstrumentMandateResourceType!) { paymentInstrumentSendAddEmail(mandate: {resourceId: $id, resourceType: $type}) { customer { id } userErrors { field message } } }"#;
    let answer = run(&mut shop, add, json!({"id": used, "type": "SUBSCRIPTIONS"}));
    assert_eq!(
        data(&answer, "paymentInstrumentSendAddEmail")["customer"]["id"],
        json!(customer)
    );
    assert_eq!(
        super::payment_methods::emails(&shop)[1]["about"],
        json!(used)
    );
    let errors = refused(
        &mut shop,
        add,
        json!({"id": used, "type": "ORDERS"}),
        "paymentInstrumentSendAddEmail",
    );
    assert_eq!(errors[0]["field"], json!(["mandate", "resourceId"]));
}

// ---- the draft API --------------------------------------------------------------------------

const CONTRACT_CREATE: &str = r#"mutation($input: SubscriptionContractCreateInput!) {
  subscriptionContractCreate(input: $input) { draft { id status customer { id } nextBillingDate billingPolicy { interval intervalCount } lines(first: 5) { nodes { id } } } userErrors { field message code } }
}"#;

fn draft(shop: &mut ShopState) -> String {
    let customer = customer(shop);
    let answer = run(
        shop,
        CONTRACT_CREATE,
        json!({"input": {
            "customerId": customer,
            "currencyCode": "USD",
            "nextBillingDate": "2026-02-15T12:00:00Z",
            "contract": {
                "billingPolicy": {"interval": "WEEK", "intervalCount": 2},
                "deliveryPolicy": {"interval": "WEEK", "intervalCount": 2},
                "deliveryMethod": {"shipping": {"address": {"address1": "12 Maple Street", "city": "Portland", "countryCode": "US", "zip": "97201"}, "shippingOption": {"title": "Standard"}}},
            },
        }}),
    );
    let created = data(&answer, "subscriptionContractCreate");
    assert_eq!(created["userErrors"], json!([]), "{created:#}");
    created["draft"]["id"].as_str().unwrap().to_string()
}

const LINE_ADD: &str = r#"mutation($draft: ID!, $input: SubscriptionLineInput!) {
  subscriptionDraftLineAdd(draftId: $draft, input: $input) { lineAdded { id title variantTitle quantity currentPrice { amount } lineDiscountedPrice { amount } sellingPlanName pricingPolicy { basePrice { amount } cycleDiscounts { afterCycle computedPrice { amount } } } } draft { linesAdded(first: 5) { nodes { id } } } userErrors { field message code } }
}"#;

const COMMIT: &str = r#"mutation($draft: ID!) { subscriptionDraftCommit(draftId: $draft) { contract { id status revisionId lines(first: 5) { nodes { id quantity currentPrice { amount } } } discounts(first: 5) { nodes { id type title } } } userErrors { field message code } } }"#;

#[test]
fn a_draft_is_made_filled_and_committed_into_a_contract() {
    let mut shop = shop();
    let errors = refused(
        &mut shop,
        CONTRACT_CREATE,
        json!({"input": {"customerId": "gid://shopify/Customer/1", "currencyCode": "USD", "nextBillingDate": "2026-02-15T12:00:00Z", "contract": {}}}),
        "subscriptionContractCreate",
    );
    assert_eq!(
        errors,
        json!([{"field": ["input", "customerId"], "message": "The customer doesn't exist.", "code": "CUSTOMER_DOES_NOT_EXIST"}])
    );
    let draft = draft(&mut shop);
    assert_eq!(topics(&mut shop), Vec::<String>::new());

    // A draft without a line does not commit.
    let errors = refused(
        &mut shop,
        COMMIT,
        json!({"draft": draft}),
        "subscriptionDraftCommit",
    );
    assert_eq!(errors[0]["code"], json!("INVALID_LINES"));

    let variant = variant(&shop);
    let plan = monthly_plan(&shop);
    let answer = run(
        &mut shop,
        LINE_ADD,
        json!({"draft": draft, "input": {"productVariantId": variant, "quantity": 3, "currentPrice": "9.00", "sellingPlanId": plan,
            "pricingPolicy": {"basePrice": "10.00", "cycleDiscounts": [{"adjustmentType": "PERCENTAGE", "adjustmentValue": {"percentage": 10}, "afterCycle": 0, "computedPrice": "9.00"}]}}}),
    );
    let added = data(&answer, "subscriptionDraftLineAdd");
    let line = &added["lineAdded"];
    assert!(
        line["id"]
            .as_str()
            .unwrap()
            .starts_with("gid://shopify/SubscriptionLine/")
    );
    assert_eq!(line["title"], json!("Merino Crew Socks"));
    assert_eq!(line["currentPrice"]["amount"], json!("9.0"));
    assert_eq!(line["lineDiscountedPrice"]["amount"], json!("27.0"));
    assert_eq!(
        line["sellingPlanName"],
        json!("Deliver every month, 10% off")
    );
    assert_eq!(
        line["pricingPolicy"],
        json!({"basePrice": {"amount": "10.0"}, "cycleDiscounts": [{"afterCycle": 0, "computedPrice": {"amount": "9.0"}}]})
    );
    assert_eq!(
        added["draft"]["linesAdded"]["nodes"],
        json!([{"id": line["id"]}])
    );
    let errors = refused(
        &mut shop,
        LINE_ADD,
        json!({"draft": draft, "input": {"productVariantId": "gid://shopify/ProductVariant/1", "quantity": 0, "currentPrice": "1"}}),
        "subscriptionDraftLineAdd",
    );
    assert_eq!(errors[0]["field"], json!(["input", "productVariantId"]));
    assert_eq!(errors[1]["code"], json!("GREATER_THAN_OR_EQUAL_TO"));

    let answer = run(&mut shop, COMMIT, json!({"draft": draft}));
    let committed = data(&answer, "subscriptionDraftCommit");
    assert_eq!(committed["contract"]["status"], json!("ACTIVE"));
    assert_eq!(committed["contract"]["revisionId"], json!("1"));
    assert_eq!(topics(&mut shop), ["subscription_contracts/create"]);
    // Committed once.
    let errors = refused(
        &mut shop,
        COMMIT,
        json!({"draft": draft}),
        "subscriptionDraftCommit",
    );
    assert_eq!(errors[0]["code"], json!("COMMITTED"));
}

#[test]
fn the_lines_and_discounts_of_a_draft_change_until_it_commits() {
    let mut shop = shop();
    let contract_id = contract(&mut shop, None);
    let answer = run(
        &mut shop,
        "mutation($id: ID!) { subscriptionContractUpdate(contractId: $id) { draft { id originalContract { id } } userErrors { message } } }",
        json!({"id": contract_id}),
    );
    let draft = data(&answer, "subscriptionContractUpdate")["draft"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let errors = refused(
        &mut shop,
        "mutation($id: ID!) { subscriptionContractUpdate(contractId: $id) { draft { id } userErrors { field message code } } }",
        json!({"id": "gid://shopify/SubscriptionContract/999"}),
        "subscriptionContractUpdate",
    );
    assert_eq!(errors[0]["field"], json!(["contractId"]));
    let line = shop.get(&draft).unwrap()["lines"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // A line updated: a patch.
    let update = r#"mutation($draft: ID!, $line: ID!, $input: SubscriptionLineUpdateInput!) { subscriptionDraftLineUpdate(draftId: $draft, lineId: $line, input: $input) { lineUpdated { quantity currentPrice { amount } } userErrors { field message code } } }"#;
    let answer = run(
        &mut shop,
        update,
        json!({"draft": draft, "line": line, "input": {"quantity": 4}}),
    );
    assert_eq!(
        data(&answer, "subscriptionDraftLineUpdate")["lineUpdated"],
        json!({"quantity": 4, "currentPrice": {"amount": "10.0"}})
    );
    let errors = refused(
        &mut shop,
        update,
        json!({"draft": draft, "line": "gid://shopify/SubscriptionLine/x", "input": {"quantity": 4}}),
        "subscriptionDraftLineUpdate",
    );
    assert_eq!(errors[0]["field"], json!(["lineId"]));

    // A discount of 25% on that line, then updated to 50%, a free shipping discount, then both
    // removed and added again.
    let add = r#"mutation($draft: ID!, $input: SubscriptionManualDiscountInput!) { subscriptionDraftDiscountAdd(draftId: $draft, input: $input) { discountAdded { id title type targetType entitledLines { all lines(first: 5) { nodes { id } } } value { ... on SubscriptionDiscountPercentageValue { percentage } } } draft { lines(first: 1) { nodes { lineDiscountedPrice { amount } discountAllocations { amount { amount } } } } } userErrors { field message code } } }"#;
    let answer = run(
        &mut shop,
        add,
        json!({"draft": draft, "input": {"title": "Loyalty", "value": {"percentage": 25}, "entitledLines": {"lines": {"add": [line]}}}}),
    );
    let added = data(&answer, "subscriptionDraftDiscountAdd");
    assert_eq!(added["discountAdded"]["type"], json!("MANUAL"));
    assert_eq!(
        added["discountAdded"]["entitledLines"],
        json!({"all": false, "lines": {"nodes": [{"id": line}]}})
    );
    assert_eq!(
        added["draft"]["lines"]["nodes"][0],
        json!({"lineDiscountedPrice": {"amount": "30.0"}, "discountAllocations": [{"amount": {"amount": "10.0"}}]})
    );
    let discount = added["discountAdded"]["id"].as_str().unwrap().to_string();
    let errors = refused(
        &mut shop,
        add,
        json!({"draft": draft, "input": {"title": "None", "entitledLines": {"all": false}}}),
        "subscriptionDraftDiscountAdd",
    );
    assert_eq!(
        errors
            .as_array()
            .unwrap()
            .iter()
            .map(|error| error["code"].clone())
            .collect::<Vec<_>>(),
        [json!("PRESENCE"), json!("NO_ENTITLED_LINES")]
    );

    let discount_update = r#"mutation($draft: ID!, $discount: ID!, $input: SubscriptionManualDiscountInput!) { subscriptionDraftDiscountUpdate(draftId: $draft, discountId: $discount, input: $input) { discountUpdated { value { ... on SubscriptionDiscountPercentageValue { percentage } } } draft { discountsUpdated(first: 5) { nodes { ... on SubscriptionManualDiscount { id } } } } userErrors { field message code } } }"#;
    let answer = run(
        &mut shop,
        discount_update,
        json!({"draft": draft, "discount": discount, "input": {"value": {"percentage": 50}}}),
    );
    assert_eq!(
        data(&answer, "subscriptionDraftDiscountUpdate")["discountUpdated"]["value"]["percentage"],
        json!(50)
    );
    let errors = refused(
        &mut shop,
        discount_update,
        json!({"draft": draft, "discount": "gid://shopify/SubscriptionManualDiscount/x", "input": {"title": "x"}}),
        "subscriptionDraftDiscountUpdate",
    );
    assert_eq!(errors[0]["field"], json!(["discountId"]));

    let free = r#"mutation($draft: ID!) { subscriptionDraftFreeShippingDiscountAdd(draftId: $draft, input: {title: "Free shipping", recurringCycleLimit: 3}) { discountAdded { id targetType recurringCycleLimit } userErrors { field message code } } }"#;
    let answer = run(&mut shop, free, json!({"draft": draft}));
    let shipping =
        data(&answer, "subscriptionDraftFreeShippingDiscountAdd")["discountAdded"].clone();
    assert_eq!(shipping["targetType"], json!("SHIPPING_LINE"));
    let free_update = r#"mutation($draft: ID!, $discount: ID!) { subscriptionDraftFreeShippingDiscountUpdate(draftId: $draft, discountId: $discount, input: {recurringCycleLimit: 6}) { discountUpdated { recurringCycleLimit } userErrors { field message code } } }"#;
    let answer = run(
        &mut shop,
        free_update,
        json!({"draft": draft, "discount": shipping["id"]}),
    );
    assert_eq!(
        data(&answer, "subscriptionDraftFreeShippingDiscountUpdate")["discountUpdated"]["recurringCycleLimit"],
        json!(6)
    );
    let errors = refused(
        &mut shop,
        free_update,
        json!({"draft": draft, "discount": discount}),
        "subscriptionDraftFreeShippingDiscountUpdate",
    );
    assert_eq!(errors[0]["field"], json!(["discountId"]));

    let remove = r#"mutation($draft: ID!, $discount: ID!) { subscriptionDraftDiscountRemove(draftId: $draft, discountId: $discount) { discountRemoved { ... on SubscriptionManualDiscount { id } } userErrors { field message code } } }"#;
    let answer = run(
        &mut shop,
        remove,
        json!({"draft": draft, "discount": shipping["id"]}),
    );
    assert_eq!(
        data(&answer, "subscriptionDraftDiscountRemove")["discountRemoved"]["id"],
        shipping["id"]
    );
    let errors = refused(
        &mut shop,
        remove,
        json!({"draft": draft, "discount": shipping["id"]}),
        "subscriptionDraftDiscountRemove",
    );
    assert_eq!(errors[0]["field"], json!(["discountId"]));

    // A code that is not one of the shop's is kept, rejected.
    let code = r#"mutation($draft: ID!, $code: String!) { subscriptionDraftDiscountCodeApply(draftId: $draft, redeemCode: $code) { appliedDiscount { redeemCode rejectionReason } userErrors { field message code } } }"#;
    let answer = run(&mut shop, code, json!({"draft": draft, "code": "NOPE"}));
    assert_eq!(
        data(&answer, "subscriptionDraftDiscountCodeApply")["appliedDiscount"],
        json!({"redeemCode": "NOPE", "rejectionReason": "NOT_FOUND"})
    );
    let errors = refused(
        &mut shop,
        code,
        json!({"draft": draft, "code": " "}),
        "subscriptionDraftDiscountCodeApply",
    );
    assert_eq!(errors[0]["code"], json!("BLANK"));

    // The line removed: the discount that listed it no longer does.
    let line_remove = r#"mutation($draft: ID!, $line: ID!) { subscriptionDraftLineRemove(draftId: $draft, lineId: $line) { lineRemoved { id } discountsUpdated { id entitledLines { all } } draft { linesRemoved(first: 5) { nodes { id } } } userErrors { field message code } } }"#;
    let answer = run(
        &mut shop,
        line_remove,
        json!({"draft": draft, "line": line}),
    );
    let removed = data(&answer, "subscriptionDraftLineRemove");
    assert_eq!(removed["lineRemoved"]["id"], json!(line));
    assert_eq!(removed["discountsUpdated"][0]["id"], json!(discount));
    assert_eq!(
        removed["draft"]["linesRemoved"]["nodes"],
        json!([{"id": line}])
    );
    let errors = refused(
        &mut shop,
        line_remove,
        json!({"draft": draft, "line": line}),
        "subscriptionDraftLineRemove",
    );
    assert_eq!(errors[0]["code"], json!("ALREADY_REMOVED"));

    // The update draft's other fields; then committed into the contract, a new revision.
    let draft_update = r#"mutation($draft: ID!, $input: SubscriptionDraftInput!) { subscriptionDraftUpdate(draftId: $draft, input: $input) { draft { note deliveryPrice { amount } } userErrors { field message code } } }"#;
    let answer = run(
        &mut shop,
        draft_update,
        json!({"draft": draft, "input": {"note": "Leave at the door", "deliveryPrice": "3.50"}}),
    );
    assert_eq!(
        data(&answer, "subscriptionDraftUpdate")["draft"],
        json!({"note": "Leave at the door", "deliveryPrice": {"amount": "3.5"}})
    );
    let errors = refused(
        &mut shop,
        draft_update,
        json!({"draft": draft, "input": {"billingPolicy": {"interval": "MONTH", "intervalCount": 0, "minCycles": 3, "maxCycles": 2}}}),
        "subscriptionDraftUpdate",
    );
    assert_eq!(
        errors
            .as_array()
            .unwrap()
            .iter()
            .map(|error| error["code"].clone())
            .collect::<Vec<_>>(),
        [
            json!("GREATER_THAN"),
            json!("SELLING_PLAN_MAX_CYCLES_MUST_BE_GREATER_THAN_MIN_CYCLES")
        ]
    );
    // No line left: put one back before committing.
    let variant = variant(&shop);
    run(
        &mut shop,
        LINE_ADD,
        json!({"draft": draft, "input": {"productVariantId": variant, "quantity": 1, "currentPrice": "12.00"}}),
    );
    shop.take_events();
    let answer = run(&mut shop, COMMIT, json!({"draft": draft}));
    let committed = data(&answer, "subscriptionDraftCommit")["contract"].clone();
    assert_eq!(committed["id"], json!(contract_id));
    assert_eq!(committed["revisionId"], json!("2"));
    assert_eq!(
        committed["lines"]["nodes"][0]["currentPrice"]["amount"],
        json!("12.0")
    );
    assert_eq!(topics(&mut shop), ["subscription_contracts/update"]);
}

#[test]
fn a_draft_of_an_older_revision_is_stale() {
    let mut shop = shop();
    let contract_id = contract(&mut shop, None);
    let update = "mutation($id: ID!) { subscriptionContractUpdate(contractId: $id) { draft { id } userErrors { message } } }";
    let first = data(
        &run(&mut shop, update, json!({"id": contract_id})),
        "subscriptionContractUpdate",
    )["draft"]["id"]
        .clone();
    run(
        &mut shop,
        "mutation($id: ID!) { subscriptionContractPause(subscriptionContractId: $id) { userErrors { message } } }",
        json!({"id": contract_id}),
    );
    let errors = refused(
        &mut shop,
        COMMIT,
        json!({"draft": first}),
        "subscriptionDraftCommit",
    );
    assert_eq!(errors[0]["code"], json!("STALE_CONTRACT"));
}

#[test]
fn an_atomic_creation_makes_the_contract_at_once() {
    let mut shop = shop();
    let method = card(&mut shop, "4242424242424242");
    shop.take_events();
    let input = atomic_input(&shop, Some(&method));
    let answer = run(&mut shop, ATOMIC, json!({"input": input}));
    let contract = data(&answer, "subscriptionContractAtomicCreate")["contract"].clone();
    assert_eq!(contract["status"], json!("ACTIVE"));
    assert_eq!(contract["createdAt"], json!("2026-01-15T12:00:00Z"));
    assert_eq!(contract["nextBillingDate"], json!("2026-02-15T12:00:00Z"));
    assert_eq!(contract["customerPaymentMethod"]["id"], json!(method));
    assert_eq!(
        contract["billingPolicy"],
        json!({"interval": "MONTH", "intervalCount": 1, "minCycles": 1, "maxCycles": 12, "cadence": {"unit": "MONTH", "count": 1}})
    );
    assert_eq!(
        contract["deliveryMethod"]["shippingOption"],
        json!({"title": "Standard", "code": "standard"})
    );
    assert_eq!(contract["linesCount"], json!({"count": 1}));
    assert_eq!(contract["lineCount"], json!(1));
    assert_eq!(
        contract["lines"]["nodes"][0]["sellingPlanName"],
        json!("Deliver every month, 10% off")
    );
    assert_eq!(
        contract["latestCommittedProjectedOrderTotals"],
        json!({"subtotal": {"amount": "20.0"}, "totalDelivery": {"amount": "5.0"}, "total": {"amount": "25.0"}})
    );
    assert_eq!(topics(&mut shop), ["subscription_contracts/create"]);

    let mut input = atomic_input(&shop, Some(&method));
    input["currencyCode"] = json!("JPY");
    let errors = refused(
        &mut shop,
        ATOMIC,
        json!({"input": input}),
        "subscriptionContractAtomicCreate",
    );
    assert_eq!(errors[0]["code"], json!("CURRENCY_NOT_ENABLED"));
    let mut input = atomic_input(&shop, Some(&method));
    input["contract"]["deliveryMethod"] = Value::Null;
    let errors = refused(
        &mut shop,
        ATOMIC,
        json!({"input": input}),
        "subscriptionContractAtomicCreate",
    );
    assert_eq!(errors[0]["code"], json!("DELIVERY_METHOD_REQUIRED"));
}

// ---- contracts ------------------------------------------------------------------------------

#[test]
fn a_contract_changes_status_until_it_is_terminated() {
    let mut shop = shop();
    let id = contract(&mut shop, None);
    shop.take_events();
    let status = |name: &str| {
        format!(
            "mutation($id: ID!) {{ {name}(subscriptionContractId: $id) {{ contract {{ status revisionId }} userErrors {{ field message code }} }} }}"
        )
    };
    for (mutation, expected, topic) in [
        (
            "subscriptionContractPause",
            "PAUSED",
            "subscription_contracts/pause",
        ),
        (
            "subscriptionContractFail",
            "FAILED",
            "subscription_contracts/fail",
        ),
        (
            "subscriptionContractActivate",
            "ACTIVE",
            "subscription_contracts/activate",
        ),
        (
            "subscriptionContractExpire",
            "EXPIRED",
            "subscription_contracts/expire",
        ),
    ] {
        let answer = run(&mut shop, &status(mutation), json!({"id": id}));
        assert_eq!(
            data(&answer, mutation)["contract"]["status"],
            json!(expected)
        );
        assert_eq!(topics(&mut shop), [topic]);
    }
    // Expired: terminated, no status changes it again (but the one it has, which is nothing).
    let errors = refused(
        &mut shop,
        &status("subscriptionContractActivate"),
        json!({"id": id}),
        "subscriptionContractActivate",
    );
    assert_eq!(
        errors,
        json!([{"field": ["subscriptionContractId"], "message": "Subscription contract status cannot be changed once terminated.", "code": "CONTRACT_TERMINATED"}])
    );
    let other = contract(&mut shop, None);
    shop.take_events();
    let answer = run(
        &mut shop,
        &status("subscriptionContractCancel"),
        json!({"id": other}),
    );
    assert_eq!(
        data(&answer, "subscriptionContractCancel")["contract"]["status"],
        json!("CANCELLED")
    );
    assert_eq!(topics(&mut shop), ["subscription_contracts/cancel"]);
    for mutation in [
        "subscriptionContractPause",
        "subscriptionContractFail",
        "subscriptionContractExpire",
    ] {
        refused(&mut shop, &status(mutation), json!({"id": other}), mutation);
    }
    refused(
        &mut shop,
        &status("subscriptionContractCancel"),
        json!({"id": "gid://shopify/SubscriptionContract/9"}),
        "subscriptionContractCancel",
    );
}

#[test]
fn a_product_change_and_a_next_billing_date_are_new_revisions() {
    let mut shop = shop();
    let id = contract(&mut shop, None);
    shop.take_events();
    let line = shop.get(&id).unwrap()["lines"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let other = {
        let product = product(&shop, "merino-crew-socks");
        crate::admin::domains::catalog::gids(shop.get(&product).unwrap().get("variants"))[1].clone()
    };
    let change = r#"mutation($id: ID!, $line: ID!, $input: SubscriptionContractProductChangeInput!) { subscriptionContractProductChange(subscriptionContractId: $id, lineId: $line, input: $input) { contract { revisionId } lineUpdated { id variantId currentPrice { amount } } userErrors { field message code } } }"#;
    let answer = run(
        &mut shop,
        change,
        json!({"id": id, "line": line, "input": {"productVariantId": other, "currentPrice": "8.50"}}),
    );
    let changed = data(&answer, "subscriptionContractProductChange");
    assert_eq!(
        changed["lineUpdated"],
        json!({"id": line, "variantId": other, "currentPrice": {"amount": "8.5"}})
    );
    assert_eq!(changed["contract"]["revisionId"], json!("2"));
    assert_eq!(topics(&mut shop), ["subscription_contracts/update"]);
    let errors = refused(
        &mut shop,
        change,
        json!({"id": id, "line": "gid://shopify/SubscriptionLine/x", "input": {"currentPrice": "1"}}),
        "subscriptionContractProductChange",
    );
    assert_eq!(errors[0]["field"], json!(["lineId"]));

    let next = r#"mutation($id: ID!, $date: DateTime!) { subscriptionContractSetNextBillingDate(contractId: $id, date: $date) { contract { nextBillingDate revisionId } userErrors { field message code } } }"#;
    let answer = run(
        &mut shop,
        next,
        json!({"id": id, "date": "2026-03-01T09:00:00-05:00"}),
    );
    assert_eq!(
        data(&answer, "subscriptionContractSetNextBillingDate")["contract"],
        json!({"nextBillingDate": "2026-03-01T14:00:00Z", "revisionId": "3"})
    );
    assert_eq!(topics(&mut shop), ["subscription_contracts/update"]);
    refused(
        &mut shop,
        next,
        json!({"id": "gid://shopify/SubscriptionContract/9", "date": "2026-03-01T09:00:00Z"}),
        "subscriptionContractSetNextBillingDate",
    );
}

#[test]
fn contracts_are_listed_filtered_and_sorted() {
    let mut shop = shop();
    let first = contract(&mut shop, None);
    let second = contract(&mut shop, None);
    run(
        &mut shop,
        "mutation($id: ID!) { subscriptionContractPause(subscriptionContractId: $id) { userErrors { message } } }",
        json!({"id": second}),
    );
    let customer = customer(&shop);
    let product = product(&shop, "merino-crew-socks");
    let read = run(
        &mut shop,
        r#"query($product: String!, $customer: ID!) {
          active: subscriptionContracts(first: 5, query: "status:active") { nodes { id } }
          socks: subscriptionContracts(first: 5, query: $product) { nodes { id } }
          byStatus: subscriptionContracts(first: 5, sortKey: STATUS) { nodes { status } }
          customer(id: $customer) { subscriptionContracts(first: 5) { nodes { id } } }
        }"#,
        json!({"product": format!("product_id:{}", gid_number(&product)), "customer": customer}),
    );
    assert_eq!(read["data"]["active"]["nodes"], json!([{"id": first}]));
    assert_eq!(
        read["data"]["socks"]["nodes"],
        json!([{"id": first}, {"id": second}])
    );
    assert_eq!(
        read["data"]["byStatus"]["nodes"],
        json!([{"status": "ACTIVE"}, {"status": "PAUSED"}])
    );
    assert_eq!(
        read["data"]["customer"]["subscriptionContracts"]["nodes"],
        json!([{"id": first}, {"id": second}])
    );
    assert!(
        read["extensions"].get("lsf").is_none(),
        "{:#}",
        read["extensions"]
    );
}

// ---- billing cycles -------------------------------------------------------------------------

const CYCLES: &str = r#"query($id: ID!, $range: SubscriptionBillingCyclesIndexRangeSelector) {
  subscriptionBillingCycles(contractId: $id, first: 20, billingCyclesIndexRangeSelector: $range) { nodes { cycleIndex cycleStartAt cycleEndAt billingAttemptExpectedDate skipped edited status } }
}"#;

#[test]
fn cycles_follow_the_billing_policy_from_the_contract_creation() {
    let mut shop = shop();
    let id = contract(&mut shop, None);
    let answer = run(
        &mut shop,
        CYCLES,
        json!({"id": id, "range": {"startIndex": 1, "endIndex": 3}}),
    );
    assert_eq!(
        answer["data"]["subscriptionBillingCycles"]["nodes"],
        json!([
            {"cycleIndex": 1, "cycleStartAt": "2026-01-15T12:00:00Z", "cycleEndAt": "2026-02-15T12:00:00Z", "billingAttemptExpectedDate": "2026-02-15T12:00:00Z", "skipped": false, "edited": false, "status": "UNBILLED"},
            // A month later at the same time in New York, summer time from March 8.
            {"cycleIndex": 2, "cycleStartAt": "2026-02-15T12:00:00Z", "cycleEndAt": "2026-03-15T11:00:00Z", "billingAttemptExpectedDate": "2026-03-15T11:00:00Z", "skipped": false, "edited": false, "status": "UNBILLED"},
            {"cycleIndex": 3, "cycleStartAt": "2026-03-15T11:00:00Z", "cycleEndAt": "2026-04-15T11:00:00Z", "billingAttemptExpectedDate": "2026-04-15T11:00:00Z", "skipped": false, "edited": false, "status": "UNBILLED"},
        ])
    );
    // By a date it holds; past `maxCycles` (12), nothing.
    let one = r#"query($input: SubscriptionBillingCycleInput!) { subscriptionBillingCycle(billingCycleInput: $input) { cycleIndex sourceContract { id } } }"#;
    let answer = run(
        &mut shop,
        one,
        json!({"input": {"contractId": id, "selector": {"date": "2026-03-20T00:00:00Z"}}}),
    );
    assert_eq!(
        answer["data"]["subscriptionBillingCycle"],
        json!({"cycleIndex": 3, "sourceContract": {"id": id}})
    );
    let answer = run(
        &mut shop,
        one,
        json!({"input": {"contractId": id, "selector": {"index": 13}}}),
    );
    assert_eq!(answer["data"]["subscriptionBillingCycle"], Value::Null);
}

#[test]
fn a_cycle_past_the_calendar_is_out_of_range() {
    let mut shop = shop();
    // Daily, without `maxCycles`: cycle 2,000,000,000 would start millions of years ahead.
    let mut input = atomic_input(&shop, None);
    input["contract"]["billingPolicy"] = json!({"interval": "DAY", "intervalCount": 1});
    input["contract"]["deliveryPolicy"] = json!({"interval": "DAY", "intervalCount": 1});
    let answer = run(&mut shop, ATOMIC, json!({"input": input}));
    let daily = data(&answer, "subscriptionContractAtomicCreate")["contract"]["id"].clone();
    // Every 2^31 - 1 weeks: its first cycle ends on the last midnight lsf computes.
    let mut input = atomic_input(&shop, None);
    input["contract"]["billingPolicy"] = json!({"interval": "WEEK", "intervalCount": i32::MAX});
    input["contract"]["deliveryPolicy"] = json!({"interval": "WEEK", "intervalCount": i32::MAX});
    let answer = run(&mut shop, ATOMIC, json!({"input": input}));
    let created = data(&answer, "subscriptionContractAtomicCreate");
    assert_eq!(created["userErrors"], json!([]), "{created:#}");
    let sparse = created["contract"]["id"].clone();

    let one = r#"query($input: SubscriptionBillingCycleInput!) { subscriptionBillingCycle(billingCycleInput: $input) { cycleIndex cycleEndAt } }"#;
    for (id, index) in [
        (&daily, 2_000_000_000i64),
        (&sparse, 2),
        (&sparse, 2_000_000_000),
    ] {
        let selector = json!({"contractId": id, "selector": {"index": index}});
        let answer = run(&mut shop, one, json!({"input": selector}));
        assert_eq!(
            answer["data"]["subscriptionBillingCycle"],
            Value::Null,
            "{answer}"
        );
        let errors = refused(
            &mut shop,
            SKIP,
            json!({"input": selector}),
            "subscriptionBillingCycleSkip",
        );
        assert_eq!(errors[0]["code"], json!("INVALID"), "{errors}");
        let answer = run(
            &mut shop,
            ATTEMPT,
            json!({"id": id, "input": {"idempotencyKey": format!("far-{index}"), "billingCycleSelector": {"index": index}}}),
        );
        let made = data(&answer, "subscriptionBillingAttemptCreate");
        assert_eq!(
            made["userErrors"][0]["code"],
            json!("CYCLE_INDEX_OUT_OF_RANGE"),
            "{made:#}"
        );
    }
    let answer = run(
        &mut shop,
        one,
        json!({"input": {"contractId": sparse, "selector": {"index": 1}}}),
    );
    assert_eq!(
        answer["data"]["subscriptionBillingCycle"],
        json!({"cycleIndex": 1, "cycleEndAt": "9999-12-30T05:00:00Z"})
    );
    let answer = run(
        &mut shop,
        CYCLES,
        json!({"id": daily, "range": {"startIndex": 1999999990, "endIndex": 2000000000}}),
    );
    assert!(answer.get("errors").is_none(), "{answer}");
}

#[test]
fn anchored_cycles_end_at_midnight_on_the_anchor_day_in_the_shops_zone() {
    let mut shop = shop();
    let mut input = atomic_input(&shop, None);
    input["contract"]["billingPolicy"] = json!({"interval": "MONTH", "intervalCount": 1, "anchors": [{"type": "MONTHDAY", "day": 31}]});
    let answer = run(&mut shop, ATOMIC, json!({"input": input}));
    let id = data(&answer, "subscriptionContractAtomicCreate")["contract"]["id"].clone();
    let answer = run(
        &mut shop,
        CYCLES,
        json!({"id": id, "range": {"startIndex": 1, "endIndex": 3}}),
    );
    let ends: Vec<Value> = answer["data"]["subscriptionBillingCycles"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|cycle| cycle["cycleEndAt"].clone())
        .collect();
    // January 31, the last of February, March 31: midnight in New York (UTC-5).
    assert_eq!(
        ends,
        [
            json!("2026-01-31T05:00:00Z"),
            json!("2026-02-28T05:00:00Z"),
            json!("2026-03-31T04:00:00Z")
        ]
    );
}

const SKIP: &str = r#"mutation($input: SubscriptionBillingCycleInput!) { subscriptionBillingCycleSkip(billingCycleInput: $input) { billingCycle { cycleIndex skipped edited } userErrors { field message code } } }"#;
const UNSKIP: &str = r#"mutation($input: SubscriptionBillingCycleInput!) { subscriptionBillingCycleUnskip(billingCycleInput: $input) { billingCycle { cycleIndex skipped edited } userErrors { field message code } } }"#;

#[test]
fn a_cycle_is_skipped_and_unskipped() {
    let mut shop = shop();
    let id = contract(&mut shop, None);
    shop.take_events();
    let input = json!({"input": {"contractId": id, "selector": {"index": 2}}});
    let answer = run(&mut shop, SKIP, input.clone());
    assert_eq!(
        data(&answer, "subscriptionBillingCycleSkip")["billingCycle"],
        json!({"cycleIndex": 2, "skipped": true, "edited": true})
    );
    let events = shop.take_events();
    assert_eq!(
        events
            .iter()
            .map(|event| event.topic.as_str())
            .collect::<Vec<_>>(),
        ["subscription_billing_cycles/skip"]
    );
    let payload = crate::admin::webhooks::payload::payload(
        &events[0].topic,
        events[0].snapshot.as_ref().unwrap(),
        &crate::admin::api::samples::ShopLookup(&shop),
    )
    .unwrap();
    assert_eq!(
        payload,
        json!({"subscription_contract_id": gid_number(&id), "cycle_start_at": "2026-02-15T07:00:00-05:00", "cycle_end_at": "2026-03-15T07:00:00-04:00", "cycle_index": 2, "contract_edit": null, "billing_attempt_expected_date": "2026-03-15T07:00:00-04:00", "skipped": true, "edited": true})
    );
    let errors = refused(
        &mut shop,
        SKIP,
        json!({"input": {"contractId": id, "selector": {"index": 0}}}),
        "subscriptionBillingCycleSkip",
    );
    assert_eq!(errors[0]["code"], json!("INVALID"));
    let answer = run(&mut shop, UNSKIP, input);
    assert_eq!(
        data(&answer, "subscriptionBillingCycleUnskip")["billingCycle"],
        json!({"cycleIndex": 2, "skipped": false, "edited": true})
    );
    assert_eq!(topics(&mut shop), ["subscription_billing_cycles/unskip"]);
    refused(
        &mut shop,
        UNSKIP,
        json!({"input": {"contractId": "gid://shopify/SubscriptionContract/9", "selector": {"index": 2}}}),
        "subscriptionBillingCycleUnskip",
    );
}

const SCHEDULE: &str = r#"mutation($input: SubscriptionBillingCycleInput!, $edit: SubscriptionBillingCycleScheduleEditInput!) { subscriptionBillingCycleScheduleEdit(billingCycleInput: $input, input: $edit) { billingCycle { cycleIndex billingAttemptExpectedDate skipped edited } userErrors { field message code } } }"#;

#[test]
fn a_cycles_billing_date_moves_within_it_and_its_edits_are_deleted() {
    let mut shop = shop();
    let id = contract(&mut shop, None);
    shop.take_events();
    let input = json!({"contractId": id, "selector": {"index": 1}});
    let answer = run(
        &mut shop,
        SCHEDULE,
        json!({"input": input, "edit": {"billingDate": "2026-02-10T12:00:00Z", "reason": "BUYER_INITIATED"}}),
    );
    assert_eq!(
        data(&answer, "subscriptionBillingCycleScheduleEdit")["billingCycle"],
        json!({"cycleIndex": 1, "billingAttemptExpectedDate": "2026-02-10T12:00:00Z", "skipped": false, "edited": true})
    );
    assert_eq!(
        topics(&mut shop),
        ["subscription_billing_cycle_edits/create"]
    );
    run(
        &mut shop,
        SCHEDULE,
        json!({"input": input, "edit": {"billingDate": "2026-02-11T12:00:00Z", "reason": "BUYER_INITIATED"}}),
    );
    assert_eq!(
        topics(&mut shop),
        ["subscription_billing_cycle_edits/update"]
    );
    let errors = refused(
        &mut shop,
        SCHEDULE,
        json!({"input": input, "edit": {"billingDate": "2026-04-01T00:00:00Z", "reason": "MERCHANT_INITIATED"}}),
        "subscriptionBillingCycleScheduleEdit",
    );
    assert_eq!(
        errors,
        json!([{"field": ["input", "billingDate"], "message": "Billing date of a cycle cannot be set to a value outside of its billing date range.", "code": "OUT_OF_BOUNDS"}])
    );
    let errors = refused(
        &mut shop,
        SCHEDULE,
        json!({"input": input, "edit": {"reason": "MERCHANT_INITIATED"}}),
        "subscriptionBillingCycleScheduleEdit",
    );
    assert_eq!(
        errors[0]["code"],
        json!("EMPTY_BILLING_CYCLE_EDIT_SCHEDULE_INPUT")
    );
    // A skip through the schedule edit.
    run(
        &mut shop,
        SCHEDULE,
        json!({"input": {"contractId": id, "selector": {"index": 3}}, "edit": {"skip": true, "reason": "DEV_INITIATED"}}),
    );
    assert_eq!(topics(&mut shop), ["subscription_billing_cycles/skip"]);

    let delete = r#"mutation($input: SubscriptionBillingCycleInput!) { subscriptionBillingCycleEditDelete(billingCycleInput: $input) { billingCycles { cycleIndex billingAttemptExpectedDate edited } userErrors { field message code } } }"#;
    let answer = run(&mut shop, delete, json!({"input": input}));
    assert_eq!(
        data(&answer, "subscriptionBillingCycleEditDelete")["billingCycles"],
        json!([{"cycleIndex": 1, "billingAttemptExpectedDate": "2026-02-15T12:00:00Z", "edited": false}])
    );
    assert_eq!(
        topics(&mut shop),
        ["subscription_billing_cycle_edits/delete"]
    );
    let errors = refused(
        &mut shop,
        delete,
        json!({"input": input}),
        "subscriptionBillingCycleEditDelete",
    );
    assert_eq!(errors[0]["code"], json!("NO_CYCLE_EDITS"));

    let all = r#"mutation($id: ID!) { subscriptionBillingCycleEditsDelete(contractId: $id, targetSelection: ALL) { billingCycles { cycleIndex skipped } userErrors { field message code } } }"#;
    let answer = run(&mut shop, all, json!({"id": id}));
    assert_eq!(
        data(&answer, "subscriptionBillingCycleEditsDelete")["billingCycles"],
        json!([{"cycleIndex": 3, "skipped": false}])
    );
    assert_eq!(
        topics(&mut shop),
        ["subscription_billing_cycle_edits/delete"]
    );
    let errors = refused(
        &mut shop,
        all,
        json!({"id": id}),
        "subscriptionBillingCycleEditsDelete",
    );
    assert_eq!(errors[0]["code"], json!("NO_CYCLE_EDITS"));
}

#[test]
fn a_new_billing_interval_changes_the_current_cycle_and_the_next_ones() {
    let mut shop = shop();
    let id = contract(&mut shop, None);
    at(&mut shop, "2026-02-01T12:00:00Z");
    let draft = data(
        &run(
            &mut shop,
            "mutation($id: ID!) { subscriptionContractUpdate(contractId: $id) { draft { id } } }",
            json!({"id": id}),
        ),
        "subscriptionContractUpdate",
    )["draft"]["id"]
        .clone();
    // Two weeks from Jan 15 is already past on Feb 1: the cycle ends two weeks from now.
    run(
        &mut shop,
        "mutation($draft: ID!) { subscriptionDraftUpdate(draftId: $draft, input: {billingPolicy: {interval: WEEK, intervalCount: 2}, deliveryPolicy: {interval: WEEK, intervalCount: 2}}) { userErrors { message } } }",
        json!({"draft": draft}),
    );
    let answer = run(&mut shop, COMMIT, json!({"draft": draft}));
    assert_eq!(
        data(&answer, "subscriptionDraftCommit")["userErrors"],
        json!([])
    );
    let answer = run(
        &mut shop,
        CYCLES,
        json!({"id": id, "range": {"startIndex": 1, "endIndex": 2}}),
    );
    let nodes = &answer["data"]["subscriptionBillingCycles"]["nodes"];
    assert_eq!(nodes[0]["cycleStartAt"], json!("2026-01-15T12:00:00Z"));
    assert_eq!(nodes[0]["cycleEndAt"], json!("2026-02-15T12:00:00Z"));
    assert_eq!(nodes[1]["cycleEndAt"], json!("2026-03-01T12:00:00Z"));
}

// ---- billing --------------------------------------------------------------------------------

const ATTEMPT: &str = r#"mutation($id: ID!, $input: SubscriptionBillingAttemptInput!) {
  subscriptionBillingAttemptCreate(subscriptionContractId: $id, subscriptionBillingAttemptInput: $input) {
    subscriptionBillingAttempt { id ready idempotencyKey state { __typename } }
    userErrors { field message code }
  }
}"#;

const READ_ATTEMPT: &str = r#"query($id: ID!) {
  subscriptionBillingAttempt(id: $id) {
    ready completedAt errorCode errorMessage
    order { id name sourceName displayFinancialStatus totalPriceSet { shopMoney { amount } } lineItems(first: 5) { nodes { quantity sellingPlan { name sellingPlanId } } } }
    state { __typename
      ... on SubscriptionBillingAttemptSuccessState { order { id } }
      ... on SubscriptionBillingAttemptFailedState { error { __typename ... on SubscriptionBillingAttemptPaymentError { paymentCode: code } ... on SubscriptionBillingAttemptInventoryError { inventoryCode: code insufficientStockProductVariants(first: 5) { nodes { id } } } ... on SubscriptionBillingAttemptGeneralError { generalCode: code } } }
      ... on SubscriptionBillingAttemptActionRequiredState { action { ... on SubscriptionBillingAttemptPaymentChallenge { nextActionUrl status } } }
    }
    subscriptionContract { lastPaymentStatus lastBillingAttemptErrorType orders(first: 5) { nodes { id } } }
  }
}"#;

fn bill(shop: &mut ShopState, contract: &str, key: &str) -> (Value, Value) {
    let answer = run(
        shop,
        ATTEMPT,
        json!({"id": contract, "input": {"idempotencyKey": key}}),
    );
    let made = data(&answer, "subscriptionBillingAttemptCreate");
    assert_eq!(made["userErrors"], json!([]), "{made:#}");
    let id = made["subscriptionBillingAttempt"]["id"].clone();
    let read = run(shop, READ_ATTEMPT, json!({"id": id}));
    (made, read["data"]["subscriptionBillingAttempt"].clone())
}

/// What is available of a variant at the shop's first location.
fn available(shop: &ShopState, variant: &str) -> i64 {
    let item = shop.get(variant).unwrap()["inventoryItem"]
        .as_str()
        .unwrap()
        .to_string();
    let location = shop.keys("Location").next().unwrap().to_string();
    crate::admin::domains::orders::model::inventory_quantity(
        shop,
        &item,
        Some(&location),
        "available",
    )
}

/// A contract in a presentment currency bills an order presented in it: the amounts in the
/// shop's currency at the market's manual rate, `presentmentMoney` in the contract's, through
/// what the order goes through next (a refund).
#[test]
fn a_contract_in_a_presentment_currency_bills_an_order_in_it() {
    let mut shop = shop();
    let shop_gid = shop.shop_gid().unwrap().to_string();
    if let Some(record) = shop.get_mut(&shop_gid) {
        record.insert("enabledPresentmentCurrencies".into(), json!(["USD", "CAD"]));
    }
    // 1 USD = 1.25 CAD.
    shop.insert(value::object(json!({
        "__typename": "Market",
        "name": "Canada",
        "currencySettings": {"baseCurrency": {"currencyCode": "CAD", "manualRate": "1.25"}, "localCurrencies": false},
    })));
    let method = card(&mut shop, "4242424242424242");
    let mut input = atomic_input(&shop, Some(&method));
    input["currencyCode"] = json!("CAD");
    let answer = run(&mut shop, ATOMIC, json!({"input": input}));
    let created = data(&answer, "subscriptionContractAtomicCreate");
    assert_eq!(created["userErrors"], json!([]), "{created:#}");
    let id = created["contract"]["id"].as_str().unwrap().to_string();
    at(&mut shop, "2026-02-15T12:00:00Z");
    let (_, attempt) = bill(&mut shop, &id, "cycle-1");
    let order_id = attempt["order"]["id"].clone();
    let read = run(
        &mut shop,
        r#"query($id: ID!) { order(id: $id) { currencyCode presentmentCurrencyCode
            totalPriceSet { shopMoney { amount currencyCode } presentmentMoney { amount currencyCode } }
            lineItems(first: 1) { nodes { id originalUnitPriceSet { shopMoney { amount } presentmentMoney { amount currencyCode } } } }
            transactions { kind amountSet { shopMoney { amount currencyCode } presentmentMoney { amount currencyCode } } } } }"#,
        json!({"id": order_id}),
    );
    let order = &read["data"]["order"];
    assert_eq!(order["currencyCode"], json!("USD"), "{read}");
    assert_eq!(order["presentmentCurrencyCode"], json!("CAD"));
    // Two pairs at 10.00 CAD and 5.00 CAD of delivery: 25.00 CAD, 20.00 USD.
    assert_eq!(
        order["totalPriceSet"],
        json!({"shopMoney": {"amount": "20.0", "currencyCode": "USD"}, "presentmentMoney": {"amount": "25.0", "currencyCode": "CAD"}})
    );
    let line = &order["lineItems"]["nodes"][0];
    assert_eq!(
        line["originalUnitPriceSet"],
        json!({"shopMoney": {"amount": "8.0"}, "presentmentMoney": {"amount": "10.0", "currencyCode": "CAD"}})
    );
    assert_eq!(
        order["transactions"][0]["amountSet"],
        json!({"shopMoney": {"amount": "20.0", "currencyCode": "USD"}, "presentmentMoney": {"amount": "25.0", "currencyCode": "CAD"}})
    );
    // A refund of one pair, built by the orders family in the shop's currency, reads in CAD.
    let refunded = run(
        &mut shop,
        r#"mutation($input: RefundInput!) { refundCreate(input: $input) {
            refund { refundLineItems(first: 1) { nodes { subtotalSet { shopMoney { amount } presentmentMoney { amount currencyCode } } } } }
            userErrors { field message } } }"#,
        json!({"input": {"orderId": order_id, "refundLineItems": [{"lineItemId": line["id"], "quantity": 1, "restockType": "NO_RESTOCK"}]}}),
    );
    let refund = &refunded["data"]["refundCreate"];
    assert_eq!(refund["userErrors"], json!([]), "{refunded}");
    assert_eq!(
        refund["refund"]["refundLineItems"]["nodes"][0]["subtotalSet"]["presentmentMoney"]["currencyCode"],
        json!("CAD")
    );
}

#[test]
fn a_billing_attempt_that_succeeds_creates_the_order_and_commits_the_stock() {
    let mut shop = shop();
    let method = card(&mut shop, "4242424242424242");
    let id = contract(&mut shop, Some(&method));
    at(&mut shop, "2026-02-15T12:00:00Z");
    shop.take_events();
    let variant = variant(&shop);
    let stock = available(&shop, &variant);
    let (made, attempt) = bill(&mut shop, &id, "cycle-1");
    // Answered pending, as Shopify; read processed.
    assert_eq!(made["subscriptionBillingAttempt"]["ready"], json!(false));
    assert_eq!(
        made["subscriptionBillingAttempt"]["state"]["__typename"],
        json!("SubscriptionBillingAttemptPendingState")
    );
    assert_eq!(attempt["ready"], json!(true));
    assert_eq!(attempt["completedAt"], json!("2026-02-15T12:00:00Z"));
    assert_eq!(
        attempt["state"]["__typename"],
        json!("SubscriptionBillingAttemptSuccessState")
    );
    let order = &attempt["order"];
    assert_eq!(order["sourceName"], json!("subscription_contract"));
    assert_eq!(order["displayFinancialStatus"], json!("PAID"));
    assert_eq!(order["totalPriceSet"]["shopMoney"]["amount"], json!("25.0"));
    assert_eq!(
        order["lineItems"]["nodes"][0]["sellingPlan"]["name"],
        json!("Deliver every month, 10% off")
    );
    assert_eq!(
        attempt["subscriptionContract"]["lastPaymentStatus"],
        json!("SUCCEEDED")
    );
    assert_eq!(
        attempt["subscriptionContract"]["orders"]["nodes"],
        json!([{"id": order["id"]}])
    );
    assert_eq!(available(&shop, &variant), stock - 2);
    let topics = topics(&mut shop);
    for topic in [
        "orders/create",
        "order_transactions/create",
        "tender_transactions/create",
        "orders/paid",
        "subscription_billing_attempts/success",
    ] {
        assert!(
            topics.iter().any(|known| known == topic),
            "{topic} in {topics:?}"
        );
    }
    // The cycle billed; the same key answers the same attempt, and bills nothing more.
    let answer = run(
        &mut shop,
        CYCLES,
        json!({"id": id, "range": {"startIndex": 1, "endIndex": 1}}),
    );
    assert_eq!(
        answer["data"]["subscriptionBillingCycles"]["nodes"][0]["status"],
        json!("BILLED")
    );
    let again = run(
        &mut shop,
        ATTEMPT,
        json!({"id": id, "input": {"idempotencyKey": "cycle-1"}}),
    );
    assert_eq!(
        data(&again, "subscriptionBillingAttemptCreate")["subscriptionBillingAttempt"]["id"],
        made["subscriptionBillingAttempt"]["id"]
    );
    assert_eq!(shop.count("Order"), 4);

    // A paused contract is not billed: refused, and nothing changes.
    run(
        &mut shop,
        "mutation($id: ID!) { subscriptionContractPause(subscriptionContractId: $id) { userErrors { message } } }",
        json!({"id": id}),
    );
    let errors = refused(
        &mut shop,
        ATTEMPT,
        json!({"id": id, "input": {"idempotencyKey": "cycle-2"}}),
        "subscriptionBillingAttemptCreate",
    );
    assert_eq!(
        errors,
        json!([{"field": ["subscriptionContractId"], "message": "Subscription contract cannot be billed if paused.", "code": "CONTRACT_PAUSED"}])
    );
}

#[test]
fn a_declined_card_fails_the_attempt_and_the_contract_says_why() {
    let mut shop = shop();
    let method = card(&mut shop, "4000 0000 0000 0002");
    let id = contract(&mut shop, Some(&method));
    shop.take_events();
    let (_, attempt) = bill(&mut shop, &id, "a");
    assert_eq!(
        attempt["state"]["error"],
        json!({"__typename": "SubscriptionBillingAttemptPaymentError", "paymentCode": "CARD_DECLINED"})
    );
    assert_eq!(attempt["errorCode"], json!("CARD_DECLINED"));
    assert_eq!(
        attempt["errorMessage"],
        json!("The card was declined by the payment processor.")
    );
    assert_eq!(attempt["order"], Value::Null);
    assert_eq!(
        attempt["subscriptionContract"]["lastPaymentStatus"],
        json!("FAILED")
    );
    assert_eq!(
        attempt["subscriptionContract"]["lastBillingAttemptErrorType"],
        json!("PAYMENT_ERROR")
    );
    assert_eq!(topics(&mut shop), ["subscription_billing_attempts/failure"]);
    // Without a payment method: a general error.
    let other = contract(&mut shop, None);
    let (_, attempt) = bill(&mut shop, &other, "b");
    assert_eq!(
        attempt["state"]["error"],
        json!({"__typename": "SubscriptionBillingAttemptGeneralError", "generalCode": "PAYMENT_METHOD_NOT_SPECIFIED"})
    );
    // The failed attempts' errors are searched by code.
    let read = run(
        &mut shop,
        r#"{ subscriptionBillingAttempts(first: 5, query: "error_code:card_declined") { nodes { errorCode } } }"#,
        json!({}),
    );
    assert_eq!(
        read["data"]["subscriptionBillingAttempts"]["nodes"],
        json!([{"errorCode": "CARD_DECLINED"}])
    );
}

#[test]
fn a_variant_short_of_stock_fails_the_attempt_unless_overselling() {
    let mut shop = shop();
    let method = card(&mut shop, "4242424242424242");
    let id = contract(&mut shop, Some(&method));
    let variant = variant(&shop);
    let item = shop.get(&variant).unwrap()["inventoryItem"]
        .as_str()
        .unwrap()
        .to_string();
    // One left at every location, where the order would commit two.
    for level in
        crate::admin::domains::catalog::gids(shop.get(&item).unwrap().get("inventoryLevels"))
    {
        let quantities = shop.get(&level).unwrap()["quantities"].clone();
        let one: Vec<Value> = quantities
            .as_array()
            .unwrap()
            .iter()
            .map(|quantity| {
                let mut quantity = quantity.clone();
                if ["available", "on_hand"].contains(&quantity["name"].as_str().unwrap()) {
                    quantity["quantity"] = json!(1);
                }
                quantity
            })
            .collect();
        shop.get_mut(&level)
            .unwrap()
            .insert("quantities".into(), json!(one));
    }
    shop.get_mut(&item)
        .unwrap()
        .insert("tracked".into(), json!(true));
    shop.get_mut(&variant)
        .unwrap()
        .insert("inventoryPolicy".into(), json!("DENY"));
    let (_, attempt) = bill(&mut shop, &id, "a");
    assert_eq!(
        attempt["state"]["error"],
        json!({"__typename": "SubscriptionBillingAttemptInventoryError", "inventoryCode": "INSUFFICIENT_INVENTORY", "insufficientStockProductVariants": {"nodes": [{"id": variant}]}})
    );
    assert_eq!(
        attempt["subscriptionContract"]["lastBillingAttemptErrorType"],
        json!("INVENTORY_ERROR")
    );
    let answer = run(
        &mut shop,
        ATTEMPT,
        json!({"id": id, "input": {"idempotencyKey": "b", "inventoryPolicy": "ALLOW_OVERSELLING"}}),
    );
    let attempt_id =
        data(&answer, "subscriptionBillingAttemptCreate")["subscriptionBillingAttempt"]["id"]
            .clone();
    let read = run(&mut shop, READ_ATTEMPT, json!({"id": attempt_id}));
    assert_eq!(
        read["data"]["subscriptionBillingAttempt"]["state"]["__typename"],
        json!("SubscriptionBillingAttemptSuccessState")
    );
}

/// Billing takes the stock where it is: the primary location has none of the variant, another
/// active location has it, and the attempt succeeds and commits there.
#[test]
fn a_billing_attempt_commits_at_the_location_that_has_the_stock() {
    let mut shop = shop();
    let method = card(&mut shop, "4242424242424242");
    let id = contract(&mut shop, Some(&method));
    let variant = variant(&shop);
    let item = shop.get(&variant).unwrap()["inventoryItem"]
        .as_str()
        .unwrap()
        .to_string();
    shop.get_mut(&item)
        .unwrap()
        .insert("tracked".into(), json!(true));
    shop.get_mut(&variant)
        .unwrap()
        .insert("inventoryPolicy".into(), json!("DENY"));
    let primary = crate::admin::domains::catalog::default_location(&shop).unwrap();
    let levels =
        crate::admin::domains::catalog::gids(shop.get(&item).unwrap().get("inventoryLevels"));
    let location_of = |shop: &ShopState, level: &str| shop.get(level).unwrap()["location"].clone();
    let (empty, stocked): (Vec<String>, Vec<String>) = levels
        .into_iter()
        .partition(|level| location_of(&shop, level) == json!(primary));
    assert!(!empty.is_empty() && !stocked.is_empty(), "two levels");
    let other = location_of(&shop, &stocked[0])
        .as_str()
        .unwrap()
        .to_string();
    for (level, units) in [(&empty[0], 0), (&stocked[0], 30)] {
        let quantities: Vec<Value> = shop.get(level).unwrap()["quantities"]
            .as_array()
            .unwrap()
            .iter()
            .map(|quantity| {
                let mut quantity = quantity.clone();
                if ["available", "on_hand"].contains(&quantity["name"].as_str().unwrap()) {
                    quantity["quantity"] = json!(units);
                }
                quantity
            })
            .collect();
        shop.get_mut(level)
            .unwrap()
            .insert("quantities".into(), json!(quantities));
    }
    at(&mut shop, "2026-02-15T12:00:00Z");
    let (_, attempt) = bill(&mut shop, &id, "stocked");
    assert_eq!(
        attempt["state"]["__typename"],
        json!("SubscriptionBillingAttemptSuccessState"),
        "{attempt:#}"
    );
    let quantity = |shop: &ShopState, location: &str, name: &str| {
        crate::admin::domains::orders::model::inventory_quantity(shop, &item, Some(location), name)
    };
    assert_eq!(quantity(&shop, &other, "committed"), 2);
    assert_eq!(quantity(&shop, &other, "available"), 28);
    assert_eq!(quantity(&shop, &primary, "available"), 0);
    let order = run(
        &mut shop,
        "query($id: ID!) { order(id: $id) { fulfillmentOrders(first: 1) { nodes { assignedLocation { location { id } } } } } }",
        json!({"id": attempt["order"]["id"]}),
    );
    assert_eq!(
        order["data"]["order"]["fulfillmentOrders"]["nodes"][0]["assignedLocation"]["location"]["id"],
        json!(other)
    );
}

#[test]
fn a_3d_secure_card_is_challenged_until_the_customer_completes_it() {
    let mut shop = shop();
    let method = card(&mut shop, "4000000000003220");
    let id = contract(&mut shop, Some(&method));
    shop.take_events();
    let (made, attempt) = bill(&mut shop, &id, "a");
    let attempt_id = made["subscriptionBillingAttempt"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let number = gid_number(&attempt_id);
    assert_eq!(
        attempt["state"]["action"],
        json!({"nextActionUrl": format!("http://127.0.0.1:9292/__lsf/admin/billing-attempts/{number}/challenge?shop=local-supply-co.myshopify.com"), "status": "OFF_SESSION_REJECTED"})
    );
    assert_eq!(
        topics(&mut shop),
        ["subscription_billing_attempts/challenged"]
    );
    super::billing::open_challenge(&mut shop, number).unwrap();
    let read = run(&mut shop, READ_ATTEMPT, json!({"id": attempt_id}));
    assert_eq!(
        read["data"]["subscriptionBillingAttempt"]["state"]["action"]["status"],
        json!("ON_SESSION_CHALLENGED")
    );
    // A challenged cycle has an attempt in progress: it is not skipped meanwhile.
    let errors = refused(
        &mut shop,
        SKIP,
        json!({"input": {"contractId": id, "selector": {"index": 1}}}),
        "subscriptionBillingCycleSkip",
    );
    assert_eq!(
        errors[0]["message"],
        json!("Billing cycle has incomplete billing attempts in progress.")
    );
    shop.take_events();
    super::billing::complete_challenge(&mut shop, number, true).unwrap();
    let read = run(&mut shop, READ_ATTEMPT, json!({"id": attempt_id}));
    assert_eq!(
        read["data"]["subscriptionBillingAttempt"]["state"]["__typename"],
        json!("SubscriptionBillingAttemptSuccessState")
    );
    assert!(topics(&mut shop).contains(&"subscription_billing_attempts/success".to_string()));
    assert_eq!(
        super::billing::complete_challenge(&mut shop, number, false).unwrap_err(),
        super::billing::ChallengeError::NotChallenged
    );
}

#[test]
fn a_cycle_is_charged_from_a_day_before_its_billing_date() {
    let mut shop = shop();
    let method = card(&mut shop, "4242424242424242");
    let id = contract(&mut shop, Some(&method));
    let charge = r#"mutation($id: ID!, $selector: SubscriptionBillingCycleSelector!) { subscriptionBillingCycleCharge(subscriptionContractId: $id, billingCycleSelector: $selector) { subscriptionBillingAttempt { id ready } userErrors { field message code } } }"#;
    let errors = refused(
        &mut shop,
        charge,
        json!({"id": id, "selector": {"index": 1}}),
        "subscriptionBillingCycleCharge",
    );
    assert_eq!(
        errors[0]["code"],
        json!("BILLING_CYCLE_CHARGE_BEFORE_EXPECTED_DATE")
    );
    at(&mut shop, "2026-02-14T13:00:00Z");
    let answer = run(
        &mut shop,
        charge,
        json!({"id": id, "selector": {"index": 1}}),
    );
    let attempt =
        data(&answer, "subscriptionBillingCycleCharge")["subscriptionBillingAttempt"].clone();
    assert_eq!(attempt["ready"], json!(false));
    assert_eq!(
        shop.get(attempt["id"].as_str().unwrap()).unwrap()["lsfCycleIndex"],
        json!(1)
    );
    // A skipped cycle is not charged.
    run(
        &mut shop,
        SKIP,
        json!({"input": {"contractId": id, "selector": {"index": 2}}}),
    );
    at(&mut shop, "2026-03-15T12:00:00Z");
    let errors = refused(
        &mut shop,
        charge,
        json!({"id": id, "selector": {"index": 2}}),
        "subscriptionBillingCycleCharge",
    );
    assert_eq!(errors[0]["code"], json!("BILLING_CYCLE_SKIPPED"));
}

#[test]
fn bulk_jobs_find_and_charge_the_cycles_of_a_week() {
    let mut shop = shop();
    let method = card(&mut shop, "4242424242424242");
    let first = contract(&mut shop, Some(&method));
    let second = contract(&mut shop, Some(&method));
    at(&mut shop, "2026-02-15T13:00:00Z");
    let range = json!({"startDate": "2026-02-14T00:00:00Z", "endDate": "2026-02-16T00:00:00Z"});
    let search = r#"mutation($range: SubscriptionBillingCyclesDateRangeSelector!) { subscriptionBillingCycleBulkSearch(billingAttemptExpectedDateRange: $range) { job { id done } userErrors { field message code } } }"#;
    let answer = run(&mut shop, search, json!({"range": range}));
    let job = data(&answer, "subscriptionBillingCycleBulkSearch")["job"].clone();
    assert_eq!(job["done"], json!(true));
    let results = r#"query($job: ID!) { subscriptionBillingCycleBulkResults(jobId: $job, first: 5) { nodes { cycleIndex status sourceContract { id } } } }"#;
    let read = run(&mut shop, results, json!({"job": job["id"]}));
    assert_eq!(
        read["data"]["subscriptionBillingCycleBulkResults"]["nodes"],
        json!([{"cycleIndex": 1, "status": "UNBILLED", "sourceContract": {"id": first}}, {"cycleIndex": 1, "status": "UNBILLED", "sourceContract": {"id": second}}])
    );
    let errors = refused(
        &mut shop,
        search,
        json!({"range": {"startDate": "2026-02-01T00:00:00Z", "endDate": "2026-02-16T00:00:00Z"}}),
        "subscriptionBillingCycleBulkSearch",
    );
    assert_eq!(errors[0]["code"], json!("INVALID_DATE_RANGE"));

    let charge = r#"mutation($range: SubscriptionBillingCyclesDateRangeSelector!) { subscriptionBillingCycleBulkCharge(billingAttemptExpectedDateRange: $range, filters: {billingAttemptStatus: NO_ATTEMPT}) { job { id done } userErrors { field message code } } }"#;
    let answer = run(&mut shop, charge, json!({"range": range}));
    let job = data(&answer, "subscriptionBillingCycleBulkCharge")["job"].clone();
    let read = run(&mut shop, results, json!({"job": job["id"]}));
    let statuses: Vec<Value> = read["data"]["subscriptionBillingCycleBulkResults"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|cycle| cycle["status"].clone())
        .collect();
    assert_eq!(statuses, [json!("BILLED"), json!("BILLED")]);
    let errors = refused(
        &mut shop,
        charge,
        json!({"range": {"startDate": "2026-02-15T00:00:00Z", "endDate": "2026-02-18T00:00:00Z"}}),
        "subscriptionBillingCycleBulkCharge",
    );
    assert_eq!(errors[0]["code"], json!("END_DATE_IN_THE_FUTURE"));
}

// ---- the calculation API --------------------------------------------------------------------

const CREATE_CALCULATE: &str = r#"mutation($input: SubscriptionContractCalculationContractCreateInput!) {
  subscriptionContractCreateCalculate(contractCreateInput: $input) { subscriptionContractCalculation { __typename ... on SubscriptionContractCalculationPending { id } } userErrors { field message code } }
}"#;

const READ_CALCULATION: &str = r#"query($id: ID!) {
  subscriptionContractCalculation(id: $id) { __typename
    ... on SubscriptionContractCalculationSuccess { id warnings { code } projectedOrderTotals { subtotal { amount } totalDelivery { amount } totalDeliveryDiscounts { amount } total { amount } }
      calculatedContract { lines(first: 5) { nodes { id quantity currentPrice { amount } } } deliveryPrice { amount } billingPolicy { interval intervalCount } } }
    ... on SubscriptionContractCalculationFailure { id errors { code field } }
  }
}"#;

const CALCULATION_COMMIT: &str = r#"mutation($id: ID!) { subscriptionContractCalculationCommit(id: $id) { contract { __typename ... on SubscriptionContract { id status nextBillingDate lines(first: 5) { nodes { quantity } } } ... on SubscriptionBillingCycleEditedContract { lines(first: 5) { nodes { quantity } } billingCycles(first: 1) { nodes { cycleIndex edited } } } } userErrors { field message code } } }"#;

fn calculation_input(shop: &ShopState) -> Value {
    json!({
        "withMerchandiseCustomizations": false,
        "customerId": customer(shop),
        "currencyCode": "USD",
        "lines": [{"productVariantLine": {"productVariantId": variant(shop), "quantity": 2, "priceOverride": {"amount": "9.00", "currencyCode": "USD"}, "discounts": []}}],
        "billingPolicy": {"cadence": {"unit": "MONTH", "count": 1}},
        "deliveryPolicy": {},
        "deliveryMethod": {"shipping": {"address": {"address1": "12 Maple Street", "city": "Portland", "countryCode": "US", "zip": "97201"}, "deliveryPrice": {"amount": "5.00", "currencyCode": "USD"}, "title": "Standard"}},
        "paymentMethod": {"none": true},
        "discountCodes": [],
        "manualDiscounts": [{"deliveryDiscount": {"title": "Free shipping for 3 months", "value": {"percentage": 100}, "recurringCycleLimit": 3}}],
        "customAttributes": [],
    })
}

#[test]
fn a_calculation_is_polled_then_committed_into_a_contract() {
    let mut shop = shop();
    let input = calculation_input(&shop);
    let answer = run(&mut shop, CREATE_CALCULATE, json!({"input": input}));
    let pending =
        data(&answer, "subscriptionContractCreateCalculate")["subscriptionContractCalculation"]
            .clone();
    assert_eq!(
        pending["__typename"],
        json!("SubscriptionContractCalculationPending")
    );
    assert_eq!(topics(&mut shop), Vec::<String>::new());
    let read = run(&mut shop, READ_CALCULATION, json!({"id": pending["id"]}));
    let success = &read["data"]["subscriptionContractCalculation"];
    assert_eq!(
        success["__typename"],
        json!("SubscriptionContractCalculationSuccess")
    );
    assert_eq!(
        success["projectedOrderTotals"],
        json!({"subtotal": {"amount": "18.0"}, "totalDelivery": {"amount": "5.0"}, "totalDeliveryDiscounts": {"amount": "5.0"}, "total": {"amount": "18.0"}})
    );
    assert_eq!(
        success["calculatedContract"]["lines"]["nodes"][0]["currentPrice"]["amount"],
        json!("9.0")
    );
    let answer = run(&mut shop, CALCULATION_COMMIT, json!({"id": pending["id"]}));
    let contract = data(&answer, "subscriptionContractCalculationCommit")["contract"].clone();
    assert_eq!(contract["__typename"], json!("SubscriptionContract"));
    assert_eq!(contract["status"], json!("ACTIVE"));
    assert_eq!(contract["nextBillingDate"], json!("2026-02-15T12:00:00Z"));
    assert_eq!(topics(&mut shop), ["subscription_contracts/create"]);
    let errors = refused(
        &mut shop,
        CALCULATION_COMMIT,
        json!({"id": pending["id"]}),
        "subscriptionContractCalculationCommit",
    );
    assert_eq!(
        errors[0]["message"],
        json!("The calculation has already been committed")
    );

    // A customer that is not one: refused. A variant that is not one: a failure to poll, which
    // does not commit.
    let mut input = calculation_input(&shop);
    input["customerId"] = json!("gid://shopify/Customer/1");
    refused(
        &mut shop,
        CREATE_CALCULATE,
        json!({"input": input}),
        "subscriptionContractCreateCalculate",
    );
    let mut input = calculation_input(&shop);
    input["lines"][0]["productVariantLine"]["productVariantId"] =
        json!("gid://shopify/ProductVariant/1");
    let answer = run(&mut shop, CREATE_CALCULATE, json!({"input": input}));
    let failed = data(&answer, "subscriptionContractCreateCalculate")["subscriptionContractCalculation"]["id"].clone();
    let read = run(&mut shop, READ_CALCULATION, json!({"id": failed}));
    assert_eq!(
        read["data"]["subscriptionContractCalculation"]["errors"][0]["code"],
        json!("MERCHANDISE_NOT_FOUND")
    );
    let errors = refused(
        &mut shop,
        CALCULATION_COMMIT,
        json!({"id": failed}),
        "subscriptionContractCalculationCommit",
    );
    assert_eq!(errors[0]["code"], json!("NOT_READY_TO_COMMIT"));
}

#[test]
fn an_update_calculation_restates_the_lines_and_keeps_the_rest() {
    let mut shop = shop();
    let id = contract(&mut shop, None);
    let line = shop.get(&id).unwrap()["lines"][0]["id"].clone();
    let variant = variant(&shop);
    let update = r#"mutation($id: ID!, $input: SubscriptionContractCalculationContractUpdateInput!) { subscriptionContractUpdateCalculate(contractId: $id, contractUpdateInput: $input) { subscriptionContractCalculation { ... on SubscriptionContractCalculationPending { id } } userErrors { field message code } } }"#;
    let answer = run(
        &mut shop,
        update,
        json!({"id": id, "input": {"withMerchandiseCustomizations": false, "lines": [{"productVariantLine": {"id": line, "productVariantId": variant, "quantity": 5, "discounts": []}}]}}),
    );
    let calculation = data(&answer, "subscriptionContractUpdateCalculate")["subscriptionContractCalculation"]["id"].clone();
    let read = run(&mut shop, READ_CALCULATION, json!({"id": calculation}));
    let calculated = &read["data"]["subscriptionContractCalculation"]["calculatedContract"];
    // Without `priceOverride`, priced from the variant; the delivery and policies kept.
    assert_eq!(
        calculated["lines"]["nodes"],
        json!([{"id": line, "quantity": 5, "currentPrice": {"amount": "16.0"}}])
    );
    assert_eq!(calculated["deliveryPrice"]["amount"], json!("5.0"));
    assert_eq!(
        calculated["billingPolicy"],
        json!({"interval": "MONTH", "intervalCount": 1})
    );
    shop.take_events();
    let answer = run(&mut shop, CALCULATION_COMMIT, json!({"id": calculation}));
    assert_eq!(
        data(&answer, "subscriptionContractCalculationCommit")["contract"]["lines"]["nodes"],
        json!([{"quantity": 5}])
    );
    assert_eq!(topics(&mut shop), ["subscription_contracts/update"]);
    let errors = refused(
        &mut shop,
        update,
        json!({"id": id, "input": {"withMerchandiseCustomizations": false, "lines": [{"productVariantLine": {"id": "gid://shopify/SubscriptionLine/x", "productVariantId": variant, "quantity": 1, "discounts": []}}]}}),
        "subscriptionContractUpdateCalculate",
    );
    assert_eq!(
        errors[0]["field"],
        json!([
            "contractUpdateInput",
            "lines",
            "0",
            "productVariantLine",
            "id"
        ])
    );
    run(
        &mut shop,
        "mutation($id: ID!) { subscriptionContractCancel(subscriptionContractId: $id) { userErrors { message } } }",
        json!({"id": id}),
    );
    let errors = refused(
        &mut shop,
        update,
        json!({"id": id, "input": {"withMerchandiseCustomizations": false}}),
        "subscriptionContractUpdateCalculate",
    );
    assert_eq!(errors[0]["code"], json!("CONTRACT_TERMINATED"));
}

#[test]
fn a_cycle_edit_calculation_edits_that_cycle_only() {
    let mut shop = shop();
    let method = card(&mut shop, "4242424242424242");
    let id = contract(&mut shop, Some(&method));
    let variant = variant(&shop);
    let edit = r#"mutation($id: ID!, $selector: SubscriptionBillingCycleSelector!, $input: SubscriptionContractCalculationBillingCycleEditInput!) { subscriptionBillingCycleContractEditCalculate(contractId: $id, billingCycleSelector: $selector, billingCycleEditInput: $input) { subscriptionContractCalculation { ... on SubscriptionContractCalculationPending { id } } userErrors { field message code } } }"#;
    let input = json!({"withMerchandiseCustomizations": false, "lines": [{"productVariantLine": {"productVariantId": variant, "quantity": 1, "priceOverride": {"amount": "10.00", "currencyCode": "USD"}, "discounts": []}}]});
    let answer = run(
        &mut shop,
        edit,
        json!({"id": id, "selector": {"index": 2}, "input": input}),
    );
    let calculation = data(&answer, "subscriptionBillingCycleContractEditCalculate")["subscriptionContractCalculation"]["id"].clone();
    shop.take_events();
    let answer = run(&mut shop, CALCULATION_COMMIT, json!({"id": calculation}));
    let edited = data(&answer, "subscriptionContractCalculationCommit")["contract"].clone();
    assert_eq!(
        edited["__typename"],
        json!("SubscriptionBillingCycleEditedContract")
    );
    assert_eq!(
        edited["billingCycles"]["nodes"],
        json!([{"cycleIndex": 2, "edited": true}])
    );
    assert_eq!(
        topics(&mut shop),
        ["subscription_billing_cycle_edits/create"]
    );
    // Cycle 2 bills one pair; the contract keeps two.
    at(&mut shop, "2026-03-15T12:00:00Z");
    let answer = run(
        &mut shop,
        ATTEMPT,
        json!({"id": id, "input": {"idempotencyKey": "c2", "billingCycleSelector": {"index": 2}}}),
    );
    let attempt =
        data(&answer, "subscriptionBillingAttemptCreate")["subscriptionBillingAttempt"]["id"]
            .clone();
    let read = run(&mut shop, READ_ATTEMPT, json!({"id": attempt}));
    assert_eq!(
        read["data"]["subscriptionBillingAttempt"]["order"]["lineItems"]["nodes"][0]["quantity"],
        json!(1)
    );
    assert_eq!(shop.get(&id).unwrap()["lines"][0]["quantity"], json!(2));
    // The edited contract of the cycle lists the order its attempt made.
    let order = read["data"]["subscriptionBillingAttempt"]["order"]["id"].clone();
    let cycle = run(
        &mut shop,
        "query($id: ID!) { subscriptionBillingCycle(billingCycleInput: {contractId: $id, selector: {index: 2}}) { editedContract { orders(first: 2) { nodes { id } } } } }",
        json!({"id": id}),
    );
    assert_eq!(
        cycle["data"]["subscriptionBillingCycle"]["editedContract"]["orders"]["nodes"],
        json!([{"id": order}])
    );
    // A contract with a current or future contract edit is not updated.
    at(&mut shop, "2026-01-20T12:00:00Z");
    let errors = refused(
        &mut shop,
        "mutation($id: ID!) { subscriptionContractUpdate(contractId: $id) { draft { id } userErrors { field message code } } }",
        json!({"id": id}),
        "subscriptionContractUpdate",
    );
    assert_eq!(errors[0]["code"], json!("HAS_FUTURE_EDITS"));
    let errors = refused(
        &mut shop,
        edit,
        json!({"id": id, "selector": {"index": 99}, "input": input}),
        "subscriptionBillingCycleContractEditCalculate",
    );
    assert_eq!(errors[0]["code"], json!("INVALID"));
}

// ---- billing cycle drafts -------------------------------------------------------------------

#[test]
fn a_cycle_draft_commits_an_edit_of_its_cycle_and_concatenates_others() {
    let mut shop = shop();
    let method = card(&mut shop, "4242424242424242");
    let anchor = contract(&mut shop, Some(&method));
    let member = contract(&mut shop, Some(&method));
    let edit = r#"mutation($input: SubscriptionBillingCycleInput!) { subscriptionBillingCycleContractEdit(billingCycleInput: $input) { draft { id billingCycle { cycleIndex } } userErrors { field message code } } }"#;
    let answer = run(
        &mut shop,
        edit,
        json!({"input": {"contractId": anchor, "selector": {"index": 1}}}),
    );
    let draft = data(&answer, "subscriptionBillingCycleContractEdit")["draft"].clone();
    assert_eq!(draft["billingCycle"], json!({"cycleIndex": 1}));
    let errors = refused(
        &mut shop,
        edit,
        json!({"input": {"contractId": anchor, "selector": {"index": 1, "date": "2026-01-20T00:00:00Z"}}}),
        "subscriptionBillingCycleContractEdit",
    );
    assert_eq!(errors[0]["code"], json!("CYCLE_SELECTOR_VALIDATE_ONE_OF"));
    // Not with `subscriptionDraftCommit`.
    let errors = refused(
        &mut shop,
        COMMIT,
        json!({"draft": draft["id"]}),
        "subscriptionDraftCommit",
    );
    assert_eq!(errors[0]["code"], json!("BILLING_CYCLE_PRESENT"));

    let concatenate = r#"mutation($draft: ID!, $cycles: [SubscriptionBillingCycleInput!]!) { subscriptionBillingCycleContractDraftConcatenate(draftId: $draft, concatenatedBillingCycleContracts: $cycles) { draft { lines(first: 5) { nodes { concatenatedOriginContract { id } } } concatenatedBillingCycles(first: 5) { nodes { cycleIndex sourceContract { id } } } } userErrors { field message code } } }"#;
    let errors = refused(
        &mut shop,
        concatenate,
        json!({"draft": draft["id"], "cycles": [{"contractId": anchor, "selector": {"index": 1}}]}),
        "subscriptionBillingCycleContractDraftConcatenate",
    );
    assert_eq!(errors[0]["code"], json!("DUPLICATE_CONCATENATED_CONTRACTS"));
    let answer = run(
        &mut shop,
        concatenate,
        json!({"draft": draft["id"], "cycles": [{"contractId": member, "selector": {"index": 1}}]}),
    );
    let concatenated =
        data(&answer, "subscriptionBillingCycleContractDraftConcatenate")["draft"].clone();
    assert_eq!(
        concatenated["lines"]["nodes"],
        json!([{"concatenatedOriginContract": null}, {"concatenatedOriginContract": {"id": member}}])
    );
    assert_eq!(
        concatenated["concatenatedBillingCycles"]["nodes"],
        json!([{"cycleIndex": 1, "sourceContract": {"id": member}}])
    );

    let commit = r#"mutation($draft: ID!) { subscriptionBillingCycleContractDraftCommit(draftId: $draft) { contract { lines(first: 5) { nodes { quantity } } } userErrors { field message code } } }"#;
    shop.take_events();
    let answer = run(&mut shop, commit, json!({"draft": draft["id"]}));
    assert_eq!(
        data(&answer, "subscriptionBillingCycleContractDraftCommit")["contract"]["lines"]["nodes"],
        json!([{"quantity": 2}, {"quantity": 2}])
    );
    assert_eq!(
        topics(&mut shop),
        [
            "subscription_billing_cycle_edits/create",
            "subscription_billing_cycle_edits/create"
        ]
    );
    let plain = self::draft(&mut shop);
    let errors = refused(
        &mut shop,
        commit,
        json!({"draft": plain}),
        "subscriptionBillingCycleContractDraftCommit",
    );
    assert_eq!(errors[0]["code"], json!("BILLING_CYCLE_ABSENT"));

    // One attempt on the member's cycle bills the group in one order, linked to both.
    at(&mut shop, "2026-02-15T12:00:00Z");
    let (_, attempt) = bill(&mut shop, &member, "group");
    let order = attempt["order"]["id"].clone();
    assert_eq!(
        attempt["order"]["lineItems"]["nodes"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    for contract in [&anchor, &member] {
        assert_eq!(shop.get(contract).unwrap()["orders"], json!([order]));
    }
}

fn draft_of_contract(shop: &mut ShopState, contract: &str) -> Value {
    let answer = run(
        shop,
        "mutation($id: ID!) { subscriptionContractUpdate(contractId: $id) { draft { id } userErrors { message } } }",
        json!({"id": contract}),
    );
    data(&answer, "subscriptionContractUpdate")["draft"]["id"].clone()
}

#[test]
fn a_code_discount_of_the_shop_applies_to_a_draft() {
    let mut shop = shop();
    let created = run(
        &mut shop,
        r#"mutation { discountCodeBasicCreate(basicCodeDiscount: {title: "Ten off", code: "SUB10", startsAt: "2026-01-01T00:00:00Z", customerSelection: {all: true}, customerGets: {value: {percentage: 0.1}, items: {all: true}, appliesOnSubscription: true}}) { userErrors { field message } } }"#,
        json!({}),
    );
    assert_eq!(
        created["data"]["discountCodeBasicCreate"]["userErrors"],
        json!([])
    );
    let id = contract(&mut shop, None);
    let draft = draft_of_contract(&mut shop, &id);
    let code = r#"mutation($draft: ID!) { subscriptionDraftDiscountCodeApply(draftId: $draft, redeemCode: "sub10") { appliedDiscount { redeemCode rejectionReason } userErrors { message } } }"#;
    let answer = run(&mut shop, code, json!({"draft": draft}));
    assert_eq!(
        data(&answer, "subscriptionDraftDiscountCodeApply")["appliedDiscount"],
        json!({"redeemCode": "sub10", "rejectionReason": null})
    );
    let answer = run(&mut shop, COMMIT, json!({"draft": draft}));
    let discounts = &data(&answer, "subscriptionDraftCommit")["contract"]["discounts"]["nodes"];
    assert_eq!(discounts[0]["type"], json!("CODE_DISCOUNT"));
    assert_eq!(discounts[0]["title"], json!("Ten off"));
    assert_eq!(
        shop.get(&id).unwrap()["lines"][0]["lineDiscountedPrice"]["amount"],
        json!("18.0")
    );
}

// ---- the store and the webhooks -------------------------------------------------------------

#[test]
fn every_payload_of_the_family_is_built_from_the_shops_records() {
    let mut shop = shop();
    let method = card(&mut shop, "4242424242424242");
    let id = contract(&mut shop, Some(&method));
    let (made, _) = bill(&mut shop, &id, "a");
    let lookup = crate::admin::api::samples::ShopLookup(&shop);
    let contract = crate::admin::webhooks::payload::payload(
        "subscription_contracts/create",
        shop.get(&id).unwrap(),
        &lookup,
    )
    .unwrap();
    assert_eq!(contract["status"], json!("active"));
    assert_eq!(
        contract["billing_policy"],
        json!({"interval": "month", "interval_count": 1, "min_cycles": 1, "max_cycles": 12})
    );
    assert_eq!(contract["revision_id"], json!("1"));
    let attempt = made["subscriptionBillingAttempt"]["id"].as_str().unwrap();
    let payload = crate::admin::webhooks::payload::payload(
        "subscription_billing_attempts/success",
        shop.get(attempt).unwrap(),
        &lookup,
    )
    .unwrap();
    assert_eq!(payload["idempotency_key"], json!("a"));
    assert_eq!(payload["ready"], json!(true));
    assert!(payload["order_id"].as_u64().is_some());
    let method_payload = crate::admin::webhooks::payload::payload(
        "customer_payment_methods/create",
        shop.get(&method).unwrap(),
        &lookup,
    )
    .unwrap();
    assert_eq!(
        method_payload["token"],
        json!(method.rsplit('/').next().unwrap())
    );
    assert_eq!(
        method_payload["payment_instrument"],
        json!({"last_digits": "4242", "month": 8, "year": 2030, "name": "Jane Doe", "brand": "Visa"})
    );
}

#[test]
fn the_familys_records_do_not_reach_the_store_data() {
    let (store, _) = lsf_core::store::load::load(
        &lsf_core::store::load::DataSource::Demo,
        &Default::default(),
    );
    let mut shop = ShopState::seeded(&store, None);
    shop.set_clock(Clock::Frozen(
        Utc.with_ymd_and_hms(2026, 1, 15, 12, 0, 0).unwrap(),
    ));
    let method = card(&mut shop, "4242424242424242");
    contract(&mut shop, Some(&method));
    let draft = draft(&mut shop);
    let _ = draft;
    assert!(crate::admin::sync::overlay(&shop, &store).is_none());
}

/// A selection of every field of a type that takes no required argument, `depth` levels down
/// (connections through `nodes`, abstract types through their possible types).
fn selection(type_name: &str, depth: usize) -> String {
    let schema = crate::admin::schema::admin();
    let Some(def) = schema.get(type_name) else {
        return String::new();
    };
    // The family's types and what they hold; the records of other families are theirs.
    let own = [
        "Subscription",
        "CustomerPaymentMethod",
        "CustomerCreditCard",
        "CustomerPaypal",
        "CustomerShopPay",
        "CustomerPaymentInstrument",
        "SellingPlan",
        "MoneyV2",
        "MailingAddress",
        "Attribute",
        "Count",
        "Weight",
    ]
    .iter()
    .any(|prefix| type_name.starts_with(prefix));
    if !own {
        return "__typename".into();
    }
    if def.is_abstract() {
        if depth == 0 {
            return "__typename".into();
        }
        let parts: Vec<String> = schema
            .possible_types(type_name)
            .iter()
            .map(|possible| format!("... on {possible} {{ {} }}", selection(possible, depth)))
            .collect();
        return format!("__typename {}", parts.join(" "));
    }
    let mut parts = vec!["__typename".to_string()];
    for (name, field) in &def.fields {
        if field.args.values().any(|arg| arg.is_required()) {
            continue;
        }
        let named = field.ty.named();
        let Some(target) = schema.get(named) else {
            continue;
        };
        // Aliased by the type: the fields of two possible types of one union may differ.
        let alias = format!("{type_name}_{name}: {name}");
        if target.is_leaf() {
            parts.push(alias);
        } else if depth > 0 {
            if schema.is_connection(named) {
                let node = schema
                    .connection_node(named)
                    .unwrap_or_default()
                    .to_string();
                parts.push(format!(
                    "{alias}(first: 2) {{ nodes {{ {} }} }}",
                    selection(&node, depth - 1)
                ));
            } else {
                parts.push(format!("{alias} {{ {} }}", selection(named, depth - 1)));
            }
        }
    }
    parts.join(" ")
}

#[test]
fn the_fields_of_the_family_are_answered_from_the_records() {
    let mut shop = shop();
    let method = card(&mut shop, "4242424242424242");
    let id = contract(&mut shop, Some(&method));
    let (made, _) = bill(&mut shop, &id, "a");
    let attempt = made["subscriptionBillingAttempt"]["id"].clone();
    let draft = draft_of_contract(&mut shop, &id);
    let input = calculation_input(&shop);
    let answer = run(&mut shop, CREATE_CALCULATE, json!({"input": input}));
    let calculation = data(&answer, "subscriptionContractCreateCalculate")["subscriptionContractCalculation"]["id"].clone();
    let variables = json!({"id": id, "attempt": attempt, "method": method, "draft": draft, "calculation": calculation});
    let mut defaulted = Vec::new();
    for (root, variable, type_name) in [
        (
            "subscriptionContract(id: $id)",
            "id",
            "SubscriptionContract",
        ),
        (
            "subscriptionBillingAttempt(id: $id)",
            "attempt",
            "SubscriptionBillingAttempt",
        ),
        (
            "customerPaymentMethod(id: $id)",
            "method",
            "CustomerPaymentMethod",
        ),
        ("subscriptionDraft(id: $id)", "draft", "SubscriptionDraft"),
        (
            "subscriptionContractCalculation(id: $id)",
            "calculation",
            "SubscriptionContractCalculation",
        ),
        (
            "subscriptionBillingCycle(billingCycleInput: {contractId: $id, selector: {index: 1}})",
            "id",
            "SubscriptionBillingCycle",
        ),
    ] {
        let query = format!(
            "query($id: ID!) {{ {root} {{ {} }} }}",
            selection(type_name, 3)
        );
        let answer = run(&mut shop, &query, json!({"id": variables[variable]}));
        if let Some(fields) = answer["extensions"]["lsf"]["defaultedFields"].as_array() {
            defaulted.extend(fields.iter().cloned());
        }
    }
    assert!(defaulted.is_empty(), "{defaulted:#?}");
}

#[test]
fn reads_decimal_inputs_in_cents() {
    // A `Decimal` input: a string rounded half away from zero at the cent, a number as a float.
    use super::model::decimal_cents;
    assert_eq!(decimal_cents(Some(&json!("9.99"))), Some(999));
    assert_eq!(decimal_cents(Some(&json!(" 0.125 "))), Some(13));
    assert_eq!(decimal_cents(Some(&json!("-0.125"))), Some(-13));
    assert_eq!(decimal_cents(Some(&json!(9.99))), Some(999));
    assert_eq!(decimal_cents(Some(&json!(-0.125))), Some(-13));
    assert_eq!(decimal_cents(Some(&json!("x"))), None);
    assert_eq!(decimal_cents(Some(&json!({"amount": "1"}))), None);
    assert_eq!(decimal_cents(None), None);
}
