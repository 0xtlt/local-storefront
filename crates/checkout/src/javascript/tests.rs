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
                .contains("build it with `lsf function build`")),
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
fn dotenv_files_are_read_as_the_cli_reads_them() {
    let env = dotenv(
        "# a comment\nPLAIN=value # not this\nexport EXPORTED=1\nDOUBLE=\"two\\nlines\"\nSINGLE='as is \\n'\n\nEMPTY=\n",
    );
    assert_eq!(env["PLAIN"], "value");
    assert_eq!(env["EXPORTED"], "1");
    assert_eq!(env["DOUBLE"], "two\nlines");
    assert_eq!(env["SINGLE"], "as is \\n");
    assert_eq!(env["EMPTY"], "");
}
