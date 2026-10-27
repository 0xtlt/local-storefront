//! The pipeline against the Functions of the fixture app: real Wasm modules, run by
//! function-runner.

mod common;

use common::*;
use lsf_checkout::app::{Selection, Severity, TargetKind};
use lsf_checkout::pipeline::{compute, compute_with};
use lsf_checkout::{CheckoutState, DiscountClass, DiscountClass::*, DiscountConfig, Metafield};
use serde_json::json;

fn discount(
    function: &str,
    title: &str,
    code: Option<&str>,
    classes: &[DiscountClass],
    settings_json: serde_json::Value,
) -> DiscountConfig {
    DiscountConfig {
        function: function.to_string(),
        title: title.to_string(),
        code: code.map(str::to_string),
        classes: classes.to_vec(),
        metafields: settings(settings_json),
    }
}

#[test]
fn the_fixture_app_loads_without_errors() {
    let app = app();
    assert_eq!(app.name.as_deref(), Some("Checkout fixtures"));
    let mut handles: Vec<_> = app.functions.iter().map(|f| f.handle.as_str()).collect();
    handles.sort();
    assert_eq!(
        handles,
        [
            "bundle-transform",
            "delivery-rules",
            "payment-rules",
            "quantity-limit",
            "volume-discount"
        ]
    );
    assert!(app.functions.iter().all(|function| function.built()));
    let errors: Vec<_> = app
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert!(errors.is_empty(), "{errors:?}");
    let volume = app.function("volume-discount").unwrap();
    assert_eq!(
        volume.targets.iter().map(|t| t.kind).collect::<Vec<_>>(),
        [TargetKind::LinesDiscounts, TargetKind::DeliveryDiscounts]
    );
    assert!(volume.input_variables.is_some());
}

#[test]
fn a_product_discount_reduces_the_lines_with_enough_items() {
    let app = app();
    let store = store();
    let session = session(&store, &[(TEE, 3), (TOTE, 1)]);
    let mut config = config();
    config.discounts = vec![discount(
        "volume-discount",
        "Volume",
        None,
        &[Product],
        json!({"minimumQuantity": 3, "percentage": 10}),
    )];
    let result = compute(
        &app,
        &Selection::only(["volume-discount"]),
        &store,
        &session,
        &CheckoutState::default(),
        &config,
    );

    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.lines.len(), 2);
    assert_eq!(result.lines[0].id, "gid://shopify/CartLine/1");
    assert_eq!(result.lines[0].subtotal, 9600);
    assert_eq!(result.lines[0].total, 8640);
    assert_eq!(result.lines[0].discount_allocations[0].amount, 960);
    assert_eq!(
        result.lines[0].discount_allocations[0].title,
        "10% off 3+ items"
    );
    assert_eq!(result.lines[1].total, 2400);

    assert_eq!(result.discounts.len(), 1);
    assert_eq!(result.discounts[0].amount, 960);
    assert_eq!(result.discounts[0].class, Product);
    assert_eq!(result.discounts[0].function, "volume-discount");
    assert_eq!(result.discounts[0].code, None);

    // 96.00 + 24.00 - 9.60 + 4.90 shipping (the first rate is selected)
    assert_eq!(result.cost.subtotal, 12000);
    assert_eq!(result.cost.discounts, 960);
    assert_eq!(result.cost.shipping, 490);
    assert_eq!(result.cost.total, 12000 - 960 + 490);
    assert_eq!(result.cost.currency_code, "USD");

    // Only the lines target ran: the discount has no SHIPPING class.
    assert_eq!(result.runs.len(), 1);
    let run = run_for(&result, "volume-discount");
    assert_eq!(run.target, "cart.lines.discounts.generate.run");
    assert_eq!(run.export, "cart_lines_discounts_generate_run");
    assert!(run.error.is_none());
    assert!(run.instructions > 0);
    assert!(run.logs.contains("volume discount"), "{}", run.logs);
    assert!(run.output.as_ref().unwrap()["operations"][0]["productDiscountsAdd"].is_object());
    assert!(run.input["cart"]["lines"].is_array());
}

#[test]
fn a_code_discount_runs_once_the_code_is_entered() {
    let app = app();
    let store = store();
    let session = session(&store, &[(TEE, 1), (MUG, 2)]);
    let mut config = config();
    config.discounts = vec![discount(
        "volume-discount",
        "Welcome",
        Some("WELCOME10"),
        &[Order],
        json!({"orderPercentage": 10}),
    )];
    let selection = Selection::only(["volume-discount"]);

    let without = compute(
        &app,
        &selection,
        &store,
        &session,
        &CheckoutState::default(),
        &config,
    );
    assert!(without.runs.is_empty());
    assert!(without.discounts.is_empty());

    let state = CheckoutState {
        discount_codes: vec!["welcome10".to_string(), "NOPE".to_string()],
        ..CheckoutState::default()
    };
    let with = compute(&app, &selection, &store, &session, &state, &config);
    assert!(with.errors.is_empty(), "{:?}", with.errors);
    // 32.00 + 44.00 = 76.00, 10 % off, shared between the two lines.
    assert_eq!(with.discounts.len(), 1);
    assert_eq!(with.discounts[0].class, Order);
    assert_eq!(with.discounts[0].amount, 760);
    assert_eq!(with.discounts[0].code.as_deref(), Some("WELCOME10"));
    assert_eq!(with.lines[0].discount_allocations[0].amount, 320);
    assert_eq!(with.lines[1].discount_allocations[0].amount, 440);
    assert_eq!(with.discounts[0].class, Order);
    // The page tells order discounts from product ones by the class of a line's allocations.
    let page = serde_json::to_value(&with.lines[0]).unwrap();
    assert_eq!(page["discountAllocations"][0]["class"], "ORDER");
    assert_eq!(with.cost.discounts, 760);
    assert_eq!(
        with.discount_codes
            .iter()
            .map(|c| (c.code.as_str(), c.applicable))
            .collect::<Vec<_>>(),
        [("welcome10", true), ("NOPE", false)]
    );
    let run = run_for(&with, "volume-discount");
    assert_eq!(run.input["triggeringDiscountCode"], "WELCOME10");
}

#[test]
fn a_shipping_discount_makes_the_selected_option_free() {
    let app = app();
    let store = store();
    let mut config = config();
    config.discounts = vec![discount(
        "volume-discount",
        "Free shipping",
        None,
        &[Shipping],
        json!({"freeShippingAbove": 50}),
    )];
    let selection = Selection::only(["volume-discount"]);

    let big = session(&store, &[(TEE, 2)]);
    let result = compute(
        &app,
        &selection,
        &store,
        &big,
        &CheckoutState::default(),
        &config,
    );
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.cost.shipping, 0);
    let group = &result.delivery_groups[0];
    assert_eq!(group.options[0].original_cost, 490);
    assert_eq!(group.options[0].cost, 0);
    assert_eq!(
        group.options[1].cost, 0,
        "every option of the group is targeted"
    );
    assert_eq!(result.discounts.len(), 1);
    assert_eq!(result.discounts[0].class, Shipping);
    assert_eq!(result.discounts[0].amount, 490);
    assert_eq!(result.discounts[0].title, "Free shipping");
    assert_eq!(
        result.cost.discounts, 0,
        "shipping discounts are not in the product/order total"
    );
    assert_eq!(result.cost.total, 6400);
    assert_eq!(
        run_for(&result, "volume-discount").target,
        "cart.delivery-options.discounts.generate.run"
    );

    let small = session(&store, &[(TOTE, 1)]);
    let result = compute(
        &app,
        &selection,
        &store,
        &small,
        &CheckoutState::default(),
        &config,
    );
    assert_eq!(result.cost.shipping, 490);
    assert!(result.discounts.is_empty());
}

#[test]
fn a_cart_without_shipping_has_no_delivery_group() {
    let app = app();
    let store = store();
    let session = session(&store, &[(GIFT_CARD, 1)]);
    let result = compute(
        &app,
        &Selection::all(),
        &store,
        &session,
        &CheckoutState::default(),
        &config(),
    );
    assert!(result.delivery_groups.is_empty());
    assert_eq!(result.cost.shipping, 0);
    assert_eq!(result.cost.total, 2500);
}

#[test]
fn delivery_customization_hides_renames_and_moves_options() {
    let app = app();
    let store = store();
    let session = session(&store, &[(TEE, 1)]);
    let mut config = config();
    config.shipping_rates.push(lsf_checkout::ShippingRate {
        handle: "pickup".into(),
        title: "Pick up".into(),
        price: 0,
        description: Some("In store".into()),
        kind: lsf_checkout::DeliveryKind::PickUp,
    });
    config.function_metafields.insert(
        "delivery-rules".to_string(),
        settings(json!({"hide": ["express"], "rename": {"standard": "Standard (3-5 days)"}, "first": "pickup"})),
    );
    let selection = Selection::only(["delivery-rules"]);

    let result = compute(
        &app,
        &selection,
        &store,
        &session,
        &CheckoutState::default(),
        &config,
    );
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let group = &result.delivery_groups[0];
    let options: Vec<_> = group
        .options
        .iter()
        .map(|o| (o.handle.as_str(), o.title.as_str()))
        .collect();
    assert_eq!(
        options,
        [("pickup", "Pick up"), ("standard", "Standard (3-5 days)")]
    );
    assert_eq!(group.options[0].kind, lsf_checkout::DeliveryKind::PickUp);
    // The first option is selected when the buyer chose nothing...
    assert_eq!(group.selected.as_deref(), Some("pickup"));
    assert_eq!(result.cost.shipping, 0);
    let run = run_for(&result, "delivery-rules");
    assert_eq!(run.target, "cart.delivery-options.transform.run");
    assert!(run.logs.contains("hiding express"));

    // ...and a choice the Function hid falls back to the first option.
    let mut state = CheckoutState::default();
    state
        .selected_delivery
        .insert(group.handle.clone(), "express".into());
    let result = compute(&app, &selection, &store, &session, &state, &config);
    assert_eq!(
        result.delivery_groups[0].selected.as_deref(),
        Some("pickup")
    );
    state
        .selected_delivery
        .insert(group.handle.clone(), "standard".into());
    let result = compute(&app, &selection, &store, &session, &state, &config);
    assert_eq!(
        result.delivery_groups[0].selected.as_deref(),
        Some("standard")
    );
    assert_eq!(result.cost.shipping, 490);
}

#[test]
fn payment_customization_hides_and_renames_methods() {
    let app = app();
    let store = store();
    let mut config = config();
    config.function_metafields.insert(
        "payment-rules".to_string(),
        settings(json!({"hideName": "Cash on Delivery", "hideAbove": 100, "rename": {"Bank Deposit": "Wire transfer"}})),
    );
    let selection = Selection::only(["payment-rules"]);

    let small = session(&store, &[(TOTE, 1)]);
    let result = compute(
        &app,
        &selection,
        &store,
        &small,
        &CheckoutState::default(),
        &config,
    );
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let names: Vec<_> = result
        .payment_methods
        .iter()
        .map(|m| m.name.as_str())
        .collect();
    assert_eq!(names, ["Wire transfer", "Credit card", "Cash on Delivery"]);
    assert_eq!(result.payment_methods[0].handle, "bank");

    let big = session(&store, &[(TEE, 4)]);
    let result = compute(
        &app,
        &selection,
        &store,
        &big,
        &CheckoutState::default(),
        &config,
    );
    let names: Vec<_> = result
        .payment_methods
        .iter()
        .map(|m| m.name.as_str())
        .collect();
    assert_eq!(names, ["Wire transfer", "Credit card"]);
    assert_eq!(
        run_for(&result, "payment-rules").target,
        "cart.payment-methods.transform.run"
    );
}

/// The payment rules of the fixture: hide Cash on Delivery from 100.00 of subtotal.
fn payment_rules() -> Vec<lsf_checkout::Metafield> {
    settings(json!({"hideName": "Cash on Delivery", "hideAbove": 100}))
}

fn method_names(result: &lsf_checkout::CheckoutResult) -> Vec<&str> {
    result
        .payment_methods
        .iter()
        .map(|method| method.name.as_str())
        .collect()
}

#[test]
fn payment_customization_sees_the_cart_after_the_discounts() {
    let app = app();
    let store = store();
    // 4 x 32.00 = 128.00, 64.00 once half of it is discounted.
    let session = session(&store, &[(TEE, 4)]);
    let mut config = config();
    config
        .function_metafields
        .insert("payment-rules".to_string(), payment_rules());
    let selection = Selection::only(["volume-discount", "payment-rules"]);

    let without = compute(
        &app,
        &selection,
        &store,
        &session,
        &CheckoutState::default(),
        &config,
    );
    assert!(without.errors.is_empty(), "{:?}", without.errors);
    assert_eq!(
        run_for(&without, "payment-rules").input["cart"]["cost"]["subtotalAmount"]["amount"],
        "128.0"
    );
    assert!(!method_names(&without).contains(&"Cash on Delivery"));

    config.discounts = vec![discount(
        "volume-discount",
        "Half price",
        None,
        &[Product],
        json!({"minimumQuantity": 1, "percentage": 50}),
    )];
    let result = compute(
        &app,
        &selection,
        &store,
        &session,
        &CheckoutState::default(),
        &config,
    );
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.lines[0].total, 6400);
    let run = run_for(&result, "payment-rules");
    assert_eq!(
        run.input["cart"]["cost"]["subtotalAmount"]["amount"],
        "64.0"
    );
    assert!(run.error.is_none());
    assert!(
        method_names(&result).contains(&"Cash on Delivery"),
        "{:?}",
        method_names(&result)
    );
}

/// The amounts a validation Function can read, whatever its own input query asks.
fn cost_seen_by_validation(
    discounts: Vec<DiscountConfig>,
) -> (lsf_checkout::CheckoutResult, serde_json::Value) {
    const QUERY: &str = r#"query Input {
        cart {
            lines { quantity cost { subtotalAmount { amount } totalAmount { amount } } }
            buyerIdentity { email }
            cost { subtotalAmount { amount } totalAmount { amount } }
        }
        buyerJourney { step }
        validation { metafield(namespace: "$app:config", key: "settings") { value } }
    }"#;
    let app = app();
    let store = store();
    let session = session(&store, &[(TEE, 4)]);
    let mut config = config();
    config.discounts = discounts;
    config.function_metafields.insert(
        "quantity-limit".to_string(),
        settings(json!({"maxQuantity": 10})),
    );
    let build = |query: &str,
                 variables: Option<&serde_json::Value>,
                 ctx: &lsf_checkout::input::InputContext| {
        let query = match ctx.owner.kind {
            lsf_checkout::input::OwnerKind::Validation => QUERY,
            _ => query,
        };
        lsf_checkout::input::build(query, variables, ctx)
    };
    let result = compute_with(
        &app,
        &Selection::only(["volume-discount", "quantity-limit"]),
        &store,
        &session,
        &CheckoutState::default(),
        &config,
        &build,
    );
    let input = run_for(&result, "quantity-limit").input.clone();
    (result, input)
}

#[test]
fn validation_sees_the_cart_after_a_product_discount() {
    let (result, input) = cost_seen_by_validation(vec![discount(
        "volume-discount",
        "Half price",
        None,
        &[Product],
        json!({"minimumQuantity": 1, "percentage": 50}),
    )]);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let cost = &input["cart"]["cost"];
    assert_eq!(cost["subtotalAmount"]["amount"], "64.0");
    // 64.00 and the 4.90 of the selected rate.
    assert_eq!(cost["totalAmount"]["amount"], "68.9");
    let line = &input["cart"]["lines"][0]["cost"];
    assert_eq!(line["subtotalAmount"]["amount"], "128.0");
    assert_eq!(line["totalAmount"]["amount"], "64.0");
}

#[test]
fn an_order_discount_is_not_in_the_subtotal_but_in_the_total() {
    let (result, input) = cost_seen_by_validation(vec![discount(
        "volume-discount",
        "Ten percent",
        None,
        &[Order],
        json!({"orderPercentage": 10}),
    )]);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.lines[0].total, 11520);
    let cost = &input["cart"]["cost"];
    // The subtotal is before the cart-level discounts, the total after them (and the rate).
    assert_eq!(cost["subtotalAmount"]["amount"], "128.0");
    assert_eq!(cost["totalAmount"]["amount"], "120.1");
    let line = &input["cart"]["lines"][0]["cost"];
    assert_eq!(line["subtotalAmount"]["amount"], "128.0");
    assert_eq!(line["totalAmount"]["amount"], "115.2");
}

#[test]
fn validation_errors_block_the_checkout() {
    let app = app();
    let store = store();
    let mut config = config();
    config.function_metafields.insert(
        "quantity-limit".to_string(),
        settings(json!({"maxQuantity": 3, "blockedEmailDomain": "blocked.test"})),
    );
    let selection = Selection::only(["quantity-limit"]);

    let fine = session(&store, &[(TEE, 3)]);
    let result = compute(
        &app,
        &selection,
        &store,
        &fine,
        &CheckoutState::default(),
        &config,
    );
    assert!(result.errors.is_empty(), "{:?}", result.errors);

    let too_many = session(&store, &[(TEE, 4)]);
    let state = CheckoutState {
        email: Some("someone@blocked.test".into()),
        ..CheckoutState::default()
    };
    let result = compute(&app, &selection, &store, &too_many, &state, &config);
    assert_eq!(result.errors.len(), 2, "{:?}", result.errors);
    assert_eq!(
        result.errors[0].message,
        "Not possible to order more than 3 of each"
    );
    assert_eq!(result.errors[0].target.as_deref(), Some("$.cart"));
    assert_eq!(result.errors[0].function.as_deref(), Some("quantity-limit"));
    assert_eq!(
        result.errors[1].target.as_deref(),
        Some("$.cart.buyerIdentity.email")
    );
    assert_eq!(
        run_for(&result, "quantity-limit").target,
        "purchase.validation.run"
    );
    // A validation error is a checkout error the Function meant, not a failed run.
    assert!(run_for(&result, "quantity-limit").error.is_none());
}

#[test]
fn a_cart_transform_updates_a_line() {
    let app = app();
    let store = store();
    let session = session(&store, &[(TEE, 2), (TOTE, 1)]);
    let mut config = config();
    config.function_metafields.insert(
        "bundle-transform".to_string(),
        settings(json!({"operations": [{"type": "update", "line": 1, "title": "Member tee", "price": "20.00"}]})),
    );
    let result = compute(
        &app,
        &Selection::only(["bundle-transform"]),
        &store,
        &session,
        &CheckoutState::default(),
        &config,
    );
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.lines[0].title, "Member tee");
    assert_eq!(result.lines[0].unit_price, 2000);
    assert_eq!(result.lines[0].subtotal, 4000);
    assert_eq!(result.lines[0].transformed_by.as_deref(), Some("update"));
    assert_eq!(result.lines[1].transformed_by, None);
    assert_eq!(result.cost.subtotal, 6400);
    assert_eq!(
        run_for(&result, "bundle-transform").target,
        "cart.transform.run"
    );
}

/// Runs the cart transform with one `expand` of the first line (two TEE at 32.00).
fn expand(operation: serde_json::Value) -> lsf_checkout::CheckoutResult {
    let app = app();
    let store = store();
    let session = session(&store, &[(TEE, 2)]);
    let mut config = config();
    config.function_metafields.insert(
        "bundle-transform".to_string(),
        settings(json!({"operations": [operation]})),
    );
    compute(
        &app,
        &Selection::only(["bundle-transform"]),
        &store,
        &session,
        &CheckoutState::default(),
        &config,
    )
}

#[test]
fn an_expanded_line_keeps_its_price_shared_by_the_variant_prices_of_the_components() {
    let result = expand(json!({
        "type": "expand", "line": 1, "title": "Duo",
        "items": [{"variant": MUG, "quantity": 2}, {"variant": TOTE, "quantity": 2}]
    }));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.lines.len(), 1, "the components are inside the line");
    let line = &result.lines[0];
    assert_eq!(line.id, "gid://shopify/CartLine/1");
    assert_eq!(line.title, "Duo");
    assert_eq!(line.transformed_by.as_deref(), Some("expand"));
    assert_eq!(line.quantity, 2);
    // The bundle keeps its price, 32.00.
    assert_eq!(line.unit_price, 3200);
    assert_eq!(line.subtotal, 6400);
    assert_eq!(line.components.len(), 2);
    assert_eq!(line.components[0].id, "gid://shopify/CartLine/2");
    assert_eq!(line.components[1].id, "gid://shopify/CartLine/3");
    assert_eq!(line.components[0].variant_id, MUG);
    assert_eq!(line.components[0].quantity, 4);
    assert_eq!(line.components[1].quantity, 4);
    // 3200 shared as 44 : 48 (2 x 22.00 : 2 x 24.00), and by unit.
    assert_eq!(line.components[0].unit_price, 765);
    assert_eq!(line.components[1].unit_price, 835);
    let shared: i64 = line.components.iter().map(|c| c.subtotal).sum();
    assert_eq!(shared, line.subtotal);
    assert_eq!(result.cost.subtotal, 6400);
}

#[test]
fn the_percentage_decrease_of_an_expand_applies_to_the_price_of_the_expanded_line() {
    let result = expand(json!({
        "type": "expand", "line": 1, "percentageDecrease": "25",
        "items": [{"variant": MUG, "quantity": 2}, {"variant": TOTE, "quantity": 2}]
    }));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let line = &result.lines[0];
    // 32.00, 25 % off, and not the sum of the variant prices of the components.
    assert_eq!(line.unit_price, 2400);
    assert_eq!(line.subtotal, 4800);
    assert_eq!(line.components[0].unit_price, 574);
    assert_eq!(line.components[1].unit_price, 626);
    let shared: i64 = line.components.iter().map(|c| c.subtotal).sum();
    assert_eq!(shared, line.subtotal);
    assert_eq!(result.cost.subtotal, 4800);
}

#[test]
fn an_expanded_line_costs_the_sum_of_the_prices_of_its_components() {
    let result = expand(json!({
        "type": "expand", "line": 1,
        "items": [
            {"variant": MUG, "quantity": 2, "price": "10.00"},
            {"variant": TOTE, "quantity": 2, "price": "5.00"}
        ]
    }));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let line = &result.lines[0];
    // Per bundle: 2 x 10.00 + 2 x 5.00.
    assert_eq!(line.unit_price, 3000);
    assert_eq!(line.subtotal, 6000);
    assert_eq!(line.components[0].unit_price, 1000);
    assert_eq!(line.components[1].unit_price, 500);
    assert_eq!(result.cost.subtotal, 6000);
}

#[test]
fn an_expand_cannot_combine_component_prices_and_an_overall_decrease() {
    let result = expand(json!({
        "type": "expand", "line": 1, "percentageDecrease": "10",
        "items": [
            {"variant": MUG, "quantity": 1, "price": "10.00"},
            {"variant": TOTE, "quantity": 1, "price": "5.00"}
        ]
    }));
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    assert!(
        result.errors[0]
            .message
            .contains("cannot_combine_price_adjustment_and_price_per_component"),
        "{:?}",
        result.errors
    );
    // The operation is refused: the line is left as it was.
    assert!(result.lines[0].components.is_empty());
    assert_eq!(result.lines[0].unit_price, 3200);
    assert_eq!(result.cost.subtotal, 6400);
}

#[test]
fn an_expand_prices_all_its_components_or_none() {
    let result = expand(json!({
        "type": "expand", "line": 1,
        "items": [{"variant": MUG, "quantity": 1, "price": "10.00"}, {"variant": TOTE, "quantity": 1}]
    }));
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    assert!(
        result.errors[0]
            .message
            .contains("expanded_items_missing_prices"),
        "{:?}",
        result.errors
    );
    assert!(result.lines[0].components.is_empty());
    assert_eq!(result.cost.subtotal, 6400);
}

#[test]
fn an_expand_cannot_decrease_the_price_by_more_than_100_percent() {
    let result = expand(json!({
        "type": "expand", "line": 1, "percentageDecrease": "101",
        "items": [{"variant": MUG, "quantity": 1}]
    }));
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    assert!(
        result.errors[0]
            .message
            .contains("invalid_price_adjustment_percentage_decrease"),
        "{:?}",
        result.errors
    );
    assert!(result.lines[0].components.is_empty());
}

#[test]
fn an_expanded_line_shares_its_price_to_the_cent_when_it_does_not_divide() {
    // Two 32.00 bundles of 3 x 22.00 and 1 x 24.00: 64.00 to share as 66 : 24.
    let result = expand(json!({
        "type": "expand", "line": 1,
        "items": [{"variant": MUG, "quantity": 3}, {"variant": TOTE, "quantity": 1}]
    }));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let line = &result.lines[0];
    assert_eq!(line.subtotal, 6400);
    let [mug, tote] = &line.components[..] else {
        panic!("{:?}", line.components)
    };
    assert_eq!((mug.quantity, tote.quantity), (6, 2));
    // The line totals are exact and add up to the total of the bundles...
    assert_eq!((mug.subtotal, tote.subtotal), (4693, 1707));
    assert_eq!((mug.total, tote.total), (4693, 1707));
    assert_eq!(mug.subtotal + tote.subtotal, line.subtotal);
    // ...the unit prices are rounded to the cent.
    assert_eq!((mug.unit_price, tote.unit_price), (782, 854));
    assert_eq!(result.cost.subtotal, 6400);
}

#[test]
fn a_function_cannot_give_negative_prices_to_expanded_items() {
    let result = expand(json!({
        "type": "expand", "line": 1,
        "items": [
            {"variant": MUG, "quantity": 1, "price": "-10.00"},
            {"variant": TOTE, "quantity": 1, "price": "5.00"}
        ]
    }));
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    assert!(
        result.errors[0].message.contains("is negative"),
        "{:?}",
        result.errors
    );
    // The operation is refused: the line is left as it was.
    assert!(result.lines[0].components.is_empty());
    assert_eq!(result.cost.subtotal, 6400);
}

#[test]
fn a_function_cannot_give_an_overflowing_price_to_expanded_items() {
    for price in ["1e300", "99999999999999999999"] {
        let result = expand(json!({
            "type": "expand", "line": 1,
            "items": [{"variant": MUG, "quantity": 1, "price": price}]
        }));
        assert_eq!(result.errors.len(), 1, "{price}: {:?}", result.errors);
        assert!(result.lines[0].components.is_empty(), "{price}");
        assert_eq!(result.cost.subtotal, 6400, "{price}");
    }
}

#[test]
fn a_function_cannot_decrease_a_price_by_a_negative_percentage() {
    let result = expand(json!({
        "type": "expand", "line": 1, "percentageDecrease": "-25",
        "items": [{"variant": MUG, "quantity": 1}]
    }));
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    assert!(result.lines[0].components.is_empty());
    assert_eq!(result.cost.subtotal, 6400);
}

#[test]
fn a_function_cannot_update_a_line_to_a_negative_or_overflowing_price() {
    for price in ["-10.00", "1e300"] {
        let app = app();
        let store = store();
        let session = session(&store, &[(TEE, 2)]);
        let mut config = config();
        config.function_metafields.insert(
            "bundle-transform".to_string(),
            settings(json!({"operations": [{"type": "update", "line": 1, "title": "Cheap", "price": price}]})),
        );
        let result = compute(
            &app,
            &Selection::only(["bundle-transform"]),
            &store,
            &session,
            &CheckoutState::default(),
            &config,
        );
        assert_eq!(result.errors.len(), 1, "{price}: {:?}", result.errors);
        // Nothing of the operation is applied, not even its title.
        assert_ne!(result.lines[0].title, "Cheap", "{price}");
        assert_eq!(result.lines[0].unit_price, 3200, "{price}");
        assert_eq!(result.cost.subtotal, 6400, "{price}");
    }
}

#[test]
fn a_function_cannot_merge_lines_at_a_negative_percentage() {
    let app = app();
    let store = store();
    let session = session(&store, &[(MUG, 1), (TOTE, 1)]);
    let mut config = config();
    config.function_metafields.insert(
        "bundle-transform".to_string(),
        settings(json!({"operations": [{
            "type": "merge", "lines": [1, 2], "parentVariant": TEE, "percentageDecrease": "-50"
        }]})),
    );
    let result = compute(
        &app,
        &Selection::only(["bundle-transform"]),
        &store,
        &session,
        &CheckoutState::default(),
        &config,
    );
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    assert_eq!(result.lines.len(), 2, "the lines are not merged");
    assert_eq!(result.cost.subtotal, 4600);
}

/// A discount Function that gives a percentage out of 0..100 (the fixture does it when its
/// configuration says so).
fn out_of_range_discount(class: DiscountClass, settings: serde_json::Value) {
    let app = app();
    let store = store();
    let session = session(&store, &[(TEE, 3)]);
    let mut config = config();
    config.discounts = vec![discount(
        "volume-discount",
        "Too much",
        None,
        &[class],
        settings,
    )];
    let result = compute(
        &app,
        &Selection::only(["volume-discount"]),
        &store,
        &session,
        &CheckoutState::default(),
        &config,
    );
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    assert!(
        result.errors[0].message.contains("between 0 and 100"),
        "{:?}",
        result.errors
    );
    // The Function ran but its output cannot be applied: the run fails.
    let run = run_for(&result, "volume-discount");
    assert!(
        run.error
            .as_deref()
            .is_some_and(|error| error.contains("between 0 and 100")),
        "{:?}",
        run.error
    );
    assert!(result.discounts.is_empty());
    assert_eq!(result.lines[0].total, result.lines[0].subtotal);
    assert_eq!(result.cost.discounts, 0);
}

#[test]
fn a_product_discount_over_100_percent_is_refused() {
    out_of_range_discount(Product, json!({"minimumQuantity": 1, "percentage": 150}));
}

#[test]
fn an_order_discount_over_100_percent_is_refused() {
    out_of_range_discount(Order, json!({"orderPercentage": 150}));
}

#[test]
fn a_cart_transform_merges_lines_into_a_bundle() {
    let app = app();
    let store = store();
    let session = session(&store, &[(MUG, 1), (TEE, 1), (TOTE, 1)]);
    let mut config = config();
    config.function_metafields.insert(
        "bundle-transform".to_string(),
        settings(json!({"operations": [{
            "type": "merge", "lines": [1, 3], "parentVariant": TEE, "title": "Set", "percentageDecrease": "50"
        }]})),
    );
    let result = compute(
        &app,
        &Selection::only(["bundle-transform"]),
        &store,
        &session,
        &CheckoutState::default(),
        &config,
    );
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.lines.len(), 2);
    let bundle = &result.lines[0];
    assert_eq!(bundle.title, "Set");
    assert_eq!(bundle.transformed_by.as_deref(), Some("merge"));
    assert_eq!(bundle.id, "gid://shopify/CartLine/4");
    assert_eq!(
        bundle
            .components
            .iter()
            .map(|c| c.variant_id)
            .collect::<Vec<_>>(),
        [MUG, TOTE]
    );
    // 22.00 + 24.00, half price.
    assert_eq!(bundle.subtotal, 2300);
    assert_eq!(result.lines[1].variant_id, TEE);
    assert_eq!(result.cost.subtotal, 2300 + 3200);
}

/// Runs a `linesMerge` of the first two lines of `cart` (a tee at 32.00, a tote at 24.00, a mug
/// at 22.00), each taking the quantity in `takes`, with the parent line at `price`.
fn merged(
    cart: &'static [(u64, u32)],
    takes: [u32; 2],
    price: Option<serde_json::Value>,
) -> lsf_checkout::CheckoutResult {
    let mut merge = json!({
        "parentVariantId": format!("gid://shopify/ProductVariant/{MUG}"),
        "cartLines": [
            {"cartLineId": LINE_1, "quantity": takes[0]},
            {"cartLineId": LINE_2, "quantity": takes[1]},
        ],
    });
    if let Some(price) = price {
        merge["price"] = price;
    }
    let result = answer_on(
        "bundle-transform",
        "run",
        0,
        json!({"operations": [{"linesMerge": merge}]}),
        |_, store| session(store, cart),
        |_| {},
    );
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    result
}

#[test]
fn a_merged_line_has_the_quantity_of_the_bundles_it_holds() {
    // Four tees and four totes: four bundles of one each.
    let result = merged(&[(TEE, 4), (TOTE, 4)], [4, 4], None);
    assert_eq!(result.lines.len(), 1);
    let bundle = &result.lines[0];
    assert_eq!(bundle.quantity, 4);
    assert_eq!(bundle.unit_price, 3200 + 2400, "the price of one bundle");
    assert_eq!(bundle.subtotal, 4 * (3200 + 2400));
    assert_eq!(bundle.total, bundle.subtotal);
    assert_eq!(result.cost.subtotal, 4 * (3200 + 2400));
    let quantities: Vec<_> = bundle.components.iter().map(|c| c.quantity).collect();
    assert_eq!(
        quantities,
        [4, 4],
        "the units of each component, in all the bundles"
    );
    let components: i64 = bundle.components.iter().map(|c| c.subtotal).sum();
    assert_eq!(
        components, bundle.subtotal,
        "the components cost what the line costs"
    );
}

#[test]
fn a_merged_line_has_the_greatest_common_divisor_of_the_merged_quantities() {
    // Four tees and six totes: two bundles of two tees and three totes.
    let result = merged(&[(TEE, 4), (TOTE, 6)], [4, 6], None);
    let bundle = &result.lines[0];
    assert_eq!(result.lines.len(), 1);
    assert_eq!(bundle.quantity, 2);
    assert_eq!(bundle.unit_price, 2 * 3200 + 3 * 2400);
    assert_eq!(bundle.subtotal, 4 * 3200 + 6 * 2400);
    assert_eq!(result.cost.subtotal, 4 * 3200 + 6 * 2400);
    let quantities: Vec<_> = bundle.components.iter().map(|c| c.quantity).collect();
    assert_eq!(quantities, [4, 6]);
    assert_eq!(bundle.sources.iter().map(|s| s.quantity).sum::<u32>(), 10);
}

#[test]
fn merged_quantities_without_a_common_divisor_make_one_bundle() {
    // Two tees and three totes: a single bundle holds them all.
    let result = merged(&[(TEE, 2), (TOTE, 3)], [2, 3], None);
    let bundle = &result.lines[0];
    assert_eq!(bundle.quantity, 1);
    assert_eq!(bundle.unit_price, 2 * 3200 + 3 * 2400);
    assert_eq!(result.cost.subtotal, 2 * 3200 + 3 * 2400);
}

#[test]
fn the_units_a_merge_does_not_take_stay_in_a_line_of_their_own() {
    // Three tees and two totes, two of each merged: two bundles, and a tee left over.
    let result = merged(&[(TEE, 3), (TOTE, 2)], [2, 2], None);
    assert_eq!(result.lines.len(), 2);
    let (bundle, left) = (&result.lines[0], &result.lines[1]);
    assert_eq!(bundle.quantity, 2);
    assert_eq!(bundle.unit_price, 3200 + 2400);
    assert_eq!(left.variant_id, TEE);
    assert_eq!(left.quantity, 1);
    assert_eq!(
        left.id, LINE_1,
        "the cart line keeps its id and the units it still holds"
    );
    assert_eq!(left.transformed_by, None);
    assert_eq!(result.cost.subtotal, 2 * (3200 + 2400) + 3200);
    // The bundle components are lines of their own, with ids the cart line does not have.
    assert!(bundle.components.iter().all(|c| c.id != LINE_1));
}

#[test]
fn a_percentage_decrease_applies_to_every_bundle_of_a_merged_line() {
    let result = merged(
        &[(TEE, 4), (TOTE, 6)],
        [4, 6],
        Some(json!({"percentageDecrease": {"value": 50}})),
    );
    let bundle = &result.lines[0];
    assert_eq!(bundle.quantity, 2);
    // A bundle is 2 tees and 3 totes: 136.00, half price.
    assert_eq!(bundle.unit_price, (2 * 3200 + 3 * 2400) / 2);
    assert_eq!(bundle.subtotal, 2 * bundle.unit_price);
    assert_eq!(result.cost.subtotal, bundle.subtotal);
    // The components keep their own prices: the adjustment is the line's.
    assert_eq!(bundle.components[0].unit_price, 3200);
    assert_eq!(bundle.components[1].unit_price, 2400);
}

#[test]
fn the_functions_after_a_merge_see_the_quantity_of_the_merged_line() {
    let app = app();
    let store = store();
    // Two tees and two mugs merged into two bundles.
    let session = session(&store, &[(TEE, 2), (MUG, 2)]);
    let mut config = config();
    config.function_metafields.insert(
        "bundle-transform".to_string(),
        settings(json!({"operations": [{
            "type": "merge", "lines": [1, 2], "parentVariant": TOTE
        }]})),
    );
    config.function_metafields.insert(
        "quantity-limit".to_string(),
        settings(json!({"maxQuantity": 1})),
    );
    config.discounts = vec![discount(
        "volume-discount",
        "Two or more",
        None,
        &[Product],
        json!({"minimumQuantity": 2, "percentage": 50}),
    )];
    let result = compute(
        &app,
        &Selection::all(),
        &store,
        &session,
        &CheckoutState::default(),
        &config,
    );
    assert_eq!(result.lines.len(), 1);
    let bundle = &result.lines[0];
    assert_eq!(bundle.quantity, 2);
    assert_eq!(bundle.subtotal, 2 * (3200 + 2200));
    // The discount takes the line: it has two units. Half of the line, not of a unit.
    assert_eq!(
        bundle.discount_allocations.len(),
        1,
        "{:?}",
        result.discounts
    );
    assert_eq!(bundle.discount_allocations[0].amount, bundle.subtotal / 2);
    assert_eq!(bundle.total, bundle.subtotal / 2);
    assert_eq!(result.cost.discounts, bundle.subtotal / 2);
    // The validation takes it too: two units are more than the limit of one.
    assert!(
        result
            .errors
            .iter()
            .any(|error| error.function.as_deref() == Some("quantity-limit")),
        "{:?}",
        result.errors
    );
    let input = &run_for(&result, "quantity-limit").input;
    assert_eq!(input["cart"]["lines"][0]["quantity"], 2, "{input}");
}

#[test]
fn discounts_apply_to_the_lines_a_cart_transform_made() {
    let app = app();
    let store = store();
    let session = session(&store, &[(TEE, 1), (TOTE, 1)]);
    let mut config = config();
    config.function_metafields.insert(
        "bundle-transform".to_string(),
        settings(json!({"operations": [{"type": "update", "line": 2, "price": "10.00"}]})),
    );
    config.discounts = vec![discount(
        "volume-discount",
        "Everything",
        None,
        &[Order],
        json!({"orderPercentage": 50}),
    )];
    let result = compute(
        &app,
        &Selection::all(),
        &store,
        &session,
        &CheckoutState::default(),
        &config,
    );
    // 32.00 + 10.00, half off: the discount saw the transformed price.
    assert_eq!(result.cost.subtotal, 4200);
    assert_eq!(result.cost.discounts, 2100);
    let order: Vec<_> = result.runs.iter().map(|run| run.handle.as_str()).collect();
    assert_eq!(
        order[0], "bundle-transform",
        "the cart transform runs first: {order:?}"
    );
}

#[test]
fn a_function_that_fails_is_reported_and_blocks_the_checkout() {
    let app = app();
    let store = store();
    let session = session(&store, &[(TEE, 1)]);
    let mut config = config();
    // Line 5 does not exist: the Function returns an error.
    config.function_metafields.insert(
        "bundle-transform".to_string(),
        settings(json!({"operations": [{"type": "update", "line": 5, "title": "x"}]})),
    );
    let result = compute(
        &app,
        &Selection::only(["bundle-transform"]),
        &store,
        &session,
        &CheckoutState::default(),
        &config,
    );
    let run = run_for(&result, "bundle-transform");
    assert!(run.output.is_none());
    let error = run.error.as_deref().expect("the run failed");
    assert!(error.contains("the cart has no line 5"), "{error}");
    assert_eq!(result.errors.len(), 1);
    assert!(result.errors[0].message.contains("bundle-transform"));
    assert_eq!(
        result.errors[0].function.as_deref(),
        Some("bundle-transform")
    );
    // The checkout is still described.
    assert_eq!(result.cost.subtotal, 3200);
}

#[test]
fn only_the_selected_extensions_run() {
    let app = app();
    let store = store();
    let session = session(&store, &[(TEE, 9)]);
    let mut config = config();
    config.discounts = vec![discount(
        "volume-discount",
        "Volume",
        None,
        &[Product],
        json!({"minimumQuantity": 1, "percentage": 10}),
    )];

    let everything = compute(
        &app,
        &Selection::all(),
        &store,
        &session,
        &CheckoutState::default(),
        &config,
    );
    let mut handles: Vec<_> = everything
        .runs
        .iter()
        .map(|run| run.handle.as_str())
        .collect();
    handles.sort();
    assert_eq!(
        handles,
        [
            "bundle-transform",
            "delivery-rules",
            "payment-rules",
            "quantity-limit",
            "volume-discount"
        ]
    );
    assert!(everything.cost.discounts > 0);

    let only = compute(
        &app,
        &Selection::only(["quantity-limit"]),
        &store,
        &session,
        &CheckoutState::default(),
        &config,
    );
    assert_eq!(only.runs.len(), 1);
    assert_eq!(
        only.cost.discounts, 0,
        "the discount's Function is not enabled"
    );
    assert_eq!(
        only.errors.len(),
        1,
        "9 items is over the default limit of 5"
    );
}

#[test]
fn a_discount_without_its_function_is_an_error() {
    let app = app();
    let store = store();
    let session = session(&store, &[(TEE, 1)]);
    let mut config = config();
    config.discounts = vec![discount("nope", "Ghost", None, &[Product], json!({}))];
    let result = compute(
        &app,
        &Selection::all(),
        &store,
        &session,
        &CheckoutState::default(),
        &config,
    );
    assert!(
        result
            .errors
            .iter()
            .any(|e| e.message.contains("no Function `nope`")),
        "{:?}",
        result.errors
    );
}

#[test]
fn the_input_builder_gets_the_context_of_each_run() {
    let app = app();
    let store = store();
    let session = session(&store, &[(TEE, 1)]);
    let mut config = config();
    config.discounts = vec![discount(
        "volume-discount",
        "Volume",
        Some("CODE"),
        &[Product],
        json!({"percentage": 5}),
    )];
    config
        .function_metafields
        .insert("quantity-limit".into(), settings(json!({"maxQuantity": 1})));
    let state = CheckoutState {
        discount_codes: vec!["code".to_string()],
        ..CheckoutState::default()
    };

    let seen = std::cell::RefCell::new(Vec::new());
    let build = |query: &str,
                 variables: Option<&serde_json::Value>,
                 ctx: &lsf_checkout::input::InputContext| {
        seen.borrow_mut().push((
            query.lines().next().unwrap_or("").to_string(),
            variables.cloned(),
            ctx.owner.id.clone(),
            ctx.owner.kind,
            ctx.triggering_code.map(str::to_string),
            ctx.entered_codes.to_vec(),
            ctx.lines.len(),
            ctx.currency.to_string(),
        ));
        // A hand-written input.
        Ok(json!({
            "cart": {"lines": [{"id": "gid://shopify/CartLine/1", "quantity": 7}], "deliveryGroups": [], "cost": {"subtotalAmount": {"amount": "1.00"}}},
            "discount": {"discountClasses": ["PRODUCT"], "metafield": {"value": ctx.owner.metafields.first().map(|m| m.value.clone())}},
            "validation": {"metafield": {"value": ctx.owner.metafields.first().map(|m| m.value.clone())}},
            "triggeringDiscountCode": ctx.triggering_code,
        }))
    };
    let result = compute_with(
        &app,
        &Selection::only(["volume-discount", "quantity-limit"]),
        &store,
        &session,
        &state,
        &config,
        &build,
    );
    // The hand-written input has a line of 7 items: the validation fails, the discount (minimum quantity 0) applies.
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    let seen = seen.into_inner();
    assert_eq!(seen.len(), 2, "{seen:?}");

    let (query, variables, id, kind, triggering, entered, lines, currency) = &seen[0];
    assert!(query.starts_with("query Input"), "{query}");
    assert_eq!(
        variables.as_ref().map(|v| v["percentage"].clone()),
        Some(json!(5)),
        "the variables are the JSON of the owner's metafield"
    );
    assert_eq!(id, "gid://shopify/DiscountCodeNode/1");
    assert_eq!(*kind, lsf_checkout::input::OwnerKind::Discount);
    assert_eq!(triggering.as_deref(), Some("CODE"));
    assert_eq!(entered, &["code".to_string()]);
    assert_eq!((*lines, currency.as_str()), (1, "USD"));

    let (_, variables, id, kind, triggering, ..) = &seen[1];
    assert!(
        variables.is_none(),
        "quantity-limit declares no input variables"
    );
    assert_eq!(id, "gid://shopify/Validation/4");
    assert_eq!(*kind, lsf_checkout::input::OwnerKind::Validation);
    assert!(triggering.is_none());
}

#[test]
fn the_metafield_of_the_variables_must_be_json() {
    let app = app();
    let store = store();
    let session = session(&store, &[(TEE, 1)]);
    let mut config = config();
    config.discounts = vec![DiscountConfig {
        function: "volume-discount".into(),
        title: "Broken".into(),
        code: None,
        classes: vec![Product],
        metafields: vec![Metafield {
            namespace: "$app:config".into(),
            key: "settings".into(),
            kind: "single_line_text_field".into(),
            value: "not json".into(),
        }],
    }];
    let result = compute(
        &app,
        &Selection::only(["volume-discount"]),
        &store,
        &session,
        &CheckoutState::default(),
        &config,
    );
    assert_eq!(result.runs.len(), 1);
    assert!(
        result.runs[0]
            .error
            .as_deref()
            .unwrap()
            .contains("is not JSON")
    );
    assert_eq!(result.errors.len(), 1);
}

#[test]
fn a_function_without_a_module_is_an_error_not_a_panic() {
    let app = app();
    let store = store();
    let session = session(&store, &[(TEE, 1)]);
    let mut app = app;
    app.functions
        .iter_mut()
        .find(|f| f.handle == "quantity-limit")
        .unwrap()
        .wasm_path = "/nowhere/function.wasm".into();
    let result = compute(
        &app,
        &Selection::only(["quantity-limit"]),
        &store,
        &session,
        &CheckoutState::default(),
        &config(),
    );
    assert_eq!(result.errors.len(), 1);
    assert!(
        result.errors[0].message.contains("no Wasm module"),
        "{:?}",
        result.errors
    );
    assert!(result.runs[0].error.is_some());
}

// ---- lines are priced like the storefront cart ----

#[test]
fn lines_take_the_quantity_price_breaks_of_the_variant() {
    let app = app();
    let mut store = store();
    let (product, variant) = store.variant_location(TEE).unwrap();
    store.products[product].variants[variant].quantity_price_breaks = vec![(3, 2500), (10, 2000)];
    let config = config();
    let selection = Selection::only(Vec::<String>::new());
    let priced = |quantity| {
        let session = session(&store, &[(TEE, quantity)]);
        let result = compute(
            &app,
            &selection,
            &store,
            &session,
            &CheckoutState::default(),
            &config,
        );
        (
            result.lines[0].unit_price,
            result.lines[0].subtotal,
            result.cost.subtotal,
        )
    };
    assert_eq!(priced(2), (3200, 6400, 6400), "below the first break");
    assert_eq!(priced(3), (2500, 7500, 7500));
    assert_eq!(priced(12), (2000, 24000, 24000));
}

/// A selling plan (42, 5.00 off) on the product of the tee.
fn add_monthly_plan(store: &mut lsf_core::Store) {
    use lsf_core::store::{
        Adjustment, CheckoutCharge, PriceAdjustment, SellingPlan, SellingPlanGroup,
    };
    let group = store.selling_plan_groups.len();
    store.selling_plan_groups.push(SellingPlanGroup {
        id: "subscribe".to_string(),
        name: "Subscribe".to_string(),
        app_id: None,
        options: vec!["Delivery".to_string()],
        plans: vec![SellingPlan {
            id: 42,
            name: "Monthly".to_string(),
            description: None,
            options: vec!["Monthly".to_string()],
            price_adjustments: vec![PriceAdjustment {
                adjustment: Adjustment::FixedAmount(500),
                order_count: None,
            }],
            recurring_deliveries: true,
            checkout_charge: CheckoutCharge::Percentage(100),
        }],
    });
    let (product, _) = store.variant_location(TEE).unwrap();
    store.products[product].selling_plan_groups = vec![group];
}

#[test]
fn lines_take_the_price_of_their_selling_plan() {
    let app = app();
    let mut store = store();
    add_monthly_plan(&mut store);
    let (product, variant) = store.variant_location(TEE).unwrap();
    // A quantity price break does not stack with the plan: the cart takes the plan's price.
    store.products[product].variants[variant].quantity_price_breaks = vec![(2, 1000)];
    let mut session = session(&store, &[(TEE, 2), (TOTE, 1)]);
    session.cart_lines[0].selling_plan = Some(42);
    let result = compute(
        &app,
        &Selection::only(Vec::<String>::new()),
        &store,
        &session,
        &CheckoutState::default(),
        &config(),
    );
    assert_eq!(result.lines[0].unit_price, 2700, "32.00 less 5.00");
    assert_eq!(result.lines[0].subtotal, 5400);
    assert_eq!(
        result.lines[1].unit_price, 2400,
        "a line without a plan keeps the price of the variant"
    );
    assert_eq!(result.cost.subtotal, 7800);
}

#[test]
fn lines_carry_the_store_data_of_their_merchandise() {
    let app = app();
    let mut store = store();
    add_monthly_plan(&mut store);
    let mut session = session(&store, &[(TEE, 1), (TOTE, 1)]);
    session.cart_lines[0].selling_plan = Some(42);
    let result = compute(
        &app,
        &Selection::only(Vec::<String>::new()),
        &store,
        &session,
        &CheckoutState::default(),
        &config(),
    );
    let (product, variant) = store.variant(TEE).unwrap();
    assert!(!product.vendor.is_empty(), "the demo product has a vendor");
    let line = &result.lines[0];
    assert_eq!(line.vendor, product.vendor);
    assert_eq!(line.product_type, product.product_type);
    let options: Vec<_> = line
        .selected_options
        .iter()
        .map(|option| (option.name.as_str(), option.value.as_str()))
        .collect();
    let expected: Vec<_> = product
        .options
        .iter()
        .map(|option| option.name.as_str())
        .zip(variant.options.iter().map(String::as_str))
        .collect();
    assert!(!options.is_empty());
    assert_eq!(options, expected);
    let plan = line.selling_plan.as_ref().expect("the plan of the line");
    assert_eq!(plan.id, "gid://shopify/SellingPlan/42");
    assert!(plan.recurring_deliveries);
    assert_eq!(result.lines[1].selling_plan, None, "a line without a plan");

    let json = serde_json::to_value(line).unwrap();
    assert_eq!(json["vendor"], product.vendor.as_str());
    assert_eq!(json["productType"], product.product_type.as_str());
    assert_eq!(json["selectedOptions"][0]["name"], options[0].0);
    assert_eq!(json["sellingPlan"]["recurringDeliveries"], true);
    assert_eq!(json["sellingPlan"]["id"], "gid://shopify/SellingPlan/42");
}

#[test]
fn the_components_of_a_bundle_carry_their_own_store_data() {
    let result = expand(json!({
        "type": "expand", "line": 1,
        "items": [{"variant": MUG, "quantity": 1}, {"variant": TOTE, "quantity": 1}]
    }));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let store = store();
    let components = &result.lines[0].components;
    assert_eq!(components.len(), 2);
    for (component, id) in components.iter().zip([MUG, TOTE]) {
        let (product, variant) = store.variant(id).unwrap();
        assert_eq!(component.vendor, product.vendor);
        assert_eq!(component.product_type, product.product_type);
        assert_eq!(component.selected_options.len(), variant.options.len());
        assert_eq!(component.selling_plan, None);
    }
}

// ---- metafields written by UI extensions, read by Functions ----

#[test]
fn a_function_reads_the_cart_metafields_the_extensions_wrote() {
    use std::cell::RefCell;
    let app = app();
    let store = store();
    let session = session(&store, &[(TEE, 1)]);
    let config = config();
    // What `applyMetafieldChange` leaves in the state of the checkout.
    let state = CheckoutState {
        metafields: vec![Metafield {
            namespace: "$app:gift".to_string(),
            key: "message".to_string(),
            kind: "single_line_text_field".to_string(),
            value: "Happy birthday".to_string(),
        }],
        ..CheckoutState::default()
    };
    // The fixture modules do not read cart metafields: the input query is asked for them besides
    // its own, and what the executor builds from it is kept.
    let seen = RefCell::new(Vec::new());
    let build = |query: &str,
                 variables: Option<&serde_json::Value>,
                 ctx: &lsf_checkout::input::InputContext| {
        let asked = query.replacen(
            "cart {",
            "cart { written: metafield(namespace: \"$app:gift\", key: \"message\") { type value } \
             absent: metafield(namespace: \"$app:gift\", key: \"nope\") { value } \
             other: metafield(namespace: \"custom\", key: \"message\") { value }",
            1,
        );
        let input = lsf_checkout::input::build(&asked, variables, ctx)?;
        seen.borrow_mut().push(input["cart"].clone());
        Ok(input)
    };
    let mut config = config;
    config
        .function_metafields
        .insert("quantity-limit".into(), settings(json!({"maxQuantity": 5})));
    let result = compute_with(
        &app,
        &Selection::only(["quantity-limit"]),
        &store,
        &session,
        &state,
        &config,
        &build,
    );
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let seen = seen.into_inner();
    assert_eq!(seen.len(), 1);
    assert_eq!(
        seen[0]["written"],
        json!({"type": "single_line_text_field", "value": "Happy birthday"})
    );
    assert!(seen[0]["absent"].is_null());
    assert!(
        seen[0]["other"].is_null(),
        "another namespace is another metafield"
    );
}

// ---- outputs a Function should not be able to get applied ----
//
// The modules below do not compute anything: they answer with what they read, and the input
// builder of the test gives them the output to answer with. Whatever a real Function could
// write, the pipeline gets to see it.

/// A Wasm module that spins `spin` times, then writes its input back as its output.
fn answering_module(export: &str, spin: u32) -> String {
    r#"
(module
  (import "wasi_snapshot_preview1" "fd_read" (func $fd_read (param i32 i32 i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (func (export "EXPORT") (local $i i32)
    (loop $spin
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br_if $spin (i32.lt_u (local.get $i) (i32.const SPIN))))
    (i32.store (i32.const 0) (i32.const 256))
    (i32.store (i32.const 4) (i32.const 4096))
    (drop (call $fd_read (i32.const 0) (i32.const 0) (i32.const 1) (i32.const 8)))
    (i32.store (i32.const 4) (i32.load (i32.const 8)))
    (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 12)))))
"#
    .replace("EXPORT", export)
    .replace("SPIN", &spin.to_string())
}

/// What the cart of `answer` holds: two tees, then a tote (lines 1 and 2).
const CART: [(u64, u32); 2] = [(TEE, 2), (TOTE, 1)];

/// Runs `handle` on a cart of [`CART`] with a module that answers `output`, and returns the
/// checkout. `settings` is the configuration the Function reads (any, the module ignores it).
fn answer(
    handle: &str,
    export: &str,
    spin: u32,
    output: serde_json::Value,
    configure: impl FnOnce(&mut lsf_checkout::CheckoutConfig),
) -> lsf_checkout::CheckoutResult {
    answer_on(
        handle,
        export,
        spin,
        output,
        |_, store| session(store, &CART),
        configure,
    )
}

/// [`answer`] on the cart that `cart` makes of the store (and the files of the app copy, that
/// it can edit first).
fn answer_on(
    handle: &str,
    export: &str,
    spin: u32,
    output: serde_json::Value,
    cart: impl FnOnce(&AppCopy, &mut lsf_core::Store) -> lsf_core::Session,
    configure: impl FnOnce(&mut lsf_checkout::CheckoutConfig),
) -> lsf_checkout::CheckoutResult {
    static COPIES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let copy = AppCopy::new(&format!(
        "answer-{handle}-{}",
        COPIES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::write(
        copy.file(handle, "dist/function.wasm"),
        answering_module(export, spin),
    )
    .unwrap();
    let mut store = store();
    let session = cart(&copy, &mut store);
    let app = copy.load();
    let mut config = config();
    config
        .function_metafields
        .insert(handle.to_string(), settings(json!({})));
    configure(&mut config);
    compute_with(
        &app,
        &Selection::only([handle]),
        &store,
        &session,
        &CheckoutState::default(),
        &config,
        &|_, _, _| Ok(output.clone()),
    )
}

fn transform(operation: serde_json::Value) -> lsf_checkout::CheckoutResult {
    answer(
        "bundle-transform",
        "run",
        0,
        json!({"operations": [operation]}),
        |_| {},
    )
}

const LINE_1: &str = "gid://shopify/CartLine/1";
const LINE_2: &str = "gid://shopify/CartLine/2";

/// The run failed with `error` in its message, and the checkout says so once.
fn assert_refused(result: &lsf_checkout::CheckoutResult, handle: &str, error: &str) {
    let run = run_for(result, handle);
    assert!(
        run.error.as_deref().is_some_and(|e| e.contains(error)),
        "{:?} does not say `{error}`",
        run.error
    );
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    assert!(
        result.errors[0].message.contains(error),
        "{:?}",
        result.errors
    );
}

/// The cart as it was before any Function ran.
fn assert_untouched(result: &lsf_checkout::CheckoutResult) {
    assert_eq!(result.lines.len(), 2);
    assert_eq!(result.lines[0].id, LINE_1);
    assert_eq!(result.lines[0].quantity, 2);
    assert_eq!(result.lines[0].unit_price, 3200);
    assert!(result.lines[0].components.is_empty());
    assert_eq!(result.lines[0].transformed_by, None);
    assert_eq!(result.lines[1].id, LINE_2);
    assert_eq!(result.lines[1].quantity, 1);
    assert_eq!(result.cost.subtotal, 8800);
    assert_eq!(result.cost.discounts, 0);
}

#[test]
fn a_merge_cannot_take_a_line_twice() {
    // The line holds 2, each source takes 1: 2 in all, but the line is named twice.
    let result = transform(json!({"linesMerge": {
        "parentVariantId": format!("gid://shopify/ProductVariant/{MUG}"),
        "cartLines": [
            {"cartLineId": LINE_1, "quantity": 1},
            {"cartLineId": LINE_1, "quantity": 1},
        ],
    }}));
    assert_refused(&result, "bundle-transform", "merged more than once");
    assert_untouched(&result);
}

#[test]
fn a_merge_cannot_take_more_than_a_line_holds() {
    let result = transform(json!({"linesMerge": {
        "parentVariantId": format!("gid://shopify/ProductVariant/{MUG}"),
        "cartLines": [
            {"cartLineId": LINE_1, "quantity": 1},
            {"cartLineId": LINE_2, "quantity": 2},
        ],
    }}));
    assert_refused(&result, "bundle-transform", "which has 1");
    assert_untouched(&result);
}

#[test]
fn a_merge_takes_what_it_asks_for_from_each_line() {
    // The control of the two tests above: one unit of the tees stays, the tote goes.
    let result = transform(json!({"linesMerge": {
        "parentVariantId": format!("gid://shopify/ProductVariant/{MUG}"),
        "cartLines": [
            {"cartLineId": LINE_1, "quantity": 1},
            {"cartLineId": LINE_2, "quantity": 1},
        ],
    }}));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(run_for(&result, "bundle-transform").error, None);
    assert_eq!(result.lines.len(), 2);
    assert_eq!(result.lines[0].transformed_by.as_deref(), Some("merge"));
    assert_eq!(result.lines[0].unit_price, 3200 + 2400);
    assert_eq!(result.lines[1].quantity, 1, "one tee stays in the cart");
}

#[test]
fn a_merge_quantity_has_to_be_a_positive_whole_number() {
    for quantity in [json!(0), json!(-1), json!(4294967297u64), json!(1.5)] {
        let result = transform(json!({"linesMerge": {
            "parentVariantId": format!("gid://shopify/ProductVariant/{MUG}"),
            "cartLines": [{"cartLineId": LINE_1, "quantity": quantity}],
        }}));
        assert_refused(&result, "bundle-transform", "quantity");
        assert_untouched(&result);
    }
}

#[test]
fn a_merge_parent_has_to_be_a_variant_id() {
    let result = transform(json!({"linesMerge": {
        "parentVariantId": "not-an-id",
        "cartLines": [{"cartLineId": LINE_1, "quantity": 1}],
    }}));
    assert_refused(&result, "bundle-transform", "is not a product variant id");
    assert_untouched(&result);
}

#[test]
fn an_expanded_item_quantity_has_to_be_a_positive_whole_number() {
    for quantity in [
        json!(-2),
        json!(0),
        json!(1.5),
        json!(2147483648u64),
        json!(4294967297u64),
        json!(u64::MAX),
        json!(-9_000_000_000_000i64),
    ] {
        let result = transform(json!({"lineExpand": {
            "cartLineId": LINE_1,
            "expandedCartItems": [
                {"merchandiseId": format!("gid://shopify/ProductVariant/{MUG}"), "quantity": 1},
                {"merchandiseId": format!("gid://shopify/ProductVariant/{TOTE}"), "quantity": quantity},
            ],
        }}));
        assert_refused(&result, "bundle-transform", "quantity");
        assert_untouched(&result);
    }
}

#[test]
fn a_quantity_of_an_expand_that_overflows_the_line_is_refused() {
    // 2 bundles of 2147483647 components: more than a line can hold.
    let result = transform(json!({"lineExpand": {
        "cartLineId": LINE_1,
        "expandedCartItems": [
            {"merchandiseId": format!("gid://shopify/ProductVariant/{MUG}"), "quantity": 2147483647},
        ],
    }}));
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    assert!(run_for(&result, "bundle-transform").error.is_some());
    assert_untouched(&result);
}

#[test]
fn an_expand_needs_items() {
    let result = transform(json!({"lineExpand": {"cartLineId": LINE_1, "expandedCartItems": []}}));
    assert_refused(&result, "bundle-transform", "expandedCartItems");
    assert_untouched(&result);
}

#[test]
fn a_function_past_the_instruction_limit_fails_and_its_output_is_not_applied() {
    let update = json!({"operations": [{"lineUpdate": {"cartLineId": LINE_1, "title": "Spun"}}]});
    // Under the limit the same output is applied (the control).
    let fast = answer("bundle-transform", "run", 1000, update.clone(), |_| {});
    let run = run_for(&fast, "bundle-transform");
    assert_eq!(run.error, None);
    assert!(run.instructions < lsf_checkout::runner::INSTRUCTIONS_LIMIT);
    assert!(fast.errors.is_empty(), "{:?}", fast.errors);
    assert_eq!(fast.lines[0].title, "Spun");

    // 2 million turns of 8 instructions: past the 11 million.
    let slow = answer("bundle-transform", "run", 2_000_000, update, |_| {});
    let run = run_for(&slow, "bundle-transform");
    assert!(run.instructions > lsf_checkout::runner::INSTRUCTIONS_LIMIT);
    assert_refused(&slow, "bundle-transform", "instructions");
    assert_ne!(slow.lines[0].title, "Spun", "the output was applied");
    assert_untouched(&slow);
}

#[test]
fn a_discount_past_the_instruction_limit_is_not_applied() {
    let output = json!({"operations": [{"productDiscountsAdd": {
        "selectionStrategy": "ALL",
        "candidates": [{
            "targets": [{"cartLine": {"id": LINE_1}}],
            "value": {"percentage": {"value": "10"}},
        }],
    }}]});
    let configure = |config: &mut lsf_checkout::CheckoutConfig| {
        config.discounts = vec![discount(
            "volume-discount",
            "Spin",
            None,
            &[Product],
            json!({}),
        )];
    };
    let slow = answer(
        "volume-discount",
        "cart_lines_discounts_generate_run",
        2_000_000,
        output,
        configure,
    );
    assert_refused(&slow, "volume-discount", "instructions");
    assert!(slow.discounts.is_empty());
    assert_untouched(&slow);
}

/// A product discount, answered as is, on [`CART`].
fn product_discount(targets: serde_json::Value) -> lsf_checkout::CheckoutResult {
    answer(
        "volume-discount",
        "cart_lines_discounts_generate_run",
        0,
        json!({"operations": [{"productDiscountsAdd": {
            "selectionStrategy": "ALL",
            "candidates": [{"targets": targets, "value": {"percentage": {"value": "10"}}}],
        }}]}),
        |config| {
            config.discounts = vec![discount(
                "volume-discount",
                "Answer",
                None,
                &[Product],
                json!({}),
            )];
        },
    )
}

#[test]
fn a_discount_target_names_a_line_that_exists_and_a_quantity_that_makes_sense() {
    // The control: a quantity of 1 out of 2 takes 10 % off 32.00.
    let result = product_discount(json!([{"cartLine": {"id": LINE_1, "quantity": 1}}]));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.cost.discounts, 320);

    for quantity in [json!(0), json!(-3), json!(4294967297u64), json!(2.5)] {
        let result = product_discount(json!([{"cartLine": {"id": LINE_1, "quantity": quantity}}]));
        assert_refused(&result, "volume-discount", "quantity");
        assert_untouched(&result);
    }
    let result = product_discount(json!([{"cartLine": {"id": "gid://shopify/CartLine/99"}}]));
    assert_refused(&result, "volume-discount", "no cart line");
    assert_untouched(&result);
}

#[test]
fn a_delivery_option_moves_to_a_valid_index_only() {
    let moves = |index: serde_json::Value| {
        answer(
            "delivery-rules",
            "run",
            0,
            json!({"operations": [{"deliveryOptionMove": {"deliveryOptionHandle": "express", "index": index}}]}),
            |_| {},
        )
    };
    let handles = |result: &lsf_checkout::CheckoutResult| -> Vec<String> {
        result.delivery_groups[0]
            .options
            .iter()
            .map(|option| option.handle.clone())
            .collect()
    };
    let result = moves(json!(0));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(handles(&result), ["express", "standard"]);

    for index in [
        json!(-1),
        json!(4294967296u64),
        json!(2147483648u64),
        json!(1.5),
    ] {
        let result = moves(index);
        assert_refused(&result, "delivery-rules", "index");
        assert_eq!(handles(&result), ["standard", "express"]);
    }
}

#[test]
fn a_payment_method_moves_to_a_valid_index_only() {
    let moves = |index: serde_json::Value| {
        answer(
            "payment-rules",
            "run",
            0,
            json!({"operations": [{"paymentMethodMove": {"paymentMethodId": "bank", "index": index}}]}),
            |_| {},
        )
    };
    let handles = |result: &lsf_checkout::CheckoutResult| -> Vec<String> {
        result
            .payment_methods
            .iter()
            .map(|method| method.handle.clone())
            .collect()
    };
    let result = moves(json!(0));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(handles(&result), ["bank", "card", "cod"]);

    for index in [
        json!(-1),
        json!(4294967296u64),
        json!(2147483648u64),
        json!(0.5),
    ] {
        let result = moves(index);
        assert_refused(&result, "payment-rules", "index");
        assert_eq!(handles(&result), ["card", "cod", "bank"]);
    }
}

// ---- discount targets that name a variant ----

/// Makes the discount Function of the copy a legacy one (`purchase.product-discount.run`,
/// `purchase.order-discount.run`), the only generation whose targets name variants.
fn make_legacy_discount(copy: &AppCopy, target: &str) {
    copy.replace(
        "volume-discount",
        "shopify.extension.toml",
        "target = \"cart.lines.discounts.generate.run\"",
        &format!("target = \"{target}\""),
    );
    copy.replace(
        "volume-discount",
        "shopify.extension.toml",
        "export = \"cart_lines_discounts_generate_run\"",
        "export = \"legacy_run\"",
    );
    std::fs::write(
        copy.file("volume-discount", "src/run.graphql"),
        "query Input { cart { lines { id } } }",
    )
    .unwrap();
}

/// Two lines of the tee, told apart by a line property, 32.00 each.
fn two_tee_lines(copy: &AppCopy, store: &mut lsf_core::Store) -> lsf_core::Session {
    make_legacy_discount(copy, "purchase.product-discount.run");
    let mut session = session(store, &[(TEE, 1), (TEE, 1)]);
    session.cart_lines[1]
        .properties
        .insert("Engraving".to_string(), "Hello".to_string());
    session
}

fn variant_discount(quantity: serde_json::Value) -> lsf_checkout::CheckoutResult {
    let mut target = json!({"id": format!("gid://shopify/ProductVariant/{TEE}")});
    if !quantity.is_null() {
        target["quantity"] = quantity;
    }
    answer_on(
        "volume-discount",
        "legacy_run",
        0,
        json!({
            "discountApplicationStrategy": "ALL",
            "discounts": [{
                "targets": [{"productVariant": target}],
                "value": {"percentage": {"value": "10"}},
            }],
        }),
        two_tee_lines,
        |config| {
            config.discounts = vec![discount(
                "volume-discount",
                "Answer",
                None,
                &[Product],
                json!({}),
            )];
        },
    )
}

#[test]
fn the_quantity_of_a_variant_target_is_one_budget_for_all_the_lines_of_the_variant() {
    // The control: without a quantity, both lines are discounted whole.
    let result = variant_discount(json!(null));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.lines.len(), 2);
    assert_eq!(result.cost.discounts, 640);

    // One item in all, not one per line: the first line only.
    let result = variant_discount(json!(1));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.cost.discounts, 320);
    assert_eq!(result.lines[0].total, 2880);
    assert_eq!(result.lines[1].total, 3200);

    // More than the cart holds takes what there is.
    assert_eq!(variant_discount(json!(5)).cost.discounts, 640);
}

#[test]
fn a_legacy_order_discount_can_target_a_variant() {
    let target = |quantity: Option<u32>| {
        let mut variant = json!({"id": format!("gid://shopify/ProductVariant/{TEE}")});
        if let Some(quantity) = quantity {
            variant["quantity"] = json!(quantity);
        }
        json!({"productVariant": variant})
    };
    let run = |targets: serde_json::Value| {
        answer_on(
            "volume-discount",
            "legacy_run",
            0,
            json!({
                "discountApplicationStrategy": "FIRST",
                "discounts": [{
                    "message": "Tees",
                    "targets": targets,
                    "value": {"percentage": {"value": "10"}},
                }],
            }),
            |copy, store| {
                make_legacy_discount(copy, "purchase.order-discount.run");
                // The tote is in the cart too, and stays whole.
                session(store, &[(TEE, 2), (TOTE, 1)])
            },
            |config| {
                config.discounts = vec![discount(
                    "volume-discount",
                    "Answer",
                    None,
                    &[Order],
                    json!({}),
                )];
            },
        )
    };

    // Both tees: 10 % of 64.00.
    let result = run(json!([target(None)]));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.cost.discounts, 640);
    assert_eq!(result.lines[0].total, 5760);
    assert_eq!(result.lines[1].total, 2400);
    // One tee out of two: 10 % of 32.00.
    let result = run(json!([target(Some(1))]));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.cost.discounts, 320);
    assert_eq!(result.lines[0].total, 6080);
    assert_eq!(result.lines[1].total, 2400);
}

// ---- cart transforms and selling plans ----

/// A cart of [`CART`] whose first line (two tees) has a selling plan.
fn plan_cart(_: &AppCopy, store: &mut lsf_core::Store) -> lsf_core::Session {
    add_monthly_plan(store);
    let mut session = session(store, &CART);
    session.cart_lines[0].selling_plan = Some(42);
    session
}

fn transform_plan_cart(operation: serde_json::Value) -> lsf_checkout::CheckoutResult {
    answer_on(
        "bundle-transform",
        "run",
        0,
        json!({"operations": [operation]}),
        plan_cart,
        |_| {},
    )
}

/// The cart as the plan made it, before any Function ran: 27.00 a tee.
fn assert_plan_cart_untouched(result: &lsf_checkout::CheckoutResult) {
    assert_eq!(result.lines.len(), 2);
    assert_eq!(result.lines[0].id, LINE_1);
    assert_eq!(result.lines[0].quantity, 2);
    assert_eq!(result.lines[0].unit_price, 2700);
    assert!(result.lines[0].components.is_empty());
    assert_eq!(result.lines[0].transformed_by, None);
    assert_eq!(result.lines[1].id, LINE_2);
    assert_eq!(result.lines[1].quantity, 1);
    assert_eq!(result.lines[1].unit_price, 2400);
    assert_eq!(result.cost.subtotal, 7800);
}

#[test]
fn a_cart_transform_cannot_expand_a_line_with_a_selling_plan() {
    let result = transform_plan_cart(json!({"lineExpand": {
        "cartLineId": LINE_1,
        "expandedCartItems": [{"merchandiseId": format!("gid://shopify/ProductVariant/{MUG}"), "quantity": 1}],
    }}));
    assert_refused(&result, "bundle-transform", "it has a selling plan");
    assert_plan_cart_untouched(&result);
}

#[test]
fn a_cart_transform_cannot_merge_a_line_with_a_selling_plan() {
    let result = transform_plan_cart(json!({"linesMerge": {
        "parentVariantId": format!("gid://shopify/ProductVariant/{MUG}"),
        "cartLines": [
            {"cartLineId": LINE_2, "quantity": 1},
            {"cartLineId": LINE_1, "quantity": 1},
        ],
    }}));
    assert_refused(&result, "bundle-transform", "it has a selling plan");
    assert_plan_cart_untouched(&result);
}

#[test]
fn a_cart_transform_cannot_update_a_line_with_a_selling_plan() {
    let result = transform_plan_cart(json!({"lineUpdate": {
        "cartLineId": LINE_1,
        "title": "Renamed",
    }}));
    assert_refused(&result, "bundle-transform", "it has a selling plan");
    assert_plan_cart_untouched(&result);

    // The control: the line without a plan can be updated.
    let result = transform_plan_cart(json!({"lineUpdate": {
        "cartLineId": LINE_2,
        "title": "Renamed",
    }}));
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.lines[1].transformed_by.as_deref(), Some("update"));
    assert_eq!(result.lines[0].transformed_by, None);
}

// ---- outputs that name a single value where the schema has a list ----

#[test]
fn a_validation_error_given_as_a_single_object_blocks_the_checkout() {
    let single = json!({"errors": {"localizedMessage": "No", "target": "$.cart"}});
    let result = answer("quantity-limit", "run", 0, single, |_| {});
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    assert_eq!(result.errors[0].message, "No");
    assert_eq!(result.errors[0].target.as_deref(), Some("$.cart"));
    assert!(run_for(&result, "quantity-limit").error.is_none());
}

#[test]
fn a_validation_add_given_single_objects_blocks_the_checkout() {
    let result = answer_on(
        "quantity-limit",
        "run",
        0,
        json!({"operations": {"validationAdd": {"errors": {"message": "No", "target": "$.cart"}}}}),
        |copy, store| {
            copy.replace(
                "quantity-limit",
                "shopify.extension.toml",
                "target = \"purchase.validation.run\"",
                "target = \"cart.validations.generate.run\"",
            );
            session(store, &CART)
        },
        |_| {},
    );
    assert_eq!(result.errors.len(), 1, "{:?}", result.errors);
    assert_eq!(result.errors[0].message, "No");
    assert_eq!(result.errors[0].target.as_deref(), Some("$.cart"));
}

#[test]
fn a_cart_transform_operation_given_as_a_single_object_is_applied() {
    let result = answer(
        "bundle-transform",
        "run",
        0,
        json!({"operations": {"lineUpdate": {"cartLineId": LINE_2, "title": "Renamed"}}}),
        |_| {},
    );
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.lines[1].transformed_by.as_deref(), Some("update"));
}

// ---- what each step of the pipeline shows of the discounts ----

const DISCOUNTS_SEEN: &str = r#"query Spy {
    cart {
        discountApplications {
            allocationMethod targetSelection targetType
            totalAllocatedAmount { amount }
            value { __typename ... on MoneyV2 { amount } ... on PricingPercentageValue { value } }
            metafield(namespace: "$app:config", key: "settings") { jsonValue }
        }
        lines { discountAllocations { discountedAmount { amount } discountApplication { targetType } } }
        deliveryGroups { discountAllocations { discountedAmount { amount } discountApplication { targetType } } }
    }
}"#;

const ALLOCATIONS_SEEN: &str =
    "query Spy { cart { lines { discountAllocations { discountedAmount { amount } } } } }";

/// Runs a cart of four tees through a product discount (half price), an order discount (10 %)
/// and a shipping discount (free), in the order the pipeline runs them, and gives what the
/// input of each Function showed of the discounts: `(owner kind, what DISCOUNTS_SEEN read)`.
fn discounts_seen_by_each_step() -> Vec<(lsf_checkout::input::OwnerKind, serde_json::Value)> {
    let app = app();
    let store = store();
    let session = session(&store, &[(TEE, 4)]);
    let mut config = config();
    config.discounts = vec![
        discount(
            "volume-discount",
            "Half price",
            None,
            &[Product],
            json!({"minimumQuantity": 1, "percentage": 50}),
        ),
        discount(
            "volume-discount",
            "Ten percent",
            None,
            &[Order],
            json!({"orderPercentage": 10}),
        ),
        discount(
            "volume-discount",
            "Free shipping",
            None,
            &[Shipping],
            json!({"freeShippingAbove": 50}),
        ),
    ];
    let seen = std::cell::RefCell::new(Vec::new());
    let build = |query: &str,
                 variables: Option<&serde_json::Value>,
                 ctx: &lsf_checkout::input::InputContext| {
        let spy = match ctx.owner.kind {
            lsf_checkout::input::OwnerKind::CartTransform => ALLOCATIONS_SEEN,
            _ => DISCOUNTS_SEEN,
        };
        let observed = lsf_checkout::input::build(spy, None, ctx)?;
        seen.borrow_mut().push((ctx.owner.kind, observed));
        lsf_checkout::input::build(query, variables, ctx)
    };
    let result = compute_with(
        &app,
        &Selection::only([
            "bundle-transform",
            "volume-discount",
            "delivery-rules",
            "payment-rules",
            "quantity-limit",
        ]),
        &store,
        &session,
        &CheckoutState::default(),
        &config,
        &build,
    );
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    // 128.00 - 64.00 - 6.40, and the shipping is free.
    assert_eq!(result.cost.total, 5760);
    seen.into_inner()
}

#[test]
fn each_function_sees_the_discounts_of_the_steps_before_it() {
    use lsf_checkout::input::OwnerKind::*;
    let seen = discounts_seen_by_each_step();
    let kinds: Vec<_> = seen.iter().map(|(kind, _)| *kind).collect();
    assert_eq!(
        kinds,
        [
            CartTransform,
            Discount,
            Discount,
            DeliveryCustomization,
            Discount,
            PaymentCustomization,
            Validation
        ]
    );
    let applications = |step: usize| seen[step].1["cart"]["discountApplications"].clone();
    let allocated = |step: usize, key: &str| {
        seen[step].1["cart"][key]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["discountAllocations"].clone())
            .collect::<Vec<_>>()
    };

    // The cart transform runs before any discount.
    assert_eq!(
        seen[0].1["cart"]["lines"][0]["discountAllocations"],
        json!([])
    );
    // The first discount sees nothing: Functions of a step see the ones that ran before.
    assert_eq!(applications(1), json!([]));
    // The order discount sees the product discount: half price, 64.00 across the line.
    let half = json!({
        "allocationMethod": "EACH", "targetSelection": "EXPLICIT", "targetType": "LINE_ITEM",
        "totalAllocatedAmount": {"amount": "64.0"},
        "value": {"__typename": "PricingPercentageValue", "value": "50.0"},
        "metafield": {"jsonValue": {"minimumQuantity": 1, "percentage": 50}},
    });
    assert_eq!(applications(2), json!([half]));
    assert_eq!(
        allocated(2, "lines"),
        [
            json!([{"discountedAmount": {"amount": "64.0"}, "discountApplication": {"targetType": "LINE_ITEM"}}])
        ]
    );

    // The delivery customization sees both, on the line, and no shipping discount yet.
    let ten = json!({
        "allocationMethod": "EACH", "targetSelection": "ALL", "targetType": "LINE_ITEM",
        "totalAllocatedAmount": {"amount": "6.4"},
        "value": {"__typename": "PricingPercentageValue", "value": "10.0"},
        "metafield": {"jsonValue": {"orderPercentage": 10}},
    });
    assert_eq!(applications(3), json!([half, ten]));
    assert_eq!(allocated(3, "lines")[0].as_array().unwrap().len(), 2);
    assert_eq!(allocated(3, "deliveryGroups"), [json!([])]);
    assert_eq!(applications(4), json!([half, ten]));
    assert_eq!(allocated(4, "deliveryGroups"), [json!([])]);

    // Payment customization and validation see the shipping discount too, on the delivery group.
    let free = json!({
        "allocationMethod": "EACH", "targetSelection": "ALL", "targetType": "SHIPPING_LINE",
        "totalAllocatedAmount": {"amount": "4.9"},
        "value": {"__typename": "PricingPercentageValue", "value": "100.0"},
        "metafield": {"jsonValue": {"freeShippingAbove": 50}},
    });
    for step in [5, 6] {
        assert_eq!(applications(step), json!([half, ten, free]), "step {step}");
        assert_eq!(
            allocated(step, "deliveryGroups"),
            [
                json!([{"discountedAmount": {"amount": "4.9"}, "discountApplication": {"targetType": "SHIPPING_LINE"}}])
            ],
            "step {step}"
        );
        assert_eq!(
            allocated(step, "lines")[0].as_array().unwrap().len(),
            2,
            "step {step}"
        );
    }
}

// ---- the placements of payment methods ----

const CARD: &str = "gid://shopify/PaymentCustomizationPaymentMethod/card";

/// Hides `card` (offered in the list and as an express button) with a payment customization
/// that answers `hide`, and gives the checkout with what a Function after it sees:
/// `(handle, placements)` of the methods of the validation Function.
fn hide_placements(
    hide: serde_json::Value,
) -> (lsf_checkout::CheckoutResult, Vec<(String, Vec<String>)>) {
    let copy = AppCopy::new(&format!("placements-{}", hide));
    for handle in ["payment-rules", "quantity-limit"] {
        std::fs::write(
            copy.file(handle, "dist/function.wasm"),
            answering_module("run", 0),
        )
        .unwrap();
    }
    let store = store();
    let session = session(&store, &CART);
    let app = copy.load();
    let mut config = config();
    config.payment_methods[0].placements = vec![
        lsf_checkout::PaymentPlacement::PaymentMethod,
        lsf_checkout::PaymentPlacement::AcceleratedCheckout,
    ];
    config
        .function_metafields
        .insert("payment-rules".into(), settings(json!({})));
    config
        .function_metafields
        .insert("quantity-limit".into(), settings(json!({})));
    let seen = std::cell::RefCell::new(Vec::new());
    let build = |_: &str,
                 _: Option<&serde_json::Value>,
                 ctx: &lsf_checkout::input::InputContext| {
        match ctx.owner.kind {
            lsf_checkout::input::OwnerKind::PaymentCustomization => {
                // What the payment customization itself sees.
                let input =
                    lsf_checkout::input::build("{ paymentMethods { id placements } }", None, ctx)?;
                seen.borrow_mut().push(input["paymentMethods"].clone());
                Ok(json!({"operations": [{"paymentMethodHide": hide.clone()}]}))
            }
            _ => {
                // The validation input has no payment methods: read them from the context.
                let methods: Vec<_> = ctx
                    .payment_methods
                    .iter()
                    .map(|method| {
                        json!({
                            "id": lsf_checkout::input::payment_method_id(&method.handle),
                            "placements": method
                                .placements
                                .iter()
                                .map(|placement| placement.graphql())
                                .collect::<Vec<_>>(),
                        })
                    })
                    .collect();
                seen.borrow_mut().push(json!(methods));
                // The validation of the fixture app is a `purchase.validation.run` one.
                Ok(json!({"errors": []}))
            }
        }
    };
    let result = compute_with(
        &app,
        &Selection::only(["payment-rules", "quantity-limit"]),
        &store,
        &session,
        &CheckoutState::default(),
        &config,
        &build,
    );
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    let seen = seen.into_inner();
    let validation = seen[1]
        .as_array()
        .unwrap()
        .iter()
        .map(|method| {
            (
                method["id"]
                    .as_str()
                    .unwrap()
                    .rsplit('/')
                    .next()
                    .unwrap()
                    .to_string(),
                method["placements"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|p| p.as_str().unwrap().to_string())
                    .collect(),
            )
        })
        .collect();
    // The payment customization sees every method with the placements of the data.
    assert_eq!(
        seen[0],
        json!([
            {"id": CARD, "placements": ["PAYMENT_METHOD", "ACCELERATED_CHECKOUT"]},
            {"id": "gid://shopify/PaymentCustomizationPaymentMethod/cod", "placements": ["PAYMENT_METHOD"]},
            {"id": "gid://shopify/PaymentCustomizationPaymentMethod/bank", "placements": ["PAYMENT_METHOD"]},
        ])
    );
    (result, validation)
}

fn placements_of(seen: &[(String, Vec<String>)], handle: &str) -> Option<Vec<String>> {
    seen.iter()
        .find(|(method, _)| method == handle)
        .map(|(_, placements)| placements.clone())
}

#[test]
fn hiding_the_list_placement_keeps_the_express_button() {
    let (result, seen) = hide_placements(json!({
        "paymentMethodId": CARD, "placements": ["PAYMENT_METHOD"]
    }));
    // Not in the list of the checkout any more...
    assert_eq!(method_names(&result), ["Cash on Delivery", "Bank Deposit"]);
    // ...but the Functions after it still see the method, offered as an express button.
    assert_eq!(
        placements_of(&seen, "card"),
        Some(vec!["ACCELERATED_CHECKOUT".to_string()])
    );
}

#[test]
fn hiding_the_express_placement_keeps_the_method_in_the_list() {
    let (result, seen) = hide_placements(json!({
        "paymentMethodId": CARD, "placements": ["ACCELERATED_CHECKOUT"]
    }));
    assert_eq!(
        method_names(&result),
        ["Credit card", "Cash on Delivery", "Bank Deposit"]
    );
    assert_eq!(
        placements_of(&seen, "card"),
        Some(vec!["PAYMENT_METHOD".to_string()])
    );
}

#[test]
fn hiding_without_placements_hides_them_all() {
    let (result, seen) = hide_placements(json!({"paymentMethodId": CARD}));
    assert_eq!(method_names(&result), ["Cash on Delivery", "Bank Deposit"]);
    assert_eq!(placements_of(&seen, "card"), None);
}
