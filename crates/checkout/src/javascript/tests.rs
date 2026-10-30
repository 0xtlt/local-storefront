use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::json;

use super::*;
use crate::app::{self, Build};

/// An app in the temporary folder with `files` (path, content), removed with the value.
struct TempApp(PathBuf);

impl TempApp {
    fn new(files: &[(&str, &str)]) -> TempApp {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "lsf-javascript-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let mut files = files.to_vec();
        files.push(("shopify.app.toml", "name = \"Test\"\nclient_id = \"abc\"\n"));
        for (path, content) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        TempApp(dir)
    }

    fn function(&self) -> app::FunctionExtension {
        let app = app::load(&self.0).unwrap();
        app.functions.into_iter().next().expect("a Function")
    }

    fn build(&self) -> Result<Built, String> {
        build(&self.function(), &self.0)
    }
}

impl Drop for TempApp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const EXTENSION: &str = r#"api_version = "2026-10"

[[extensions]]
name = "Bulk"
handle = "bulk"
type = "function"

  [[extensions.targeting]]
  target = "cart.lines.discounts.generate.run"
  input_query = "src/cart_lines_discounts_generate_run.graphql"
  export = "cart-lines-discounts-generate-run"

  [extensions.build]
  command = ""
  path = "dist/function.wasm"
"#;

const PACKAGE: &str = r#"{ "name": "bulk", "type": "module", "dependencies": { "@shopify/shopify_function": "2.0.1" } }"#;

const RUN: &str = r#"export function cartLinesDiscountsGenerateRun(input: { cart: { lines: { id: string }[] } }) {
  console.log(process.env.GREETING);
  return { operations: input.cart.lines.map((line) => ({ line: line.id })) };
}
"#;

fn function_files<'a>(package: &'a str, extension: &'a str) -> Vec<(&'a str, &'a str)> {
    vec![
        ("extensions/bulk/shopify.extension.toml", extension),
        ("extensions/bulk/package.json", package),
        (
            "extensions/bulk/src/index.ts",
            "export * from './cart_lines_discounts_generate_run';\n",
        ),
        (
            "extensions/bulk/src/cart_lines_discounts_generate_run.ts",
            RUN,
        ),
        (
            "extensions/bulk/src/cart_lines_discounts_generate_run.graphql",
            "query Input { cart { lines { id } } }\n",
        ),
    ]
}

#[test]
fn a_javascript_function_is_built_by_lsf_and_runs() {
    let mut files = function_files(PACKAGE, EXTENSION);
    files.push((".env", "GREETING=\"hello from .env\"\n"));
    let app = TempApp::new(&files);
    let function = app.function();
    assert!(function.javascript.is_some());
    assert_eq!(function.build(), Some(Build::JavaScript));
    assert!(
        app::load(&app.0)
            .unwrap()
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic
                .message
                .contains("build it with `lsf app function build bulk` (or `shopify app build`)")),
    );

    let built = app.build().unwrap();
    let dir = function.dir.clone();
    assert_eq!(built.module, dir.join("dist/function.wasm"));
    assert_eq!(built.javascript, dir.join("dist/function.js"));
    assert_eq!(built.types, None);
    assert!(
        std::fs::read_to_string(&built.javascript)
            .unwrap()
            .contains("\"hello from .env\"")
    );
    let execution = crate::runner::run(
        &built.module,
        "cart-lines-discounts-generate-run",
        &json!({ "cart": { "lines": [{ "id": "a" }] } }),
    );
    assert_eq!(execution.error, None);
    assert_eq!(
        execution.output,
        Some(json!({ "operations": [{ "line": "a" }] }))
    );
    assert_eq!(execution.logs.trim(), "hello from .env");
    assert!(app.function().built());
}

#[test]
fn a_build_command_wins_over_the_javascript_build() {
    let extension = EXTENSION.replace("command = \"\"", "command = \"make\"");
    let app = TempApp::new(&function_files(PACKAGE, &extension));
    assert_eq!(app.function().build(), Some(Build::Command("make")));
}

#[test]
fn the_library_must_be_the_one_shopify_s_cli_builds_with_javy() {
    let cases = [
        (
            r#"{ "name": "bulk" }"#,
            "add `\"@shopify/shopify_function\": \"2.0.1\"` to the dependencies of package.json (lsf does not need it installed)",
        ),
        (
            r#"{ "dependencies": { "@shopify/shopify_function": "~1.0.0" } }"#,
            "lsf builds Functions written with @shopify/shopify_function 2.x, and this one asks for `~1.0.0`: update it, or build it with `shopify app build`",
        ),
    ];
    for (package, expected) in cases {
        let app = TempApp::new(&function_files(package, EXTENSION));
        assert_eq!(app.build().unwrap_err(), expected);
    }
    // `^2.0.0` and an installed 2.x are fine.
    let app = TempApp::new(&function_files(
        r#"{ "devDependencies": { "@shopify/shopify_function": "^2.0.0" } }"#,
        EXTENSION,
    ));
    assert!(app.build().is_ok());
}

#[test]
fn an_export_that_is_not_kebab_case_is_an_error() {
    let extension = EXTENSION.replace(
        "export = \"cart-lines-discounts-generate-run\"",
        "export = \"cart_lines_discounts_generate_run\"",
    );
    let app = TempApp::new(&function_files(PACKAGE, &extension));
    assert_eq!(
        app.build().unwrap_err(),
        "the export `cart_lines_discounts_generate_run` of cart.lines.discounts.generate.run is \
         not kebab-case (lowercase letters, digits and hyphens), as Shopify's CLI requires of a \
         JavaScript Function"
    );
}

#[test]
fn globs_match_files_of_the_function() {
    let app = TempApp::new(&[
        ("f/src/a.graphql", ""),
        ("f/src/b.graphql", ""),
        ("f/src/b.ts", ""),
        ("f/src/nested/c.graphql", ""),
        ("f/node_modules/x/d.graphql", ""),
    ]);
    let dir = app.0.join("f");
    assert_eq!(
        glob(&dir, "src/*.graphql"),
        ["src/a.graphql", "src/b.graphql"]
    );
    assert_eq!(
        glob(&dir, "./src/**/*.graphql"),
        ["src/a.graphql", "src/b.graphql", "src/nested/c.graphql"]
    );
    assert_eq!(glob(&dir, "src/?.ts"), ["src/b.ts"]);
    assert!(glob(&dir, "**/d.graphql").is_empty());
}

#[test]
fn documents_globs_exclude_choose_and_go_up_as_graphql_codegen_s() {
    let app = TempApp::new(&[
        ("f/src/a.graphql", ""),
        ("f/src/b.graphql", ""),
        ("f/src/legacy/old.graphql", ""),
        ("f/queries/c.graphql", ""),
        ("shared/d.graphql", ""),
    ]);
    let dir = app.0.join("f");
    let documents = |patterns: &[&str]| {
        let patterns: Vec<String> = patterns.iter().map(|p| p.to_string()).collect();
        documents_of(&dir, &patterns)
    };
    assert_eq!(
        documents(&["src/**/*.graphql", "!src/legacy/*.graphql"]).unwrap(),
        ["src/a.graphql", "src/b.graphql"]
    );
    assert_eq!(
        documents(&["{src,queries}/*.graphql", "!src/b.graphql"]).unwrap(),
        ["queries/c.graphql", "src/a.graphql"]
    );
    assert_eq!(
        documents(&["../shared/*.graphql"]).unwrap(),
        ["../shared/d.graphql"]
    );
    assert_eq!(
        documents(&["src/[ab].graphql"]).unwrap_err(),
        "the documents glob `src/[ab].graphql` uses a syntax lsf does not read (only `*`, `**`, \
         `?`, `{a,b}` and `!`)"
    );
}

#[test]
fn dotenv_files_are_read_as_the_cli_reads_them() {
    let env = dotenv(
        "# a comment\nPLAIN=value # not this\nexport EXPORTED=1\nDOUBLE=\"two\\nlines\"\nSINGLE='as is \\n'\n\nEMPTY=\n",
    );
    assert_eq!(env["PLAIN"], "value");
    assert_eq!(env["EXPORTED"], "1");
    assert_eq!(env["DOUBLE"], "two\nlines");
    assert_eq!(env["SINGLE"], "as is \\n");
    assert_eq!(env["EMPTY"], "");

    let env = dotenv("HOST=\"example.com\" # prod\nHASH=\"a#b\"\nESCAPED='it\\'s'\n");
    assert_eq!(env["HOST"], "example.com");
    assert_eq!(env["HASH"], "a#b");
    assert_eq!(env["ESCAPED"], "it\\'s");
}

/// Builds a Function of the playground with lsf, from its source, in an app of its own: the
/// module is the one committed in the playground, and does what the module Shopify's CLI built
/// from the same source (`tests/fixtures/shopify-cli/<handle>.wasm`) does: the same output, the
/// same logs, as many instructions within 1%, for each of `inputs`.
fn assert_built_like_shopify_cli(handle: &str, export: &str, inputs: &[serde_json::Value]) {
    let playground = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/checkout-playground/extensions")
        .join(handle);
    let mut files = Vec::new();
    for path in ["shopify.extension.toml", "package.json"] {
        files.push(path.to_string());
    }
    for entry in std::fs::read_dir(playground.join("src")).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().to_string();
        files.push(format!("src/{name}"));
    }
    let files: Vec<(String, String)> = files
        .iter()
        .map(|path| {
            let content = std::fs::read_to_string(playground.join(path)).unwrap();
            (format!("extensions/{handle}/{path}"), content)
        })
        .collect();
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, content)| (path.as_str(), content.as_str()))
        .collect();
    let app = TempApp::new(&files);
    let built = app.build().unwrap();
    assert_eq!(
        std::fs::read(&built.module).unwrap(),
        std::fs::read(playground.join("dist/function.wasm")).unwrap(),
        "{handle}: the committed module is not the one lsf builds: `mise run playground:functions`"
    );

    let cli = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/shopify-cli")
        .join(format!("{handle}.wasm"));
    for input in inputs {
        let ours = crate::runner::run(&built.module, export, input);
        let theirs = crate::runner::run(&cli, export, input);
        assert_eq!(theirs.error, None, "{handle}: {input}");
        assert_eq!(ours.error, None, "{handle}: {input}");
        assert_eq!(ours.output, theirs.output, "{handle}: {input}");
        assert_eq!(ours.logs, theirs.logs, "{handle}: {input}");
        let difference = ours.instructions.abs_diff(theirs.instructions) as f64;
        assert!(
            difference <= theirs.instructions as f64 * 0.01,
            "{handle}: {} instructions, and {} with Shopify's CLI, for {input}",
            ours.instructions,
            theirs.instructions
        );
    }
}

#[test]
fn the_typescript_function_built_by_lsf_runs_as_the_one_shopify_s_cli_built() {
    let line = |id: &str, quantity: u32, title: &str| {
        json!({
            "id": id,
            "quantity": quantity,
            "merchandise": { "__typename": "ProductVariant", "product": { "title": title } }
        })
    };
    let settings = json!({ "jsonValue": { "percentage": 10, "minimumQuantity": 2 } });
    assert_built_like_shopify_cli(
        "discount-typescript",
        "cart-lines-discounts-generate-run",
        &[
            json!({
                "cart": { "lines": [line("gid://shopify/CartLine/1", 2, "Mug"), line("gid://shopify/CartLine/2", 1, "Tee")] },
                "discount": { "discountClasses": ["PRODUCT"], "metafield": settings }
            }),
            json!({
                "cart": { "lines": [line("gid://shopify/CartLine/1", 5, "Mug")] },
                "discount": { "discountClasses": ["ORDER"], "metafield": settings }
            }),
            json!({
                "cart": { "lines": [line("gid://shopify/CartLine/1", 5, "Mug")] },
                "discount": { "discountClasses": ["PRODUCT"], "metafield": null }
            }),
        ],
    );
}

#[test]
fn the_javascript_function_built_by_lsf_runs_as_the_one_shopify_s_cli_built() {
    let line = |quantity: u32, title: &str| {
        json!({
            "quantity": quantity,
            "merchandise": { "__typename": "ProductVariant", "product": { "title": title } }
        })
    };
    let settings = json!({ "jsonValue": { "maximumUnits": 30 } });
    assert_built_like_shopify_cli(
        "validation-javascript",
        "cart-validations-generate-run",
        &[
            json!({
                "cart": { "lines": [line(20, "Mug"), line(20, "Tee")] },
                "validation": { "metafield": settings }
            }),
            json!({
                "cart": { "lines": [line(3, "Mug"), { "quantity": 1, "merchandise": { "__typename": "CustomProduct" } }] },
                "validation": { "metafield": settings }
            }),
            json!({
                "cart": { "lines": [line(99, "Mug")] },
                "validation": { "metafield": null }
            }),
        ],
    );
}

/// Shopify's template of a checkout UI extension in Preact (`shopify app generate extension
/// --template checkout_ui --flavor preact`), with no `node_modules`.
const PREACT_EXTENSION: &[(&str, &str)] = &[
    (
        "extensions/banner/shopify.extension.toml",
        r#"api_version = "2026-10"

[[extensions]]
name = "Banner"
handle = "banner"
type = "ui_extension"

[[extensions.targeting]]
module = "./src/Checkout.jsx"
target = "purchase.checkout.block.render"
"#,
    ),
    (
        "extensions/banner/package.json",
        r#"{ "name": "banner", "private": true, "dependencies": { "preact": "^10.10.x", "@preact/signals": "^2.3.x", "@shopify/ui-extensions": "2026.10.x" } }"#,
    ),
    (
        "extensions/banner/tsconfig.json",
        r#"{
  // As the template writes it.
  "compilerOptions": {
    "jsx": "react-jsx",
    "jsxImportSource": "preact",
    "target": "ES2020",
  },
  "include": ["./src", "./shopify.d.ts"]
}"#,
    ),
    (
        "extensions/banner/src/Checkout.jsx",
        r#"import '@shopify/ui-extensions/preact';
import {render} from "preact";

export default async () => {
  render(<Extension />, document.body)
};

function Extension() {
  return (
    <s-banner heading="Banner">
      <s-text>{shopify.i18n.translate("welcome", {target: <s-text type="emphasis">{shopify.extension.target}</s-text>})}</s-text>
    </s-banner>
  );
}
"#,
    ),
];

#[test]
fn a_preact_checkout_ui_extension_is_built_without_node_modules() {
    let app = TempApp::new(PREACT_EXTENSION);
    let shopify_app = app::load(&app.0).unwrap();
    let extension = &shopify_app.ui_extensions[0];
    assert!(extension.buildable());
    assert!(shopify_app.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("build it with `lsf app build banner` (or `shopify app build`)")
    }));
    let bundle = build_extension(extension, &app.0).unwrap();
    assert_eq!(bundle, extension.dir.join("dist/banner.js"));
    let script = std::fs::read_to_string(&bundle).unwrap();
    assert!(
        script.contains("purchase.checkout.block.render"),
        "{script}"
    );
    assert!(script.contains("s-banner"), "{script}");
    assert!(script.contains("setSignals"), "{script}");
    assert!(
        !script.contains("import "),
        "a script, not a module: {script}"
    );
}

/// The playground's checkout UI extensions built from their sources: built again by lsf, in an
/// app of their own, they are the bundles committed in the playground. What those do next to
/// the bundles Shopify's CLI builds is checked by `assets/tests/built-extensions.test.mjs`.
#[test]
fn the_playground_s_preact_extensions_are_the_bundles_lsf_builds() {
    let playground =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/checkout-playground/extensions");
    for handle in ["ui-preact", "ui-preact-typescript"] {
        let source = playground.join(handle);
        let mut files = Vec::new();
        let mut stack = vec![source.clone()];
        while let Some(folder) = stack.pop() {
            for entry in std::fs::read_dir(&folder).unwrap().flatten() {
                let path = entry.path();
                let relative = path.strip_prefix(&source).unwrap().to_path_buf();
                if relative.starts_with("dist") {
                    continue;
                }
                if path.is_dir() {
                    stack.push(path);
                } else {
                    let name = format!(
                        "extensions/{handle}/{}",
                        relative.to_string_lossy().replace('\\', "/")
                    );
                    files.push((name, std::fs::read_to_string(&path).unwrap()));
                }
            }
        }
        let files: Vec<(&str, &str)> = files
            .iter()
            .map(|(path, content)| (path.as_str(), content.as_str()))
            .collect();
        let app = TempApp::new(&files);
        let shopify_app = app::load(&app.0).unwrap();
        let bundle = build_extension(shopify_app.ui_extension(handle).unwrap(), &app.0).unwrap();
        assert_eq!(
            std::fs::read_to_string(bundle).unwrap(),
            std::fs::read_to_string(source.join("dist").join(format!("{handle}.js"))).unwrap(),
            "{handle}: the committed bundle is not the one lsf builds: `lsf app build {handle} --app examples/checkout-playground`"
        );
    }
}
