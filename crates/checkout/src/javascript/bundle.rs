//! One JavaScript file for an extension and all it imports, as esbuild writes it for Shopify's
//! CLI: a Function's `dist/function.js`, the ES module Javy compiles, or a checkout UI extension's
//! `dist/<handle>.js`, a minified script.
//!
//! The entry is the one Shopify's CLI generates and hands esbuild as `stdin` (TypeScript and
//! JSX): for a Function, an export per target that hands the user's function to
//! `@shopify/shopify_function/run`; for a checkout UI extension, a `shopify.extend` per target.
//!
//! Each module is read and its TypeScript and JSX removed (oxc's transformer, which also turns
//! `enum`s into objects; JSX calls the automatic runtime of the `jsxImportSource` its
//! `tsconfig.json` names, `react` by default). Then, as esbuild does:
//!
//! - **Tree shaking**: only what the Function's exports use, and what has side effects, is kept.
//!   `generated/api.ts` declares every enum of the Function API (some 250 country codes, …), and
//!   running them all would cost the Function millions of instructions.
//! - **Enum inlining**: `DiscountClass.Product`, of an enum whose members are constants, becomes
//!   `"PRODUCT"`, so that the enum is not needed at all.
//! - `process.env.NAME` is replaced by the value given for `NAME` (Shopify's CLI gives the app's
//!   `.env` and the environment), and a named import that its module does not export is an error.
//!
//! - **Scope hoisting**: the ES modules kept share one scope, in the order they run, their
//!   top-level names renamed where they would clash, and an imported name is the name it was
//!   declared with. CommonJS modules (`require`, `module.exports`) and JSON files are functions
//!   that run the first time something needs them. `import()`, and `require` of an ES module, are
//!   not bundled.
//!
//! A module's path in the comments of the bundle, and in messages, is relative to the
//! extension's folder; a package of lsf's own copy is named by its path in `node_modules`.
//!
//! The parts, in the order a build goes through them: each module is read (`graph`, with
//! `source` for its TypeScript and JSX, `read` and `purity` for what each statement does),
//! what the extension needs is found (`live`), and the bundle is written (`write`).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::Path;

use oxc::allocator::Allocator;
use oxc::codegen::Codegen;
use oxc::minifier::{CompressOptions, MangleOptions, Minifier, MinifierOptions};
use oxc::parser::Parser;
use oxc::span::SourceType;

use super::resolve::Resolver;

mod graph;
mod live;
mod purity;
mod read;
mod source;
mod text;
mod write;

use graph::*;
use live::*;
use purity::*;
use read::*;
use source::*;
use text::*;

/// What to bundle.
pub struct Options<'a> {
    /// The extension's folder: the entry is resolved from it, and paths in messages are
    /// relative to it.
    pub root: &'a Path,
    /// The entry Shopify's CLI generates, read as esbuild reads its `stdin`: TypeScript with
    /// JSX.
    pub entry: &'a str,
    pub resolver: &'a Resolver,
    /// The values of `process.env.NAME`, by name.
    pub env: &'a BTreeMap<String, String>,
    /// Whether a `.js` file may hold JSX, as Shopify's CLI has esbuild read those of a checkout
    /// UI extension.
    pub jsx_in_js: bool,
    pub format: Format,
    /// Whether the bundle is minified (whitespace, syntax and names), as a checkout UI
    /// extension's is.
    pub minify: bool,
}

/// What the bundle is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// An ES module that exports what the entry exports: what Javy compiles.
    Module,
    /// A script that runs everything in a function of its own, as esbuild's `iife`.
    Script,
}

/// The JavaScript of the extension.
pub fn bundle(options: &Options) -> Result<String, String> {
    let mut graph = Graph {
        root: options.root,
        env: options.env,
        resolver: options.resolver,
        jsx_in_js: options.jsx_in_js,
        entry: options.entry,
        modules: Vec::new(),
        ids: HashMap::new(),
        queue: VecDeque::new(),
        exported: Vec::new(),
        jsx_sources: HashMap::new(),
    };
    let entry = graph.id(Source::Entry);
    while let Some(id) = graph.queue.pop_front() {
        graph.analyze(id)?;
    }
    graph.exported = (0..graph.modules.len())
        .map(|id| graph.compute_exports_of(id))
        .collect();
    graph.check_imports()?;
    graph.check_requires()?;

    let entry_exports: Vec<String> = match options.format {
        Format::Module => graph.modules[entry].exports.keys().cloned().collect(),
        Format::Script => Vec::new(),
    };
    let roots: Vec<(usize, String)> = entry_exports
        .iter()
        .map(|name| (entry, name.clone()))
        .collect();
    let live = graph.live(&roots, &[entry]);
    let layout = graph.layout(&live, &[entry]);

    let mut code = String::new();
    if !layout.commonjs.is_empty() || !layout.namespaces.is_empty() {
        code.push_str(RUNTIME);
    }
    for &id in &layout.commonjs {
        code.push_str(&graph.emit_commonjs(id, &layout));
    }
    for &id in &layout.order {
        code.push_str(&graph.emit(id, &live, &layout));
    }
    if !entry_exports.is_empty() {
        let exported: Vec<String> = entry_exports
            .iter()
            .map(|name| {
                let (local, _) = layout.expression(graph.target(entry, Some(name)));
                if &local == name {
                    local
                } else {
                    format!("{local} as {name}")
                }
            })
            .collect();
        code.push_str(&format!("export {{ {} }};\n", exported.join(", ")));
    }
    if options.format == Format::Script {
        code = format!("(() => {{\n{code}}})();\n");
    }
    check_syntax(&code, options.format)?;
    Ok(if options.minify { minify(&code) } else { code })
}

/// The bundle minified as esbuild's `minify`: whitespace, syntax, and the names of what the
/// script's function declares.
fn minify(code: &str) -> String {
    let allocator = Allocator::default();
    let mut program = Parser::new(&allocator, code, SourceType::cjs())
        .parse()
        .program;
    let minified = Minifier::new(MinifierOptions {
        mangle: Some(MangleOptions::default()),
        mangle_properties: None,
        compress: Some(CompressOptions::smallest()),
    })
    .minify(&allocator, &mut program);
    Codegen::new()
        .with_options(oxc::codegen::CodegenOptions::minify())
        .with_scoping(minified.scoping)
        .build(&program)
        .code
}

/// What CommonJS modules and namespace objects need: getters on an object, a CommonJS module run
/// once, and its view as an ES module (its `default` is `module.exports`, unless it says it was an
/// ES module), as esbuild's `__commonJS` and `__toESM`.
const RUNTIME: &str = r#"var __lsf_export = (target, getters) => {
  for (var name in getters) {
    Object.defineProperty(target, name, { get: getters[name], enumerable: true });
  }
};
var __lsf_reexport = (target, source) => {
  for (let name in source) {
    if (name !== "default" && !Object.prototype.hasOwnProperty.call(target, name)) {
      Object.defineProperty(target, name, { get: () => source[name], enumerable: true });
    }
  }
};
var __lsf_commonjs = (body) => {
  var module;
  return () => {
    if (!module) {
      module = { exports: {} };
      body.call(module.exports, module.exports, module);
    }
    return module.exports;
  };
};
var __lsf_toESM = (exports) => {
  var namespace = {};
  if (exports !== null && (typeof exports === "object" || typeof exports === "function")) {
    __lsf_reexport(namespace, exports);
  }
  var value = exports && exports.__esModule ? exports.default : exports;
  Object.defineProperty(namespace, "default", { value: value, enumerable: true });
  return namespace;
};
"#;

/// The whole bundle parses: what lsf writes cannot be a syntax error Javy or a browser would stop
/// on.
fn check_syntax(code: &str, format: Format) -> Result<(), String> {
    let allocator = Allocator::default();
    let source_type = match format {
        Format::Module => SourceType::mjs(),
        Format::Script => SourceType::cjs(),
    };
    let parsed = Parser::new(&allocator, code, source_type).parse();
    match parsed.diagnostics.iter().next() {
        Some(error) => Err(format!(
            "lsf wrote JavaScript that does not parse ({}): please report it",
            describe(error, "dist/function.js", code)
        )),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests;
