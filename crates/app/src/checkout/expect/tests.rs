use lsf_core::store::model::ScenarioExpectInput;
use serde_json::json;

use super::*;
use crate::checkout::types::{
    Cost, DeliveryGroup, DeliveryKind, DiscountClass, KeyValue, PaymentPlacement,
};

fn method(handle: &str, name: &str, placement: PaymentPlacement) -> PaymentMethod {
    PaymentMethod {
        handle: handle.into(),
        name: name.into(),
        placements: vec![placement],
    }
}

fn option(handle: &str, title: &str, cost: Cents) -> DeliveryOption {
    DeliveryOption {
        handle: handle.into(),
        title: title.into(),
        description: None,
        kind: DeliveryKind::Shipping,
        original_cost: cost,
        cost,
    }
}

fn line(sku: Option<&str>, title: &str, quantity: u32, unit_price: Cents) -> ResultLine {
    ResultLine {
        sku: sku.map(str::to_string),
        title: title.into(),
        quantity,
        unit_price,
        subtotal: unit_price * quantity as i64,
        total: unit_price * quantity as i64,
        ..ResultLine::default()
    }
}

/// A checkout as a payment customization that hides PayPal from the express buttons, a delivery
/// customization, a discount, a cart transform and a validation leave it.
fn checkout() -> CheckoutResult {
    let mut bundle = line(None, "Summer bundle", 1, 4500);
    bundle.components = vec![
        line(Some("TEE-BLK-M"), "Tee", 1, 2000),
        line(Some("MUG-OAT"), "Mug", 1, 2500),
    ];
    bundle.attributes = vec![KeyValue {
        key: "_bundle".into(),
        value: "summer".into(),
    }];
    let mut tote = line(Some("TOTE-NAT"), "Canvas Tote Bag", 3, 2400);
    tote.total = 6480;
    CheckoutResult {
        lines: vec![bundle, tote],
        discounts: vec![AppliedDiscount {
            title: "10% off 3 or more".into(),
            code: Some("SAVE10".into()),
            class: DiscountClass::Product,
            amount: 720,
            function: "volume-discount".into(),
            details: Default::default(),
        }],
        delivery_groups: vec![DeliveryGroup {
            handle: "group-1".into(),
            line_ids: vec![],
            options: vec![
                option("pickup", "Pick up in store", 0),
                option("standard", "Standard (3-5 days)", 490),
            ],
            selected: Some("pickup".into()),
            discount_allocations: vec![],
        }],
        payment_methods: vec![
            method("card", "Card (secure)", PaymentPlacement::PaymentMethod),
            method("paypal", "PayPal", PaymentPlacement::PaymentMethod),
        ],
        express_methods: vec![
            method(
                "shop_pay",
                "Shop Pay",
                PaymentPlacement::AcceleratedCheckout,
            ),
            method(
                "apple_pay",
                "Apple Pay",
                PaymentPlacement::AcceleratedCheckout,
            ),
        ],
        errors: vec![CheckoutError {
            message: "No more than 5 of an item".into(),
            target: Some("$.cart".into()),
            function: Some("quantity-limit".into()),
        }],
        cost: Cost {
            subtotal: 11700,
            discounts: 720,
            shipping: 0,
            tax: 0,
            total: 10980,
            due_now: 10980,
            due_later: 0,
            checkout_charge_amount: 11700,
            currency_code: "USD".into(),
        },
        ..CheckoutResult::default()
    }
}

fn expect(value: serde_json::Value) -> ScenarioExpectInput {
    serde_json::from_value(value).expect("an expect block")
}

fn check(value: serde_json::Value) -> Outcome {
    evaluate(&expect(value), &checkout())
}

/// The assertions that are not met.
fn unmet(value: serde_json::Value) -> Vec<String> {
    check(value)
        .unmet
        .iter()
        .map(|unmet| unmet.assertion.clone())
        .collect()
}

#[test]
fn nothing_expected_checks_nothing() {
    assert_eq!(check(json!({})), Outcome::default());
}

#[test]
fn payment_methods_and_express_buttons_by_handle_or_name() {
    // The app of PayPal Express: hidden from the express buttons, still in the list.
    let met = check(json!({
        "express_methods": {"excludes": ["paypal", "PayPal"], "equals": ["shop_pay", "Apple Pay"]},
        "payment_methods": {"includes": ["paypal", {"handle": "card", "name": "Card (secure)"}], "excludes": ["Credit card"], "equals": ["card", "paypal"]}
    }));
    assert_eq!(met.checked, 7, "{met:?}");
    assert!(met.unmet.is_empty(), "{met:?}");

    // Present, absent, renamed, reordered: each fails on its own.
    assert_eq!(
        unmet(json!({
            "express_methods": {"includes": ["paypal"]},
            "payment_methods": {
                "excludes": ["card"],
                "includes": [{"handle": "card", "name": "Credit card"}],
                "equals": ["paypal", "card"]
            }
        })),
        [
            "payment_methods.includes",
            "payment_methods.excludes",
            "payment_methods.equals",
            "express_methods.includes",
        ]
    );
}

#[test]
fn an_unmet_expectation_says_what_was_expected_and_what_there_is() {
    let outcome = check(json!({"express_methods": {"includes": ["paypal"]}}));
    assert_eq!(
        outcome.unmet,
        [Unmet {
            assertion: "express_methods.includes".into(),
            expected: json!("paypal"),
            // A string names a method by its handle or its name: both are shown.
            actual: json!([
                {"handle": "shop_pay", "name": "Shop Pay"},
                {"handle": "apple_pay", "name": "Apple Pay"}
            ]),
        }]
    );
    // An object: the fields it gives, amounts as decimal strings, whatever way they were written.
    let outcome =
        check(json!({"delivery_options": {"includes": [{"handle": "pickup", "cost": 500}]}}));
    assert_eq!(
        outcome.unmet[0].expected,
        json!({"handle": "pickup", "cost": "5.00"})
    );
    assert_eq!(
        outcome.unmet[0].actual,
        json!([{"handle": "pickup", "cost": "0.00"}, {"handle": "standard", "cost": "4.90"}])
    );
    // One field: the values alone.
    let outcome = check(json!({"errors": {"equals": []}}));
    assert_eq!(outcome.unmet[0].assertion, "errors.equals");
    assert_eq!(outcome.unmet[0].expected, json!([]));
    assert_eq!(
        outcome.unmet[0].actual,
        json!(["No more than 5 of an item"])
    );
    let outcome = check(json!({"discounts": {"equals": [{"amount": "7.20"}, {"amount": "1.00"}]}}));
    assert_eq!(outcome.unmet[0].actual, json!(["7.20"]));
}

#[test]
fn delivery_options_hidden_renamed_reordered_and_their_cost() {
    let met = check(json!({"delivery_options": {
        "includes": ["Standard (3-5 days)", {"handle": "pickup", "title": "Pick up in store", "cost": "0.00"}],
        "excludes": ["express", {"handle": "standard", "cost": 0}],
        "equals": ["pickup", "standard"]
    }}));
    assert_eq!((met.checked, met.unmet.len()), (5, 0), "{met:?}");
    assert_eq!(
        unmet(json!({"delivery_options": {
            "includes": [{"handle": "standard", "title": "Standard"}, {"handle": "standard", "cost": "5.00"}],
            "excludes": ["Pick up in store"],
            "equals": ["standard", "pickup"]
        }})),
        [
            "delivery_options.includes",
            "delivery_options.includes",
            "delivery_options.excludes",
            "delivery_options.equals"
        ]
    );
}

#[test]
fn discounts_by_title_or_code_and_their_amount() {
    let met = check(json!({"discounts": {
        "includes": ["10% off 3 or more", "SAVE10", {"title": "10% off 3 or more", "class": "PRODUCT", "amount": "7.20", "code": "SAVE10"}],
        "excludes": ["WELCOME"],
        "equals": [{"amount": 720}]
    }}));
    assert_eq!((met.checked, met.unmet.len()), (5, 0), "{met:?}");
    assert_eq!(
        unmet(json!({"discounts": {
            "includes": [{"title": "10% off 3 or more", "amount": "7.00"}, {"class": "ORDER"}],
            "excludes": ["SAVE10"],
            "equals": []
        }})),
        [
            "discounts.includes",
            "discounts.includes",
            "discounts.excludes",
            "discounts.equals"
        ]
    );
    // A discount without a code is not named by one.
    let mut result = checkout();
    result.discounts[0].code = None;
    let outcome = evaluate(
        &expect(json!({"discounts": {"includes": ["SAVE10"]}})),
        &result,
    );
    assert_eq!(
        outcome.unmet[0].actual,
        json!([{"title": "10% off 3 or more"}])
    );
}

#[test]
fn lines_after_a_cart_transform() {
    let met = check(json!({"lines": {
        "includes": [
            "TOTE-NAT",
            {"title": "Summer bundle", "quantity": 1, "price": "45.00", "components": ["TEE-BLK-M", "Mug"], "attributes": {"_bundle": "summer"}},
            {"sku": "TOTE-NAT", "quantity": 3, "price": 2400, "total": "64.80", "components": []}
        ],
        "excludes": ["TEE-BLK-M", "Tee"],
        "equals": ["Summer bundle", "Canvas Tote Bag"]
    }}));
    assert_eq!((met.checked, met.unmet.len()), (6, 0), "{met:?}");
    assert_eq!(
        unmet(json!({"lines": {
            "includes": [
                {"title": "Summer bundle", "components": ["MUG-OAT", "TEE-BLK-M"]},
                {"title": "Summer bundle", "components": ["TEE-BLK-M"]},
                {"title": "Summer bundle", "attributes": {"_bundle": "winter"}},
                {"sku": "TOTE-NAT", "total": "72.00"}
            ],
            "equals": ["TOTE-NAT", "Summer bundle"]
        }})),
        [
            "lines.includes",
            "lines.includes",
            "lines.includes",
            "lines.includes",
            "lines.equals"
        ]
    );
    // Components by their SKU, or their title when they have none.
    let outcome =
        check(json!({"lines": {"includes": [{"title": "Summer bundle", "components": []}]}}));
    assert_eq!(
        outcome.unmet[0].actual,
        json!([
            {"title": "Summer bundle", "components": ["TEE-BLK-M", "MUG-OAT"]},
            {"title": "Canvas Tote Bag", "components": []}
        ])
    );
    // A line without a SKU shows none.
    let outcome = check(json!({"lines": {"equals": []}}));
    assert_eq!(
        outcome.unmet[0].actual,
        json!([{"title": "Summer bundle"}, {"sku": "TOTE-NAT", "title": "Canvas Tote Bag"}])
    );
}

#[test]
fn validation_errors_by_message_and_target() {
    let met = check(json!({"errors": {
        "includes": ["No more than 5 of an item", {"message": "No more than 5 of an item", "target": "$.cart"}],
        "excludes": ["Enter a street number"]
    }}));
    assert_eq!((met.checked, met.unmet.len()), (3, 0), "{met:?}");
    assert_eq!(
        unmet(json!({"errors": {
            "includes": ["No more than 5", {"target": "$.cart.buyerIdentity.email"}],
            "excludes": [{"target": "$.cart"}],
            "equals": []
        }})),
        [
            "errors.includes",
            "errors.includes",
            "errors.excludes",
            "errors.equals"
        ]
    );
}

#[test]
fn totals_exactly() {
    let met = check(json!({"totals": {
        "subtotal": "117.00", "discounts": 720, "shipping": "0", "tax": 0,
        "total": "109.80", "due_now": "109.80", "due_later": "0.00"
    }}));
    assert_eq!((met.checked, met.unmet.len()), (7, 0), "{met:?}");
    let outcome =
        check(json!({"totals": {"total": "110.00", "discounts": "7.20", "shipping": 490}}));
    assert_eq!(outcome.checked, 3);
    assert_eq!(
        outcome.unmet,
        [
            Unmet {
                assertion: "totals.shipping".into(),
                expected: json!("4.90"),
                actual: json!("0.00"),
            },
            Unmet {
                assertion: "totals.total".into(),
                expected: json!("110.00"),
                actual: json!("109.80"),
            },
        ]
    );
}

#[test]
fn amounts_read_as_decimal_strings() {
    assert_eq!(amount(0), json!("0.00"));
    assert_eq!(amount(5), json!("0.05"));
    assert_eq!(amount(12345), json!("123.45"));
    assert_eq!(amount(-490), json!("-4.90"));
}
