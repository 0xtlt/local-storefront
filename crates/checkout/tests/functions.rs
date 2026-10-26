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

#[test]
fn a_cart_transform_expands_a_line_into_components() {
    let app = app();
    let store = store();
    let session = session(&store, &[(TEE, 2)]);
    let mut config = config();
    config.function_metafields.insert(
        "bundle-transform".to_string(),
        settings(json!({"operations": [{
            "type": "expand", "line": 1, "title": "Duo", "percentageDecrease": "10",
            "items": [{"variant": MUG, "quantity": 2}, {"variant": TOTE, "quantity": 1, "price": "10.00"}]
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
    assert_eq!(result.lines.len(), 1, "the components are inside the line");
    let line = &result.lines[0];
    assert_eq!(line.id, "gid://shopify/CartLine/1");
    assert_eq!(line.title, "Duo");
    assert_eq!(line.transformed_by.as_deref(), Some("expand"));
    assert_eq!(line.quantity, 2);
    // Per bundle: 2 x 22.00 + 10.00 = 54.00, 10 % off = 48.60.
    assert_eq!(line.unit_price, 4860);
    assert_eq!(line.subtotal, 9720);
    assert_eq!(line.components.len(), 2);
    assert_eq!(line.components[0].variant_id, MUG);
    assert_eq!(line.components[0].quantity, 4);
    assert_eq!(line.components[1].unit_price, 1000);
    assert_eq!(line.components[0].id, "gid://shopify/CartLine/2");
    assert_eq!(line.components[1].id, "gid://shopify/CartLine/3");
    assert_eq!(result.cost.subtotal, 9720);
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

#[test]
fn lines_take_the_price_of_their_selling_plan() {
    use lsf_core::store::{
        Adjustment, CheckoutCharge, PriceAdjustment, SellingPlan, SellingPlanGroup,
    };
    let app = app();
    let mut store = store();
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
    let (product, variant) = store.variant_location(TEE).unwrap();
    store.products[product].selling_plan_groups = vec![group];
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
