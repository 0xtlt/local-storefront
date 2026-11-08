//! The commands a Shopify app is checked with in CI, run in the app's folder: `lsf validate`,
//! `lsf app check`, `lsf app function run`, and `lsf render --strict` for a theme.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A copy of the fixture app of lsf-app, in a folder of the test's own.
struct App(PathBuf);

impl App {
    fn new(name: &str) -> App {
        let path =
            std::env::temp_dir().join(format!("lsf-app-commands-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        copy(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../app/tests/fixtures/app"),
            &path,
        );
        App(path)
    }

    fn write(&self, file: &str, text: &str) {
        let path = self.0.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        // The links of builds (`target/` to a cache) are not the app's.
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            copy(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn lsf(directory: &Path, args: &[&str]) -> Output {
    lsf_as(directory, args, None)
}

/// [`lsf`], run as `LSF_COMMAND` says it is (the npm launcher sets `npx local-storefront`).
fn lsf_as(directory: &Path, args: &[&str], command: Option<&str>) -> Output {
    let mut lsf = Command::new(env!("CARGO_BIN_EXE_lsf"));
    lsf.args(args)
        .current_dir(directory)
        .env_remove("LSF_THEME")
        .env_remove("LSF_DATA")
        .env_remove("LSF_APP")
        .env_remove("LSF_COMMAND");
    if let Some(command) = command {
        lsf.env("LSF_COMMAND", command);
    }
    lsf.output().unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn checkout_check_runs_every_scenario_and_fails_on_one_that_fails() {
    let app = App::new("check");
    // The demo store, as `lsf init` writes it.
    assert!(lsf(&app.0, &["init"]).status.success());
    // No scenario: nothing is checked, and that is not a pass.
    let output = lsf(&app.0, &["app", "check"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("no scenario to check"),
        "{}",
        text(&output)
    );
    app.write(
        "shopify-local/checkouts/tees.json",
        r#"{"session": {"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 2}]}}, "extensions": ["quantity-limit"]}"#,
    );
    // In the app's folder: no `--app`, and its own store data.
    let output = lsf(&app.0, &["app", "check", "--format", "json"]);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("JSON: {}", text(&output)));
    assert_eq!(json["total"], 1, "{json}");
    assert_eq!(json["checked"], 1, "{json}");
    assert_eq!(json["ok"], output.status.success(), "{json}");

    // A scenario that does not validate fails the check.
    app.write(
        "shopify-local/checkouts/typo.json",
        r#"{"session": {"cart": {"items": [{"variant": "NOPE", "quantity": 1}]}}}"#,
    );
    let output = lsf(&app.0, &["app", "check"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(text(&output).contains("invalid  typo"), "{}", text(&output));
    // Counted in the singular.
    assert!(
        text(&output).contains("1 error, 0 warnings in the store data"),
        "{}",
        text(&output)
    );
}

/// Makes `quantity-limit` read its settings as the variables of its input query: without them
/// in the store data, it fails.
fn needs_settings(app: &App) {
    let query = std::fs::read_to_string(app.0.join("extensions/quantity-limit/src/run.graphql"))
        .unwrap()
        .replacen("query Input {", "query Input($tags: [String!]!) {", 1)
        .replacen(
            "buyerIdentity {",
            "buyerIdentity {\n      customer { hasAnyTag(tags: $tags) }",
            1,
        );
    app.write("extensions/quantity-limit/src/run.graphql", &query);
    let toml = std::fs::read_to_string(
        app.0
            .join("extensions/quantity-limit/shopify.extension.toml"),
    )
    .unwrap();
    app.write(
        "extensions/quantity-limit/shopify.extension.toml",
        &format!("{toml}\n  [extensions.input.variables]\n  namespace = \"$app:config\"\n  key = \"settings\"\n"),
    );
}

#[test]
fn checkout_init_gives_the_functions_the_settings_their_input_query_reads() {
    let app = App::new("settings");
    needs_settings(&app);
    assert!(lsf(&app.0, &["init"]).status.success());
    app.write(
        "shopify-local/checkouts/tees.json",
        r#"{"session": {"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 2}]}}, "extensions": ["quantity-limit"]}"#,
    );
    // Without its settings, it cannot run, and the check says where they go.
    let output = lsf(&app.0, &["app", "check"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("`checkout.functions.quantity-limit.metafields`"),
        "{}",
        text(&output)
    );

    // `app init` writes them, with empty values: every Function runs.
    let output = lsf(&app.0, &["app", "init"]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("app-settings.json"),
        "{}",
        text(&output)
    );
    let settings: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(app.0.join("shopify-local/app-settings.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        settings["checkout"]["functions"]["quantity-limit"]["metafields"]["$app:config.settings"],
        serde_json::json!({"type": "json", "value": {"tags": []}})
    );
    let output = lsf(&app.0, &["app", "check"]);
    assert!(
        !text(&output).contains("quantity-limit ·"),
        "{}",
        text(&output)
    );

    // What is missing from the file is added to it again, and what is filled in stays.
    app.write(
        "shopify-local/app-settings.json",
        r#"{"checkout": {"functions": {"other": {"metafields": {"$app:config.mode": "kept"}}}}}"#,
    );
    let output = lsf(&app.0, &["app", "init"]);
    assert!(
        text(&output).contains("added   to app-settings.json"),
        "{}",
        text(&output)
    );
    let settings: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(app.0.join("shopify-local/app-settings.json")).unwrap(),
    )
    .unwrap();
    let functions = &settings["checkout"]["functions"];
    assert_eq!(functions["other"]["metafields"]["$app:config.mode"], "kept");
    assert_eq!(
        functions["quantity-limit"]["metafields"]["$app:config.settings"]["value"],
        serde_json::json!({"tags": []})
    );
}

#[test]
fn validate_checks_the_app_of_the_folder_too() {
    let app = App::new("validate");
    // Without store data, the app is checked with the demo store, as its checkout runs.
    let output = lsf(&app.0, &["validate"]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(text(&output).contains("demo store"), "{}", text(&output));

    app.write(
        "shopify-local/shop.json",
        r#"{"shop": {"name": "Test shop", "currency": "USD"}}"#,
    );
    // A UI extension on an API version lsf does not run is an error of the app.
    app.write(
        "extensions/old-ui/shopify.extension.toml",
        "api_version = \"2025-07\"\n\n[[extensions]]\nname = \"Old\"\nhandle = \"old-ui\"\ntype = \"ui_extension\"\n\n  [[extensions.targeting]]\n  target = \"purchase.checkout.block.render\"\n",
    );
    let output = lsf(&app.0, &["validate", "--format", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], false);
    // The errors of the app count with those of the data.
    assert_eq!(json["errors"], json["app"]["errors"], "{json}");
    assert!(
        json["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["code"] == "app"
                && diagnostic["file"] == "extensions/old-ui"),
        "{json}"
    );
    assert!(json["errors"].as_u64() > Some(0), "{json}");
    assert!(
        json["app"]["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic["extension"] == "old-ui"
                && diagnostic["severity"] == "error"),
        "{json}"
    );
}

#[test]
fn function_run_finds_the_app_and_says_why_a_function_did_not_run() {
    let app = App::new("run");
    assert!(lsf(&app.0, &["init"]).status.success());
    // No discount names it: it never runs, and says so.
    let output = lsf(
        &app.0,
        &[
            "app",
            "function",
            "run",
            "volume-discount",
            "--session",
            r#"{"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 3}]}}"#,
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("not run: volume-discount: a discount Function runs for a discount of the store, and none names it"),
        "{}",
        text(&output)
    );
    // `--session` brings data for this run, as `PUT /__lsf/session` does: a discount that names
    // it, and it runs.
    let output = lsf(
        &app.0,
        &[
            "app",
            "function",
            "run",
            "volume-discount",
            "--session",
            r#"{"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 3}]},
                "data": {"checkout": {"discounts": [{"function": "volume-discount", "title": "Volume"}]}}}"#,
        ],
    );
    assert!(output.status.success(), "{}", text(&output));
    assert!(!text(&output).contains("not run"), "{}", text(&output));
    // Without a handle, the checkout's Functions: those the shop has an enabled owner of. The
    // others say why they did not run.
    let output = lsf(
        &app.0,
        &[
            "app",
            "function",
            "run",
            "--session",
            r#"{"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 1}]}}"#,
        ],
    );
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output)
            .contains("Not run: payment-rules: the shop has no payment customization that runs it"),
        "{}",
        text(&output)
    );
    assert!(
        text(&output).contains("(name it to run it anyway: `lsf app function run payment-rules`)"),
        "{}",
        text(&output)
    );
    // Named, it runs.
    let output = lsf(
        &app.0,
        &[
            "app",
            "function",
            "run",
            "payment-rules",
            "--session",
            r#"{"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 1}]}}"#,
        ],
    );
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("== payment-rules"),
        "{}",
        text(&output)
    );
    // Each payment method with its placements.
    assert!(
        text(&output).contains("payment: Credit card [PAYMENT_METHOD], "),
        "{}",
        text(&output)
    );
    // A typo is suggested.
    let output = lsf(&app.0, &["app", "function", "run", "volume-discont"]);
    assert!(
        text(&output).contains("did you mean `volume-discount`?"),
        "{}",
        text(&output)
    );
}

#[test]
fn function_run_says_when_the_scenario_leaves_the_function_out() {
    let app = App::new("left-out");
    assert!(lsf(&app.0, &["init"]).status.success());
    app.write(
        "shopify-local/checkouts/only-limit.json",
        r#"{"extensions": ["quantity-limit"],
            "session": {"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 3}]}},
            "checkout": {"discounts": [{"function": "volume-discount", "title": "Volume"}]}}"#,
    );
    let output = lsf(
        &app.0,
        &[
            "app",
            "function",
            "run",
            "volume-discount",
            "--scenario",
            "only-limit",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains(
            "not run: volume-discount: the scenario's `extensions` leaves it out: add \
             \"volume-discount\" to the `extensions` of checkouts/only-limit.json, or remove the \
             list to run every extension"
        ),
        "{}",
        text(&output)
    );
}

#[test]
fn app_check_names_the_functions_no_scenario_ran() {
    let app = App::new("ran-nowhere");
    assert!(lsf(&app.0, &["init"]).status.success());
    // No discount names `volume-discount`: it runs in no scenario.
    app.write(
        "shopify-local/checkouts/tees.json",
        r#"{"session": {"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 2}]}}, "extensions": ["quantity-limit"]}"#,
    );
    let only = ["app", "check", "--extension", "volume-discount"];
    let output = lsf(&app.0, &only);
    // A warning: the exit code is the check's.
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains(
            "warning  volume-discount: ran in no scenario, so nothing checks it (a discount \
             Function runs only for a discount that names it"
        ),
        "{}",
        text(&output)
    );
    // The summary says so too, rather than that every Function ran.
    assert!(
        text(&output).contains(": no Function failed, 1 Function ran in no scenario\n"),
        "{}",
        text(&output)
    );
    assert!(
        text(&output).contains("checked 1 of 1 scenario in "),
        "{}",
        text(&output)
    );
    assert!(
        !text(&output).contains("every Function ran"),
        "{}",
        text(&output)
    );
    let output = lsf(&app.0, &[&only[..], &["--format", "json"]].concat());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("JSON: {}", text(&output)));
    assert_eq!(json["ok"], true, "{json}");
    assert_eq!(
        json["ranInNoScenario"],
        serde_json::json!(["volume-discount"])
    );

    // A scenario with a discount that names it: it ran.
    app.write(
        "shopify-local/checkouts/discounted.json",
        r#"{"session": {"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 3}]}},
            "checkout": {"discounts": [{"function": "volume-discount", "title": "Volume"}]}}"#,
    );
    let output = lsf(&app.0, &[&only[..], &["--format", "json"]].concat());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("JSON: {}", text(&output)));
    assert_eq!(json["ranInNoScenario"], serde_json::json!([]), "{json}");
    let output = lsf(&app.0, &only);
    assert!(
        !text(&output).contains("ran in no scenario"),
        "{}",
        text(&output)
    );
    assert!(
        text(&output).contains("checked 2 of 2 scenarios in "),
        "{}",
        text(&output)
    );
    assert!(
        text(&output).contains(": every Function ran\n"),
        "{}",
        text(&output)
    );
}

/// A scenario in which every Function of the fixture app changes the checkout: the payment
/// customization hides Cash on Delivery and renames the card, the delivery customization hides
/// express, renames standard and moves pickup first, the cart transform reprices the mug, the
/// discount takes 10 % off the tees, and the validation refuses more than 2 of an item when the
/// order is placed.
fn every_function(expect: serde_json::Value) -> String {
    let settings = |value: serde_json::Value| serde_json::json!({"metafields": {"$app:config.settings": {"type": "json", "value": value}}});
    serde_json::json!({
        "session": {"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 3}, {"variant": "MUG-OAT", "quantity": 1}]}},
        "entered": {
            "email": "ada@example.com",
            "shipping_address": {"last_name": "Lovelace", "address1": "1 Main St", "city": "Portland", "country_code": "US", "province_code": "OR", "zip": "97201"}
        },
        "checkout": {
            "shipping_rates": [
                {"handle": "standard", "title": "Standard", "price": "5.00"},
                {"handle": "express", "title": "Express", "price": "15.00"},
                {"handle": "pickup", "title": "Pick up in store", "price": 0, "kind": "pick_up"}
            ],
            "payment_methods": [
                {"handle": "card", "name": "Credit card"},
                {"handle": "cod", "name": "Cash on Delivery"},
                {"handle": "paypal", "name": "PayPal", "placements": ["PAYMENT_METHOD", "ACCELERATED_CHECKOUT"]},
                {"handle": "shop_pay", "name": "Shop Pay", "placements": ["ACCELERATED_CHECKOUT"]}
            ],
            "discounts": [{"function": "volume-discount", "title": "Volume", "classes": ["PRODUCT"],
                "metafields": {"$app:config.settings": {"type": "json", "value": {"minimumQuantity": 3, "percentage": 10}}}}],
            "functions": {
                "payment-rules": settings(serde_json::json!({"hideName": "Cash on Delivery", "hideAbove": 50, "rename": {"Credit card": "Card (secure)"}})),
                "delivery-rules": settings(serde_json::json!({"hide": ["express"], "rename": {"standard": "Standard (3-5 days)"}, "first": "pickup"})),
                "bundle-transform": settings(serde_json::json!({"operations": [{"type": "update", "line": 2, "title": "Member mug", "price": "7.50"}]})),
                "quantity-limit": settings(serde_json::json!({"maxQuantity": 2, "onlyAtCompletion": true}))
            },
            "tax_rate": 10
        },
        "extensions": ["payment-rules", "delivery-rules", "bundle-transform", "quantity-limit", "volume-discount"],
        "expect": expect
    })
    .to_string()
}

#[test]
fn app_check_checks_what_the_functions_decide() {
    let app = App::new("expect");
    assert!(lsf(&app.0, &["init"]).status.success());
    // What every Function decided, as the scenario expects it.
    app.write(
        "shopify-local/checkouts/everything.json",
        &every_function(serde_json::json!({
            "payment_methods": {"equals": ["Card (secure)", "paypal"], "excludes": ["cod"]},
            "express_methods": {"includes": ["paypal", "shop_pay"]},
            "delivery_options": {
                "equals": [{"handle": "pickup", "cost": 0}, {"handle": "standard", "title": "Standard (3-5 days)", "cost": "5.00"}],
                "excludes": ["express"]
            },
            "discounts": {"includes": [{"title": "10% off 3+ items", "class": "PRODUCT", "amount": "9.60"}]},
            "lines": {"equals": [{"sku": "TEE-BLK-M", "quantity": 3, "total": "86.40"}, {"sku": "MUG-OAT", "title": "Member mug", "price": "7.50"}]},
            "errors": {"includes": [{"message": "Not possible to order more than 2 of each", "target": "$.cart"}]},
            "totals": {"subtotal": "103.50", "discounts": "9.60", "shipping": 0, "tax": "9.39", "total": "103.29"}
        })),
    );
    // The validation checks nothing while the buyer is on the page.
    app.write(
        "shopify-local/checkouts/on-the-page.json",
        &every_function(serde_json::json!({"step": "interaction", "errors": {"equals": []}})),
    );
    // No `expect`: only that its Functions run.
    app.write(
        "shopify-local/checkouts/plain.json",
        r#"{"session": {"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 1}]}}, "extensions": ["payment-rules"]}"#,
    );
    let output = lsf(&app.0, &["app", "check"]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("checked 3 of 3 scenarios in ")
            && text(&output).contains(": every Function ran, 15 expectations met\n"),
        "{}",
        text(&output)
    );
    let output = lsf(&app.0, &["app", "check", "--format", "json"]);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("JSON: {}", text(&output)));
    assert_eq!(json["ok"], true, "{json}");
    assert_eq!(
        json["expectations"],
        serde_json::json!({"checked": 15, "unmet": []})
    );

    // PayPal in the express buttons, a total, and the validation on the page: three are wrong.
    app.write(
        "shopify-local/checkouts/everything.json",
        &every_function(serde_json::json!({
            "express_methods": {"excludes": ["paypal"]},
            "payment_methods": {"includes": ["paypal"]},
            "totals": {"total": "100.00", "tax": "9.39"}
        })),
    );
    app.write(
        "shopify-local/checkouts/on-the-page.json",
        &every_function(serde_json::json!({
            "step": "interaction",
            "errors": {"includes": ["Not possible to order more than 2 of each"]}
        })),
    );
    let output = lsf(&app.0, &["app", "check"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    let printed = text(&output);
    for block in [
        "expect   everything: express_methods.excludes\n\
         \x20        expected: no entry matching \"paypal\"\n\
         \x20        actual:   [{\"handle\": \"paypal\", \"name\": \"PayPal\"}, {\"handle\": \"shop_pay\", \"name\": \"Shop Pay\"}]\n",
        "expect   everything: totals.total\n\
         \x20        expected: \"100.00\"\n\
         \x20        actual:   \"103.29\"\n",
        "expect   on-the-page: errors.includes (step: interaction)\n\
         \x20        expected: an entry matching \"Not possible to order more than 2 of each\"\n\
         \x20        actual:   []\n",
    ] {
        assert!(printed.contains(block), "missing\n{block}in\n{printed}");
    }
    assert!(
        printed.contains(": 0 failures, 0 invalid scenarios, 3 of 5 expectations unmet\n"),
        "{printed}"
    );
    let output = lsf(&app.0, &["app", "check", "--format", "json"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("JSON: {}", text(&output)));
    assert_eq!(json["ok"], false, "{json}");
    assert_eq!(json["expectations"]["checked"], 5, "{json}");
    assert_eq!(
        json["expectations"]["unmet"][0],
        serde_json::json!({
            "scenario": "everything",
            "file": "checkouts/everything.json",
            "step": "completion",
            "assertion": "express_methods.excludes",
            "expected": "paypal",
            "actual": [{"handle": "paypal", "name": "PayPal"}, {"handle": "shop_pay", "name": "Shop Pay"}]
        })
    );
    assert_eq!(
        json["expectations"]["unmet"][2]["step"], "interaction",
        "{json}"
    );
    assert_eq!(
        json["expectations"]["unmet"][2]["actual"],
        serde_json::json!([]),
        "{json}"
    );

    // A handle `excludes` names that the checkout does not offer checks nothing: `validate`
    // warns, and the scenario still runs.
    app.write(
        "shopify-local/checkouts/everything.json",
        &every_function(serde_json::json!({"express_methods": {"excludes": ["paypall"]}})),
    );
    let output = lsf(&app.0, &["validate"]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains(
            "warning[expect_checks_nothing]: the checkout offers no payment method \"paypall\""
        ) && text(&output).contains("did you mean \"paypal\"?"),
        "{}",
        text(&output)
    );
}

/// The words of a JSON description: its keys, and the placeholders of its values.
fn words(text: &str) -> std::collections::BTreeSet<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

/// The keys of a JSON value, nested ones included.
fn keys(json: &serde_json::Value, into: &mut std::collections::BTreeSet<String>) {
    match json {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                into.insert(key.clone());
                keys(value, into);
            }
        }
        serde_json::Value::Array(items) => items.iter().for_each(|item| keys(item, into)),
        _ => {}
    }
}

#[test]
fn app_check_help_describes_the_json_it_prints() {
    let app = App::new("check-help");
    needs_settings(&app);
    assert!(lsf(&app.0, &["init"]).status.success());
    // A Function that fails, and a scenario that does not validate: every list has an entry.
    app.write(
        "shopify-local/checkouts/tees.json",
        // An expectation that is not met, of an amount: its values have no keys of their own.
        r#"{"session": {"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 2}]}}, "extensions": ["quantity-limit", "bundle-transform", "delivery-rules", "payment-rules", "volume-discount"], "expect": {"totals": {"total": "1.00"}}}"#,
    );
    app.write(
        "shopify-local/checkouts/typo.json",
        r#"{"session": {"cart": {"items": [{"variant": "NOPE", "quantity": 1}]}}}"#,
    );
    let output = lsf(&app.0, &["app", "check", "--format", "json"]);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("JSON: {}", text(&output)));
    assert!(!json["failures"].as_array().unwrap().is_empty(), "{json}");
    assert!(!json["invalid"].as_array().unwrap().is_empty(), "{json}");
    assert!(
        !json["expectations"]["unmet"].as_array().unwrap().is_empty(),
        "{json}"
    );
    let mut printed = std::collections::BTreeSet::new();
    keys(&json, &mut printed);
    // `error`, said when there is no scenario, is a key of the failures too.
    printed.insert("error".to_string());
    // The handles of `ranInNoScenario` are said `[handle]`.
    printed.insert("handle".to_string());

    let help = text(&lsf(&app.0, &["app", "check", "--help"]));
    let line = help
        .lines()
        .find(|line| line.trim_start().starts_with("- json:"))
        .unwrap_or_else(|| panic!("{help}"));
    let described = &line[line.find('{').unwrap()..=line.rfind('}').unwrap()];
    assert_eq!(words(described), printed, "{line}");
    assert!(line.contains("`error` when there is no scenario"), "{line}");

    // Counted in the singular: the scenario that validates is the one that runs.
    let output = lsf(&app.0, &["app", "check"]);
    assert!(
        text(&output).contains("checked 1 of 1 scenario in "),
        "{}",
        text(&output)
    );
    assert!(
        text(&output).contains(
            ": 1 failure, 1 invalid scenario, 1 of 1 expectation unmet, 1 Function ran in no scenario\n"
        ),
        "{}",
        text(&output)
    );
    assert!(
        text(&output).contains("1 error, 0 warnings in the store data"),
        "{}",
        text(&output)
    );
}

#[test]
fn app_check_says_what_a_function_threw() {
    let app = App::new("threw");
    assert!(lsf(&app.0, &["init"]).status.success());
    app.write(
        "extensions/boom/shopify.extension.toml",
        r#"api_version = "2026-10"

[[extensions]]
name = "Boom"
handle = "boom"
type = "function"

  [[extensions.targeting]]
  target = "cart.validations.generate.run"
  input_query = "src/cart_validations_generate_run.graphql"
  export = "cart-validations-generate-run"
"#,
    );
    app.write(
        "extensions/boom/package.json",
        r#"{ "name": "boom", "type": "module", "dependencies": { "@shopify/shopify_function": "2.0.1" } }"#,
    );
    app.write(
        "extensions/boom/src/index.js",
        "export * from './cart_validations_generate_run';\n",
    );
    app.write(
        "extensions/boom/src/cart_validations_generate_run.js",
        "export function cartValidationsGenerateRun(input) {\n  console.log('checking');\n  throw new Error('boom b2b');\n}\n",
    );
    app.write(
        "extensions/boom/src/cart_validations_generate_run.graphql",
        "query Input { cart { lines { quantity } } }\n",
    );
    app.write(
        "shopify-local/checkouts/b2b.json",
        r#"{"session": {"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 1}]}}, "extensions": ["boom"]}"#,
    );
    let check = ["app", "check", "--build", "--extension", "boom"];
    let output = lsf(&app.0, &check);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    // The message the Function threw, not the lines of its stack.
    assert!(
        text(&output).contains(
            "fails    boom · cart.validations.generate.run: the Function trapped: Error: boom b2b (in b2b)"
        ),
        "{}",
        text(&output)
    );
    let output = lsf(&app.0, &[&check[..], &["--format", "json"]].concat());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("JSON: {}", text(&output)));
    let failure = &json["failures"][0];
    assert_eq!(
        failure["error"], "the Function trapped: Error: boom b2b",
        "{json}"
    );
    let logs = failure["logs"].as_str().unwrap();
    assert!(logs.starts_with("checking\nError: boom b2b\n"), "{logs}");
}

#[test]
fn installed_with_npm_the_hints_say_npx_local_storefront() {
    let app = App::new("npx");
    assert!(lsf(&app.0, &["init"]).status.success());
    std::fs::remove_file(app.0.join("extensions/quantity-limit/dist/function.wasm")).unwrap();
    let npx = Some("npx local-storefront");
    // No scenario yet: `app init` writes them, and says what to run next.
    let output = lsf_as(&app.0, &["app", "check"], npx);
    assert!(
        text(&output).contains("`npx local-storefront app init` writes a set"),
        "{}",
        text(&output)
    );
    let output = lsf_as(&app.0, &["app", "init"], npx);
    assert!(
        text(&output).contains("next: `npx local-storefront app dev --app "),
        "{}",
        text(&output)
    );
    let output = lsf_as(&app.0, &["app", "check"], npx);
    let advice = "build it with `npx local-storefront app build quantity-limit` (or `--build`)";
    assert!(text(&output).contains(advice), "{}", text(&output));
    assert!(!text(&output).contains("`lsf "), "{}", text(&output));
    // A command run outside the app names itself as it was typed.
    let output = lsf_as(&std::env::temp_dir(), &["app", "check"], npx);
    assert!(
        text(&output).contains("`npx local-storefront app check` needs the app"),
        "{}",
        text(&output)
    );
}

#[test]
fn a_missing_build_says_to_run_lsf_app_build() {
    let app = App::new("unbuilt");
    assert!(lsf(&app.0, &["init"]).status.success());
    std::fs::remove_file(app.0.join("extensions/quantity-limit/dist/function.wasm")).unwrap();
    app.write(
        "shopify-local/checkouts/tees.json",
        r#"{"session": {"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 2}]}}, "extensions": ["quantity-limit"]}"#,
    );
    let advice = "build it with `lsf app build quantity-limit` (or `--build`)";
    for args in [
        &["app", "check"][..],
        &[
            "app",
            "function",
            "run",
            "quantity-limit",
            "--scenario",
            "tees",
        ][..],
    ] {
        let output = lsf(&app.0, args);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {}", text(&output));
        assert!(
            text(&output).contains(advice),
            "{args:?}: {}",
            text(&output)
        );
        assert!(
            !text(&output).contains("shopify app build"),
            "{args:?}: {}",
            text(&output)
        );
    }
}

#[test]
fn render_strict_fails_on_a_page_that_is_not_there() {
    let theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("../theme/tests/fixtures/theme");
    let theme = theme.to_str().unwrap();
    let output = lsf(
        Path::new("/"),
        &["--theme", theme, "render", "/products/nope", "--strict"],
    );
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("nothing at /products/nope"),
        "{}",
        text(&output)
    );
    let output = lsf(
        Path::new("/"),
        &["--theme", theme, "render", "/", "--strict"],
    );
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    assert!(
        text(&output).contains(" · 0 Liquid errors · "),
        "{}",
        text(&output)
    );
}

#[test]
fn init_and_app_init_count_in_the_singular_for_one() {
    let app = App::new("counted");
    let output = lsf(&app.0, &["init"]);
    assert!(
        text(&output).contains(" files, kept 0 existing ones, refreshed schema/"),
        "{}",
        text(&output)
    );
    // One file to write again.
    std::fs::remove_file(app.0.join("shopify-local/README.md")).unwrap();
    let output = lsf(&app.0, &["init"]);
    assert!(
        text(&output).contains(": wrote 1 file, kept "),
        "{}",
        text(&output)
    );

    let output = lsf(&app.0, &["app", "init"]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains(" scenarios, kept 0 existing ones, for "),
        "{}",
        text(&output)
    );
    assert!(!text(&output).contains("(s)"), "{}", text(&output));
    // One scenario to write again.
    let scenario = std::fs::read_dir(app.0.join("shopify-local/checkouts"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .unwrap();
    std::fs::remove_file(scenario).unwrap();
    let output = lsf(&app.0, &["app", "init"]);
    assert!(
        text(&output).contains(": wrote 1 scenario, kept "),
        "{}",
        text(&output)
    );
}

#[test]
fn check_says_the_pages_of_classic_customer_accounts_are_not_rendered() {
    let theme = std::env::temp_dir().join(format!("lsf-classic-theme-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&theme);
    std::fs::create_dir_all(theme.join("layout")).unwrap();
    std::fs::create_dir_all(theme.join("templates/customers")).unwrap();
    std::fs::write(
        theme.join("layout/theme.liquid"),
        "{{ content_for_layout }}",
    )
    .unwrap();
    std::fs::write(theme.join("templates/customers/login.liquid"), "login").unwrap();
    let text_output = lsf(&theme, &["check"]);
    let output = lsf(&theme, &["check", "--format", "json"]);
    std::fs::remove_dir_all(&theme).ok();
    assert!(
        text(&text_output).contains(
            "templates/customers: warning: the 1 page of templates/customers/ is for classic \
             customer accounts, which lsf does not have: it is never rendered."
        ),
        "{}",
        text(&text_output)
    );
    // A warning, not a problem: the theme is fine on Shopify.
    assert_eq!(output.status.code(), Some(0), "{}", text(&output));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], true, "{json}");
    assert_eq!(
        json["diagnostics"][0]["code"], "classic_customer_accounts",
        "{json}"
    );
    assert_eq!(json["diagnostics"][0]["severity"], "warning", "{json}");
}

#[test]
fn every_path_routes_lists_renders_with_render_strict() {
    let theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("../theme/tests/fixtures/theme");
    let theme = theme.to_str().unwrap();
    // The demo store: the fixture theme has no data of its own.
    let output = lsf(Path::new("/"), &["--theme", theme, "routes"]);
    assert!(output.status.success(), "{}", text(&output));
    let routes = String::from_utf8(output.stdout).unwrap();
    let routes: Vec<&str> = routes.lines().collect();
    assert!(routes.contains(&"/products/ceramic-mug"), "{routes:?}");
    // The stand-in of the customer accounts is the server's, not the theme's.
    assert!(!routes.contains(&"/account"), "{routes:?}");
    for route in routes {
        let output = lsf(
            Path::new("/"),
            &["--theme", theme, "render", route, "--strict"],
        );
        assert!(output.status.success(), "{route}: {}", text(&output));
    }
}

#[test]
fn function_schema_writes_the_schema_of_the_functions_api_and_version() {
    let app = App::new("schema");
    let folder = app.0.join("extensions/payment-rules");
    let _ = std::fs::remove_file(folder.join("schema.graphql"));
    // In the Function's folder, as `shopify app function schema`.
    let output = lsf(&folder, &["app", "function", "schema"]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("payment_customization"),
        "{}",
        text(&output)
    );
    let written = std::fs::read_to_string(folder.join("schema.graphql")).unwrap();
    assert!(written.starts_with("schema {"), "{}", &written[..40]);
    assert!(written.contains("input PaymentMethodHideOperation"));
    // `--stdout`, by handle from the app's folder: the same text, nothing written.
    std::fs::remove_file(folder.join("schema.graphql")).unwrap();
    let output = lsf(
        &app.0,
        &["app", "function", "schema", "payment-rules", "--stdout"],
    );
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(String::from_utf8_lossy(&output.stdout), written);
    assert!(!folder.join("schema.graphql").exists());
    // `--path` to another one.
    let output = lsf(
        &app.0,
        &[
            "app",
            "function",
            "schema",
            "--path",
            "extensions/delivery-rules",
            "--stdout",
        ],
    );
    assert!(output.status.success(), "{}", text(&output));
    assert!(String::from_utf8_lossy(&output.stdout).contains("DeliveryOptionHideOperation"));
    // Out of a Function's folder, without a handle: it says how to name one.
    let output = lsf(&app.0, &["app", "function", "schema"]);
    assert!(!output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("is not the folder of a Function"),
        "{}",
        text(&output)
    );
}
