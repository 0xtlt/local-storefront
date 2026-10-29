//! The commands a Shopify app is checked with in CI, run in the app's folder: `lsf validate`,
//! `lsf checkout check`, `lsf function run`, and `lsf render --strict` for a theme.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A copy of the fixture app of lsf-checkout, in a folder of the test's own.
struct App(PathBuf);

impl App {
    fn new(name: &str) -> App {
        let path =
            std::env::temp_dir().join(format!("lsf-app-commands-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        copy(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../checkout/tests/fixtures/app"),
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
    Command::new(env!("CARGO_BIN_EXE_lsf"))
        .args(args)
        .current_dir(directory)
        .env_remove("LSF_THEME")
        .env_remove("LSF_DATA")
        .env_remove("LSF_APP")
        .output()
        .unwrap()
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
    let output = lsf(&app.0, &["checkout", "check"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("no scenario to check"),
        "{}",
        text(&output)
    );
    app.write(
        "shopify-local/checkouts/tees.json",
        r#"{"session": {"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 2}]}}}"#,
    );
    // In the app's folder: no `--app`, and its own store data.
    let output = lsf(&app.0, &["checkout", "check", "--format", "json"]);
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
    let output = lsf(&app.0, &["checkout", "check"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(text(&output).contains("invalid  typo"), "{}", text(&output));
}

#[test]
fn checkout_init_gives_the_functions_the_settings_their_input_query_reads() {
    let app = App::new("settings");
    // `quantity-limit` reads its settings as the variables of its input query.
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
    assert!(lsf(&app.0, &["init"]).status.success());
    app.write(
        "shopify-local/checkouts/tees.json",
        r#"{"session": {"cart": {"items": [{"variant": "TEE-BLK-M", "quantity": 2}]}}}"#,
    );
    // Without its settings, it cannot run, and the check says where they go.
    let output = lsf(&app.0, &["checkout", "check"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("`checkout.functions.quantity-limit.metafields`"),
        "{}",
        text(&output)
    );

    // `checkout init` writes them, with empty values: every Function runs.
    let output = lsf(&app.0, &["checkout", "init"]);
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
    let output = lsf(&app.0, &["checkout", "check"]);
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
    let output = lsf(&app.0, &["checkout", "init"]);
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
    // A typo is suggested.
    let output = lsf(&app.0, &["function", "run", "volume-discont"]);
    assert!(
        text(&output).contains("did you mean `volume-discount`?"),
        "{}",
        text(&output)
    );
}

#[test]
fn render_strict_fails_on_a_page_that_is_not_there() {
    let theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures/theme");
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
        text(&text_output).contains("templates/customers: warning: the 1 page(s)"),
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
