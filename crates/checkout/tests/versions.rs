//! The API versions: each Function runs at the version it declares, against the schema of that
//! version. The fixture app mixes 2026-10 and 2027-01; the same modules are also run with the
//! whole app moved to each supported version.

mod common;

use common::*;
use lsf_checkout::app::{Selection, Severity};
use lsf_checkout::pipeline::compute;
use lsf_checkout::version::ApiVersion;
use lsf_checkout::{CheckoutResult, CheckoutState, DiscountClass::*, DiscountConfig};
use serde_json::json;

fn run(app: &lsf_checkout::app::App, lines: &[(u64, u32)]) -> CheckoutResult {
    let store = store();
    let session = session(&store, lines);
    let mut config = config();
    config.discounts = vec![DiscountConfig {
        function: "volume-discount".to_string(),
        title: "Volume".to_string(),
        code: None,
        classes: vec![Product, Shipping],
        metafields: settings(
            json!({"minimumQuantity": 2, "percentage": 10, "freeShippingAbove": 1}),
        ),
    }];
    config.function_metafields.insert(
        "delivery-rules".to_string(),
        settings(json!({"hide": ["express"], "rename": {"standard": "Standard (3-5 days)"}})),
    );
    config.function_metafields.insert(
        "payment-rules".to_string(),
        settings(json!({"rename": {"Bank Deposit": "Wire transfer"}})),
    );
    config.function_metafields.insert(
        "bundle-transform".to_string(),
        settings(json!({"operations": [{"type": "update", "line": 1, "title": "Member price"}]})),
    );
    compute(
        app,
        &Selection::all(),
        &store,
        &session,
        &CheckoutState::default(),
        &config,
    )
}

fn errors(app: &lsf_checkout::app::App) -> Vec<String> {
    app.diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.to_string())
        .collect()
}

#[test]
fn the_fixture_app_mixes_the_two_versions() {
    let app = app();
    let version = |handle: &str| app.function(handle).unwrap().version.map(|v| v.to_string());
    assert_eq!(version("volume-discount").as_deref(), Some("2026-10"));
    assert_eq!(version("delivery-rules").as_deref(), Some("2026-10"));
    assert_eq!(version("bundle-transform").as_deref(), Some("2026-10"));
    assert_eq!(version("payment-rules").as_deref(), Some("2027-01"));
    assert_eq!(version("quantity-limit").as_deref(), Some("2027-01"));
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));
    let result = run(&app, &[(TEE, 3)]);
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(
        result.runs.len(),
        6,
        "{:?}",
        result.runs.iter().map(|r| &r.target).collect::<Vec<_>>()
    );
}

#[test]
fn every_function_runs_at_each_supported_version() {
    let versions: Vec<_> = ApiVersion::all().collect();
    assert_eq!(
        versions.iter().map(|v| v.as_str()).collect::<Vec<_>>(),
        ["2026-10", "2027-01"],
        "the two versions the checkout starts with"
    );
    for version in versions {
        let copy = AppCopy::new(&format!("each-{version}"));
        copy.set_api_version(version.as_str());
        let app = copy.load();
        // `gift-message` declares a metafield without `owner_type`, which 2027-01 refuses: the
        // extension is not what this test runs.
        let errors: Vec<_> = errors(&app)
            .into_iter()
            .filter(|error| !error.starts_with("gift-message:"))
            .collect();
        assert!(errors.is_empty(), "{version}: {errors:?}");
        assert!(
            app.functions.iter().all(|f| f.version == Some(version)),
            "{version}"
        );

        let result = run(&app, &[(TEE, 3)]);
        assert!(result.errors.is_empty(), "{version}: {:?}", result.errors);
        // Every Function ran, and what each returned was applied.
        let mut handles: Vec<_> = result.runs.iter().map(|run| run.handle.as_str()).collect();
        handles.sort();
        handles.dedup();
        assert_eq!(handles.len(), 5, "{version}: {handles:?}");
        assert!(result.cost.discounts > 0, "{version}");
        assert_eq!(result.lines[0].title, "Member price", "{version}");
        let group = &result.delivery_groups[0];
        assert_eq!(group.options.len(), 1, "{version}: express is hidden");
        assert_eq!(group.options[0].title, "Standard (3-5 days)", "{version}");
        let names: Vec<_> = result
            .payment_methods
            .iter()
            .map(|m| m.name.as_str())
            .collect();
        assert!(names.contains(&"Wire transfer"), "{version}: {names:?}");
    }
}

#[test]
fn a_version_that_is_not_supported_is_an_error_and_the_function_is_not_run() {
    let copy = AppCopy::new("unsupported");
    copy.replace(
        "delivery-rules",
        "shopify.extension.toml",
        "api_version = \"2026-10\"",
        "api_version = \"2025-10\"",
    );
    let app = copy.load();
    let delivery = app.function("delivery-rules").unwrap();
    assert!(delivery.version.is_none());
    assert_eq!(delivery.api_version.as_deref(), Some("2025-10"));
    let errors = errors(&app);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].starts_with(
            "delivery-rules: api_version \"2025-10\" is not supported (supported: 2026-10, 2027-01)"
        ),
        "{errors:?}"
    );
    assert!(errors[0].ends_with("the Function is not run"), "{errors:?}");

    let result = run(&app, &[(TEE, 3)]);
    assert!(
        result.runs.iter().all(|run| run.handle != "delivery-rules"),
        "not run"
    );
    // The other Functions are not affected, and the delivery options are not customized.
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(result.delivery_groups[0].options.len(), 2);
}

#[test]
fn a_missing_version_is_an_error_too() {
    let copy = AppCopy::new("missing");
    copy.replace(
        "payment-rules",
        "shopify.extension.toml",
        "api_version = \"2027-01\"\n",
        "",
    );
    let app = copy.load();
    assert!(app.function("payment-rules").unwrap().version.is_none());
    let errors = errors(&app);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("no `api_version` (supported: 2026-10, 2027-01)"),
        "{errors:?}"
    );
}

#[test]
fn a_discount_whose_function_has_no_supported_version_is_an_error() {
    let copy = AppCopy::new("discount-version");
    copy.replace(
        "volume-discount",
        "shopify.extension.toml",
        "api_version = \"2026-10\"",
        "api_version = \"2026-07\"",
    );
    let app = copy.load();
    let result = run(&app, &[(TEE, 3)]);
    assert!(
        result
            .runs
            .iter()
            .all(|run| run.handle != "volume-discount")
    );
    assert!(!result.errors.is_empty());
    assert!(
        result
            .errors
            .iter()
            .all(|e| e.message.contains("api_version is not supported")),
        "{:?}",
        result.errors
    );
}

#[test]
fn a_field_the_schema_does_not_have_is_an_error_and_names_the_version() {
    for version in ApiVersion::all() {
        let copy = AppCopy::new(&format!("field-{version}"));
        copy.set_api_version(version.as_str());
        copy.replace(
            "delivery-rules",
            "src/run.graphql",
            "cart {",
            "cart {\n    note",
        );
        let app = copy.load();
        let result = run(&app, &[(TEE, 3)]);
        let error = &result.errors[0].message;
        assert!(
            error.contains(&format!("not valid in API version {version}"))
                && error.contains("`cart.note` does not exist on `Cart`"),
            "{error}"
        );
        // The query is checked before the Function runs: the run is recorded as failed.
        let run = run_for(&result, "delivery-rules");
        assert!(run.output.is_none() && run.instructions == 0);
        assert!(run.error.as_deref().unwrap().contains("`cart.note`"));
    }
}

#[test]
fn the_fields_added_in_2026_10_are_in_both_versions() {
    // `Customer.createdAt` and `CartDeliveryOption.metafield`, in the changelog of 2026-10.
    for version in ApiVersion::all() {
        let copy = AppCopy::new(&format!("added-{version}"));
        copy.set_api_version(version.as_str());
        copy.replace(
            "delivery-rules",
            "src/run.graphql",
            "handle\n        title",
            "handle\n        title\n        metafield(namespace: \"custom\", key: \"carrier\") { value }",
        );
        copy.replace(
            "delivery-rules",
            "src/run.graphql",
            "cart {",
            "cart {\n    buyerIdentity { customer { createdAt } }",
        );
        let app = copy.load();
        let result = run(&app, &[(TEE, 3)]);
        assert!(result.errors.is_empty(), "{version}: {:?}", result.errors);
        let input = &run_for(&result, "delivery-rules").input;
        assert!(
            input["cart"]["buyerIdentity"]["customer"].is_null(),
            "guest: {input}"
        );
        assert!(
            input["cart"]["deliveryGroups"][0]["deliveryOptions"][0]["metafield"].is_null(),
            "{input}"
        );
    }
}

#[test]
fn a_legacy_target_gets_the_operations_its_version_still_has() {
    // The delivery customization API has no operation left for `purchase.delivery-customization.run`.
    for version in ApiVersion::all() {
        let copy = AppCopy::new(&format!("legacy-delivery-{version}"));
        copy.set_api_version(version.as_str());
        copy.replace(
            "delivery-rules",
            "shopify.extension.toml",
            "cart.delivery-options.transform.run",
            "purchase.delivery-customization.run",
        );
        let app = copy.load();
        let warning = app
            .diagnostics
            .iter()
            .find(|d| d.extension.as_deref() == Some("delivery-rules"))
            .expect("a warning about the legacy target");
        assert_eq!(warning.severity, Severity::Warning);
        assert_eq!(
            warning.message,
            format!(
                "`purchase.delivery-customization.run` is a legacy target: use `cart.delivery-options.transform.run`; API version {version} takes no operation from it"
            )
        );
        let result = run(&app, &[(TEE, 3)]);
        let error = &result.errors[0].message;
        assert!(
            error.contains(&format!(
                "does not fit `FunctionRunResult` in API version {version}"
            )) && error.contains("`operations[0]` has no field `deliveryOptionRename`"),
            "{error}"
        );
        // Nothing was applied.
        assert_eq!(result.delivery_groups[0].options.len(), 2);
    }
}

#[test]
fn a_legacy_payment_target_only_sets_payment_terms() {
    let copy = AppCopy::new("legacy-payment");
    copy.replace(
        "payment-rules",
        "shopify.extension.toml",
        "cart.payment-methods.transform.run",
        "purchase.payment-customization.run",
    );
    let app = copy.load();
    assert!(
        app.diagnostics.iter().any(|d| d
            .message
            .ends_with("only takes the operations paymentTermsSet")),
        "{:?}",
        app.diagnostics
    );
    let result = run(&app, &[(TEE, 3)]);
    assert!(
        result.errors[0]
            .message
            .contains("`operations[0]` has no field `paymentMethodRename`"),
        "{:?}",
        result.errors
    );
    // The payment methods are the ones of the configuration.
    assert_eq!(result.payment_methods[2].name, "Bank Deposit");
}

#[test]
fn the_validation_output_follows_its_target() {
    // `purchase.validation.run` returns `{ errors }`; the current target returns operations.
    let ok = run(&app(), &[(TEE, 6)]);
    assert_eq!(ok.errors.len(), 1, "{:?}", ok.errors);
    assert!(
        ok.errors[0]
            .message
            .contains("Not possible to order more than 5")
    );

    let copy = AppCopy::new("validation-target");
    copy.replace(
        "quantity-limit",
        "shopify.extension.toml",
        "purchase.validation.run",
        "cart.validations.generate.run",
    );
    let app = copy.load();
    let result = run(&app, &[(TEE, 6)]);
    let error = &result.errors[0].message;
    assert!(
        error.contains("does not fit `CartValidationsGenerateRunResult` in API version 2027-01")
            && error.contains("the output has no field `errors`")
            && error.contains("`operations` is required"),
        "{error}"
    );
}

#[test]
fn a_cart_transform_of_the_first_generation_has_its_own_operation_names() {
    // `purchase.cart-transform.run` returns `expand`/`merge`/`update`, `cart.transform.run`
    // `lineExpand`/`linesMerge`/`lineUpdate`: the fixture returns the latter.
    let copy = AppCopy::new("legacy-transform");
    copy.replace(
        "bundle-transform",
        "shopify.extension.toml",
        "cart.transform.run",
        "purchase.cart-transform.run",
    );
    let app = copy.load();
    assert!(
        app.diagnostics.iter().any(|d| d
            .message
            .ends_with("only takes the operations expand, merge, update")),
        "{:?}",
        app.diagnostics
    );
    let result = run(&app, &[(TEE, 3)]);
    assert!(
        result.errors[0]
            .message
            .contains("`operations[0]` has no field `lineUpdate`"),
        "{:?}",
        result.errors
    );
}

#[test]
fn a_checkout_ui_extension_on_an_unsupported_version_is_an_error_and_not_offered() {
    let copy = AppCopy::new("ui-unsupported");
    copy.replace(
        "gift-message",
        "shopify.extension.toml",
        "api_version = \"2026-10\"",
        "api_version = \"2025-07\"",
    );
    let app = copy.load();
    let gift = app.ui_extension("gift-message").unwrap();
    assert!(gift.version.is_none());
    assert_eq!(gift.api_version.as_deref(), Some("2025-07"));
    assert!(app.ui_extension("order-notice").unwrap().version.is_some());
    let errors = errors(&app);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(
        errors[0],
        "gift-message: api_version \"2025-07\" is not supported (supported: 2026-10, 2027-01): the extension is not offered to the checkout page"
    );
}

#[test]
fn the_fixture_metafields_follow_the_rule_of_their_version() {
    let app = app();
    let gift = app.ui_extension("gift-message").unwrap();
    assert_eq!(gift.metafields.len(), 1);
    assert_eq!(gift.metafields[0].owner_type, None);
    let notice = app.ui_extension("order-notice").unwrap();
    assert_eq!(notice.metafields[0].owner_type.as_deref(), Some("CART"));
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));

    // The same declaration without `owner_type` is fine on 2026-10 and an error on 2027-01.
    let copy = AppCopy::new("ui-owner-type");
    copy.replace(
        "order-notice",
        "shopify.extension.toml",
        "  owner_type = \"CART\"\n",
        "",
    );
    let errors = errors(&copy.load());
    assert_eq!(
        errors,
        [
            "order-notice: API version 2027-01 requires `owner_type` on every metafield declaration: `$app:notice.dismissed` has none"
        ]
    );
}
