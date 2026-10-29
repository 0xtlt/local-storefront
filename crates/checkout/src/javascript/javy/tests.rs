use super::*;

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/javy")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn text(name: &str) -> String {
    String::from_utf8(fixture(name)).unwrap()
}

#[test]
fn the_module_is_the_one_javy_writes() {
    let module = module(&text("discount.js"), &["cart-lines-discounts-generate-run"]).unwrap();
    assert_eq!(module, fixture("discount.wasm"));
}

#[test]
fn the_exports_keep_the_order_of_the_extension() {
    let module = module(
        &text("three-exports.js"),
        &["beta-fetch", "alpha-run", "gamma-run"],
    )
    .unwrap();
    assert_eq!(module, fixture("three-exports.wasm"));
}

#[test]
fn without_an_export_the_module_only_has_start() {
    let module = module(&text("three-exports.js"), &[]).unwrap();
    assert_eq!(module, fixture("no-export.wasm"));
}

#[test]
fn javascript_the_plugin_cannot_compile_is_an_error() {
    // The plugin traps on a syntax error, as with Javy's CLI: the bundler reports those first.
    let error = module("export function run() {", &["run"]).unwrap_err();
    assert_eq!(
        error,
        "the Javy plugin could not compile the JavaScript: wasm trap: out of bounds memory access"
    );
}

#[test]
fn a_module_that_throws_is_compiled_and_throws_when_run() {
    // QuickJS reports what goes wrong at run time, through the Function's logs.
    assert!(module("throw new Error('no');", &[]).is_ok());
}

#[test]
fn an_export_calls_the_function_in_camel_case() {
    assert_eq!(
        camel_case("cart-lines-discounts-generate-run"),
        "cartLinesDiscountsGenerateRun"
    );
    assert_eq!(camel_case("run"), "run");
    assert_eq!(
        camel_case("purchase-validation-fetch"),
        "purchaseValidationFetch"
    );
}
