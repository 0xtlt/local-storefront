//! The app family through the resolver: billing, discounts, the Functions' owners, staged
//! uploads and bulk operations, over the rich fixture and its app. What changed, the payloads,
//! the user errors (and that they change nothing), and the events of each mutation.

mod support;

use lsf_admin::domains::app::{billing, uploads};
use lsf_admin::graphql::{self, Request};
use lsf_admin::resolve::{Registry, ShopResolver};
use lsf_admin::state::ShopState;
use lsf_admin::sync;
use lsf_core::Store;
use serde_json::{Map, Value, json};

const ORIGIN: &str = "http://127.0.0.1:9292";

fn rich() -> (Store, ShopState) {
    let store = support::fixture_store("rich");
    let app = support::fixture_app();
    let state = ShopState::seeded(&store, Some(&app));
    (store, state)
}

/// The answer to a request, `extensions` without `cost`.
fn run(shop: &mut ShopState, query: &str, variables: Value) -> Value {
    let mut resolver = ShopResolver::new(shop, Registry::standard()).with_origin(ORIGIN);
    let variables = match variables {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    let request = Request {
        query: query.to_string(),
        variables,
        operation_name: None,
    };
    let mut answer = graphql::run(lsf_admin::schema::admin(), &mut resolver, &request, None);
    answer.extensions.extend(resolver.extensions());
    answer.extensions.remove("cost");
    answer.to_json()
}

/// The topics of the events taken from the shop, without lsf's own change marker.
fn topics(shop: &mut ShopState) -> Vec<String> {
    shop.take_events()
        .into_iter()
        .map(|event| event.topic)
        .filter(|topic| topic != lsf_admin::domains::app::CHANGED)
        .collect()
}

fn data<'v>(answer: &'v Value, field: &str) -> &'v Value {
    let value = &answer["data"][field];
    assert!(!value.is_null(), "no {field}: {answer}");
    value
}

const SUBSCRIPTION_CREATE: &str = r#"mutation AppSubscriptionCreate($name: String!, $returnUrl: URL!, $test: Boolean, $trialDays: Int, $replacementBehavior: AppSubscriptionReplacementBehavior, $lineItems: [AppSubscriptionLineItemInput!]!) {
  appSubscriptionCreate(name: $name, returnUrl: $returnUrl, test: $test, trialDays: $trialDays, replacementBehavior: $replacementBehavior, lineItems: $lineItems) {
    appSubscription { id name status test trialDays currentPeriodEnd returnUrl
      lineItems { id plan { pricingDetails { __typename
        ... on AppRecurringPricing { price { amount currencyCode } interval discount { priceAfterDiscount { amount } value { ... on AppSubscriptionDiscountPercentage { percentage } } } }
        ... on AppUsagePricing { balanceUsed { amount currencyCode } cappedAmount { amount currencyCode } terms } } } } }
    confirmationUrl
    userErrors { field message }
  }
}"#;

fn subscription_variables(name: &str) -> Value {
    json!({
        "name": name,
        "returnUrl": "http://localhost:3000/app/billing?x=1",
        "test": true,
        "trialDays": 7,
        "lineItems": [
            {"plan": {"appRecurringPricingDetails": {"price": {"amount": 10, "currencyCode": "USD"}, "interval": "EVERY_30_DAYS",
                "discount": {"value": {"percentage": 0.2}, "durationLimitInIntervals": 3}}}},
            {"plan": {"appUsagePricingDetails": {"terms": "$1 per 100 emails", "cappedAmount": {"amount": "20", "currencyCode": "USD"}}}}
        ]
    })
}

/// Creates a subscription and approves it: its GID, its usage line's GID, its charge number.
fn active_subscription(shop: &mut ShopState, name: &str) -> (String, String, u64) {
    let created = run(shop, SUBSCRIPTION_CREATE, subscription_variables(name));
    let payload = data(&created, "appSubscriptionCreate");
    let id = payload["appSubscription"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let usage = payload["appSubscription"]["lineItems"][1]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let number = lsf_admin::state::parse_gid(&id).unwrap().1;
    billing::decide(shop, number, billing::Decision::Approve).unwrap();
    shop.take_events();
    (id, usage, number)
}

#[test]
fn a_subscription_is_created_pending_and_approved_on_the_confirmation_page() {
    let (_, mut shop) = rich();
    let created = run(
        &mut shop,
        SUBSCRIPTION_CREATE,
        subscription_variables("Pro"),
    );
    let payload = data(&created, "appSubscriptionCreate");
    assert_eq!(payload["userErrors"], json!([]), "{created}");
    let subscription = &payload["appSubscription"];
    assert_eq!(subscription["status"], json!("PENDING"));
    assert_eq!(subscription["test"], json!(true));
    assert_eq!(subscription["trialDays"], json!(7));
    assert_eq!(subscription["currentPeriodEnd"], Value::Null);
    let id = subscription["id"].as_str().unwrap();
    let number = lsf_admin::state::parse_gid(id).unwrap().1;
    assert_eq!(
        payload["confirmationUrl"],
        json!(format!(
            "{ORIGIN}/__lsf/admin/charges/{number}?shop={}",
            shop.domain()
        ))
    );
    let lines = subscription["lineItems"].as_array().unwrap();
    assert_eq!(
        lines[0]["id"],
        json!(format!(
            "gid://shopify/AppSubscriptionLineItem/{number}?v=1&index=0"
        ))
    );
    assert_eq!(
        lines[0]["plan"]["pricingDetails"],
        json!({"__typename": "AppRecurringPricing", "price": {"amount": "10.0", "currencyCode": "USD"},
            "interval": "EVERY_30_DAYS", "discount": {"priceAfterDiscount": {"amount": "8.0"}, "value": {"percentage": 0.2}}})
    );
    assert_eq!(
        lines[1]["plan"]["pricingDetails"],
        json!({"__typename": "AppUsagePricing", "balanceUsed": {"amount": "0.0", "currencyCode": "USD"},
            "cappedAmount": {"amount": "20.0", "currencyCode": "USD"}, "terms": "$1 per 100 emails"})
    );
    // Waiting for the merchant is no status change: no webhook.
    assert!(topics(&mut shop).is_empty());

    let installation = "{ currentAppInstallation { activeSubscriptions { id } allSubscriptions(first: 5) { nodes { id status } } } }";
    let before = run(&mut shop, installation, Value::Null);
    assert_eq!(
        before["data"]["currentAppInstallation"]["activeSubscriptions"],
        json!([])
    );

    let charge = billing::charge(&shop, number).unwrap();
    assert!(charge.decidable());
    assert_eq!(charge.lines.len(), 2);
    let redirect = billing::decide(&mut shop, number, billing::Decision::Approve).unwrap();
    assert_eq!(
        redirect,
        format!("http://localhost:3000/app/billing?x=1&charge_id={number}")
    );
    assert_eq!(topics(&mut shop), ["app_subscriptions/update"]);
    let after = run(&mut shop, installation, Value::Null);
    let installation = &after["data"]["currentAppInstallation"];
    assert_eq!(installation["activeSubscriptions"], json!([{"id": id}]));
    assert_eq!(
        installation["allSubscriptions"]["nodes"],
        json!([{"id": id, "status": "ACTIVE"}])
    );
    let period = run(
        &mut shop,
        &format!(
            r#"{{ node(id: "{id}") {{ ... on AppSubscription {{ status currentPeriodEnd }} }} }}"#
        ),
        Value::Null,
    );
    // The trial's end: 7 days after the frozen clock of the data.
    assert_eq!(
        period["data"]["node"],
        json!({"status": "ACTIVE", "currentPeriodEnd": "2026-03-22T10:00:00Z"})
    );
    // Decided once: there is nothing more to decide.
    assert_eq!(
        billing::decide(&mut shop, number, billing::Decision::Decline),
        Err(billing::DecisionError::NotPending("ACTIVE".to_string()))
    );
}

#[test]
fn approving_a_new_subscription_cancels_the_active_one_and_declining_keeps_it() {
    let (_, mut shop) = rich();
    let (first, _, _) = active_subscription(&mut shop, "Basic");
    let created = run(
        &mut shop,
        SUBSCRIPTION_CREATE,
        subscription_variables("Pro"),
    );
    let second = data(&created, "appSubscriptionCreate")["appSubscription"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let number = lsf_admin::state::parse_gid(&second).unwrap().1;
    shop.take_events();
    billing::decide(&mut shop, number, billing::Decision::Approve).unwrap();
    let events: Vec<(String, String)> = shop
        .take_events()
        .into_iter()
        .map(|event| (event.topic, event.gid))
        .collect();
    assert_eq!(
        events,
        [
            ("app_subscriptions/update".to_string(), first.clone()),
            ("app_subscriptions/update".to_string(), second.clone()),
        ]
    );
    let statuses = run(
        &mut shop,
        &format!(
            r#"{{ a: node(id: "{first}") {{ ... on AppSubscription {{ status }} }} b: node(id: "{second}") {{ ... on AppSubscription {{ status }} }} }}"#
        ),
        Value::Null,
    );
    assert_eq!(
        statuses["data"],
        json!({"a": {"status": "CANCELLED"}, "b": {"status": "ACTIVE"}})
    );

    let third = run(
        &mut shop,
        SUBSCRIPTION_CREATE,
        subscription_variables("Max"),
    );
    let third = data(&third, "appSubscriptionCreate")["appSubscription"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let number = lsf_admin::state::parse_gid(&third).unwrap().1;
    billing::decide(&mut shop, number, billing::Decision::Decline).unwrap();
    let statuses = run(
        &mut shop,
        &format!(
            r#"{{ a: node(id: "{second}") {{ ... on AppSubscription {{ status }} }} b: node(id: "{third}") {{ ... on AppSubscription {{ status }} }} }}"#
        ),
        Value::Null,
    );
    assert_eq!(
        statuses["data"],
        json!({"a": {"status": "ACTIVE"}, "b": {"status": "DECLINED"}})
    );
}

#[test]
fn subscription_errors_change_nothing() {
    let (_, mut shop) = rich();
    let before = shop.count("AppSubscription");
    let mut variables = subscription_variables(" ");
    variables["lineItems"] = json!([
        {"plan": {"appRecurringPricingDetails": {"price": {"amount": 10, "currencyCode": "USD"}}}},
        {"plan": {"appRecurringPricingDetails": {"price": {"amount": 20, "currencyCode": "USD"}}}},
    ]);
    let answer = run(&mut shop, SUBSCRIPTION_CREATE, variables);
    let payload = data(&answer, "appSubscriptionCreate");
    assert_eq!(payload["appSubscription"], Value::Null);
    assert_eq!(payload["confirmationUrl"], Value::Null);
    assert_eq!(
        payload["userErrors"],
        json!([
            {"field": ["name"], "message": "Name can't be blank"},
            {"field": ["lineItems"], "message": "Only one pricing plan can be defined for each available type."},
        ])
    );
    assert_eq!(shop.count("AppSubscription"), before);
    assert!(shop.take_events().is_empty());
}

const USAGE: &str = r#"mutation appUsageRecordCreate($description: String!, $price: MoneyInput!, $subscriptionLineItemId: ID!, $idempotencyKey: String) {
  appUsageRecordCreate(description: $description, price: $price, subscriptionLineItemId: $subscriptionLineItemId, idempotencyKey: $idempotencyKey) {
    userErrors { field message }
    appUsageRecord { id description idempotencyKey price { amount currencyCode }
      subscriptionLineItem { id plan { pricingDetails { ... on AppUsagePricing { balanceUsed { amount currencyCode } cappedAmount { amount currencyCode } terms } } } } }
  }
}"#;

#[test]
fn usage_records_are_capped_idempotent_and_warn_at_ninety_percent() {
    let (_, mut shop) = rich();
    let (subscription, usage, _) = active_subscription(&mut shop, "Pro");
    let charge = |shop: &mut ShopState, amount: &str, key: Option<&str>| {
        run(
            shop,
            USAGE,
            json!({"description": "100 emails", "price": {"amount": amount, "currencyCode": "USD"},
                "subscriptionLineItemId": usage, "idempotencyKey": key}),
        )
    };
    let first = charge(&mut shop, "10", Some("a"));
    let record = &data(&first, "appUsageRecordCreate")["appUsageRecord"];
    assert_eq!(
        record["price"],
        json!({"amount": "10.0", "currencyCode": "USD"})
    );
    assert_eq!(
        record["subscriptionLineItem"]["plan"]["pricingDetails"]["balanceUsed"],
        json!({"amount": "10.0", "currencyCode": "USD"})
    );
    assert!(topics(&mut shop).is_empty());

    // The same key: the same record, nothing charged.
    let again = charge(&mut shop, "10", Some("a"));
    assert_eq!(
        data(&again, "appUsageRecordCreate")["appUsageRecord"]["id"],
        record["id"]
    );
    assert_eq!(shop.count("AppUsageRecord"), 1);

    // Crossing 90% of the capped amount.
    let near = charge(&mut shop, "8", None);
    assert_eq!(data(&near, "appUsageRecordCreate")["userErrors"], json!([]));
    let events: Vec<(String, String)> = shop
        .take_events()
        .into_iter()
        .filter(|event| event.topic != lsf_admin::domains::app::CHANGED)
        .map(|event| (event.topic, event.gid))
        .collect();
    assert_eq!(
        events,
        [(
            "app_subscriptions/approaching_capped_amount".to_string(),
            subscription.clone()
        )]
    );

    // Over the capped amount: shopify.dev's error, nothing changed.
    let over = charge(&mut shop, "5", None);
    assert_eq!(
        data(&over, "appUsageRecordCreate"),
        &json!({"userErrors": [{"field": null, "message": "Failed to create usage charge"}], "appUsageRecord": null})
    );
    assert_eq!(shop.count("AppUsageRecord"), 2);
    let long = charge(&mut shop, "1", Some(&"k".repeat(256)));
    assert_eq!(
        data(&long, "appUsageRecordCreate")["userErrors"],
        json!([{"field": ["idempotencyKey"], "message": "Idempotency key exceeds the maximum length."}])
    );
}

#[test]
fn capped_amounts_trials_and_cancellations() {
    let (_, mut shop) = rich();
    let (subscription, usage, number) = active_subscription(&mut shop, "Pro");
    let update = run(
        &mut shop,
        r#"mutation($id: ID!, $cappedAmount: MoneyInput!) { appSubscriptionLineItemUpdate(id: $id, cappedAmount: $cappedAmount) { confirmationUrl appSubscription { id } userErrors { field message } } }"#,
        json!({"id": usage, "cappedAmount": {"amount": "50", "currencyCode": "USD"}}),
    );
    let payload = data(&update, "appSubscriptionLineItemUpdate");
    assert_eq!(payload["userErrors"], json!([]));
    assert!(
        payload["confirmationUrl"]
            .as_str()
            .unwrap()
            .contains(&format!("/charges/{number}?"))
    );
    // Until the merchant approves, the capped amount is the old one.
    let capped = |shop: &mut ShopState| {
        run(
            shop,
            &format!(r#"{{ node(id: "{subscription}") {{ ... on AppSubscription {{ lineItems {{ plan {{ pricingDetails {{ ... on AppUsagePricing {{ cappedAmount {{ amount }} }} }} }} }} }} }} }}"#),
            Value::Null,
        )["data"]["node"]["lineItems"][1]["plan"]["pricingDetails"]["cappedAmount"]["amount"]
            .clone()
    };
    assert_eq!(capped(&mut shop), json!("20.0"));
    assert_eq!(
        billing::charge(&shop, number)
            .unwrap()
            .pending_capped_amount,
        Some("50.00 USD".to_string())
    );
    shop.take_events();
    billing::decide(&mut shop, number, billing::Decision::Approve).unwrap();
    assert_eq!(topics(&mut shop), ["app_subscriptions/update"]);
    assert_eq!(capped(&mut shop), json!("50.0"));

    let extend = r#"mutation($id: ID!, $days: Int!) { appSubscriptionTrialExtend(id: $id, days: $days) { appSubscription { trialDays currentPeriodEnd } userErrors { field message code } } }"#;
    let extended = run(&mut shop, extend, json!({"id": subscription, "days": 3}));
    assert_eq!(
        data(&extended, "appSubscriptionTrialExtend"),
        &json!({"appSubscription": {"trialDays": 10, "currentPeriodEnd": "2026-03-25T10:00:00Z"}, "userErrors": []})
    );
    let missing = run(
        &mut shop,
        extend,
        json!({"id": "gid://shopify/AppSubscription/999", "days": 3}),
    );
    assert_eq!(
        data(&missing, "appSubscriptionTrialExtend")["userErrors"],
        json!([{"field": ["id"], "message": "The app subscription wasn't found.", "code": "SUBSCRIPTION_NOT_FOUND"}])
    );

    shop.take_events();
    let cancel = r#"mutation appSubscriptionCancel($id: ID!, $prorate: Boolean) { appSubscriptionCancel(id: $id, prorate: $prorate) { appSubscription { id status } userErrors { field message } } }"#;
    let cancelled = run(
        &mut shop,
        cancel,
        json!({"id": subscription, "prorate": true}),
    );
    assert_eq!(
        data(&cancelled, "appSubscriptionCancel"),
        &json!({"appSubscription": {"id": subscription, "status": "CANCELLED"}, "userErrors": []})
    );
    assert_eq!(topics(&mut shop), ["app_subscriptions/update"]);
    let not_active = run(&mut shop, extend, json!({"id": subscription, "days": 3}));
    assert_eq!(
        data(&not_active, "appSubscriptionTrialExtend")["userErrors"][0]["code"],
        json!("SUBSCRIPTION_NOT_ACTIVE")
    );
}

#[test]
fn one_time_purchases_are_approved_or_declined() {
    let (_, mut shop) = rich();
    let create = r#"mutation test($name: String!, $price: MoneyInput!, $returnUrl: URL!, $test: Boolean) {
      appPurchaseOneTimeCreate(name: $name, price: $price, returnUrl: $returnUrl, test: $test) {
        appPurchaseOneTime { id name test status price { amount currencyCode } } confirmationUrl userErrors { field message } } }"#;
    let created = run(
        &mut shop,
        create,
        json!({"name": "1000 imports", "price": {"amount": 10, "currencyCode": "USD"}, "returnUrl": "http://localhost:3000/done", "test": true}),
    );
    let payload = data(&created, "appPurchaseOneTimeCreate");
    assert_eq!(payload["appPurchaseOneTime"]["status"], json!("PENDING"));
    let id = payload["appPurchaseOneTime"]["id"].as_str().unwrap();
    let number = lsf_admin::state::parse_gid(id).unwrap().1;
    // Charges share their numbers: a subscription after it takes the next.
    let subscription = run(
        &mut shop,
        SUBSCRIPTION_CREATE,
        subscription_variables("Pro"),
    );
    let next = data(&subscription, "appSubscriptionCreate")["appSubscription"]["id"].clone();
    assert_eq!(
        next,
        json!(format!("gid://shopify/AppSubscription/{}", number + 1))
    );
    shop.take_events();
    let redirect = billing::decide(&mut shop, number, billing::Decision::Decline).unwrap();
    assert_eq!(
        redirect,
        format!("http://localhost:3000/done?charge_id={number}")
    );
    assert_eq!(topics(&mut shop), ["app_purchases_one_time/update"]);
    let check = run(
        &mut shop,
        "{ currentAppInstallation { oneTimePurchases(first: 250, sortKey: CREATED_AT) { edges { node { id name test status } } pageInfo { hasNextPage endCursor } } } }",
        Value::Null,
    );
    assert_eq!(
        check["data"]["currentAppInstallation"]["oneTimePurchases"]["edges"],
        json!([{"node": {"id": id, "name": "1000 imports", "test": true, "status": "DECLINED"}}])
    );
    let blank = run(
        &mut shop,
        create,
        json!({"name": "", "price": {"amount": 0, "currencyCode": "USD"}, "returnUrl": "http://localhost:3000/done"}),
    );
    assert_eq!(
        data(&blank, "appPurchaseOneTimeCreate")["userErrors"],
        json!([
            {"field": ["name"], "message": "Name can't be blank"},
            {"field": ["price", "amount"], "message": "Price must be greater than 0"},
        ])
    );
}

const AUTOMATIC_APP_CREATE: &str = r#"mutation discountAutomaticAppCreate($automaticAppDiscount: DiscountAutomaticAppInput!) {
  discountAutomaticAppCreate(automaticAppDiscount: $automaticAppDiscount) {
    automaticAppDiscount { discountId title status startsAt endsAt discountClasses
      appDiscountType { appKey functionId title discountClasses app { title } }
      combinesWith { orderDiscounts productDiscounts shippingDiscounts } }
    userErrors { field message code }
  }
}"#;

#[test]
fn an_app_discount_is_created_with_its_function_and_configuration_and_syncs() {
    let (store, mut shop) = rich();
    let answer = run(
        &mut shop,
        AUTOMATIC_APP_CREATE,
        json!({"automaticAppDiscount": {
            "title": "Spring sale",
            "functionHandle": "volume-discount",
            "discountClasses": ["PRODUCT"],
            "startsAt": "2026-03-01T00:00:00+01:00",
            "combinesWith": {"orderDiscounts": true},
            "metafields": [{"namespace": "$app:config", "key": "settings", "type": "json", "value": "{\"percent\": 15}"}],
        }}),
    );
    let payload = data(&answer, "discountAutomaticAppCreate");
    assert_eq!(payload["userErrors"], json!([]), "{answer}");
    let discount = &payload["automaticAppDiscount"];
    assert_eq!(discount["title"], json!("Spring sale"));
    assert_eq!(discount["status"], json!("ACTIVE"));
    assert_eq!(discount["startsAt"], json!("2026-02-28T23:00:00Z"));
    assert_eq!(discount["endsAt"], Value::Null);
    assert_eq!(discount["discountClasses"], json!(["PRODUCT"]));
    assert_eq!(
        discount["combinesWith"],
        json!({"orderDiscounts": true, "productDiscounts": false, "shippingDiscounts": false})
    );
    let function =
        lsf_admin::domains::app::function_id(Some(&support::fixture_app()), "volume-discount");
    assert_eq!(
        discount["appDiscountType"],
        json!({"appKey": "11111111111111111111111111111111", "functionId": function, "title": "Volume discount",
            "discountClasses": ["PRODUCT"], "app": {"title": "Admin fixtures"}})
    );
    let node = discount["discountId"].as_str().unwrap().to_string();
    assert_eq!(topics(&mut shop), ["discounts/create"]);

    // The configuration is the node's metafield, in the app's namespace.
    let read = run(
        &mut shop,
        &format!(
            r#"{{ discountNode(id: "{}") {{ id discount {{ __typename ... on DiscountAutomaticApp {{ title }} }} metafield(namespace: "$app:config", key: "settings") {{ jsonValue ownerType }} }} }}"#,
            node.replace("DiscountAutomaticNode", "DiscountNode")
        ),
        Value::Null,
    );
    assert_eq!(
        read["data"]["discountNode"],
        json!({"id": node.replace("DiscountAutomaticNode", "DiscountNode"),
            "discount": {"__typename": "DiscountAutomaticApp", "title": "Spring sale"},
            "metafield": {"jsonValue": {"percent": 15}, "ownerType": "DISCOUNT"}})
    );

    // The store sync writes it back as a discount of the checkout, with its Function and its
    // settings.
    let overlay = sync::overlay(&shop, &store).expect("the discount syncs");
    let discounts = &overlay.data["checkout"]["discounts"];
    let written = discounts
        .as_array()
        .unwrap()
        .iter()
        .find(|discount| discount["title"] == json!("Spring sale"))
        .unwrap_or_else(|| panic!("not synced: {discounts}"));
    assert_eq!(written["function"], json!("volume-discount"));
    assert_eq!(written["classes"], json!(["PRODUCT"]));
    assert_eq!(
        written["metafields"]["$app:config.settings"]["value"],
        json!({"percent": 15})
    );
}

#[test]
fn app_discount_errors_change_nothing() {
    let (_, mut shop) = rich();
    let before = shop.count("DiscountAutomaticNode");
    let cases = [
        (
            json!({"title": "X", "functionHandle": "nope"}),
            json!([{"field": ["automaticAppDiscount", "functionHandle"],
                "message": "Function nope not found. Ensure that it is released in the current app (11111111111111111111111111111111), and that the app is installed.",
                "code": "INVALID"}]),
        ),
        (
            json!({"title": "X", "functionHandle": "volume-discount", "functionId": "x"}),
            json!([{"field": ["automaticAppDiscount", "functionHandle"], "message": "Only one of function ID or function handle is allowed.", "code": "MULTIPLE_FUNCTION_IDENTIFIERS"}]),
        ),
        (
            json!({"title": " ", "functionHandle": "payment-rules", "startsAt": "2026-03-02T00:00:00Z", "endsAt": "2026-03-01T00:00:00Z"}),
            json!([
                {"field": ["automaticAppDiscount", "title"], "message": "Title can't be blank", "code": "BLANK"},
                {"field": ["automaticAppDiscount", "endsAt"], "message": "Ends at needs to be after starts_at", "code": "END_DATE_BEFORE_START_DATE"},
                {"field": ["automaticAppDiscount", "functionHandle"], "message": "Function payment-rules is not a discount function: it implements the payment_customization API.", "code": "INVALID"},
            ]),
        ),
        (
            json!({"title": "X", "functionHandle": "volume-discount", "metafields": [{"namespace": "$app", "key": "cfg", "type": "json", "value": "{nope"}]}),
            json!([{"field": ["automaticAppDiscount", "metafields", "0", "value"], "message": "Value is invalid JSON: unexpected token at '{nope' (key must be a string at line 1 column 2).", "code": "INVALID"}]),
        ),
    ];
    for (input, errors) in cases {
        let answer = run(
            &mut shop,
            AUTOMATIC_APP_CREATE,
            json!({ "automaticAppDiscount": input }),
        );
        let payload = data(&answer, "discountAutomaticAppCreate");
        assert_eq!(payload["automaticAppDiscount"], Value::Null);
        assert_eq!(payload["userErrors"], errors);
    }
    assert_eq!(shop.count("DiscountAutomaticNode"), before);
    assert!(shop.take_events().is_empty());

    // A code that another discount has (`WELCOME10` of the data), whatever its case.
    let taken = run(
        &mut shop,
        r#"mutation($input: DiscountCodeAppInput!) { discountCodeAppCreate(codeAppDiscount: $input) { codeAppDiscount { title } userErrors { field message code } } }"#,
        json!({"input": {"title": "Again", "code": "welcome10", "functionHandle": "volume-discount"}}),
    );
    assert_eq!(
        data(&taken, "discountCodeAppCreate")["userErrors"],
        json!([{"field": ["codeAppDiscount", "code"], "message": "Code must be unique. Please try a different code.", "code": "TAKEN"}])
    );
}

#[test]
fn discounts_are_updated_activated_deactivated_and_deleted() {
    let (_, mut shop) = rich();
    let created = run(
        &mut shop,
        r#"mutation($input: DiscountCodeAppInput!) { discountCodeAppCreate(codeAppDiscount: $input) { codeAppDiscount { discountId codes(first: 2) { nodes { code } } usageLimit appliesOncePerCustomer } userErrors { field message } } }"#,
        json!({"input": {"title": "VIP", "code": "VIP20", "functionHandle": "volume-discount", "usageLimit": 100,
            "metafields": [{"namespace": "$app:config", "key": "settings", "type": "json", "value": "{}"}]}}),
    );
    let payload = data(&created, "discountCodeAppCreate");
    assert_eq!(
        payload["codeAppDiscount"]["codes"]["nodes"],
        json!([{"code": "VIP20"}])
    );
    assert_eq!(payload["codeAppDiscount"]["usageLimit"], json!(100));
    let node = payload["codeAppDiscount"]["discountId"]
        .as_str()
        .unwrap()
        .to_string();
    shop.take_events();

    let updated = run(
        &mut shop,
        r#"mutation($id: ID!, $input: DiscountCodeAppInput!) { discountCodeAppUpdate(id: $id, codeAppDiscount: $input) { codeAppDiscount { title codes(first: 1) { nodes { code } } } userErrors { field message } } }"#,
        json!({"id": node, "input": {"title": "VIP club", "code": "VIP25"}}),
    );
    assert_eq!(
        data(&updated, "discountCodeAppUpdate"),
        &json!({"codeAppDiscount": {"title": "VIP club", "codes": {"nodes": [{"code": "VIP25"}]}}, "userErrors": []})
    );
    assert_eq!(topics(&mut shop), ["discounts/update"]);
    let by_code = run(
        &mut shop,
        r#"{ codeDiscountNodeByCode(code: "vip25") { id } discountCodesCount { count } }"#,
        Value::Null,
    );
    assert_eq!(by_code["data"]["codeDiscountNodeByCode"]["id"], json!(node));
    assert_eq!(by_code["data"]["discountCodesCount"]["count"], json!(2));

    let deactivate = run(
        &mut shop,
        &format!(
            r#"mutation {{ discountCodeDeactivate(id: "{node}") {{ codeDiscountNode {{ codeDiscount {{ ... on DiscountCodeApp {{ status startsAt endsAt }} }} }} userErrors {{ message }} }} }}"#
        ),
        Value::Null,
    );
    assert_eq!(
        data(&deactivate, "discountCodeDeactivate")["codeDiscountNode"]["codeDiscount"],
        json!({"status": "EXPIRED", "startsAt": "2026-03-15T10:00:00Z", "endsAt": "2026-03-15T10:00:00Z"})
    );
    let activate = run(
        &mut shop,
        &format!(
            r#"mutation {{ discountCodeActivate(id: "{node}") {{ codeDiscountNode {{ codeDiscount {{ ... on DiscountCodeApp {{ status endsAt }} }} }} }} }}"#
        ),
        Value::Null,
    );
    assert_eq!(
        data(&activate, "discountCodeActivate")["codeDiscountNode"]["codeDiscount"],
        json!({"status": "ACTIVE", "endsAt": null})
    );
    assert_eq!(topics(&mut shop), ["discounts/update", "discounts/update"]);

    let metafields_before = shop.count("Metafield");
    let deleted = run(
        &mut shop,
        &format!(
            r#"mutation {{ discountCodeDelete(id: "{node}") {{ deletedCodeDiscountId userErrors {{ message }} }} }}"#
        ),
        Value::Null,
    );
    assert_eq!(
        data(&deleted, "discountCodeDelete")["deletedCodeDiscountId"],
        json!(node)
    );
    assert!(!shop.contains(&node));
    assert_eq!(shop.count("Metafield"), metafields_before - 1);
    assert_eq!(topics(&mut shop), ["discounts/delete"]);
    let gone = run(
        &mut shop,
        &format!(
            r#"mutation {{ discountCodeDelete(id: "{node}") {{ deletedCodeDiscountId userErrors {{ field message }} }} }}"#
        ),
        Value::Null,
    );
    assert_eq!(
        data(&gone, "discountCodeDelete")["userErrors"],
        json!([{"field": ["id"], "message": "Discount does not exist"}])
    );
}

#[test]
fn discount_nodes_list_search_and_sort_every_kind() {
    let (_, mut shop) = rich();
    let basic = run(
        &mut shop,
        r#"mutation($input: DiscountCodeBasicInput!) { discountCodeBasicCreate(basicCodeDiscount: $input) {
            codeDiscountNode { id codeDiscount { ... on DiscountCodeBasic { title summary discountClasses customerGets { value { ... on DiscountPercentage { percentage } } items { ... on AllDiscountItems { allItems } } } minimumRequirement { ... on DiscountMinimumSubtotal { greaterThanOrEqualToSubtotal { amount currencyCode } } } } } }
            userErrors { field message } } }"#,
        json!({"input": {"title": "Ten off", "code": "TEN", "startsAt": "2026-01-01T00:00:00Z",
            "customerGets": {"value": {"percentage": 0.1}, "items": {"all": true}},
            "minimumRequirement": {"subtotal": {"greaterThanOrEqualToSubtotal": "50.00"}}}}),
    );
    let node = &data(&basic, "discountCodeBasicCreate")["codeDiscountNode"];
    assert_eq!(
        node["codeDiscount"],
        json!({"title": "Ten off", "summary": "10% off entire order", "discountClasses": ["ORDER"],
            "customerGets": {"value": {"percentage": 0.1}, "items": {"allItems": true}},
            "minimumRequirement": {"greaterThanOrEqualToSubtotal": {"amount": "50.0", "currencyCode": "EUR"}}})
    );
    let scheduled = run(
        &mut shop,
        r#"mutation { discountAutomaticFreeShippingCreate(freeShippingAutomaticDiscount: {title: "Later", startsAt: "2027-01-01T00:00:00Z"}) { automaticDiscountNode { id } userErrors { message } } }"#,
        Value::Null,
    );
    assert_eq!(
        data(&scheduled, "discountAutomaticFreeShippingCreate")["userErrors"],
        json!([])
    );

    let list = |shop: &mut ShopState, query: &str| -> Vec<String> {
        let answer = run(
            shop,
            &format!(
                r#"{{ discountNodes(first: 10, query: "{query}", sortKey: TITLE) {{ nodes {{ id discount {{ ... on DiscountCodeBasic {{ title }} ... on DiscountCodeApp {{ title }} ... on DiscountAutomaticApp {{ title }} ... on DiscountAutomaticFreeShipping {{ title }} }} }} }} }}"#
            ),
            Value::Null,
        );
        answer["data"]["discountNodes"]["nodes"]
            .as_array()
            .unwrap_or_else(|| panic!("{answer}"))
            .iter()
            .map(|node| node["discount"]["title"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        list(&mut shop, ""),
        ["Later", "Ten off", "Volume", "Welcome"]
    );
    assert_eq!(list(&mut shop, "method:code"), ["Ten off", "Welcome"]);
    assert_eq!(list(&mut shop, "status:scheduled"), ["Later"]);
    assert_eq!(list(&mut shop, "type:app"), ["Volume", "Welcome"]);
    assert_eq!(list(&mut shop, "code:TEN"), ["Ten off"]);
    let ids = run(
        &mut shop,
        "{ discountNodes(first: 1) { nodes { id } } discountNodesCount(query: \"type:app\") { count } }",
        Value::Null,
    );
    assert_eq!(
        ids["data"],
        json!({"discountNodes": {"nodes": [{"id": "gid://shopify/DiscountNode/1"}]}, "discountNodesCount": {"count": 2}})
    );

    // Bulk: the code discounts the search finds are deleted at once.
    let bulk = run(
        &mut shop,
        r#"mutation { discountCodeBulkDelete(search: "title:Ten") { job { done } userErrors { field message code } } }"#,
        Value::Null,
    );
    assert_eq!(
        data(&bulk, "discountCodeBulkDelete"),
        &json!({"job": {"done": true}, "userErrors": []})
    );
    assert_eq!(list(&mut shop, "method:code"), ["Welcome"]);
    let both = run(
        &mut shop,
        r#"mutation { discountCodeBulkDelete(ids: ["gid://shopify/DiscountCodeNode/2"], search: "x") { job { done } userErrors { field message code } } }"#,
        Value::Null,
    );
    assert_eq!(
        data(&both, "discountCodeBulkDelete")["userErrors"],
        json!([{"field": null, "message": "Only one of 'ids', 'search' or 'saved_search_id' is allowed.", "code": "TOO_MANY_ARGUMENTS"}])
    );
}

#[test]
fn redeem_codes_are_added_in_bulk_each_failing_alone() {
    let (_, mut shop) = rich();
    shop.take_events();
    let added = run(
        &mut shop,
        r#"mutation { discountRedeemCodeBulkAdd(discountId: "gid://shopify/DiscountCodeNode/2", codes: [{code: "A1"}, {code: "WELCOME10"}, {code: "A2"}]) {
            bulkCreation { done codesCount importedCount failedCount codes(first: 5) { nodes { code errors { field message code } discountRedeemCode { code } } } }
            userErrors { message } } }"#,
        Value::Null,
    );
    let creation = &data(&added, "discountRedeemCodeBulkAdd")["bulkCreation"];
    assert_eq!(creation["done"], json!(true));
    assert_eq!(
        (
            creation["importedCount"].clone(),
            creation["failedCount"].clone()
        ),
        (json!(2), json!(1))
    );
    assert_eq!(
        creation["codes"]["nodes"][1],
        json!({"code": "WELCOME10", "errors": [{"field": ["code"], "message": "Code must be unique. Please try a different code.", "code": "TAKEN"}], "discountRedeemCode": null})
    );
    assert_eq!(
        topics(&mut shop),
        ["discounts/redeemcode_added", "discounts/redeemcode_added"]
    );
    let removed = run(
        &mut shop,
        r#"mutation { discountCodeRedeemCodeBulkDelete(discountId: "gid://shopify/DiscountCodeNode/2", search: "code:A1") { job { done } userErrors { message } } }"#,
        Value::Null,
    );
    assert_eq!(
        data(&removed, "discountCodeRedeemCodeBulkDelete")["job"]["done"],
        json!(true)
    );
    assert_eq!(topics(&mut shop), ["discounts/redeemcode_removed"]);
    let codes = run(
        &mut shop,
        r#"{ codeDiscountNode(id: "gid://shopify/DiscountCodeNode/2") { codeDiscount { ... on DiscountCodeApp { codes(first: 5) { nodes { code } } codesCount { count } } } } }"#,
        Value::Null,
    );
    assert_eq!(
        codes["data"]["codeDiscountNode"]["codeDiscount"],
        json!({"codes": {"nodes": [{"code": "WELCOME10"}, {"code": "A2"}]}, "codesCount": {"count": 2}})
    );
}

#[test]
fn bulk_discount_mutations_run_a_saved_search_of_their_resource() {
    let (_, mut shop) = rich();
    let saved = |shop: &mut ShopState, name: &str, query: &str, resource: &str| -> String {
        let made = run(
            shop,
            "mutation($input: SavedSearchCreateInput!) { savedSearchCreate(input: $input) { savedSearch { id } userErrors { message } } }",
            json!({"input": {"name": name, "query": query, "resourceType": resource}}),
        );
        data(&made, "savedSearchCreate")["savedSearch"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    shop.take_events();
    // An empty list of ids removes no code.
    let none = run(
        &mut shop,
        r#"mutation { discountCodeRedeemCodeBulkDelete(discountId: "gid://shopify/DiscountCodeNode/2", ids: []) { job { done } userErrors { message } } }"#,
        Value::Null,
    );
    assert_eq!(
        data(&none, "discountCodeRedeemCodeBulkDelete")["userErrors"],
        json!([])
    );
    assert!(topics(&mut shop).is_empty());
    // A saved search of redeem codes names the codes to remove.
    let codes = saved(
        &mut shop,
        "Welcome",
        "code:WELCOME10",
        "DISCOUNT_REDEEM_CODE",
    );
    shop.take_events();
    let removed = run(
        &mut shop,
        r#"mutation($saved: ID!) { discountCodeRedeemCodeBulkDelete(discountId: "gid://shopify/DiscountCodeNode/2", savedSearchId: $saved) { job { done } userErrors { message } } }"#,
        json!({"saved": codes}),
    );
    assert_eq!(
        data(&removed, "discountCodeRedeemCodeBulkDelete")["userErrors"],
        json!([])
    );
    assert_eq!(topics(&mut shop), ["discounts/redeemcode_removed"]);
    // A saved search of discounts names the discounts; one of customers is refused.
    let customers = saved(&mut shop, "Everyone", "title:Ten", "CUSTOMER");
    let refused = run(
        &mut shop,
        "mutation($saved: ID!) { discountCodeBulkDelete(savedSearchId: $saved) { job { done } userErrors { field message code } } }",
        json!({"saved": customers}),
    );
    assert_eq!(
        data(&refused, "discountCodeBulkDelete")["userErrors"],
        json!([{"field": ["savedSearchId"], "message": "Saved search does not exist", "code": "INVALID"}])
    );
    let discounts = saved(&mut shop, "Welcome", "title:Welcome", "PRICE_RULE");
    shop.take_events();
    let deleted = run(
        &mut shop,
        "mutation($saved: ID!) { discountCodeBulkDelete(savedSearchId: $saved) { job { done } userErrors { field message code } } }",
        json!({"saved": discounts}),
    );
    assert_eq!(
        data(&deleted, "discountCodeBulkDelete"),
        &json!({"job": {"done": true}, "userErrors": []})
    );
    assert!(topics(&mut shop).contains(&"discounts/delete".to_string()));
}

#[test]
fn function_owners_are_created_with_their_function_and_configuration() {
    let (store, mut shop) = rich();
    let functions = run(
        &mut shop,
        r#"{ shopifyFunctions(first: 10, apiType: "payment_customization") { nodes { handle apiType app { title } } } }"#,
        Value::Null,
    );
    assert_eq!(
        functions["data"]["shopifyFunctions"]["nodes"],
        json!([{"handle": "payment-rules", "apiType": "payment_customization", "app": {"title": "Admin fixtures"}}])
    );
    let create = r#"mutation($input: PaymentCustomizationInput!) { paymentCustomizationCreate(paymentCustomization: $input) {
        paymentCustomization { id title enabled functionId shopifyFunction { handle } metafield(namespace: "$app:config", key: "rules") { jsonValue } }
        userErrors { field message code } } }"#;
    let created = run(
        &mut shop,
        create,
        json!({"input": {"title": "Hide COD over 100", "enabled": true, "functionHandle": "payment-rules",
            "metafields": [{"namespace": "$app:config", "key": "rules", "type": "json", "value": "{\"hide\": [\"cod\"], \"over\": 100}"}]}}),
    );
    let payload = data(&created, "paymentCustomizationCreate");
    assert_eq!(payload["userErrors"], json!([]), "{created}");
    let customization = &payload["paymentCustomization"];
    assert_eq!(
        customization["shopifyFunction"]["handle"],
        json!("payment-rules")
    );
    assert_eq!(
        customization["metafield"]["jsonValue"],
        json!({"hide": ["cod"], "over": 100})
    );
    let id = customization["id"].as_str().unwrap().to_string();
    // The configuration is the Function's settings at the checkout.
    let overlay = sync::overlay(&shop, &store).expect("the settings sync");
    assert_eq!(
        overlay.data["checkout"]["functions"]["payment-rules"]["metafields"]["$app:config.rules"]["value"],
        json!({"hide": ["cod"], "over": 100}),
        "{:#?}",
        overlay.data["checkout"]
    );

    let errors = [
        (
            json!({"title": "X", "enabled": true}),
            json!([{"field": ["paymentCustomization", "functionHandle"], "message": "Either function_id or function_handle must be provided.", "code": "MISSING_FUNCTION_IDENTIFIER"}]),
        ),
        (
            json!({"title": "X", "enabled": true, "functionHandle": "delivery-rules"}),
            json!([{"field": ["paymentCustomization", "functionHandle"], "message": "Function does not implement the required interface.", "code": "FUNCTION_DOES_NOT_IMPLEMENT"}]),
        ),
        (
            json!({"functionHandle": "nope"}),
            json!([
                {"field": ["paymentCustomization", "functionHandle"], "message": "Function not found.", "code": "FUNCTION_NOT_FOUND"},
                {"field": ["paymentCustomization", "title"], "message": "Required input field must be present.", "code": "REQUIRED_INPUT_FIELD"},
                {"field": ["paymentCustomization", "enabled"], "message": "Required input field must be present.", "code": "REQUIRED_INPUT_FIELD"},
            ]),
        ),
    ];
    let count = shop.count("PaymentCustomization");
    for (input, expected) in errors {
        let answer = run(&mut shop, create, json!({ "input": input }));
        assert_eq!(
            data(&answer, "paymentCustomizationCreate")["userErrors"],
            expected
        );
    }
    assert_eq!(shop.count("PaymentCustomization"), count);

    let activation = run(
        &mut shop,
        r#"mutation($ids: [ID!]!) { paymentCustomizationActivation(ids: $ids, enabled: false) { ids userErrors { field message code } } }"#,
        json!({"ids": [id]}),
    );
    assert_eq!(
        data(&activation, "paymentCustomizationActivation"),
        &json!({"ids": [id], "userErrors": []})
    );
    let listed = run(
        &mut shop,
        "{ paymentCustomizations(first: 5, query: \"enabled:false\") { nodes { id title } } }",
        Value::Null,
    );
    assert_eq!(
        listed["data"]["paymentCustomizations"]["nodes"],
        json!([{"id": id, "title": "Hide COD over 100"}])
    );
    let deleted = run(
        &mut shop,
        &format!(
            r#"mutation {{ paymentCustomizationDelete(id: "{id}") {{ deletedId userErrors {{ code }} }} }}"#
        ),
        Value::Null,
    );
    assert_eq!(
        data(&deleted, "paymentCustomizationDelete"),
        &json!({"deletedId": id, "userErrors": []})
    );

    // A cart transform per Function: the seed has one for `bundles`.
    let transform = run(
        &mut shop,
        r#"mutation { cartTransformCreate(functionHandle: "bundles") { cartTransform { id } userErrors { field message code } } }"#,
        Value::Null,
    );
    assert_eq!(
        data(&transform, "cartTransformCreate")["userErrors"],
        json!([{"field": ["functionHandle"], "message": "A cart transform function already exists for the provided function_id.", "code": "FUNCTION_ALREADY_REGISTERED"}])
    );
    let validation = run(
        &mut shop,
        r#"mutation { validationCreate(validation: {functionHandle: "order-limits", enable: true, blockOnFailure: true}) { validation { title enabled blockOnFailure shopifyFunction { handle } } userErrors { message } } }"#,
        Value::Null,
    );
    assert_eq!(
        data(&validation, "validationCreate"),
        &json!({"validation": {"title": "Order limits", "enabled": true, "blockOnFailure": true, "shopifyFunction": {"handle": "order-limits"}}, "userErrors": []})
    );
}

#[test]
fn staged_upload_targets_are_on_lsf() {
    let (_, mut shop) = rich();
    let answer = run(
        &mut shop,
        r#"mutation stagedUploadsCreate($input: [StagedUploadInput!]!) { stagedUploadsCreate(input: $input) { stagedTargets { url resourceUrl parameters { name value } } userErrors { field message } } }"#,
        json!({"input": [
            {"resource": "BULK_MUTATION_VARIABLES", "filename": "vars.jsonl", "mimeType": "text/jsonl", "httpMethod": "POST"},
            {"resource": "IMAGE", "filename": "https://x.example/a/hero.jpg", "mimeType": "image/jpeg"},
            {"resource": "IMAGE", "filename": "unsupported-file.xyz", "mimeType": "application/xyz", "httpMethod": "POST"},
        ]}),
    );
    let payload = data(&answer, "stagedUploadsCreate");
    let targets = payload["stagedTargets"].as_array().unwrap();
    let bucket = format!("{ORIGIN}/__lsf/admin/uploads/{}", shop.domain());
    assert_eq!(targets[0]["url"], json!(bucket));
    assert_eq!(targets[0]["resourceUrl"], Value::Null);
    let names: Vec<&str> = targets[0]["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|parameter| parameter["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "Content-Type",
            "success_action_status",
            "acl",
            "key",
            "x-goog-date",
            "x-goog-credential",
            "x-goog-algorithm",
            "x-goog-signature",
            "policy"
        ]
    );
    let key = targets[0]["parameters"][3]["value"].as_str().unwrap();
    assert!(
        key.starts_with("tmp/") && key.ends_with("/bulk_op_vars"),
        "{key}"
    );
    // A PUT target: the object itself, signed.
    let resource = targets[1]["resourceUrl"].as_str().unwrap();
    assert!(
        resource.starts_with(&format!("{bucket}/tmp/")),
        "{resource}"
    );
    assert!(
        resource.contains("/products/") && resource.ends_with("/hero.jpg"),
        "{resource}"
    );
    assert!(
        targets[1]["url"]
            .as_str()
            .unwrap()
            .starts_with(&format!("{resource}?X-Goog-Algorithm="))
    );
    assert_eq!(
        targets[1]["parameters"],
        json!([{"name": "content_type", "value": "image/jpeg"}, {"name": "acl", "value": "private"}])
    );
    assert_eq!(
        targets[2],
        json!({"url": null, "resourceUrl": null, "parameters": []})
    );
    assert_eq!(
        payload["userErrors"],
        json!([{"field": ["input", "2", "mimeType"], "message": "unsupported-file.xyz: (application/xyz) is not a recognized format"}])
    );
    // What a client uploads is read back by its URL or its key.
    uploads::store(&mut shop, key, b"{}".to_vec(), "text/jsonl");
    assert_eq!(&*uploads::bytes(&shop, key).unwrap().bytes, b"{}");
    assert_eq!(
        &*uploads::bytes(&shop, &format!("{bucket}/{key}"))
            .unwrap()
            .bytes,
        b"{}"
    );
}

#[test]
fn a_bulk_query_writes_every_node_as_jsonl() {
    let (store, mut shop) = rich();
    let query =
        r#"{ products { edges { node { id title variants { edges { node { id sku } } } } } } }"#;
    let answer = run(
        &mut shop,
        r#"mutation($query: String!) { bulkOperationRunQuery(query: $query) { bulkOperation { id status type objectCount url } userErrors { field message code } } }"#,
        json!({ "query": query }),
    );
    let payload = data(&answer, "bulkOperationRunQuery");
    assert_eq!(payload["userErrors"], json!([]), "{answer}");
    let operation = &payload["bulkOperation"];
    // As Shopify answers it when it starts.
    assert_eq!(operation["status"], json!("CREATED"));
    assert_eq!(operation["objectCount"], json!("0"));
    assert_eq!(operation["url"], Value::Null);
    let id = operation["id"].as_str().unwrap().to_string();
    assert_eq!(topics(&mut shop), ["bulk_operations/finish"]);

    let done = run(
        &mut shop,
        &format!(
            r#"{{ bulkOperation(id: "{id}") {{ status errorCode objectCount rootObjectCount fileSize url query }} currentBulkOperation {{ id }} }}"#
        ),
        Value::Null,
    );
    let operation = &done["data"]["bulkOperation"];
    assert_eq!(operation["status"], json!("COMPLETED"));
    assert_eq!(done["data"]["currentBulkOperation"]["id"], json!(id));
    let variants: usize = store
        .products
        .iter()
        .map(|product| product.variants.len())
        .sum();
    assert_eq!(
        operation["rootObjectCount"],
        json!(store.products.len().to_string())
    );
    assert_eq!(
        operation["objectCount"],
        json!((store.products.len() + variants).to_string())
    );
    let number = lsf_admin::state::parse_gid(&id).unwrap().1;
    assert_eq!(
        operation["url"],
        json!(format!(
            "{ORIGIN}/__lsf/admin/bulk/{}/{number}.jsonl",
            shop.domain()
        ))
    );
    let file =
        lsf_admin::domains::app::bulk::result_file(&shop, &format!("{number}.jsonl")).unwrap();
    assert_eq!(operation["fileSize"], json!(file.bytes.len().to_string()));
    let lines: Vec<Value> = String::from_utf8_lossy(&file.bytes)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let first_product = &lines[0];
    assert!(first_product.get("variants").is_none(), "{first_product}");
    assert!(first_product["title"].is_string());
    assert_eq!(lines[1]["__parentId"], first_product["id"]);

    let listed = run(
        &mut shop,
        r#"{ a: bulkOperations(first: 5, query: "status:completed operation_type:query") { nodes { id } }
             b: bulkOperations(first: 5, query: "status:running") { nodes { id } } }"#,
        Value::Null,
    );
    assert_eq!(listed["data"]["a"]["nodes"], json!([{"id": id}]));
    assert_eq!(listed["data"]["b"]["nodes"], json!([]));
    assert!(listed["extensions"].get("search").is_none(), "{listed}");

    let refused = run(
        &mut shop,
        r#"mutation { bulkOperationRunQuery(query: "{ shop { name } }") { bulkOperation { id } userErrors { field message code } } }"#,
        Value::Null,
    );
    assert_eq!(
        data(&refused, "bulkOperationRunQuery"),
        &json!({"bulkOperation": null, "userErrors": [{"field": ["query"], "message": "Bulk queries must contain at least one connection.", "code": "INVALID"}]})
    );
    let cancel = run(
        &mut shop,
        &format!(
            r#"mutation {{ bulkOperationCancel(id: "{id}") {{ bulkOperation {{ status }} userErrors {{ message }} }} }}"#
        ),
        Value::Null,
    );
    assert_eq!(
        data(&cancel, "bulkOperationCancel"),
        &json!({"bulkOperation": {"status": "COMPLETED"}, "userErrors": [{"message": "A bulk operation cannot be canceled when it is completed"}]})
    );
}

#[test]
fn a_bulk_mutation_runs_once_per_line_of_its_staged_upload() {
    let (_, mut shop) = rich();
    let products = shop.count("Product");
    let key = "tmp/1/bulk/abc/bulk_op_vars";
    uploads::store(
        &mut shop,
        key,
        b"{\"product\": {\"title\": \"Bulk one\"}}\n{\"product\": {\"title\": \" \"}}\n\n{\"product\": {\"title\": \"Bulk two\"}}\n".to_vec(),
        "text/jsonl",
    );
    let mutation = "mutation call($product: ProductCreateInput!) { productCreate(product: $product) { product { title } userErrors { field message } } }";
    let answer = run(
        &mut shop,
        r#"mutation($mutation: String!, $path: String!) { bulkOperationRunMutation(mutation: $mutation, stagedUploadPath: $path) { bulkOperation { id type status } userErrors { field message code } } }"#,
        json!({"mutation": mutation, "path": key}),
    );
    let payload = data(&answer, "bulkOperationRunMutation");
    assert_eq!(payload["userErrors"], json!([]), "{answer}");
    assert_eq!(payload["bulkOperation"]["type"], json!("MUTATION"));
    assert_eq!(shop.count("Product"), products + 2);
    assert_eq!(
        topics(&mut shop)
            .iter()
            .filter(|topic| *topic == "products/create" || *topic == "bulk_operations/finish")
            .collect::<Vec<_>>(),
        [
            "products/create",
            "products/create",
            "bulk_operations/finish"
        ]
    );
    let id = payload["bulkOperation"]["id"].as_str().unwrap();
    let number = lsf_admin::state::parse_gid(id).unwrap().1;
    let file =
        lsf_admin::domains::app::bulk::result_file(&shop, &format!("{number}.jsonl")).unwrap();
    let lines: Vec<Value> = String::from_utf8_lossy(&file.bytes)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        lines,
        [
            json!({"data": {"productCreate": {"product": {"title": "Bulk one"}, "userErrors": []}}, "__lineNumber": 0}),
            json!({"data": {"productCreate": {"product": null, "userErrors": [{"field": ["title"], "message": "Title can't be blank"}]}}, "__lineNumber": 1}),
            json!({"data": {"productCreate": {"product": {"title": "Bulk two"}, "userErrors": []}}, "__lineNumber": 2}),
        ]
    );
    let missing = run(
        &mut shop,
        r#"mutation($mutation: String!) { bulkOperationRunMutation(mutation: $mutation, stagedUploadPath: "tmp/1/bulk/none/bulk_op_vars") { bulkOperation { id } userErrors { field message code } } }"#,
        json!({ "mutation": mutation }),
    );
    assert_eq!(
        data(&missing, "bulkOperationRunMutation")["userErrors"][0]["code"],
        json!("NO_SUCH_FILE")
    );
}

#[test]
fn the_app_and_its_installation_are_read() {
    let (_, mut shop) = rich();
    let answer = run(
        &mut shop,
        r#"{ app { title handle apiKey } appByHandle(handle: "admin-fixtures") { title }
            appByKey(apiKey: "11111111111111111111111111111111") { handle } other: appByKey(apiKey: "x") { handle }
            currentAppInstallation { launchUrl accessScopes { handle } app { title } activeSubscriptions { id } }
            appInstallations(first: 5) { nodes { app { apiKey } } } }"#,
        Value::Null,
    );
    let data = &answer["data"];
    assert_eq!(
        data["app"],
        json!({"title": "Admin fixtures", "handle": "admin-fixtures", "apiKey": "11111111111111111111111111111111"})
    );
    assert_eq!(data["appByHandle"]["title"], json!("Admin fixtures"));
    assert_eq!(data["appByKey"]["handle"], json!("admin-fixtures"));
    assert_eq!(data["other"], Value::Null);
    let installation = &data["currentAppInstallation"];
    assert_eq!(installation["app"]["title"], json!("Admin fixtures"));
    assert_eq!(installation["activeSubscriptions"], json!([]));
    assert_eq!(
        installation["accessScopes"][0]["handle"],
        json!("read_products")
    );
    assert_eq!(
        data["appInstallations"]["nodes"],
        json!([{"app": {"apiKey": "11111111111111111111111111111111"}}])
    );
}

#[test]
fn delivery_customizations_validations_and_cart_transforms_change_and_go() {
    let (_, mut shop) = rich();
    let created = run(
        &mut shop,
        r#"mutation { deliveryCustomizationCreate(deliveryCustomization: {functionHandle: "delivery-rules", title: "Rename express", enabled: true}) {
            deliveryCustomization { id title enabled functionId } userErrors { field message code } } }"#,
        Value::Null,
    );
    let customization = &data(&created, "deliveryCustomizationCreate")["deliveryCustomization"];
    let id = customization["id"].as_str().unwrap().to_string();
    let updated = run(
        &mut shop,
        &format!(
            r#"mutation {{ deliveryCustomizationUpdate(id: "{id}", deliveryCustomization: {{title: "Rename", functionHandle: "payment-rules"}}) {{ deliveryCustomization {{ title }} userErrors {{ field message code }} }} }}"#
        ),
        Value::Null,
    );
    assert_eq!(
        data(&updated, "deliveryCustomizationUpdate")["userErrors"],
        json!([{"field": ["deliveryCustomization", "functionHandle"], "message": "Function does not implement the required interface for this delivery customization.", "code": "FUNCTION_DOES_NOT_IMPLEMENT"}])
    );
    let renamed = run(
        &mut shop,
        &format!(
            r#"mutation {{ deliveryCustomizationUpdate(id: "{id}", deliveryCustomization: {{title: "Rename"}}) {{ deliveryCustomization {{ title enabled }} userErrors {{ code }} }} }}"#
        ),
        Value::Null,
    );
    assert_eq!(
        data(&renamed, "deliveryCustomizationUpdate"),
        &json!({"deliveryCustomization": {"title": "Rename", "enabled": true}, "userErrors": []})
    );
    let missing = run(
        &mut shop,
        r#"mutation { deliveryCustomizationDelete(id: "gid://shopify/DeliveryCustomization/999") { deletedId userErrors { field message code } } }"#,
        Value::Null,
    );
    assert_eq!(
        data(&missing, "deliveryCustomizationDelete"),
        &json!({"deletedId": null, "userErrors": [{"field": ["id"], "message": "Delivery customization not found.", "code": "DELIVERY_CUSTOMIZATION_NOT_FOUND"}]})
    );

    // A validation's `enable` defaults to false, on an update too (2026-10's input).
    let validation = shop.keys("Validation").next().unwrap().to_string();
    let disabled = run(
        &mut shop,
        &format!(
            r#"mutation {{ validationUpdate(id: "{validation}", validation: {{title: "Max 5"}}) {{ validation {{ title enabled }} userErrors {{ code }} }} }}"#
        ),
        Value::Null,
    );
    assert_eq!(
        data(&disabled, "validationUpdate")["validation"],
        json!({"title": "Max 5", "enabled": false})
    );
    let deleted = run(
        &mut shop,
        &format!(r#"mutation {{ validationDelete(id: "{validation}") {{ deletedId }} }}"#),
        Value::Null,
    );
    assert_eq!(
        data(&deleted, "validationDelete")["deletedId"],
        json!(validation)
    );

    let transform = shop.keys("CartTransform").next().unwrap().to_string();
    let gone = run(
        &mut shop,
        &format!(
            r#"mutation {{ cartTransformDelete(id: "{transform}") {{ deletedId userErrors {{ code }} }} }}"#
        ),
        Value::Null,
    );
    assert_eq!(
        data(&gone, "cartTransformDelete")["deletedId"],
        json!(transform)
    );
    let again = run(
        &mut shop,
        r#"mutation { cartTransformCreate(functionHandle: "bundles", blockOnFailure: true) { cartTransform { functionId blockOnFailure } userErrors { code } } }"#,
        Value::Null,
    );
    assert_eq!(
        data(&again, "cartTransformCreate")["cartTransform"]["blockOnFailure"],
        json!(true)
    );
}

#[test]
fn buy_x_get_y_and_free_shipping_discounts_are_records() {
    let (_, mut shop) = rich();
    let unknown = run(
        &mut shop,
        r#"mutation { discountCodeBxgyCreate(bxgyCodeDiscount: {title: "2 for 1", code: "TWOFORONE",
            customerBuys: {value: {quantity: "2"}, items: {products: {productsToAdd: ["gid://shopify/Product/1"]}}}}) {
            codeDiscountNode { id } userErrors { field message code } } }"#,
        Value::Null,
    );
    assert_eq!(
        data(&unknown, "discountCodeBxgyCreate")["userErrors"],
        json!([{"field": ["bxgyCodeDiscount", "customerBuys", "items", "products", "productsToAdd"],
            "message": "Product gid://shopify/Product/1 does not exist", "code": "INVALID"}])
    );
    let product = shop.keys("Product").next().unwrap().to_string();
    let bxgy = run(
        &mut shop,
        &r#"mutation { discountCodeBxgyCreate(bxgyCodeDiscount: {title: "2 for 1", code: "TWOFORONE",
            customerBuys: {value: {quantity: "2"}, items: {products: {productsToAdd: ["PRODUCT"]}}},
            customerGets: {value: {discountOnQuantity: {quantity: "1", effect: {percentage: 1.0}}}, items: {all: true}}, usesPerOrderLimit: 1}) {
            codeDiscountNode { codeDiscount { ... on DiscountCodeBxgy { title discountClasses status
              customerBuys { value { ... on DiscountQuantity { quantity } } items { ... on DiscountProducts { products(first: 1) { nodes { id } } } } }
              customerGets { value { ... on DiscountOnQuantity { quantity { quantity } effect { ... on DiscountPercentage { percentage } } } } } } } }
            userErrors { field message } } }"#
            .replace("PRODUCT", &product),
        Value::Null,
    );
    let discount = &data(&bxgy, "discountCodeBxgyCreate")["codeDiscountNode"]["codeDiscount"];
    assert_eq!(
        discount["customerBuys"]["items"]["products"]["nodes"],
        json!([{"id": product}])
    );
    assert_eq!(discount["discountClasses"], json!(["PRODUCT"]));
    assert_eq!(discount["customerBuys"]["value"]["quantity"], json!("2"));
    assert_eq!(
        discount["customerGets"]["value"],
        json!({"quantity": {"quantity": "1"}, "effect": {"percentage": 1.0}})
    );
    let shipping = run(
        &mut shop,
        r#"mutation { discountCodeFreeShippingCreate(freeShippingCodeDiscount: {title: "Ship free", code: "SHIPFREE",
            destination: {countries: {add: [FR, DE]}}, maximumShippingPrice: "10.00", appliesOncePerCustomer: true}) {
            codeDiscountNode { codeDiscount { ... on DiscountCodeFreeShipping { discountClasses appliesOncePerCustomer
              maximumShippingPrice { amount } destinationSelection { ... on DiscountCountries { countries } } } } }
            userErrors { field message } } }"#,
        Value::Null,
    );
    assert_eq!(
        data(&shipping, "discountCodeFreeShippingCreate")["codeDiscountNode"]["codeDiscount"],
        json!({"discountClasses": ["SHIPPING"], "appliesOncePerCustomer": true,
            "maximumShippingPrice": {"amount": "10.0"}, "destinationSelection": {"countries": ["FR", "DE"]}})
    );
    assert_eq!(topics(&mut shop), ["discounts/create", "discounts/create"]);
}

/// A record deleted while a list still names it leaves that list, as Shopify never lists a
/// deleted resource: the answer is not `data: null` for a `null` in a list of non-null records,
/// and the counts follow what is listed.
#[test]
fn deleted_records_leave_the_lists_that_named_them() {
    let (store, mut shop) = rich();
    let dress =
        lsf_admin::domains::catalog::product_gid(store.product_by_handle("linen-dress").unwrap());
    let variant = lsf_admin::domains::catalog::variant_gid(
        &store.product_by_handle("linen-dress").unwrap().variants[0],
    );
    let definition = run(
        &mut shop,
        r#"mutation { metafieldDefinitionCreate(definition: {name: "VIP", namespace: "custom", key: "zz_tier", type: "single_line_text_field", ownerType: CUSTOMER}) { createdDefinition { id } userErrors { message } } }"#,
        Value::Null,
    );
    assert_eq!(
        data(&definition, "metafieldDefinitionCreate")["userErrors"],
        json!([])
    );
    let customer = run(
        &mut shop,
        r#"mutation { customerCreate(input: {email: "gone@rich.example", metafields: [{namespace: "custom", key: "zz_tier", type: "single_line_text_field", value: "gold"}]}) { customer { id } userErrors { message } } }"#,
        Value::Null,
    );
    let customer = data(&customer, "customerCreate")["customer"]["id"].clone();
    let discount = run(
        &mut shop,
        r#"mutation($input: DiscountCodeBasicInput!) { discountCodeBasicCreate(basicCodeDiscount: $input) { codeDiscountNode { id } userErrors { field message } } }"#,
        json!({"input": {"title": "Dress", "code": "DRESS", "startsAt": "2026-01-01T00:00:00Z",
            "customerGets": {"value": {"percentage": 0.1}, "items": {"products": {"productsToAdd": [dress]}}},
            "context": {"customers": {"add": [customer]}}}}),
    );
    assert_eq!(
        data(&discount, "discountCodeBasicCreate")["userErrors"],
        json!([])
    );
    let group = shop.keys("SellingPlanGroup").next().unwrap().to_string();
    let added = run(
        &mut shop,
        r#"mutation($id: ID!, $variants: [ID!]!) { sellingPlanGroupAddProductVariants(id: $id, productVariantIds: $variants) { userErrors { message } } }"#,
        json!({"id": group, "variants": [variant]}),
    );
    assert_eq!(
        data(&added, "sellingPlanGroupAddProductVariants")["userErrors"],
        json!([])
    );

    let counts = run(
        &mut shop,
        r#"{ product: metafieldDefinitions(first: 5, ownerType: PRODUCT, namespace: "custom", key: "care") { nodes { metafieldsCount } }
            customer: metafieldDefinitions(first: 5, ownerType: CUSTOMER, namespace: "custom", key: "zz_tier") { nodes { metafieldsCount } } }"#,
        Value::Null,
    );
    assert_eq!(
        counts["data"]["product"]["nodes"][0]["metafieldsCount"],
        json!(1),
        "{counts}"
    );
    assert_eq!(
        counts["data"]["customer"]["nodes"][0]["metafieldsCount"],
        json!(1),
        "{counts}"
    );
    let deleted = run(
        &mut shop,
        r#"mutation($product: ID!, $customer: ID!) {
            productDelete(input: {id: $product}) { deletedProductId userErrors { message } }
            customerDelete(input: {id: $customer}) { deletedCustomerId userErrors { message } } }"#,
        json!({"product": dress, "customer": customer}),
    );
    assert_eq!(
        data(&deleted, "productDelete")["userErrors"],
        json!([]),
        "{deleted}"
    );
    assert_eq!(
        data(&deleted, "customerDelete")["userErrors"],
        json!([]),
        "{deleted}"
    );

    let answer = run(
        &mut shop,
        r#"query($group: ID!) {
            codeDiscountNodes(first: 10, query: "title:Dress") { nodes { codeDiscount { ... on DiscountCodeBasic {
                customerGets { items { ... on DiscountProducts { products(first: 5) { nodes { id } } } } }
                context { ... on DiscountCustomers { customers { id } } } } } } }
            sellingPlanGroup(id: $group) { productVariants(first: 5) { nodes { id } } productVariantsCount { count } }
            product: metafieldDefinitions(first: 5, ownerType: PRODUCT, namespace: "custom", key: "care") { nodes { metafieldsCount metafields(first: 5) { nodes { id } } } }
            customer: metafieldDefinitions(first: 5, ownerType: CUSTOMER, namespace: "custom", key: "zz_tier") { nodes { metafieldsCount metafields(first: 5) { nodes { id } } } }
        }"#,
        json!({"group": group}),
    );
    assert!(answer.get("errors").is_none(), "{answer}");
    assert_eq!(
        answer["data"]["codeDiscountNodes"]["nodes"][0]["codeDiscount"],
        json!({"customerGets": {"items": {"products": {"nodes": []}}}, "context": {"customers": []}})
    );
    assert_eq!(
        answer["data"]["sellingPlanGroup"],
        json!({"productVariants": {"nodes": []}, "productVariantsCount": {"count": 0}})
    );
    for owner in ["product", "customer"] {
        assert_eq!(
            answer["data"][owner]["nodes"][0],
            json!({"metafieldsCount": 0, "metafields": {"nodes": []}}),
            "{owner}"
        );
    }
    // The definitions and the selling plan group no longer name what was deleted.
    let named = |shop: &ShopState, key: &str, field: &str| -> usize {
        shop.get(key).unwrap()[field].as_array().map_or(0, Vec::len)
    };
    assert_eq!(named(&shop, &group, "productVariants"), 0);
    for definition in shop
        .keys("MetafieldDefinition")
        .map(str::to_string)
        .collect::<Vec<_>>()
    {
        let record = shop.get(&definition).unwrap();
        for metafield in record["metafields"].as_array().unwrap() {
            assert!(
                shop.contains(metafield.as_str().unwrap()),
                "{definition} names {metafield}"
            );
        }
    }
}

/// A discount's percentage is a fraction from 0.0 to 1.0, as Shopify takes it: another is
/// refused and nothing is created.
#[test]
fn a_discount_percentage_outside_zero_to_one_is_refused() {
    let (_, mut shop) = rich();
    let create = |shop: &mut ShopState, value: Value| {
        run(
            shop,
            r#"mutation($input: DiscountCodeBasicInput!) { discountCodeBasicCreate(basicCodeDiscount: $input) { codeDiscountNode { id } userErrors { field message code } } }"#,
            json!({"input": {"title": "Range", "code": "RANGE", "startsAt": "2026-01-01T00:00:00Z",
                "customerGets": {"value": value, "items": {"all": true}}}}),
        )
    };
    for percentage in [1e300, 1.5, -0.1] {
        let answer = create(&mut shop, json!({"percentage": percentage}));
        assert_eq!(
            data(&answer, "discountCodeBasicCreate"),
            &json!({"codeDiscountNode": null, "userErrors": [{
                "field": ["basicCodeDiscount", "customerGets", "value", "percentage"],
                "message": "Percentage must be between 0.0 and 1.0",
                "code": "VALUE_OUTSIDE_RANGE",
            }]}),
            "{percentage}"
        );
    }
    assert!(topics(&mut shop).is_empty());
    let answer = create(&mut shop, json!({"percentage": 1.0}));
    assert_eq!(
        data(&answer, "discountCodeBasicCreate")["userErrors"],
        json!([])
    );
}
