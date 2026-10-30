//! The checkout playground (`examples/checkout-playground`): its store data and every scenario
//! are valid, the scenarios of `lsf app init` run no broken Function, and the Functions of
//! each `fn-*` scenario do what its description says.
//!
//! No network is needed. `discount-network` and `validation-network` call the mock server of
//! the playground (`node tools/fake-api.mjs`, `http://localhost:8787`). The tests never depend
//! on what listens there: they run on a copy of the store data whose URLs lead to a port nothing
//! listens on (the requests answer 502), and to a server of their own for the answers.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use lsf_checkout::{CheckoutResult, FunctionRun};

use super::*;

fn playground() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/checkout-playground")
}

fn scenarios() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(playground().join("shopify-local/checkouts"))
        .expect("the playground has scenarios")
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            name.strip_suffix(".json").map(str::to_string)
        })
        .collect();
    names.sort();
    names
}

/// The URL of the mock server in the store data.
const FAKE_API: &str = "http://localhost:8787";

/// A copy of the store data in the temporary folder, removed with the value (a test that fails
/// removes it too).
struct DataCopy(PathBuf);

impl Drop for DataCopy {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A copy of the store data of the playground whose network Functions call `base` instead of
/// the mock server.
fn data_calling(base: &str) -> DataCopy {
    static COPIES: AtomicUsize = AtomicUsize::new(0);
    let to = std::env::temp_dir().join(format!(
        "lsf-playground-{}-{}",
        std::process::id(),
        COPIES.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&to);
    copy_dir(&playground().join("shopify-local"), &to);
    let store = to.join("store.json");
    let text = std::fs::read_to_string(&store).unwrap();
    assert!(text.contains(FAKE_API), "the store data calls {FAKE_API}");
    std::fs::write(&store, text.replace(FAKE_API, base)).unwrap();
    DataCopy(to)
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            copy_dir(&path, &to.join(entry.file_name()));
        } else {
            std::fs::copy(&path, to.join(entry.file_name())).unwrap();
        }
    }
}

thread_local! {
    /// The store data the tests run on, one copy per test (each runs on a thread of its own,
    /// which removes it when it ends): the network Functions call a port nothing listens on.
    static DATA: DataCopy = {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        data_calling(&format!("http://127.0.0.1:{port}"))
    };
}

/// A server of the test that answers as the mock server does: 12% off, and the slate mug out of
/// stock. Its base URL.
fn serve_fake_api() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream);
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                continue;
            }
            let mut length = 0;
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) == 0 || header.trim().is_empty() {
                    break;
                }
                let (name, value) = header.split_once(':').unwrap_or(("", ""));
                if name.eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            let target = request_line.split(' ').nth(1).unwrap_or("");
            let (status, answer) = if target == "/discount" {
                (
                    "200 OK",
                    r#"{"percentage":12,"message":"Partner price"}"#.to_string(),
                )
            } else if let Some(skus) = target.strip_prefix("/stock?skus=") {
                let out: Vec<&str> = skus.split(',').filter(|sku| *sku == "MUG-SLT").collect();
                (
                    "200 OK",
                    serde_json::json!({ "unavailable": out }).to_string(),
                )
            } else {
                ("404 Not Found", "{}".to_string())
            };
            let mut stream = reader.into_inner();
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                answer.len()
            );
        }
    });
    base
}

fn run_at(scenario: &str, step: Step) -> CheckoutResult {
    DATA.with(|data| run_on(&data.0, scenario, step))
}

fn run_on(data: &Path, scenario: &str, step: Step) -> CheckoutResult {
    let dir = playground();
    let args = RunArgs {
        handles: vec![],
        app: Some(dir.clone()),
        build: false,
        session: None,
        scenario: Some(scenario.to_string()),
        checkout: None,
        locale: None,
        step,
        format: Format::Json,
    };
    match compute(&dir, Some(data), &[], &args) {
        Ok(ran) => ran.result,
        Err(error) => panic!("{scenario}: {error}"),
    }
}

fn run(scenario: &str) -> CheckoutResult {
    run_at(scenario, Step::Completion)
}

/// The handles of the Functions that ran, once each, in order.
fn ran(result: &CheckoutResult) -> Vec<&str> {
    let mut handles: Vec<&str> = Vec::new();
    for run in &result.runs {
        if !handles.contains(&run.handle.as_str()) {
            handles.push(&run.handle);
        }
    }
    handles
}

fn the_run<'a>(result: &'a CheckoutResult, handle: &str, target: &str) -> &'a FunctionRun {
    result
        .runs
        .iter()
        .find(|run| run.handle == handle && run.target == target)
        .unwrap_or_else(|| panic!("no run of {handle} at {target}: {:?}", ran(result)))
}

fn assert_ok(result: &CheckoutResult) {
    let failed: Vec<String> = result
        .runs
        .iter()
        .filter_map(|run| Some(format!("{}: {}", run.handle, run.error.as_ref()?)))
        .collect();
    assert!(failed.is_empty(), "{failed:?}");
}

/// The amount, in cents, of the applied discount with this title.
fn discount(result: &CheckoutResult, title: &str) -> i64 {
    result
        .discounts
        .iter()
        .find(|discount| discount.title == title)
        .unwrap_or_else(|| {
            let titles: Vec<&str> = result.discounts.iter().map(|d| d.title.as_str()).collect();
            panic!("no discount {title:?}: {titles:?}")
        })
        .amount
}

fn errors(result: &CheckoutResult) -> Vec<&str> {
    result
        .errors
        .iter()
        .map(|error| error.message.as_str())
        .collect()
}

fn options(result: &CheckoutResult) -> Vec<(&str, &str)> {
    result.delivery_groups[0]
        .options
        .iter()
        .map(|option| (option.handle.as_str(), option.title.as_str()))
        .collect()
}

fn payment_methods(result: &CheckoutResult) -> Vec<&str> {
    result
        .payment_methods
        .iter()
        .map(|method| method.name.as_str())
        .collect()
}

fn lines(result: &CheckoutResult) -> Vec<(&str, u32, i64)> {
    result
        .lines
        .iter()
        .map(|line| (line.title.as_str(), line.quantity, line.total))
        .collect()
}

#[test]
fn the_store_data_and_every_scenario_are_valid() {
    let dir = playground();
    let local = App::open_without_theme(&dir, Some(&dir.join("shopify-local")), &[]).unwrap();
    let (store, diagnostics) = local.load_store();
    let problems: Vec<String> = diagnostics
        .items
        .iter()
        .filter(|item| item.severity == Severity::Error)
        .map(|item| format!("{}{}: {}", item.file, item.path, item.message))
        .collect();
    assert!(problems.is_empty(), "{problems:#?}");
    assert_eq!(store.scenarios.len(), scenarios().len());
    for scenario in &store.scenarios {
        assert!(
            scenario.problems.is_empty(),
            "{}: {:?}",
            scenario.name,
            scenario.problems
        );
    }
}

#[test]
fn every_function_is_built_and_in_a_scenario() {
    let app = lsf_checkout::app::load(&playground()).unwrap();
    assert!(
        app.functions.len() >= 25,
        "{} Functions",
        app.functions.len()
    );
    let mut named = std::collections::BTreeSet::new();
    for name in scenarios() {
        let text = std::fs::read_to_string(
            playground().join(format!("shopify-local/checkouts/{name}.json")),
        )
        .unwrap();
        let scenario: serde_json::Value = serde_json::from_str(&text).unwrap();
        for handle in scenario["extensions"].as_array().into_iter().flatten() {
            named.insert(handle.as_str().unwrap().to_string());
        }
    }
    for function in &app.functions {
        assert!(
            named.contains(&function.handle),
            "{} is in no scenario",
            function.handle
        );
        assert_eq!(
            function.built(),
            function.handle != "broken-missing-build",
            "{}: built",
            function.handle
        );
    }
    // What is wrong with the app is the broken-* and listed-* Functions, the ui-broken-* checkout
    // UI extensions, and the legacy targets.
    for diagnostic in &app.diagnostics {
        let handle = diagnostic.extension.as_deref().unwrap_or("");
        assert!(
            handle.starts_with("broken-")
                || handle.starts_with("listed-")
                || handle.starts_with("ui-broken-")
                || diagnostic.message.contains("legacy target"),
            "{diagnostic}"
        );
    }
}

#[test]
fn the_init_scenarios_run_no_broken_function() {
    for name in [
        "abroad",
        "b2b",
        "customer",
        "guest",
        "large-cart",
        "pre-order",
        "shop-look",
        "subscription",
    ] {
        let result = run(name);
        assert_ok(&result);
        assert!(result.errors.is_empty(), "{name}: {:?}", errors(&result));
        // Nor a legacy target: an everyday scenario has nothing to warn about.
        assert!(
            ran(&result)
                .iter()
                .all(|handle| !handle.starts_with("broken-")
                    && !handle.contains("network")
                    && !handle.ends_with("-legacy")),
            "{name}: {:?}",
            ran(&result)
        );
        assert!(ran(&result).len() >= 20, "{name}: {:?}", ran(&result));
        // Their checkout UI extensions are working ones too.
        let listed = listed(name);
        assert!(
            listed.iter().any(|handle| handle.starts_with("ui-")),
            "{name}: {listed:?}"
        );
        assert!(
            listed
                .iter()
                .all(|handle| !handle.starts_with("ui-broken-")),
            "{name}: {listed:?}"
        );
    }
    // The Functions see what each of them is about.
    assert_eq!(discount(&run("customer"), "VIP: 20% off"), 1238);
    assert_eq!(
        discount(&run("b2b"), "Northwind Seattle: 12% contract price"),
        2784
    );
    assert_eq!(discount(&run("abroad"), "10% off in market europe"), 619);
}

/// The `extensions` list of a scenario.
fn listed(name: &str) -> Vec<String> {
    let text =
        std::fs::read_to_string(playground().join(format!("shopify-local/checkouts/{name}.json")))
            .unwrap();
    let scenario: serde_json::Value = serde_json::from_str(&text).unwrap();
    scenario["extensions"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|handle| handle.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn the_mix_scenarios_run_functions_and_checkout_ui_extensions_together() {
    let names: Vec<String> = scenarios()
        .into_iter()
        .filter(|name| name.starts_with("mix-"))
        .collect();
    assert!(names.len() >= 5, "{names:?}");
    for name in &names {
        let listed = listed(name);
        for handle in &listed {
            assert!(
                playground().join("extensions").join(handle).is_dir(),
                "{name}: no extension {handle}"
            );
        }
        let ui = listed
            .iter()
            .filter(|handle| handle.starts_with("ui-"))
            .count();
        assert!(ui > 0 && ui < listed.len(), "{name}: {listed:?}");
        // The legacy targets have scenarios of their own (`fn-*-legacy`).
        assert!(
            listed
                .iter()
                .all(|handle| handle.starts_with("ui-") || !handle.ends_with("-legacy")),
            "{name}: {listed:?}"
        );
        let result = run(name);
        assert_ok(&result);
        assert!(result.errors.is_empty(), "{name}: {:?}", errors(&result));
        // Every Function it lists ran: the UI extensions are the rest of the list.
        assert_eq!(
            ran(&result).len(),
            listed.len() - ui,
            "{name}: {:?}",
            ran(&result)
        );
    }
    assert_eq!(
        discount(&run("mix-b2b"), "Northwind Seattle: 12% contract price"),
        2784
    );
}

#[test]
fn the_network_functions_read_what_the_server_answers() {
    let base = serve_fake_api();
    let data = data_calling(&base);

    let result = run_on(&data.0, "fn-discount-network", Step::Completion);
    assert_ok(&result);
    let fetch = the_run(
        &result,
        "discount-network",
        "cart.lines.discounts.generate.fetch",
    );
    let exchange = fetch.exchange.as_ref().expect("the request was sent");
    assert_eq!(exchange.request.url, format!("{base}/discount"));
    assert_eq!(exchange.response.status, 200);
    assert_eq!(discount(&result, "Partner price: 12% off"), 768);

    let result = run_on(&data.0, "fn-validation-network", Step::Completion);
    assert_ok(&result);
    let fetch = the_run(
        &result,
        "validation-network",
        "cart.validations.generate.fetch",
    );
    let exchange = fetch.exchange.as_ref().expect("the request was sent");
    assert_eq!(
        exchange.request.url,
        format!("{base}/stock?skus=MUG-SLT,TOTE-NAT")
    );
    assert_eq!(exchange.response.status, 200);
    assert_eq!(errors(&result), ["Ceramic Mug is out of stock"]);
}

#[test]
fn every_fn_scenario_does_what_it_says() {
    let names: Vec<String> = scenarios()
        .into_iter()
        .filter(|name| name.starts_with("fn-"))
        .collect();
    assert!(names.len() >= 40, "{names:?}");
    for name in &names {
        let result = run(name);
        if !name.starts_with("fn-broken-") {
            assert_ok(&result);
        }
        check(name, &result);
    }
}

/// A few facts about the outcome of each `fn-*` scenario.
fn check(name: &str, result: &CheckoutResult) {
    let only = |handle: &str| assert_eq!(ran(result), [handle], "{name}");
    match name {
        // Discounts.
        "fn-discount-tag-percentage" => {
            only("discount-tag-percentage");
            // 15% of the overshirt (68.00) and of the two mugs (44.00), not of the tee.
            assert_eq!(discount(result, "15% off"), 1680);
        }
        "fn-discount-typescript" => {
            only("discount-typescript");
            // 10% of the two mugs (44.00), not of the single tee.
            assert_eq!(discount(result, "10% off 2 or more"), 440);
            // Written in TypeScript and built by Shopify's CLI: its module runs on Shopify's
            // Javy plugin, and what it gives `console.log` is its log.
            let module = std::fs::read(
                playground().join("extensions/discount-typescript/dist/function.wasm"),
            )
            .unwrap();
            let plugin = b"shopify_functions_javy_v";
            assert!(module.windows(plugin.len()).any(|bytes| bytes == plugin));
            let run = the_run(
                result,
                "discount-typescript",
                "cart.lines.discounts.generate.run",
            );
            assert!(run.logs.contains("Ceramic Mug: 2 units"), "{}", run.logs);
        }
        "fn-discount-order-fixed" => {
            only("discount-order-fixed");
            assert_eq!(discount(result, "10.00 off orders over 100"), 1000);
        }
        "fn-discount-buy-two-get-one" => {
            only("discount-buy-two-get-one");
            // One small pair free (14.00 each at 3+), two large pairs (16.00).
            let free: i64 = result
                .discounts
                .iter()
                .map(|discount| discount.amount)
                .sum();
            assert_eq!(free, 1400 + 3200);
        }
        "fn-discount-free-shipping" => {
            only("discount-free-shipping");
            assert_eq!(discount(result, "Free shipping over 75"), 490);
            assert_eq!(result.cost.shipping, 0);
        }
        "fn-discount-vip-customer" => {
            only("discount-vip-customer");
            assert_eq!(discount(result, "VIP: 20% off"), 1280);
        }
        "fn-discount-b2b-location" => {
            only("discount-b2b-location");
            assert_eq!(
                discount(result, "Northwind Seattle: 12% contract price"),
                2784
            );
        }
        "fn-discount-by-market" => {
            only("discount-by-market");
            assert_eq!(discount(result, "10% off in market europe"), 680);
        }
        "fn-discount-codes" => {
            assert_eq!(discount(result, "SAVE10: 10% off"), 880);
            assert_eq!(discount(result, "WELCOME5: 5.00 off"), 500);
            let applicable: Vec<(&str, bool)> = result
                .discount_codes
                .iter()
                .map(|code| (code.code.as_str(), code.applicable))
                .collect();
            assert_eq!(
                applicable,
                [
                    ("SAVE10", true),
                    ("WELCOME5", true),
                    ("BIG20", false),
                    ("NOPE", false)
                ]
            );
        }
        "fn-discount-from-metafields" => {
            only("discount-from-metafields");
            // 10% of the overshirt and the beanie, from the shop metafield.
            assert_eq!(discount(result, "Weekend sale: 10% off"), 680 + 340);
        }
        "fn-discount-lines-and-shipping" => {
            assert_eq!(result.runs.len(), 2);
            assert_eq!(discount(result, "10 items: 5% off"), 840);
            assert_eq!(discount(result, "10 items: 50% off shipping"), 245);
        }
        "fn-discount-legacy-product" => {
            the_run(
                result,
                "discount-legacy-product",
                "purchase.product-discount.run",
            );
            assert_eq!(discount(result, "10% off (legacy)"), 660);
        }
        "fn-discount-legacy-order" => {
            the_run(
                result,
                "discount-legacy-order",
                "purchase.order-discount.run",
            );
            assert_eq!(discount(result, "5.00 off (legacy)"), 500);
        }
        "fn-discount-legacy-shipping" => {
            the_run(
                result,
                "discount-legacy-shipping",
                "purchase.shipping-discount.run",
            );
            assert_eq!(
                result.delivery_groups[0].selected.as_deref(),
                Some("express")
            );
            assert_eq!(discount(result, "Express: 2.00 off (legacy)"), 200);
        }
        "fn-discount-not-applied" => {
            assert_eq!(discount(result, "3.00 off shipping"), 300);
            let skipped: Vec<&str> = result
                .skipped_discounts
                .iter()
                .map(|d| d.title.as_str())
                .collect();
            assert_eq!(skipped, ["1.00 off shipping"]);
        }
        "fn-discount-network" => {
            let fetch = the_run(
                result,
                "discount-network",
                "cart.lines.discounts.generate.fetch",
            );
            let exchange = fetch.exchange.as_ref().expect("the request was sent");
            assert!(exchange.request.url.ends_with("/discount"));
            // Nothing answers (see `data`); the answers are in
            // `the_network_functions_read_what_the_server_answers`.
            assert_eq!(exchange.response.status, 502);
            assert!(result.discounts.is_empty());
            let run = the_run(
                result,
                "discount-network",
                "cart.lines.discounts.generate.run",
            );
            assert!(run.logs.contains("the server answered 502"), "{}", run.logs);
        }
        // Delivery customizations.
        "fn-delivery-hide-express" => {
            only("delivery-hide-express");
            assert!(
                !options(result)
                    .iter()
                    .any(|(handle, _)| *handle == "express"),
                "{:?}",
                options(result)
            );
        }
        "fn-delivery-rename-by-language" => {
            assert_eq!(
                options(result)[0],
                ("standard", "Standard (3 à 5 jours ouvrés)")
            );
            assert_eq!(options(result)[3], ("pickup", "Retrait en boutique"));
        }
        "fn-delivery-reorder" => {
            assert_eq!(options(result)[0].0, "pickup");
        }
        "fn-delivery-legacy" => {
            let run = the_run(
                result,
                "delivery-legacy",
                "purchase.delivery-customization.run",
            );
            assert_eq!(
                run.output.as_ref().unwrap()["operations"],
                serde_json::json!([])
            );
            assert_eq!(options(result).len(), 4);
        }
        // Payment customizations.
        "fn-payment-hide-cod" => {
            assert!(
                !payment_methods(result).contains(&"Cash on Delivery"),
                "{:?}",
                payment_methods(result)
            );
        }
        "fn-payment-hide-for-b2b" => {
            assert_eq!(
                payment_methods(result),
                ["Credit card", "Bank Deposit", "Invoice (Net 30)"]
            );
        }
        "fn-payment-rename" => {
            assert!(payment_methods(result).contains(&"Bank transfer (2 to 3 business days)"));
            assert!(payment_methods(result).contains(&"Cash on delivery (pay the courier)"));
        }
        "fn-payment-reorder" => {
            assert_eq!(payment_methods(result)[..2], ["PayPal", "Credit card"]);
        }
        "fn-payment-legacy" => {
            let run = the_run(
                result,
                "payment-legacy",
                "purchase.payment-customization.run",
            );
            let output = run.output.as_ref().unwrap();
            assert_eq!(
                output["operations"][0]["paymentTermsSet"]["paymentTerms"]["net"]["dueInDays"],
                30
            );
            assert_eq!(payment_methods(result).len(), 5);
        }
        // Validations.
        "fn-validation-minimum-order" => {
            assert_eq!(
                errors(result),
                ["The minimum order is 20.00 USD: add 4.00 more"]
            );
        }
        "fn-validation-javascript" => {
            only("validation-javascript");
            assert_eq!(
                errors(result),
                ["At most 30 items per order (40 in the cart)"]
            );
            assert_eq!(result.errors[0].target.as_deref(), Some("$.cart"));
            // Plain JavaScript, built by lsf: the module runs on Shopify's Javy plugin.
            let module = std::fs::read(
                playground().join("extensions/validation-javascript/dist/function.wasm"),
            )
            .unwrap();
            let plugin = b"shopify_functions_javy_v";
            assert!(module.windows(plugin.len()).any(|bytes| bytes == plugin));
            let run = the_run(
                result,
                "validation-javascript",
                "cart.validations.generate.run",
            );
            assert!(
                run.logs
                    .contains("40 items: Ceramic Mug, Organic Cotton T-Shirt"),
                "{}",
                run.logs
            );
        }
        "fn-validation-max-quantity" => {
            assert_eq!(
                errors(result),
                ["Limited Edition Print: 2 per order at most (3 in the cart)"]
            );
            assert_eq!(
                result.errors[0].target.as_deref(),
                Some("$.cart.lines[0].quantity")
            );
        }
        "fn-validation-blocked-country" => {
            assert_eq!(errors(result), ["We do not ship to DE yet"]);
        }
        "fn-validation-gift-note" => {
            assert_eq!(
                errors(result),
                ["Gift Box: write the gift note that goes in the box"]
            );
        }
        "fn-validation-street-number" => {
            assert_eq!(errors(result), ["\"Harbor Way\" has no street number"]);
            // While the buyer types, nothing.
            let typing = run_at(name, Step::Interaction);
            assert!(typing.errors.is_empty(), "{:?}", errors(&typing));
        }
        "fn-validation-legacy" => {
            the_run(result, "validation-legacy", "purchase.validation.run");
            assert_eq!(
                errors(result),
                ["Addresses at blocked.example cannot order"]
            );
        }
        "fn-validation-network" => {
            let fetch = the_run(
                result,
                "validation-network",
                "cart.validations.generate.fetch",
            );
            let exchange = fetch.exchange.as_ref().expect("the request was sent");
            assert!(
                exchange
                    .request
                    .url
                    .ends_with("/stock?skus=MUG-SLT,TOTE-NAT"),
                "{}",
                exchange.request.url
            );
            assert_eq!(exchange.response.status, 502);
            assert!(result.errors.is_empty(), "{:?}", errors(result));
        }
        // Cart transforms.
        "fn-cart-transform-expand" => {
            assert_eq!(lines(result), [("Weekend Bundle (bundle)", 1, 5000)]);
            assert_eq!(result.lines[0].components.len(), 2);
        }
        "fn-cart-transform-merge" => {
            // One set at 10% off (32.00 + 34.00), the second tee alone.
            assert_eq!(
                lines(result),
                [
                    ("Cold-Weather Set", 1, 5940),
                    ("Organic Cotton T-Shirt", 1, 3200)
                ]
            );
        }
        "fn-cart-transform-update" => {
            assert_eq!(
                lines(result),
                [
                    ("Organic Cotton T-Shirt (member price)", 1, 2800),
                    ("Linen Overshirt (member price)", 1, 5800),
                    ("Canvas Tote Bag", 1, 2400)
                ]
            );
        }
        "fn-cart-transform-properties" => {
            assert_eq!(
                lines(result),
                [
                    ("Ceramic Mug, engraved \"Ada\"", 1, 2800),
                    ("Canvas Tote Bag, gift wrapped", 1, 2700),
                    ("Cold-Weather Set", 1, 6600),
                    ("Weekend Bundle", 1, 5000)
                ]
            );
        }
        "fn-cart-transform-legacy" => {
            the_run(
                result,
                "cart-transform-legacy",
                "purchase.cart-transform.run",
            );
            assert_eq!(lines(result)[0], ("Gift Box (free)", 1, 0));
        }
        // Edge cases.
        "fn-broken-panic" => {
            let run = the_run(result, "broken-panic", "cart.validations.generate.run");
            assert!(
                run.error
                    .as_deref()
                    .unwrap()
                    .contains("panicked: index out of bounds"),
                "{:?}",
                run.error
            );
            assert!(
                run.logs.contains("about to read line 2 of 1"),
                "{}",
                run.logs
            );
            assert_eq!(result.errors.len(), 1);
        }
        "fn-broken-instruction-limit" => {
            let run = the_run(
                result,
                "broken-instruction-limit",
                "cart.payment-methods.transform.run",
            );
            assert!(run.instructions > 11_000_000, "{}", run.instructions);
            assert!(
                run.error
                    .as_deref()
                    .unwrap()
                    .contains("Shopify stops a Function past 11000000")
            );
        }
        "fn-broken-bad-output" => {
            let run = the_run(
                result,
                "broken-bad-output",
                "cart.delivery-options.transform.run",
            );
            let error = run.error.as_deref().unwrap();
            assert!(
                error.contains("`operations[0].deliveryOptionHide` has no field `handle`"),
                "{error}"
            );
            assert_eq!(options(result).len(), 4, "the output is not applied");
        }
        "fn-broken-logs" => {
            let run = the_run(result, "broken-logs", "cart.validations.generate.run");
            assert!(run.error.is_none());
            assert!(run.logs.contains("line 0: 2 x TOTE-NAT"), "{}", run.logs);
            assert!(run.logs.contains("all good: no error"));
            assert!(result.errors.is_empty());
        }
        "fn-broken-huge-output" => {
            let run = the_run(
                result,
                "broken-huge-output",
                "cart.delivery-options.transform.run",
            );
            assert!(run.error.is_none(), "{:?}", run.error);
            assert_eq!(
                run.output.as_ref().unwrap()["operations"]
                    .as_array()
                    .unwrap()
                    .len(),
                2000
            );
            assert_eq!(options(result)[0].1, "Standard, renamed 2000 times");
        }
        "fn-broken-missing-build" => {
            let run = the_run(
                result,
                "broken-missing-build",
                "cart.validations.generate.run",
            );
            assert!(
                run.error.as_deref().unwrap().contains("no Wasm module"),
                "{:?}",
                run.error
            );
        }
        "fn-broken-unknown-target" => {
            // The known target runs; the unknown one is reported when the app loads.
            assert_eq!(result.runs.len(), 1);
            assert_eq!(result.runs[0].target, "cart.validations.generate.run");
            assert!(result.runs[0].error.is_none());
        }
        "fn-broken-api-version" => {
            assert!(result.runs.is_empty(), "{:?}", ran(result));
        }
        "fn-broken-big-input" => {
            let run = the_run(result, "broken-big-input", "cart.validations.generate.run");
            assert!(run.error.is_none(), "{:?}", run.error);
            let input = &run.input;
            assert_eq!(input["cart"]["buyerIdentity"]["customer"]["vip"], true);
            assert_eq!(input["cart"]["lines"][0]["engraving"]["value"], "Ada");
            assert_eq!(input["shop"]["sale"]["jsonValue"]["label"], "Weekend sale");
            assert!(input.to_string().len() > 3000);
        }
        "fn-broken-fetch-timeout" => {
            assert_eq!(result.runs.len(), 1, "the .run target does not run");
            assert!(result.runs[0].exchange.is_none(), "no request is sent");
            assert_eq!(
                errors(result),
                [
                    "Function `broken-fetch-timeout`: fetch request: policy.readTimeoutMs must be between 100 and 2000 milliseconds, got 5000"
                ]
            );
        }
        "fn-listed" => {
            assert!(result.runs.is_empty(), "{:?}", ran(result));
        }
        "fn-everything" => {
            assert!(ran(result).len() >= 34, "{:?}", ran(result));
            assert!(
                ran(result)
                    .iter()
                    .all(|handle| !handle.starts_with("broken-"))
            );
            assert!(result.errors.is_empty(), "{:?}", errors(result));
            assert_eq!(discount(result, "VIP: 20% off"), 4110);
            assert!(
                lines(result)
                    .iter()
                    .any(|(title, _, _)| *title == "Cold-Weather Set")
            );
            assert!(
                lines(result)
                    .iter()
                    .any(|(title, _, total)| *title == "Gift Box (free)" && *total == 0)
            );
        }
        other => panic!("{other}: add what the scenario shows to `check`"),
    }
}
