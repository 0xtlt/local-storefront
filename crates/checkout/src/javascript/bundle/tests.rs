use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

use super::*;
use crate::javascript::javy;

/// A Function's folder in the temporary folder, with `files` (path, content), removed with the
/// value.
struct Folder(PathBuf);

impl Folder {
    fn new(files: &[(&str, &str)]) -> Folder {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "lsf-bundle-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        for (path, content) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        Folder(dir)
    }

    fn bundle(&self, exports: &[&str]) -> Result<String, String> {
        self.bundle_with(exports, &BTreeMap::new())
    }

    fn bundle_with(
        &self,
        exports: &[&str],
        env: &BTreeMap<String, String>,
    ) -> Result<String, String> {
        let entry = ["src/index.ts", "src/index.js"]
            .into_iter()
            .map(|entry| self.0.join(entry))
            .find(|entry| entry.is_file())
            .expect("an entry");
        let exports: Vec<String> = exports.iter().map(|export| export.to_string()).collect();
        bundle(&Options {
            root: &self.0,
            entry: &entry,
            exports: &exports,
            env,
        })
    }

    /// Bundles, builds the module, runs `export` with `input`: the output and the logs.
    fn run(&self, export: &str, input: Value) -> (Value, String) {
        let code = self.bundle(&[export]).unwrap();
        let kebab: String = export
            .chars()
            .flat_map(|c| {
                if c.is_uppercase() {
                    vec!['-', c.to_ascii_lowercase()]
                } else {
                    vec![c]
                }
            })
            .collect();
        let module = javy::module(&code, &[&kebab]).unwrap();
        let wasm = self.0.join("dist/function.wasm");
        std::fs::create_dir_all(wasm.parent().unwrap()).unwrap();
        std::fs::write(&wasm, module).unwrap();
        let execution = crate::runner::run(&wasm, &kebab, &input);
        assert!(execution.error.is_none(), "{:?}\n{code}", execution.error);
        (execution.output.unwrap(), execution.logs)
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Generated types as graphql-codegen writes them: an enum, used as a value, and types.
const API: &str = r#"export type Maybe<T> = T | null;
export enum DiscountClass {
  Order = 'ORDER',
  Product = 'PRODUCT',
  Shipping = 'SHIPPING'
}
export type CartInput = { cart: { lines: Array<{ id: string, quantity: number }> }, discount: { discountClasses: Array<DiscountClass> } };
export type CartLinesDiscountsGenerateRunResult = { operations: Array<unknown> };
"#;

#[test]
fn a_typescript_function_as_shopify_generates_it_runs() {
    let folder = Folder::new(&[
        ("generated/api.ts", API),
        (
            "src/index.ts",
            "export * from \"./cart_lines_discounts_generate_run\";\n",
        ),
        (
            "src/cart_lines_discounts_generate_run.ts",
            r#"import {
  DiscountClass,
  type CartInput,
  type CartLinesDiscountsGenerateRunResult,
} from "../generated/api";
import type { Maybe } from "../generated/api";

interface Line { id: string; quantity: number }

export function cartLinesDiscountsGenerateRun(input: CartInput): CartLinesDiscountsGenerateRunResult {
  const lines: Line[] = input.cart.lines.filter((line) => line.quantity >= 2);
  const none: Maybe<string> = null;
  console.log(`${lines.length} lines`, none);
  if (!input.discount.discountClasses.includes(DiscountClass.Product)) {
    return { operations: [] };
  }
  return { operations: lines.map((line) => ({ line: line.id })) };
}
"#,
        ),
    ]);
    let input = json!({
        "cart": { "lines": [{ "id": "a", "quantity": 2 }, { "id": "b", "quantity": 1 }] },
        "discount": { "discountClasses": ["PRODUCT"] }
    });
    let (output, logs) = folder.run("cartLinesDiscountsGenerateRun", input);
    assert_eq!(output, json!({ "operations": [{ "line": "a" }] }));
    assert_eq!(logs.trim(), "1 lines null");
}

#[test]
fn imports_are_live_bindings_read_where_they_are_used() {
    let folder = Folder::new(&[
        (
            "src/counter.js",
            r#"export let count = 0;
export function increment() { count += 1; return this; }
export default function label() { return `count ${count}`; }
"#,
        ),
        (
            "src/index.js",
            r#"import label, { count, increment } from "./counter.js";
import * as counter from "./counter";
export function run() {
  const self = increment();
  const before = { count };
  counter.increment();
  return { before, after: count, label: label(), self: self === undefined, keys: Object.keys(counter).sort() };
}
"#,
        ),
    ]);
    let (output, _) = folder.run("run", json!({}));
    assert_eq!(
        output,
        json!({
            "before": { "count": 1 },
            "after": 2,
            "label": "count 2",
            "self": true,
            "keys": ["count", "default", "increment"]
        })
    );
}

#[test]
fn a_function_declared_above_an_import_can_use_it() {
    let folder = Folder::new(&[
        ("src/value.ts", "export const value = 41;\n"),
        (
            "src/index.ts",
            "export function run() { return { value: value + 1 }; }\nimport { value } from './value';\n",
        ),
    ]);
    assert_eq!(folder.run("run", json!({})).0, json!({ "value": 42 }));
}

#[test]
fn reexports_defaults_and_classes() {
    let folder = Folder::new(&[
        (
            "src/shapes.ts",
            r#"export default class { area() { return 2; } }
export class Square { constructor(public side: number) {} area() { return this.side ** 2; } }
export const enum Unit { Meter = 'm' }
export enum Kind { Flat, Round }
"#,
        ),
        (
            "src/more.ts",
            "export { default as Shape, Square as Box } from './shapes';\nexport * as shapes from './shapes';\n",
        ),
        (
            "src/index.ts",
            r#"import { Shape, Box, shapes } from './more';
import { Kind } from './shapes';
export function run() {
  return { shape: new Shape().area(), box: new Box(3).area(), round: shapes.Kind.Round, flat: Kind.Flat, name: Kind[1] };
}
"#,
        ),
    ]);
    assert_eq!(
        folder.run("run", json!({})).0,
        json!({ "shape": 2, "box": 9, "round": 1, "flat": 0, "name": "Round" })
    );
}

#[test]
fn commonjs_modules_and_json_are_bundled() {
    let folder = Folder::new(&[
        (
            "node_modules/legacy/package.json",
            r#"{ "name": "legacy", "main": "lib/main.js" }"#,
        ),
        (
            "node_modules/legacy/lib/main.js",
            "var helper = require('./helper');\nmodule.exports = function legacy(x) { return helper.twice(x); };\nmodule.exports.version = '1.0';\n",
        ),
        (
            "node_modules/legacy/lib/helper.js",
            "exports.twice = function (x) { return x * 2; };\n",
        ),
        ("src/config.json", r#"{ "factor": 3, "name": "config" }"#),
        (
            "src/index.ts",
            r#"import legacy, { version } from 'legacy';
import config from './config.json';
const helper = require('legacy/lib/helper');
export function run() {
  return { legacy: legacy(2), version, config: config.factor, helper: helper.twice(5) };
}
"#,
        ),
    ]);
    assert_eq!(
        folder.run("run", json!({})).0,
        json!({ "legacy": 4, "version": "1.0", "config": 3, "helper": 10 })
    );
}

#[test]
fn packages_resolve_through_their_exports() {
    let folder = Folder::new(&[
        (
            "node_modules/@acme/money/package.json",
            r#"{
  "name": "@acme/money",
  "exports": {
    ".": { "require": "./cjs/index.js", "import": "./esm/index.js" },
    "./format/*": "./esm/format/*.js"
  }
}"#,
        ),
        (
            "node_modules/@acme/money/esm/index.js",
            "export const kind = 'esm';\n",
        ),
        (
            "node_modules/@acme/money/cjs/index.js",
            "exports.kind = 'cjs';\n",
        ),
        (
            "node_modules/@acme/money/esm/format/cents.js",
            "export default (cents) => (cents / 100).toFixed(2);\n",
        ),
        (
            "src/index.js",
            r#"import { kind } from '@acme/money';
import cents from '@acme/money/format/cents';
export function run() { return { kind, required: require('@acme/money').kind, price: cents(1234) }; }
"#,
        ),
    ]);
    assert_eq!(
        folder.run("run", json!({})).0,
        json!({ "kind": "esm", "required": "cjs", "price": "12.34" })
    );
}

#[test]
fn process_env_is_replaced_by_the_values_given() {
    let folder = Folder::new(&[(
        "src/index.js",
        "export function run() { return { url: process.env.API_URL, mode: process.env.NODE_ENV }; }\n",
    )]);
    let env = BTreeMap::from([
        ("API_URL".to_string(), "https://example.com".to_string()),
        ("NODE_ENV".to_string(), "development".to_string()),
    ]);
    let code = folder.bundle_with(&["run"], &env).unwrap();
    assert!(code.contains(r#"url: "https://example.com""#), "{code}");
    assert!(code.contains(r#"mode: "development""#), "{code}");
    assert!(!code.contains("process.env"), "{code}");
}

#[test]
fn an_export_the_entry_does_not_have_is_an_error() {
    let folder = Folder::new(&[(
        "src/index.ts",
        "export function cartLinesDiscountsGenerateRun() {}\n",
    )]);
    assert_eq!(
        folder
            .bundle(&[
                "cartLinesDiscountsGenerateRun",
                "cartDeliveryOptionsDiscountsGenerateRun"
            ])
            .unwrap_err(),
        "src/index.ts does not export a function `cartDeliveryOptionsDiscountsGenerateRun`"
    );
}

#[test]
fn an_import_of_a_name_that_is_not_exported_is_an_error() {
    let folder = Folder::new(&[
        ("src/api.ts", "export enum Kind { A }\n"),
        (
            "src/index.ts",
            "import { Knd } from './api';\nexport function run() { return Knd; }\n",
        ),
    ]);
    assert_eq!(
        folder.bundle(&["run"]).unwrap_err(),
        "src/index.ts:1: src/api.ts does not export `Knd`"
    );
}

#[test]
fn a_syntax_error_says_where() {
    let folder = Folder::new(&[("src/index.ts", "export function run() {\n  return {;\n}\n")]);
    let error = folder.bundle(&["run"]).unwrap_err();
    assert!(error.starts_with("src/index.ts:2:"), "{error}");
}

#[test]
fn what_cannot_be_bundled_is_an_error() {
    let cases = [
        (
            "import fs from 'fs';\nexport function run() {}\n",
            "src/index.js: cannot import `fs`: it is a module of Node, and a Function does not run on Node",
        ),
        (
            "import x from './missing';\nexport function run() {}\n",
            "src/index.js: cannot find `./missing`",
        ),
        (
            "import x from 'left-pad';\nexport function run() {}\n",
            "src/index.js: cannot find the package of `left-pad` in a node_modules folder: install it",
        ),
        (
            "export async function run() { return import('./other.js'); }\n",
            "src/index.js:1: `import()` cannot be bundled into a Function: import the module",
        ),
    ];
    for (source, expected) in cases {
        let folder = Folder::new(&[("src/index.js", source)]);
        assert_eq!(folder.bundle(&["run"]).unwrap_err(), expected);
    }
}

#[test]
fn the_shopify_function_library_comes_from_node_modules_when_it_is_there() {
    let folder = Folder::new(&[
        (
            "node_modules/@shopify/shopify_function/run.ts",
            "export default function (f: any) { ShopifyFunction.writeOutput({ wrapped: f(ShopifyFunction.readInput()) }); }\n",
        ),
        (
            "node_modules/@shopify/shopify_function/package.json",
            r#"{ "name": "@shopify/shopify_function", "version": "2.0.1", "main": "index.ts" }"#,
        ),
        (
            "src/index.js",
            "export function run(input) { return input.x; }\n",
        ),
    ]);
    assert_eq!(
        folder.run("run", json!({ "x": 1 })).0,
        json!({ "wrapped": 1 })
    );
}

#[test]
fn top_level_names_that_clash_are_renamed() {
    let folder = Folder::new(&[
        (
            "src/a.ts",
            "const value = 1;\nexport function getA() { return value; }\nexport { value as a };\n",
        ),
        (
            "src/b.ts",
            "const value = 2;\nexport function getB() { return value; }\n",
        ),
        (
            "src/index.ts",
            r#"import { a, getA } from './a';
import { getB } from './b';
export function run() {
  // A local of the same name as a's top-level `value`: `a` must still read a's.
  const value = 10;
  const { getA: local } = { getA };
  return { a, getA: local(), getB: getB(), value };
}
"#,
        ),
    ]);
    assert_eq!(
        folder.run("run", json!({})).0,
        json!({ "a": 1, "getA": 1, "getB": 2, "value": 10 })
    );
}

#[test]
fn what_has_side_effects_runs_and_what_is_not_used_is_left_out() {
    let folder = Folder::new(&[
        (
            "src/setup.js",
            "globalThis.ready = 'yes';\nconsole.log('setup ran');\n",
        ),
        (
            "src/api.ts",
            r#"export enum Country { CA = 'CA', FR = 'FR', US = 'US' }
export enum Unused { A = 'A' }
export function unusedHelper() { return Country.FR; }
export const used = (code: Country) => code === Country.CA;
"#,
        ),
        (
            "src/index.ts",
            r#"import './setup';
import { Country, used } from './api';
export function run() {
  return { ready: globalThis.ready, canada: used(Country.CA) };
}
"#,
        ),
    ]);
    let code = folder.bundle(&["run"]).unwrap();
    for left_out in ["Unused", "unusedHelper", "var Country"] {
        assert!(!code.contains(left_out), "{left_out} in\n{code}");
    }
    assert!(code.contains(r#""CA" /* CA */"#), "{code}");
    let (output, logs) = folder.run("run", json!({}));
    assert_eq!(output, json!({ "ready": "yes", "canada": true }));
    assert_eq!(logs.trim(), "setup ran");
}

#[test]
fn an_enum_read_as_an_object_is_kept() {
    let folder = Folder::new(&[
        (
            "src/api.ts",
            "export enum Country { CA = 'CA', FR = 'FR' }\n",
        ),
        (
            "src/index.ts",
            "import { Country } from './api';\nexport function run() { return Object.values(Country); }\n",
        ),
    ]);
    assert_eq!(folder.run("run", json!({})).0, json!(["CA", "FR"]));
}

#[test]
fn a_require_of_an_es_module_is_an_error() {
    let folder = Folder::new(&[
        ("src/other.js", "export const value = 1;\n"),
        (
            "src/index.js",
            "export function run() { return require('./other.js').value; }\n",
        ),
    ]);
    assert_eq!(
        folder.bundle(&["run"]).unwrap_err(),
        "src/index.js: `require` of src/other.js, an ES module, cannot be bundled: import it"
    );
}
