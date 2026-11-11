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
    // Counted in the singular, and its problem printed once, with it.
    assert!(
        text(&output).contains(": 0 failures, 1 invalid scenario"),
        "{}",
        text(&output)
    );
    assert_eq!(text(&output).matches("\"NOPE\" is neither").count(), 1);
}

/// A checkout UI extension on an `api_version` lsf does not run: `old-ui`, built.
fn old_ui_extension(app: &App) {
    app.write(
        "extensions/old-ui/shopify.extension.toml",
        r#"api_version = "2026-07"

[[extensions]]
name = "Old UI"
handle = "old-ui"
type = "ui_extension"

[[extensions.targeting]]
target = "purchase.checkout.block.render"
module = "./src/Checkout.jsx"
"#,
    );
    app.write(
        "extensions/old-ui/src/Checkout.jsx",
        "export default function Extension() { return null; }\n",
    );
    app.write("extensions/old-ui/dist/old-ui.js", "// built\n");
}

/// Makes the module of the Function `handle` an hour older than its files, as after an edit
/// with no build.
fn make_stale(app: &App, handle: &str) {
    let module = app
        .0
        .join(format!("extensions/{handle}/dist/function.wasm"));
    std::fs::OpenOptions::new()
        .write(true)
        .open(module)
        .unwrap()
        .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600))
        .unwrap();
}

/// A theme app extension, `badges`, with a block that has no `{% schema %}`.
fn block_without_schema(app: &App) {
    app.write(
        "extensions/badges/shopify.extension.toml",
        "name = \"Badges\"\ntype = \"theme\"\nhandle = \"badges\"\n",
    );
    app.write(
        "extensions/badges/blocks/badge.liquid",
        "<p>no schema</p>\n",
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
    // Named, it runs, and the output says the shop's checkout would not run it.
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
    assert!(
        text(&output).contains(
            "Named: payment-rules runs because this run names it, but the checkout of the store data does not run it: it has no payment customization of it. This command reads the store data on disk, not a running lsf"
        ),
        "{}",
        text(&output)
    );
    // With a disabled owner, the same, with why; with an enabled one, nothing to say.
    for (enabled, said) in [(false, true), (true, false)] {
        let session = format!(
            r#"{{"cart": {{"items": [{{"variant": "TEE-BLK-M", "quantity": 1}}]}},
                "data": {{"checkout": {{"functions": {{"payment-rules": {{"enabled": {enabled}}}}}}}}}}}"#
        );
        let output = lsf(
            &app.0,
            &[
                "app",
                "function",
                "run",
                "payment-rules",
                "--session",
                &session,
            ],
        );
        assert!(output.status.success(), "{}", text(&output));
        assert_eq!(
            text(&output).contains("its payment customization is disabled"),
            said,
            "{}",
            text(&output)
        );
        assert_eq!(text(&output).contains("Named: "), said, "{}", text(&output));
    }
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
        serde_json::json!({"checked": 15, "unchecked": [], "unmet": []})
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

    // `--extension` leaves out Functions the two scenarios with an `expect` run: their checkout
    // is another one, so their `expect` is not checked, and the Function kept still runs.
    let output = lsf(&app.0, &["app", "check", "--extension", "payment-rules"]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains(
            ": every Function ran, the `expect` of 2 scenarios not checked \
             (--extension leaves out a Function they run)\n"
        ) && !text(&output).contains("expect   "),
        "{}",
        text(&output)
    );
    let output = lsf(
        &app.0,
        &[
            "app",
            "check",
            "--extension",
            "payment-rules",
            "--format",
            "json",
        ],
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("JSON: {}", text(&output)));
    assert_eq!(
        json["expectations"],
        serde_json::json!({"checked": 0, "unchecked": ["everything", "on-the-page"], "unmet": []})
    );
    // With every Function the scenarios name, they are checked again.
    let mut every = vec!["app", "check"];
    for handle in [
        "payment-rules",
        "delivery-rules",
        "bundle-transform",
        "quantity-limit",
        "volume-discount",
    ] {
        every.extend(["--extension", handle]);
    }
    assert_eq!(lsf(&app.0, &every).status.code(), Some(1));

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
    // A UI extension the checkout does not offer, a module older than its files, a theme app
    // extension Shopify rejects.
    old_ui_extension(&app);
    make_stale(&app, "quantity-limit");
    block_without_schema(&app);
    let output = lsf(&app.0, &["app", "check", "--format", "json"]);
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("JSON: {}", text(&output)));
    assert!(!json["failures"].as_array().unwrap().is_empty(), "{json}");
    assert!(!json["invalid"].as_array().unwrap().is_empty(), "{json}");
    assert!(!json["notOffered"].as_array().unwrap().is_empty(), "{json}");
    assert!(!json["stale"].as_array().unwrap().is_empty(), "{json}");
    assert!(
        !json["themeExtensions"].as_array().unwrap().is_empty(),
        "{json}"
    );
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
            ": 1 failure, 1 invalid scenario, 1 of 1 expectation unmet, 1 extension not offered, 1 stale module, 1 error in theme app extensions, 1 Function ran in no scenario\n"
        ),
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

/// A JavaScript validation Function, `boom`, whose run is `body`, and a scenario for each of
/// `quantities` (`q<quantity>`), with a line of that quantity.
fn app_with_a_validation(name: &str, body: &str, quantities: &[u32]) -> App {
    let app = App::new(name);
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
        &format!("export function cartValidationsGenerateRun(input) {{\n{body}\n}}\n"),
    );
    app.write(
        "extensions/boom/src/cart_validations_generate_run.graphql",
        "query Input { cart { lines { quantity } } }\n",
    );
    for quantity in quantities {
        app.write(
            &format!("shopify-local/checkouts/q{quantity}.json"),
            &format!(
                r#"{{"session": {{"cart": {{"items": [{{"variant": "TEE-BLK-M", "quantity": {quantity}}}]}}}}, "extensions": ["boom"]}}"#
            ),
        );
    }
    app
}

/// One Function that fails in three ways in three scenarios: each error is printed with its
/// scenario and its logs, not the first one for all three. The endless loop stops at the
/// instruction cap, in a moment, not after 10 s.
#[test]
fn app_check_says_each_way_a_function_fails() {
    let app = app_with_a_validation(
        "three-ways",
        "  const quantity = input.cart.lines[0].quantity;\n  console.log(`quantity ${quantity}`);\n  \
         if (quantity === 1) throw new Error('boom');\n  \
         if (quantity === 2) return { operations: [{ validationAdd: { errorz: [] } }] };\n  \
         while (true) {}",
        &[1, 2, 3],
    );
    let check = ["app", "check", "--build", "--extension", "boom"];
    let started = std::time::Instant::now();
    let output = lsf(&app.0, &check);
    let said = text(&output);
    assert_eq!(output.status.code(), Some(1), "{said}");
    assert!(
        said.contains(
            "fails    boom · cart.validations.generate.run: the Function trapped: Error: boom (in q1)"
        ),
        "{said}"
    );
    assert!(
        said.contains("`operations[0].validationAdd` has no field `errorz`")
            && said.contains("(in q2)"),
        "{said}"
    );
    assert!(
        said.contains(
            "fails    boom · cart.validations.generate.run: exceeded Shopify's limit of 11,000,000 instructions: stopped at 110,000,000 (an endless loop?) (in q3)"
        ),
        "{said}"
    );
    assert!(said.contains("3 failures"), "{said}");
    // The build, three runs and one loop to the cap: far from the 10 s of the timeout.
    assert!(
        started.elapsed() < std::time::Duration::from_secs(8),
        "{:?}",
        started.elapsed()
    );

    let output = lsf(&app.0, &[&check[..], &["--format", "json"]].concat());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("JSON: {}", text(&output)));
    let failures = json["failures"].as_array().unwrap();
    assert_eq!(failures.len(), 3, "{json}");
    for (failure, quantity) in failures.iter().zip(["1", "2", "3"]) {
        assert_eq!(
            failure["scenarios"],
            serde_json::json!([format!("q{quantity}")]),
            "{json}"
        );
        let logs = failure["logs"].as_str().unwrap();
        assert!(
            logs.starts_with(&format!("quantity {quantity}\n")),
            "{logs}"
        );
    }
}

/// `app function run` says what a Function threw, prints no `-0.00` discount, and a build that
/// fails exits with 1, as a Function that fails does.
#[test]
fn function_run_says_what_a_function_threw_and_a_failed_build_exits_with_1() {
    let app = app_with_a_validation(
        "run-threw",
        "  console.log('checking');\n  throw new Error('boom');",
        &[1],
    );
    let output = lsf(
        &app.0,
        &[
            "app",
            "function",
            "run",
            "boom",
            "--scenario",
            "q1",
            "--build",
        ],
    );
    let said = text(&output);
    assert_eq!(output.status.code(), Some(1), "{said}");
    assert!(
        said.contains("\nerror: the Function trapped: Error: boom\n"),
        "{said}"
    );
    assert!(said.contains("logs:\n  checking\n"), "{said}");
    assert!(said.contains(" · discounts 0.00 USD · "), "{said}");
    assert!(!said.contains("-0.00"), "{said}");

    // A syntax error: the user's code does not build.
    app.write(
        "extensions/boom/src/cart_validations_generate_run.js",
        "export function cartValidationsGenerateRun(input) {\n",
    );
    for args in [
        &["app", "function", "build", "boom"][..],
        &["app", "build", "boom"],
        &["app", "check", "--build", "--extension", "boom"],
    ] {
        let output = lsf(&app.0, args);
        let said = text(&output);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {said}");
        assert!(
            said.contains("error: the build of boom failed: "),
            "{args:?}: {said}"
        );
    }
}

/// `--save-input` writes the input of a scenario's run, and `--input` (a file, or `-` for
/// stdin) runs the Function on it, as Shopify's CLI does, or on a fixture of the unit tests of
/// Shopify's templates, whose output it checks.
#[test]
fn function_run_takes_an_input_and_saves_one() {
    let app = app_with_a_validation(
        "input",
        "  const quantity = input.cart.lines[0].quantity;\n  console.log(`quantity ${quantity}`);\n  \
         return { operations: quantity > 2 ? [{ validationAdd: { errors: [{ message: 'too many', target: '$.cart' }] } }] : [] };",
        &[1, 3],
    );
    let saved = app.0.join("q3.json");
    let output = lsf(
        &app.0,
        &[
            "app",
            "function",
            "run",
            "boom",
            "--scenario",
            "q3",
            "--build",
            "--save-input",
            saved.to_str().unwrap(),
        ],
    );
    assert!(output.status.success(), "{}", text(&output));
    assert!(text(&output).contains("wrote "), "{}", text(&output));
    let input: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&saved).unwrap()).unwrap();
    assert_eq!(input["cart"]["lines"][0]["quantity"], 3, "{input}");

    // The file, in the Function's folder without naming it.
    let output = lsf(
        &app.0.join("extensions/boom"),
        &[
            "app",
            "function",
            "run",
            "--input",
            saved.to_str().unwrap(),
            "--format",
            "json",
        ],
    );
    assert!(output.status.success(), "{}", text(&output));
    let run: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        run["output"]["operations"][0]["validationAdd"]["errors"][0]["message"], "too many",
        "{run}"
    );
    assert_eq!(run["logs"], "quantity 3\n", "{run}");

    // Stdin.
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_lsf"))
        .args(["app", "function", "run", "boom", "--input", "-"])
        .current_dir(&app.0)
        .env_remove("LSF_APP")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(
        child.stdin.as_mut().unwrap(),
        br#"{"cart": {"lines": [{"quantity": 1}]}}"#,
    )
    .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("logs:\n  quantity 1\n"),
        "{}",
        text(&output)
    );

    // A fixture of Shopify's templates: its output is checked.
    let fixture = |output: &str| {
        let path = app.0.join("fixture.json");
        std::fs::write(
            &path,
            format!(
                r#"{{"payload": {{"export": "cart-validations-generate-run", "target": "cart.validations.generate.run", "input": {{"cart": {{"lines": [{{"quantity": 1}}]}}}}, "output": {output}}}}}"#
            ),
        )
        .unwrap();
        lsf(
            &app.0,
            &[
                "app",
                "function",
                "run",
                "boom",
                "--input",
                path.to_str().unwrap(),
            ],
        )
    };
    let output = fixture(r#"{"operations": []}"#);
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("the output is the one the fixture expects"),
        "{}",
        text(&output)
    );
    let output = fixture(r#"{"operations": [{"validationAdd": {"errors": []}}]}"#);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("error: the output is not the one the fixture expects"),
        "{}",
        text(&output)
    );

    // An output out of the schema fails, as in a checkout.
    std::fs::write(
        app.0.join("bad.json"),
        r#"{"cart": {"lines": [{"quantity": 3}]}}"#,
    )
    .unwrap();
    app.write(
        "extensions/boom/src/cart_validations_generate_run.js",
        "export function cartValidationsGenerateRun(input) {\n  return { operations: [{ validationAdd: { errorz: [] } }] };\n}\n",
    );
    let output = lsf(
        &app.0,
        &[
            "app", "function", "run", "boom", "--build", "--input", "bad.json",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("has no field `errorz`"),
        "{}",
        text(&output)
    );

    // A target `--export` does not name.
    let output = lsf(
        &app.0,
        &[
            "app", "function", "run", "boom", "--input", "bad.json", "--export", "nope",
        ],
    );
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));
    assert!(text(&output).contains("--export nope: boom has no such target (it has cart-validations-generate-run (cart.validations.generate.run))"), "{}", text(&output));
}

/// `app function typegen` writes the types of a JavaScript Function, as Shopify's CLI does.
#[test]
fn function_typegen_writes_the_types_of_a_javascript_function() {
    let app = app_with_a_validation("typegen", "  return { operations: [] };", &[]);
    app.write(
        "extensions/boom/package.json",
        r#"{ "name": "boom", "type": "module", "dependencies": { "@shopify/shopify_function": "2.0.1" },
  "codegen": { "schema": "schema.graphql", "documents": "src/*.graphql", "generates": { "./generated/api.ts": { "plugins": ["typescript", "typescript-operations"] } } } }"#,
    );
    let output = lsf(
        &app.0.join("extensions/boom/src"),
        &["app", "function", "typegen"],
    );
    assert!(output.status.success(), "{}", text(&output));
    let types = std::fs::read_to_string(app.0.join("extensions/boom/generated/api.ts")).unwrap();
    assert!(types.contains("export type InputQuery"), "{types}");
    // A Rust or other Function says why it writes nothing.
    let output = lsf(&app.0, &["app", "function", "typegen", "quantity-limit"]);
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("typegen: quantity-limit: not a JavaScript Function built by lsf"),
        "{}",
        text(&output)
    );
}

/// A scenario that does not validate is printed once, with its problems indented as a block.
#[test]
fn app_check_prints_an_invalid_scenario_once() {
    let app = App::new("invalid-once");
    assert!(lsf(&app.0, &["init"]).status.success());
    app.write(
        "shopify-local/checkouts/bad.json",
        r#"{"titel": "x", "session": {"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 1}]}}}"#,
    );
    let output = lsf(&app.0, &["app", "check"]);
    let said = text(&output);
    assert_eq!(output.status.code(), Some(1), "{said}");
    assert_eq!(said.matches("unknown field \"titel\"").count(), 1, "{said}");
    assert!(
        said.contains(
            "invalid  bad (checkouts/bad.json)\n         error[unknown_field]: unknown field \"titel\"\n           --> checkouts/bad.json at /titel\n"
        ),
        "{said}"
    );
    assert!(said.contains(": 0 failures, 1 invalid scenario"), "{said}");
    assert!(!said.contains("no scenario to check"), "{said}");
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

    // A customer account UI extension is counted as one, not as a checkout UI extension.
    app.write(
        "extensions/account-block/shopify.extension.toml",
        r#"api_version = "2026-10"

[[extensions]]
name = "Account block"
handle = "account-block"
type = "ui_extension"

  [[extensions.targeting]]
  target = "customer-account.order-index.block.render"
  module = "./src/Block.jsx"
"#,
    );
    let output = lsf(&app.0, &["app", "init"]);
    assert!(
        text(&output).contains(
            "for 5 Functions, 2 checkout UI extensions and 1 customer account UI extension of "
        ),
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

/// A scenario that names an extension the app does not have does not run: `app check` says so
/// and fails, rather than passing with nothing checked.
#[test]
fn app_check_fails_on_a_scenario_that_names_a_missing_extension() {
    let app = App::new("missing-extension");
    assert!(lsf(&app.0, &["init"]).status.success());
    app.write(
        "shopify-local/checkouts/ghost.json",
        r#"{"extensions": ["missing-function"], "session": {"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 1}]}}}"#,
    );
    let output = lsf(&app.0, &["app", "check", "--format", "json"]);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert_eq!(report["ok"], false, "{report}");
    let invalid = report["invalid"].as_array().unwrap();
    assert!(
        invalid
            .iter()
            .any(|scenario| scenario["scenario"] == "ghost"
                && scenario["problems"][0]
                    .as_str()
                    .unwrap()
                    .contains("missing-function")),
        "{report}"
    );
}

/// `--input --build`, in a Function's folder: it builds that Function only, not another
/// extension of the app that does not build.
#[test]
fn function_run_with_input_builds_only_its_function() {
    let app = app_with_a_validation("input-build", "  return { operations: [] };", &[]);
    app.write(
        "extensions/broken/shopify.extension.toml",
        r#"api_version = "2026-10"

[[extensions]]
name = "Broken"
handle = "broken"
type = "function"

  [[extensions.targeting]]
  target = "cart.validations.generate.run"
  input_query = "src/run.graphql"
  export = "cart-validations-generate-run"
"#,
    );
    app.write(
        "extensions/broken/package.json",
        r#"{ "name": "broken", "type": "module", "dependencies": { "@shopify/shopify_function": "2.0.1" } }"#,
    );
    app.write("extensions/broken/src/index.js", "export function (\n");
    app.write(
        "extensions/broken/src/run.graphql",
        "query Input { cart { lines { quantity } } }\n",
    );
    app.write("input.json", r#"{"cart": {"lines": [{"quantity": 1}]}}"#);
    let input = app.0.join("input.json");
    let output = lsf(
        &app.0.join("extensions/boom"),
        &[
            "app",
            "function",
            "run",
            "--input",
            input.to_str().unwrap(),
            "--build",
        ],
    );
    let said = text(&output);
    assert!(output.status.success(), "{said}");
    assert!(!said.contains("broken"), "{said}");
}

/// An extension the checkout does not offer fails `app check`, with why: named with
/// `--extension`, by a scenario, or not named at all. A discount Function a scenario names on an
/// `api_version` lsf does not run is not said to run in no scenario.
#[test]
fn app_check_fails_on_an_extension_the_checkout_does_not_offer() {
    let app = App::new("not-offered");
    assert!(lsf(&app.0, &["init"]).status.success());
    app.write(
        "shopify-local/checkouts/tees.json",
        r#"{"session": {"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 2}]}}, "extensions": ["quantity-limit"]}"#,
    );
    old_ui_extension(&app);
    for args in [
        &["app", "check"][..],
        &["app", "check", "--extension", "old-ui"],
    ] {
        let output = lsf(&app.0, args);
        assert_eq!(output.status.code(), Some(1), "{}", text(&output));
        assert!(
            text(&output).contains(
                "fails    old-ui: api_version \"2026-07\" is not supported (supported: 2026-10, 2027-01): the extension is not offered to the checkout page"
            ),
            "{}",
            text(&output)
        );
        assert!(
            text(&output).contains("1 extension not offered"),
            "{}",
            text(&output)
        );
    }
    std::fs::remove_dir_all(app.0.join("extensions/old-ui")).unwrap();

    // A discount Function a scenario's discount names, on a version lsf does not run.
    let toml = app
        .0
        .join("extensions/volume-discount/shopify.extension.toml");
    let text_of = std::fs::read_to_string(&toml).unwrap();
    std::fs::write(
        &toml,
        text_of.replacen("api_version = \"2026-10\"", "api_version = \"2026-07\"", 1),
    )
    .unwrap();
    app.write(
        "shopify-local/checkouts/discount.json",
        r#"{"session": {"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 3}]}}, "extensions": ["volume-discount"], "checkout": {"discounts": [{"function": "volume-discount", "title": "Volume"}]}}"#,
    );
    let output = lsf(&app.0, &["app", "check", "--extension", "volume-discount"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output)
            .contains("fails    volume-discount: api_version \"2026-07\" is not supported"),
        "{}",
        text(&output)
    );
    assert!(
        !text(&output).contains("ran in no scenario"),
        "{}",
        text(&output)
    );
    let output = lsf(
        &app.0,
        &[
            "app",
            "check",
            "--extension",
            "volume-discount",
            "--format",
            "json",
        ],
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ok"], false, "{json}");
    assert_eq!(json["notOffered"][0]["handle"], "volume-discount", "{json}");
    assert_eq!(json["ranInNoScenario"], serde_json::json!([]), "{json}");
}

/// What Shopify rejects in a theme app extension fails `validate`, `app check` and `app build`,
/// with `1`, as a problem to fix.
#[test]
fn theme_app_extension_errors_fail_validate_check_and_build() {
    let app = App::new("theme-errors");
    assert!(lsf(&app.0, &["init"]).status.success());
    app.write(
        "shopify-local/checkouts/tees.json",
        r#"{"session": {"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 2}]}}, "extensions": ["quantity-limit"]}"#,
    );
    let output = lsf(&app.0, &["app", "check"]);
    assert!(output.status.success(), "{}", text(&output));
    block_without_schema(&app);
    let error = "extensions/badges/blocks/badge.liquid: a block needs a {% schema %}";
    for args in [
        &["validate"][..],
        &["app", "check"],
        &["app", "build", "badges"],
    ] {
        let output = lsf(&app.0, args);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {}", text(&output));
        assert!(text(&output).contains(error), "{args:?}: {}", text(&output));
    }
    let output = lsf(&app.0, &["app", "check"]);
    assert!(
        text(&output).contains(&format!("invalid  badges: {error}")),
        "{}",
        text(&output)
    );
    // `--extension` naming others only leaves it out.
    let output = lsf(&app.0, &["app", "check", "--extension", "quantity-limit"]);
    assert!(output.status.success(), "{}", text(&output));
    let output = lsf(&app.0, &["app", "check", "--extension", "badges"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
}

/// A Function whose files changed after its module was built: `app check` fails on it unless
/// `--build`, and `app function run` builds it first, as `shopify app function run` does.
#[test]
fn a_module_older_than_its_files_is_built_first_or_fails_the_check() {
    let app = app_with_a_validation("stale", "  return { operations: [] };", &[1]);
    let check = ["app", "check", "--extension", "boom"];
    let output = lsf(&app.0, &[&check[..], &["--build"]].concat());
    assert!(output.status.success(), "{}", text(&output));
    let output = lsf(&app.0, &check);
    assert!(output.status.success(), "{}", text(&output));

    app.write(
        "extensions/boom/src/cart_validations_generate_run.js",
        "export function cartValidationsGenerateRun(input) {\n  console.log('new');\n  return { operations: [] };\n}\n",
    );
    let output = lsf(&app.0, &check);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains(
            "stale    boom: extensions/boom/src/cart_validations_generate_run.js changed after its module was built, which runs the old code: run with --build (or `lsf app build boom`)"
        ),
        "{}",
        text(&output)
    );
    assert!(
        text(&output).contains("1 stale module"),
        "{}",
        text(&output)
    );

    // Built first, and the run is the new code's.
    let output = lsf(
        &app.0,
        &["app", "function", "run", "boom", "--scenario", "q1"],
    );
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains("boom: extensions/boom/src/cart_validations_generate_run.js changed after its module was built: building it first"),
        "{}",
        text(&output)
    );
    assert!(text(&output).contains("logs:\n  new"), "{}", text(&output));
    let output = lsf(&app.0, &check);
    assert!(output.status.success(), "{}", text(&output));
}

/// `--save-input` creates the folders of its file, as the quickstart's
/// `tests/fixtures/six-totes.json`.
#[test]
fn save_input_creates_the_folders_of_its_file() {
    let app = app_with_a_validation("save-folders", "  return { operations: [] };", &[1]);
    let output = lsf(
        &app.0,
        &[
            "app",
            "function",
            "run",
            "boom",
            "--build",
            "--scenario",
            "q1",
            "--save-input",
            "tests/fixtures/q1.json",
        ],
    );
    assert!(output.status.success(), "{}", text(&output));
    let saved: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(app.0.join("tests/fixtures/q1.json")).unwrap(),
    )
    .unwrap();
    assert!(saved["cart"]["lines"].is_array(), "{saved}");
}

/// `--input` takes a folder of fixtures, as the unit tests of Shopify's templates, prints where
/// an output is not the one expected, refuses a fixture whose target or export is not the
/// Function's, and needs no handle when the app has one Function.
#[test]
fn function_run_input_runs_a_folder_of_fixtures() {
    let app = app_with_a_validation("fixtures", "  return { operations: [] };", &[1]);
    // The app has one Function: `boom`.
    for extension in [
        "quantity-limit",
        "payment-rules",
        "gift-message",
        "volume-discount",
        "bundle-transform",
        "order-notice",
        "delivery-rules",
    ] {
        std::fs::remove_dir_all(app.0.join("extensions").join(extension)).unwrap();
    }
    let fixture = |name: &str, target: &str, export: &str, output: &str| {
        app.write(
            &format!("tests/fixtures/{name}.json"),
            &format!(
                r#"{{"payload": {{"export": "{export}", "target": "{target}", "input": {{"cart": {{"lines": [{{"quantity": 1}}]}}}}, "output": {output}}}}}"#
            ),
        );
    };
    let (target, export) = (
        "cart.validations.generate.run",
        "cart-validations-generate-run",
    );
    fixture("a-pass", target, export, r#"{"operations": []}"#);
    let output = lsf(
        &app.0,
        &[
            "app",
            "function",
            "run",
            "--build",
            "--input",
            "tests/fixtures",
        ],
    );
    assert!(output.status.success(), "{}", text(&output));
    assert!(
        text(&output).contains(
            "pass     tests/fixtures/a-pass.json\nran 1 fixture of boom: 1 passed, 0 failed"
        ),
        "{}",
        text(&output)
    );

    fixture(
        "b-fail",
        target,
        export,
        r#"{"operations": [{"validationAdd": {"errors": [{"message": "Too many", "target": "$.cart"}]}}]}"#,
    );
    fixture(
        "c-target",
        "cart.payment-methods.transform.run",
        export,
        r#"{"operations": []}"#,
    );
    fixture("d-export", target, "nope-export", r#"{"operations": []}"#);
    let output = lsf(
        &app.0,
        &["app", "function", "run", "--input", "tests/fixtures"],
    );
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    let said = text(&output);
    for line in [
        "fails    tests/fixtures/b-fail.json: the output is not the one the fixture expects:\n           operations[0]\n             expected: {\"validationAdd\":{\"errors\":[{\"message\":\"Too many\",\"target\":\"$.cart\"}]}}\n             actual:   (nothing)\n",
        "fails    tests/fixtures/c-target.json: the fixture's target `cart.payment-methods.transform.run` is not a target of boom",
        "fails    tests/fixtures/d-export.json: the fixture's export `nope-export` is not the export of `cart.validations.generate.run`",
        "ran 4 fixtures of boom: 1 passed, 3 failed",
    ] {
        assert!(said.contains(line), "{line}\n---\n{said}");
    }
    let output = lsf(
        &app.0,
        &[
            "app",
            "function",
            "run",
            "--input",
            "tests/fixtures",
            "--format",
            "json",
        ],
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["failed"], 3, "{json}");
    assert_eq!(
        json["fixtures"][1]["run"]["mismatches"][0]["path"], "operations[0]",
        "{json}"
    );

    // One fixture: a wrong target is an error of the fixture (1), a mismatch prints its path.
    let output = lsf(
        &app.0,
        &[
            "app",
            "function",
            "run",
            "--input",
            "tests/fixtures/d-export.json",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    fixture(
        "e-value",
        target,
        export,
        r#"{"operations": [{"validationAdd": {"errors": []}}]}"#,
    );
    let output = lsf(
        &app.0,
        &[
            "app",
            "function",
            "run",
            "boom",
            "--input",
            "tests/fixtures/e-value.json",
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output)
            .contains("error: the output is not the one the fixture expects:\n  operations[0]\n"),
        "{}",
        text(&output)
    );
}
