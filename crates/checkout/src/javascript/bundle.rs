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

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};

use oxc::allocator::Allocator;
use oxc::ast::ast::{
    Argument, ArrayExpressionElement, AssignmentTargetPropertyIdentifier, BindingIdentifier,
    BindingProperty, CallExpression, Class, ClassElement, Declaration,
    ExportDefaultDeclarationKind, Expression, IdentifierReference, ImportDeclarationSpecifier,
    ImportExpression, ObjectProperty, ObjectPropertyKind, Program, PropertyKey,
    SimpleAssignmentTarget, Statement, StaticMemberExpression, TSEnumDeclaration, TSEnumMemberName,
    TaggedTemplateExpression, UnaryOperator,
};
use oxc::ast_visit::{Visit, walk};
use oxc::codegen::Codegen;
use oxc::diagnostics::OxcDiagnostic;
use oxc::minifier::{CompressOptions, MangleOptions, Minifier, MinifierOptions};
use oxc::parser::Parser;
use oxc::semantic::{ScopeFlags, Scoping, SemanticBuilder, SymbolId};
use oxc::span::{GetSpan, SourceType, Span};
use oxc::transformer::{JsxRuntime, TransformOptions, Transformer};

use super::packages;
use super::resolve::{Kind, Resolver};

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

/// Where a module comes from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Source {
    /// The entry Shopify's CLI generates.
    Entry,
    File(PathBuf),
}

struct Graph<'a> {
    root: &'a Path,
    env: &'a BTreeMap<String, String>,
    resolver: &'a Resolver,
    jsx_in_js: bool,
    entry: &'a str,
    modules: Vec<Module>,
    ids: HashMap<Source, usize>,
    queue: VecDeque<usize>,
    /// The `jsxImportSource` of the `tsconfig.json` that rules each folder.
    jsx_sources: HashMap<PathBuf, String>,
    /// What each module exports ([`Graph::exports_of`]), once every module is read.
    exported: Vec<Option<BTreeSet<String>>>,
}

/// What a module is, from its first reading.
struct Module {
    source: Source,
    /// The path relative to the Function's folder, for messages.
    name: String,
    /// The JavaScript, types removed; for a JSON file, the JSON.
    javascript: String,
    form: Form,
    /// The modules its `import`s and `export … from`s name, in order, each once.
    requests: Vec<(String, usize)>,
    /// Its top-level statements, in order (an ES module's).
    statements: Vec<StatementUse>,
    /// What each imported binding reads.
    imports: HashMap<String, Import>,
    /// The statements that declare each other top-level name.
    declared_by: HashMap<String, Vec<usize>>,
    exports: BTreeMap<String, Export>,
    /// The modules of its `export * from`.
    stars: Vec<usize>,
    /// The constant members of the enums it declares, written as JavaScript, by enum.
    enums: Enums,
    /// The modules its `require` calls name, by where the call starts in `javascript`.
    requires: BTreeMap<u32, usize>,
    /// The names it imports from other modules, to check they are exported.
    named: Vec<NamedImport>,
    /// Every name it declares, in any scope: a top-level name of another module must not be
    /// one of them, or the module could not read it.
    symbols: BTreeSet<String>,
    /// The globals it reads, which no top-level name may hide.
    globals: BTreeSet<String>,
}

/// The constant members of enums, written as JavaScript, by member, by enum.
type Enums = HashMap<String, HashMap<String, String>>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Form {
    Esm,
    CommonJs,
    Json,
}

#[derive(Default)]
struct StatementUse {
    /// Whether running it can do something else than declare what it declares.
    effects: bool,
    /// The top-level names it reads.
    uses: Vec<Use>,
    /// The modules it `require`s, by where the call starts.
    requires: Vec<(u32, usize)>,
}

enum Use {
    Name(String),
    /// `name.property`, where `name` is imported: an enum member, or a name of a namespace.
    Member(String, String),
}

#[derive(Clone)]
struct Import {
    from: usize,
    /// The name imported; `None` for a namespace (`* as name`).
    name: Option<String>,
}

#[derive(Clone)]
enum Export {
    Local(String),
    From(usize, String),
    Namespace(usize),
}

struct NamedImport {
    module: usize,
    from: usize,
    name: String,
    line: usize,
    /// A re-export in TypeScript (`export { Foo } from`): a name its module does not export is a
    /// type, which esbuild leaves out.
    maybe_type: bool,
}

/// What the Function needs of each module.
struct Live {
    /// The modules that run.
    needed: Vec<bool>,
    /// The statements kept, by module.
    statements: Vec<Vec<bool>>,
    /// The exports read, by module.
    exports: Vec<BTreeSet<String>>,
    /// The modules whose every export is read (a namespace, a `require`).
    whole: Vec<bool>,
}

enum Work {
    Export(usize, String),
    Whole(usize),
    Name(usize, String),
    Statement(usize, usize),
}

impl Graph<'_> {
    /// The id of a module, read later if it is new.
    fn id(&mut self, source: Source) -> usize {
        if let Some(&id) = self.ids.get(&source) {
            return id;
        }
        let id = self.modules.len();
        let name = match &source {
            Source::File(path) => display(path, self.root),
            Source::Entry => "<stdin>".to_string(),
        };
        self.modules.push(Module {
            source: source.clone(),
            name,
            javascript: String::new(),
            form: Form::Esm,
            requests: Vec::new(),
            statements: Vec::new(),
            imports: HashMap::new(),
            declared_by: HashMap::new(),
            exports: BTreeMap::new(),
            stars: Vec::new(),
            enums: HashMap::new(),
            requires: BTreeMap::new(),
            named: Vec::new(),
            symbols: BTreeSet::new(),
            globals: BTreeSet::new(),
        });
        self.ids.insert(source, id);
        self.queue.push_back(id);
        id
    }

    /// Reads a module: its JavaScript, what it imports, declares, uses and exports.
    fn analyze(&mut self, id: usize) -> Result<(), String> {
        let name = self.modules[id].name.clone();
        let (text, dir, path) = match &self.modules[id].source {
            Source::File(path) => {
                let text = std::fs::read_to_string(path)
                    .map_err(|error| format!("cannot read {name}: {error}"))?;
                let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
                (text, dir, path.clone())
            }
            Source::Entry => (
                self.entry.to_string(),
                self.root.to_path_buf(),
                self.root.join("stdin.tsx"),
            ),
        };
        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if extension == "json" {
            serde_json::from_str::<serde_json::Value>(&text)
                .map_err(|error| format!("{name} is not valid JSON: {error}"))?;
            let module = &mut self.modules[id];
            module.form = Form::Json;
            module.javascript = text;
            return Ok(());
        }
        let jsx = matches!(extension, "jsx" | "tsx")
            || self.jsx_in_js && matches!(extension, "js" | "mjs" | "cjs");
        let source_type = SourceType::from_path(&path)
            .unwrap_or_else(|_| SourceType::unambiguous())
            .with_jsx(jsx);
        let (javascript, enums) = if source_type.is_typescript() || jsx {
            let import_source = self.jsx_source(&dir);
            transform(&text, source_type, &path, &name, &import_source)?
        } else {
            (text, HashMap::new())
        };

        let allocator = Allocator::default();
        let parsed = Parser::new(&allocator, &javascript, javascript_type(source_type)).parse();
        if let Some(error) = parsed.diagnostics.iter().next() {
            return Err(describe(error, &name, &javascript));
        }
        let program = parsed.program;
        let semantic = SemanticBuilder::new().build(&program);
        let scoping = semantic.semantic.scoping();
        let unresolved = scoping.root_unresolved_references();
        let commonjs = !parsed.module_record.has_module_syntax
            && (unresolved.contains_key("module") || unresolved.contains_key("exports"));
        let symbols = scoping.symbol_names().map(str::to_string).collect();
        let globals = unresolved.keys().map(|name| name.to_string()).collect();

        let resolver = self.resolver;
        let mut resolve = |specifier: &str, kind: Kind| -> Result<usize, String> {
            let resolved = resolver
                .resolve(specifier, &dir, kind)
                .map_err(|error| format!("{name}: {error}"))?;
            Ok(self.id(Source::File(resolved)))
        };
        let mut reader = Reader {
            text: &javascript,
            name: &name,
            typescript: source_type.is_typescript(),
            scoping,
            resolve: &mut resolve,
            module: id,
            requests: Vec::new(),
            imports: HashMap::new(),
            declared_by: HashMap::new(),
            exports: BTreeMap::new(),
            stars: Vec::new(),
            named: Vec::new(),
            statement: StatementUse::default(),
            error: None,
        };
        let statements = reader.read(&program)?;
        let Reader {
            requests,
            imports,
            declared_by,
            exports,
            stars,
            named,
            ..
        } = reader;
        let module = &mut self.modules[id];
        module.requires = statements
            .iter()
            .flat_map(|statement| statement.requires.iter().copied())
            .collect();
        module.requests = requests;
        module.imports = imports;
        module.declared_by = declared_by;
        module.exports = exports;
        module.stars = stars;
        module.named = named;
        module.statements = statements;
        module.enums = enums;
        module.symbols = symbols;
        module.globals = globals;
        module.form = if commonjs { Form::CommonJs } else { Form::Esm };
        module.javascript = javascript.clone();
        Ok(())
    }

    /// The `jsxImportSource` of the closest `tsconfig.json` (or `jsconfig.json`) of `dir`, or of
    /// what it `extends`; `react`, esbuild's default, without one.
    fn jsx_source(&mut self, dir: &Path) -> String {
        if let Some(source) = self.jsx_sources.get(dir) {
            return source.clone();
        }
        let source = dir
            .ancestors()
            .find_map(|folder| {
                ["tsconfig.json", "jsconfig.json"]
                    .iter()
                    .map(|name| folder.join(name))
                    .find(|path| path.is_file())
            })
            .and_then(|config| jsx_import_source(&config, 0))
            .unwrap_or_else(|| "react".to_string());
        self.jsx_sources.insert(dir.to_path_buf(), source.clone());
        source
    }

    /// The names a module exports, with those of its `export * from`; `None` when a CommonJS
    /// module is among them.
    fn exports_of(&self, id: usize) -> Option<&BTreeSet<String>> {
        self.exported[id].as_ref()
    }

    fn compute_exports_of(&self, id: usize) -> Option<BTreeSet<String>> {
        let mut names = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut stack = vec![(id, true)];
        while let Some((id, own)) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            let module = &self.modules[id];
            match module.form {
                Form::Esm => {}
                Form::CommonJs => return None,
                Form::Json => {
                    names.insert("default".to_string());
                    continue;
                }
            }
            for name in module.exports.keys() {
                // `export *` passes everything but `default` on.
                if own || name != "default" {
                    names.insert(name.clone());
                }
            }
            stack.extend(module.stars.iter().map(|&star| (star, false)));
        }
        Some(names)
    }

    /// Every named import of an ES module is exported by it, as esbuild requires.
    fn check_imports(&self) -> Result<(), String> {
        for module in &self.modules {
            for import in &module.named {
                let target = &self.modules[import.from];
                // A JSON file's keys are its exports.
                if target.form == Form::Json || import.maybe_type {
                    continue;
                }
                if self
                    .exports_of(import.from)
                    .is_some_and(|names| !names.contains(&import.name))
                {
                    let importer = &self.modules[import.module];
                    return Err(if importer.source == Source::Entry {
                        format!("{} does not export `{}`", target.name, import.name)
                    } else {
                        format!(
                            "{}:{}: {} does not export `{}`",
                            importer.name, import.line, target.name, import.name
                        )
                    });
                }
            }
        }
        Ok(())
    }

    /// A `require` names a CommonJS module or a JSON file: lsf does not bundle an ES module a
    /// `require` runs (its module would run at the `require`, not in the order of the imports).
    fn check_requires(&self) -> Result<(), String> {
        for module in &self.modules {
            for &required in module.requires.values() {
                let target = &self.modules[required];
                if target.form == Form::Esm {
                    return Err(format!(
                        "{}: `require` of {}, an ES module, cannot be bundled: import it",
                        module.name, target.name
                    ));
                }
            }
        }
        Ok(())
    }

    /// What `name` (`None`: the namespace) exported by module `id` is, through `export … from`,
    /// `export * as` and `export *`.
    fn target(&self, id: usize, name: Option<&str>) -> Target {
        let mut seen = BTreeSet::new();
        let (mut id, mut name) = (id, name.map(str::to_string));
        loop {
            let module = &self.modules[id];
            if module.form != Form::Esm {
                return Target::CommonJs(id, name);
            }
            let Some(wanted) = name.clone() else {
                return Target::Namespace(id);
            };
            if !seen.insert((id, wanted.clone())) {
                return Target::Local(id, wanted);
            }
            match module.exports.get(&wanted) {
                Some(Export::Local(local)) => return Target::Local(id, local.clone()),
                Some(Export::From(from, imported)) => (id, name) = (*from, Some(imported.clone())),
                Some(Export::Namespace(from)) => (id, name) = (*from, None),
                None => {
                    // The star that exports it; else a CommonJS one, whose names are not known.
                    let star = module
                        .stars
                        .iter()
                        .copied()
                        .find(|&star| {
                            self.exports_of(star)
                                .is_some_and(|names| names.contains(&wanted))
                        })
                        .or_else(|| {
                            module
                                .stars
                                .iter()
                                .copied()
                                .find(|&star| self.exports_of(star).is_none())
                        });
                    match star {
                        Some(star) => id = star,
                        None => return Target::Local(id, wanted),
                    }
                }
            }
        }
    }

    /// The constant `imported.property` reads in module `id`, when `imported` is an enum of
    /// another module whose `property` is a constant.
    fn enum_member(&self, id: usize, imported: &str, property: &str) -> Option<&str> {
        let import = self.modules[id].imports.get(imported)?;
        let Target::Local(module, local) = self.target(import.from, Some(import.name.as_deref()?))
        else {
            return None;
        };
        self.modules[module]
            .enums
            .get(&local)?
            .get(property)
            .map(String::as_str)
    }

    /// What the Function needs: from its `roots` (module, export) and the modules that always
    /// run (`entries`), the exports read, the statements kept, the modules that run.
    fn live(&self, roots: &[(usize, String)], entries: &[usize]) -> Live {
        let count = self.modules.len();
        let mut live = Live {
            needed: vec![false; count],
            statements: self
                .modules
                .iter()
                .map(|module| vec![false; module.statements.len()])
                .collect(),
            exports: vec![BTreeSet::new(); count],
            whole: vec![false; count],
        };
        let mut work: VecDeque<Work> = VecDeque::new();
        for (id, module) in self.modules.iter().enumerate() {
            if module.form == Form::CommonJs {
                work.push_back(Work::Whole(id));
            }
            for (index, statement) in module.statements.iter().enumerate() {
                if statement.effects {
                    work.push_back(Work::Statement(id, index));
                }
            }
        }
        for (id, name) in roots {
            work.push_back(Work::Export(*id, name.clone()));
        }
        while let Some(item) = work.pop_front() {
            match item {
                Work::Export(id, name) => {
                    let module = &self.modules[id];
                    if module.form != Form::Esm {
                        work.push_back(Work::Whole(id));
                        continue;
                    }
                    if !live.exports[id].insert(name.clone()) {
                        continue;
                    }
                    match module.exports.get(&name) {
                        Some(Export::Local(local)) => {
                            work.push_back(Work::Name(id, local.clone()));
                        }
                        Some(Export::From(from, imported)) => {
                            work.push_back(Work::Export(*from, imported.clone()));
                        }
                        Some(Export::Namespace(from)) => work.push_back(Work::Whole(*from)),
                        None => {
                            for &star in &module.stars {
                                if self
                                    .exports_of(star)
                                    .is_none_or(|names| names.contains(&name))
                                {
                                    work.push_back(Work::Export(star, name.clone()));
                                }
                            }
                        }
                    }
                }
                Work::Whole(id) => {
                    if live.whole[id] {
                        continue;
                    }
                    live.whole[id] = true;
                    let module = &self.modules[id];
                    for name in module.exports.keys() {
                        work.push_back(Work::Export(id, name.clone()));
                    }
                    for &star in &module.stars {
                        work.push_back(Work::Whole(star));
                    }
                    for &required in module.requires.values() {
                        work.push_back(Work::Whole(required));
                    }
                }
                Work::Name(id, local) => {
                    let module = &self.modules[id];
                    if let Some(import) = module.imports.get(&local) {
                        work.push_back(match &import.name {
                            Some(name) => Work::Export(import.from, name.clone()),
                            None => Work::Whole(import.from),
                        });
                    } else if let Some(statements) = module.declared_by.get(&local) {
                        for &index in statements {
                            work.push_back(Work::Statement(id, index));
                        }
                    }
                }
                Work::Statement(id, index) => {
                    if live.statements[id][index] {
                        continue;
                    }
                    live.statements[id][index] = true;
                    let module = &self.modules[id];
                    let statement = &module.statements[index];
                    for used in &statement.uses {
                        match used {
                            Use::Name(name) => work.push_back(Work::Name(id, name.clone())),
                            Use::Member(name, property) => {
                                if self.enum_member(id, name, property).is_some() {
                                    continue;
                                }
                                match module.imports.get(name) {
                                    Some(Import { from, name: None }) => {
                                        work.push_back(Work::Export(*from, property.clone()));
                                    }
                                    _ => work.push_back(Work::Name(id, name.clone())),
                                }
                            }
                        }
                    }
                    for &(_, required) in &statement.requires {
                        work.push_back(Work::Whole(required));
                    }
                }
            }
        }

        // A module runs when something of it is kept, or when it is the way to a module that
        // runs (its import runs that module, in order).
        for id in 0..count {
            live.needed[id] = live.whole[id]
                || !live.exports[id].is_empty()
                || live.statements[id].iter().any(|&kept| kept);
        }
        for &entry in entries {
            live.needed[entry] = true;
        }
        loop {
            let mut changed = false;
            for id in 0..count {
                if !live.needed[id]
                    && self.modules[id]
                        .requests
                        .iter()
                        .any(|&(_, request)| live.needed[request])
                {
                    live.needed[id] = true;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        live
    }

    /// Where each module goes and the names it uses: the ES modules in the order they run
    /// (each after what it imports), the CommonJS ones as functions first.
    fn layout(&self, live: &Live, entries: &[usize]) -> Layout {
        let mut layout = Layout::default();
        let mut seen = vec![false; self.modules.len()];
        for &entry in entries {
            self.visit(entry, live, &mut seen, &mut layout.order);
        }
        for (id, module) in self.modules.iter().enumerate() {
            if live.needed[id] && module.form != Form::Esm {
                layout.commonjs.push(id);
                layout.wrappers.insert(id, format!("__lsf_commonjs{id}"));
                layout.interop.insert(id, format!("__lsf_import{id}"));
            }
        }
        for &id in &layout.order {
            if live.whole[id] {
                layout.namespaces.insert(id, format!("__lsf_namespace{id}"));
            }
            for &(_, request) in &self.modules[id].requests {
                if layout.wrappers.contains_key(&request) {
                    layout.owner.entry(request).or_insert(id);
                }
            }
        }

        // A top-level name keeps its name unless another module declares the same (in any
        // scope: an import of it there would read that one), a module reads a global of that
        // name, or a module before took it.
        let mut declaring: HashMap<&str, usize> = HashMap::new();
        // The globals the CommonJS modules and the runtime read, as well as the ES modules: a
        // top-level name would hide them.
        let mut taken: BTreeSet<String> = BTreeSet::from(["Object".to_string()]);
        for &id in &layout.commonjs {
            taken.extend(self.modules[id].globals.iter().cloned());
        }
        for &id in &layout.order {
            let module = &self.modules[id];
            for symbol in &module.symbols {
                *declaring.entry(symbol).or_default() += 1;
            }
            taken.extend(module.globals.iter().cloned());
        }
        let all: BTreeSet<&str> = declaring.keys().copied().collect();
        for &id in &layout.order {
            let module = &self.modules[id];
            let mut names: Vec<(&usize, &String)> = module
                .declared_by
                .iter()
                .map(|(name, statements)| (statements.iter().min().expect("one"), name))
                .collect();
            names.sort();
            for (_, name) in names {
                let others = declaring.get(name.as_str()).copied().unwrap_or(0)
                    > usize::from(module.symbols.contains(name));
                let renamed = if taken.contains(name) || others {
                    (1..)
                        .map(|n| format!("{name}_{n}"))
                        .find(|candidate| {
                            !taken.contains(candidate) && !all.contains(candidate.as_str())
                        })
                        .expect("a free name")
                } else {
                    name.clone()
                };
                taken.insert(renamed.clone());
                layout.names.insert((id, name.clone()), renamed);
            }
        }
        layout
    }

    /// Puts the ES modules `id` imports, then `id`, in `order`.
    fn visit(&self, id: usize, live: &Live, seen: &mut [bool], order: &mut Vec<usize>) {
        if seen[id] || !live.needed[id] || self.modules[id].form != Form::Esm {
            return;
        }
        seen[id] = true;
        for &(_, request) in &self.modules[id].requests {
            self.visit(request, live, seen, order);
        }
        order.push(id);
    }

    /// A CommonJS module, or a JSON file: a function that runs it the first time it is needed.
    fn emit_commonjs(&self, id: usize, layout: &Layout) -> String {
        let module = &self.modules[id];
        let body = if module.form == Form::Json {
            format!("module.exports = {};", module.javascript.trim())
        } else {
            let allocator = Allocator::default();
            let source_type = SourceType::from_path("module.js")
                .expect("a known extension")
                .with_commonjs(true);
            let program = Parser::new(&allocator, &module.javascript, source_type)
                .parse()
                .program;
            let semantic = SemanticBuilder::new().build(&program);
            let mut writer = Writer::new(self, id, semantic.semantic.scoping(), layout);
            if let Some(hashbang) = &program.hashbang {
                writer.edit(hashbang.span, "");
            }
            for statement in &program.body {
                writer.visit_statement(statement);
            }
            apply(&module.javascript, &mut writer.edits)
        };
        format!(
            "// {}\nvar {} = __lsf_commonjs(function (exports, module) {{\n{body}\n}});\n",
            module.name, layout.wrappers[&id]
        )
    }

    /// An ES module, in the shared scope: what it does not need left out, its imports read
    /// where they are declared, its top-level names renamed where they clash.
    fn emit(&self, id: usize, live: &Live, layout: &Layout) -> String {
        let module = &self.modules[id];
        let text = &module.javascript;
        let allocator = Allocator::default();
        let program = Parser::new(&allocator, text, javascript_type(SourceType::mjs()))
            .parse()
            .program;
        let semantic = SemanticBuilder::new().build(&program);
        let scoping = semantic.semantic.scoping();
        let mut writer = Writer::new(self, id, scoping, layout);
        if let Some(hashbang) = &program.hashbang {
            writer.edit(hashbang.span, "");
        }
        for symbol in scoping.symbol_ids() {
            if scoping.symbol_scope_id(symbol) != scoping.root_scope_id() {
                continue;
            }
            let name = scoping.symbol_name(symbol);
            if let Some(import) = module.imports.get(name) {
                if import.name.is_none() {
                    writer.namespaces.insert(symbol, import.from);
                }
                // A namespace only read through its members has no object: they are read where
                // they are declared.
                if let Some(read) = layout.read(&self.target(import.from, import.name.as_deref())) {
                    writer.reads.insert(symbol, read);
                }
            } else if let Some(renamed) = layout.names.get(&(id, name.to_string()))
                && renamed != name
            {
                writer.renames.insert(symbol, renamed.clone());
            }
        }
        let default = || layout.names[&(id, "__lsf_default".to_string())].clone();

        for (index, statement) in program.body.iter().enumerate() {
            let kept = live.statements[id][index];
            match statement {
                Statement::ImportDeclaration(_)
                | Statement::ExportFromDeclaration(_)
                | Statement::ExportAllDeclaration(_)
                | Statement::ExportNamedDeclaration(_) => writer.edit(statement.span(), ""),
                _ if !kept => writer.edit(statement.span(), ""),
                Statement::ExportDeclaration(export) => {
                    let declaration = &export.declaration;
                    writer.edit(Span::new(export.span.start, declaration.span().start), "");
                    writer.visit_declaration(declaration);
                }
                Statement::ExportDefaultDeclaration(export) => {
                    let start = export.span.start;
                    match &export.declaration {
                        ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                            if function.id.is_some() {
                                writer.edit(Span::new(start, function.span.start), "");
                            } else {
                                let head = format!(
                                    "{}function{} {}",
                                    if function.r#async { "async " } else { "" },
                                    if function.generator { "*" } else { "" },
                                    default(),
                                );
                                writer.edit(Span::new(start, function.params.span.start), head);
                            }
                            writer.visit_function(function, ScopeFlags::Function);
                        }
                        ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                            if class.id.is_some() {
                                writer.edit(Span::new(start, class.span.start), "");
                            } else {
                                writer.edit(
                                    Span::new(start, class.span.start + "class".len() as u32),
                                    format!("class {}", default()),
                                );
                            }
                            writer.visit_class(class);
                        }
                        kind => {
                            if let Some(expression) = kind.as_expression() {
                                writer.edit(
                                    Span::new(start, expression.span().start),
                                    format!("var {} = ", default()),
                                );
                                writer.visit_expression(expression);
                            } else {
                                writer.edit(export.span, "");
                            }
                        }
                    }
                }
                statement => writer.visit_statement(statement),
            }
        }
        let body = apply(text, &mut writer.edits);

        let mut head = format!("// {}\n", module.name);
        for &(_, request) in &module.requests {
            if layout.owner.get(&request) == Some(&id) {
                head.push_str(&format!(
                    "var {} = __lsf_toESM({}());\n",
                    layout.interop[&request], layout.wrappers[&request]
                ));
            }
        }
        if let Some(namespace) = layout.namespaces.get(&id) {
            let getters: Vec<String> = module
                .exports
                .keys()
                .filter_map(|name| {
                    let (value, _) = layout.read(&self.target(id, Some(name)))?;
                    Some(format!("{}: () => {value}", quote(name)))
                })
                .collect();
            head.push_str(&format!(
                "var {namespace} = {{}};\n__lsf_export({namespace}, {{ {} }});\n",
                getters.join(", ")
            ));
            for star in &module.stars {
                if let Some((source, _)) = layout.read(&self.target(*star, None)) {
                    head.push_str(&format!("__lsf_reexport({namespace}, {source});\n"));
                }
            }
        }
        format!("{head}{}\n", body.trim_end())
    }
}

/// What an import reads.
enum Target {
    /// A top-level name of an ES module.
    Local(usize, String),
    /// The namespace object of an ES module.
    Namespace(usize),
    /// A CommonJS module's (or JSON file's) view as an ES module, or a name of it.
    CommonJs(usize, Option<String>),
}

/// Where the modules go and the names they use, in the bundle.
#[derive(Default)]
struct Layout {
    /// The ES modules, in the order they run.
    order: Vec<usize>,
    /// The CommonJS modules and JSON files.
    commonjs: Vec<usize>,
    /// The final name of each top-level name of an ES module.
    names: HashMap<(usize, String), String>,
    /// The object of each ES module read as a namespace.
    namespaces: HashMap<usize, String>,
    /// The function that runs each CommonJS module.
    wrappers: HashMap<usize, String>,
    /// The view of each CommonJS module as an ES module.
    interop: HashMap<usize, String>,
    /// The module that makes that view, the first to import the CommonJS module.
    owner: HashMap<usize, usize>,
}

impl Layout {
    /// The expression that reads `target`, and whether it is a plain name.
    fn expression(&self, target: Target) -> (String, bool) {
        self.read(&target)
            .expect("what the Function needs has a name in the bundle")
    }

    /// The expression that reads `target`, and whether it is a plain name; `None` for a name
    /// no module declares (a type re-exported in TypeScript), or a namespace that has no object.
    fn read(&self, target: &Target) -> Option<(String, bool)> {
        Some(match target {
            Target::Local(id, name) => (self.names.get(&(*id, name.clone()))?.clone(), true),
            Target::Namespace(id) => (self.namespaces.get(id)?.clone(), true),
            Target::CommonJs(id, None) => (self.interop.get(id)?.clone(), true),
            Target::CommonJs(id, Some(name)) => (member(self.interop.get(id)?, name), false),
        })
    }
}

/// The JavaScript a module is read as, after its types are removed.
fn javascript_type(source_type: SourceType) -> SourceType {
    SourceType::from_path("module.js")
        .expect("a known extension")
        .with_commonjs(source_type.is_commonjs())
}

/// `path` relative to `root`, with `/`; a file of lsf's packages by its path in `node_modules`.
fn display(path: &Path, root: &Path) -> String {
    if let Some(relative) = packages::relative(path) {
        return relative;
    }
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// The JavaScript of a TypeScript or JSX module: types, `import type`s and imports only used as
/// types removed, `enum`s and `namespace`s turned into objects, JSX turned into calls of the
/// automatic runtime of `jsx_source` (`preact/jsx-runtime`), nothing lowered past ES2022. With the
/// constant members of its top-level enums.
fn transform(
    text: &str,
    source_type: SourceType,
    path: &Path,
    name: &str,
    jsx_source: &str,
) -> Result<(String, Enums), String> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, text, source_type).parse();
    if let Some(error) = parsed.diagnostics.iter().next() {
        return Err(describe(error, name, text));
    }
    let mut program = parsed.program;
    let enums = constant_enums(&program);
    let semantic = SemanticBuilder::new().with_enum_eval(true).build(&program);
    let mut options = TransformOptions::from_target("es2022").expect("a known target");
    options.jsx.runtime = JsxRuntime::Automatic;
    options.jsx.import_source = Some(jsx_source.to_string());
    let transformed = Transformer::new(&allocator, path, &options)
        .build_with_scoping(semantic.semantic.into_scoping(), &mut program);
    if let Some(error) = transformed.diagnostics.iter().next() {
        return Err(describe(error, name, text));
    }
    Ok((Codegen::new().build(&program).code, enums))
}

/// The `compilerOptions.jsxImportSource` of a `tsconfig.json`, or of the one it `extends`
/// (a path). The file may hold comments and trailing commas, as TypeScript allows.
fn jsx_import_source(config: &Path, depth: usize) -> Option<String> {
    let text = std::fs::read_to_string(config).ok()?;
    let value: serde_json::Value = serde_json::from_str(&jsonc(&text)).ok()?;
    if let Some(source) = value["compilerOptions"]["jsxImportSource"].as_str() {
        return Some(source.to_string());
    }
    let extends = value["extends"].as_str()?;
    if depth > 8 || !(extends.starts_with('.') || Path::new(extends).is_absolute()) {
        return None;
    }
    // As TypeScript: the path, else the path with `.json` after it.
    let parent = config.parent()?.join(extends);
    let parent = if parent.is_file() {
        parent
    } else {
        let mut with_json = parent.into_os_string();
        with_json.push(".json");
        PathBuf::from(with_json)
    };
    jsx_import_source(&parent, depth + 1)
}

/// JSON with comments and trailing commas, as plain JSON.
fn jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                '\\' => out.extend(chars.next()),
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push(c);
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut last = ' ';
                for c in chars.by_ref() {
                    if last == '*' && c == '/' {
                        break;
                    }
                    last = c;
                }
            }
            (',', _) => {
                // A comma before `}` or `]` is dropped.
                let rest: String = chars.clone().collect();
                if !matches!(rest.trim_start().chars().next(), Some('}' | ']')) {
                    out.push(c);
                }
            }
            _ => out.push(c),
        }
    }
    out
}

/// The enums declared once at the top level whose members are all constants (string or number
/// literals, or numbers counted from the member before), each member written as JavaScript.
fn constant_enums(program: &Program) -> Enums {
    let mut enums = Enums::new();
    let mut declared: HashMap<String, usize> = HashMap::new();
    for statement in &program.body {
        let declaration: &TSEnumDeclaration = match statement {
            Statement::TSEnumDeclaration(declaration) => declaration,
            Statement::ExportDeclaration(export) => match &export.declaration {
                Declaration::TSEnumDeclaration(declaration) => declaration,
                _ => continue,
            },
            _ => continue,
        };
        let name = declaration.id.name.to_string();
        *declared.entry(name.clone()).or_default() += 1;
        if declaration.declare {
            continue;
        }
        if let Some(members) = constant_members(declaration) {
            enums.insert(name, members);
        }
    }
    enums.retain(|name, _| declared[name] == 1);
    enums
}

fn constant_members(declaration: &TSEnumDeclaration) -> Option<HashMap<String, String>> {
    let mut members = HashMap::new();
    let mut next: Option<f64> = Some(0.0);
    for member in &declaration.body.members {
        let key = match &member.id {
            TSEnumMemberName::Identifier(name) => name.name.to_string(),
            TSEnumMemberName::String(literal) | TSEnumMemberName::ComputedString(literal) => {
                literal.value.to_string()
            }
            TSEnumMemberName::ComputedTemplateString(_) => return None,
        };
        let value = match &member.initializer {
            None => {
                let number = next?;
                next = Some(number + 1.0);
                number_literal(number)
            }
            Some(Expression::StringLiteral(literal)) => {
                next = None;
                quote(&literal.value)
            }
            Some(Expression::NumericLiteral(literal)) => {
                next = Some(literal.value + 1.0);
                number_literal(literal.value)
            }
            Some(Expression::UnaryExpression(unary))
                if unary.operator == UnaryOperator::UnaryNegation =>
            {
                let Expression::NumericLiteral(literal) = &unary.argument else {
                    return None;
                };
                next = Some(-literal.value + 1.0);
                number_literal(-literal.value)
            }
            _ => return None,
        };
        members.insert(key, value);
    }
    Some(members)
}

/// A number as JavaScript writes it, for the integers enums count with and other literals.
fn number_literal(number: f64) -> String {
    if number.fract() == 0.0 && number.abs() < 1e15 {
        format!("{}", number as i64)
    } else {
        format!("{number}")
    }
}

/// `name:line:column: message` for a diagnostic of oxc about `text`.
fn describe(error: &OxcDiagnostic, name: &str, text: &str) -> String {
    match error.labels.first() {
        Some(label) => {
            let (line, column) = position(text, label.offset() as usize);
            format!("{name}:{line}:{column}: {}", error.message)
        }
        None => format!("{name}: {}", error.message),
    }
}

/// The line and column (from 1) of a byte offset.
fn position(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let column = before.len() - before.rfind('\n').map_or(0, |newline| newline + 1) + 1;
    (line, column)
}

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

/// A JavaScript string literal.
fn quote(text: &str) -> String {
    serde_json::to_string(text).expect("a string serializes")
}

/// `object.name`, or `object["name"]` when the name is not an identifier.
fn member(object: &str, name: &str) -> String {
    let identifier = !name.is_empty()
        && name.chars().enumerate().all(|(index, c)| {
            c == '_' || c == '$' || c.is_alphabetic() || (index > 0 && c.is_numeric())
        });
    if identifier {
        format!("{object}.{name}")
    } else {
        format!("{object}[{}]", quote(name))
    }
}

/// `text` with `edits` (start, end, replacement) applied. An edit inside one already applied is
/// skipped (a removed statement takes what it contains with it).
fn apply(text: &str, edits: &mut [(u32, u32, String)]) -> String {
    edits.sort_by_key(|(start, end, _)| (*start, std::cmp::Reverse(*end)));
    let mut out = String::with_capacity(text.len() + edits.len() * 16);
    let mut at = 0;
    for (start, end, replacement) in edits.iter() {
        let (start, end) = (*start as usize, *end as usize);
        if start < at {
            continue;
        }
        out.push_str(&text[at..start]);
        out.push_str(replacement);
        at = end;
    }
    out.push_str(&text[at..]);
    out
}

/// The names a declaration binds.
fn declared(declaration: &Declaration) -> Vec<String> {
    match declaration {
        Declaration::VariableDeclaration(variables) => variables
            .declarations
            .iter()
            .flat_map(|declarator| declarator.id.get_binding_identifiers())
            .map(|binding| binding.name.to_string())
            .collect(),
        Declaration::FunctionDeclaration(function) => {
            function.id.iter().map(|id| id.name.to_string()).collect()
        }
        Declaration::ClassDeclaration(class) => {
            class.id.iter().map(|id| id.name.to_string()).collect()
        }
        _ => Vec::new(),
    }
}

/// Whether `reference` names no binding: a global.
fn is_global(scoping: &Scoping, reference: &IdentifierReference) -> bool {
    reference
        .reference_id
        .get()
        .is_none_or(|id| scoping.get_reference(id).symbol_id().is_none())
}

/// The literal of a `require("…")` call, if that is what `call` is: `Err` when it requires
/// something else than a string.
fn required(call: &CallExpression, scoping: &Scoping) -> Option<Result<String, ()>> {
    let Expression::Identifier(callee) = &call.callee else {
        return None;
    };
    if callee.name != "require" || !is_global(scoping, callee) {
        return None;
    }
    Some(match (call.arguments.len(), call.arguments.first()) {
        (1, Some(Argument::StringLiteral(literal))) => Ok(literal.value.to_string()),
        _ => Err(()),
    })
}

/// Reads a module (first phase): its imports, exports, and for each top-level statement what it
/// declares, uses and whether it has side effects.
struct Reader<'s, 'r> {
    text: &'s str,
    name: &'s str,
    /// Whether the module was TypeScript.
    typescript: bool,
    scoping: &'s Scoping,
    resolve: &'r mut dyn FnMut(&str, Kind) -> Result<usize, String>,
    module: usize,
    requests: Vec<(String, usize)>,
    imports: HashMap<String, Import>,
    declared_by: HashMap<String, Vec<usize>>,
    exports: BTreeMap<String, Export>,
    stars: Vec<usize>,
    named: Vec<NamedImport>,
    /// The statement being read.
    statement: StatementUse,
    error: Option<String>,
}

impl Reader<'_, '_> {
    fn line(&self, offset: u32) -> usize {
        position(self.text, offset as usize).0
    }

    /// The module `specifier` names, as an ES module import.
    fn request(&mut self, specifier: &str) -> Result<usize, String> {
        if let Some((_, id)) = self.requests.iter().find(|(name, _)| name == specifier) {
            return Ok(*id);
        }
        let id = (self.resolve)(specifier, Kind::Import)?;
        self.requests.push((specifier.to_string(), id));
        Ok(id)
    }

    fn named(&mut self, from: usize, name: &str, offset: u32) {
        let line = self.line(offset);
        self.named.push(NamedImport {
            module: self.module,
            from,
            name: name.to_string(),
            line,
            maybe_type: false,
        });
    }

    fn read(&mut self, program: &Program) -> Result<Vec<StatementUse>, String> {
        // The imports first: they are hoisted, and the modules they name run in the order of the
        // statements that name them.
        for statement in &program.body {
            match statement {
                Statement::ImportDeclaration(import) => {
                    let from = self.request(&import.source.value)?;
                    for specifier in import.specifiers.iter().flatten() {
                        let (local, name) = match specifier {
                            ImportDeclarationSpecifier::ImportSpecifier(named) => {
                                let imported = named.imported.name().to_string();
                                self.named(from, &imported, named.span.start);
                                (&named.local, Some(imported))
                            }
                            ImportDeclarationSpecifier::ImportDefaultSpecifier(default) => {
                                self.named(from, "default", default.span.start);
                                (&default.local, Some("default".to_string()))
                            }
                            ImportDeclarationSpecifier::ImportNamespaceSpecifier(namespace) => {
                                (&namespace.local, None)
                            }
                        };
                        self.imports
                            .insert(local.name.to_string(), Import { from, name });
                    }
                }
                Statement::ExportFromDeclaration(export) => {
                    self.request(&export.source.value)?;
                }
                Statement::ExportAllDeclaration(export) => {
                    self.request(&export.source.value)?;
                }
                _ => {}
            }
        }

        let mut statements = Vec::with_capacity(program.body.len());
        for (index, statement) in program.body.iter().enumerate() {
            self.statement = StatementUse::default();
            match statement {
                Statement::ImportDeclaration(_) => {}
                Statement::ExportFromDeclaration(export) => {
                    let from = self.request(&export.source.value)?;
                    for specifier in &export.specifiers {
                        let local = specifier.local.name().to_string();
                        self.named(from, &local, specifier.span.start);
                        if self.typescript {
                            self.named.last_mut().expect("just pushed").maybe_type = true;
                        }
                        self.exports.insert(
                            specifier.exported.name().to_string(),
                            Export::From(from, local),
                        );
                    }
                }
                Statement::ExportAllDeclaration(export) => {
                    let from = self.request(&export.source.value)?;
                    match &export.exported {
                        Some(name) => {
                            self.exports
                                .insert(name.name().to_string(), Export::Namespace(from));
                        }
                        None => self.stars.push(from),
                    }
                }
                Statement::ExportNamedDeclaration(export) => {
                    for specifier in &export.specifiers {
                        let exported = specifier.exported.name().to_string();
                        let local = specifier.local.name().to_string();
                        let target = match self.imports.get(&local) {
                            Some(Import {
                                from,
                                name: Some(name),
                            }) => Export::From(*from, name.clone()),
                            Some(Import { from, name: None }) => Export::Namespace(*from),
                            None => Export::Local(local),
                        };
                        self.exports.insert(exported, target);
                    }
                }
                Statement::ExportDeclaration(export) => {
                    for name in declared(&export.declaration) {
                        self.declare(&name, index);
                        self.exports.insert(name.clone(), Export::Local(name));
                    }
                    self.statement.effects = !declaration_is_pure(&export.declaration, self);
                    self.visit_declaration(&export.declaration);
                }
                Statement::ExportDefaultDeclaration(export) => {
                    let local = match &export.declaration {
                        ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
                            self.visit_function(function, ScopeFlags::Function);
                            function.id.as_ref().map(|id| id.name.to_string())
                        }
                        ExportDefaultDeclarationKind::ClassDeclaration(class) => {
                            self.statement.effects = !class_is_pure(class, self);
                            self.visit_class(class);
                            class.id.as_ref().map(|id| id.name.to_string())
                        }
                        kind => {
                            if let Some(expression) = kind.as_expression() {
                                self.statement.effects = !expression_is_pure(expression, self);
                                self.visit_expression(expression);
                            }
                            None
                        }
                    };
                    let local = local.unwrap_or_else(|| "__lsf_default".to_string());
                    self.declare(&local, index);
                    self.exports
                        .insert("default".to_string(), Export::Local(local));
                }
                statement => {
                    match statement {
                        Statement::FunctionDeclaration(function) => {
                            if let Some(id) = &function.id {
                                self.declare(&id.name, index);
                            }
                        }
                        Statement::ClassDeclaration(class) => {
                            if let Some(id) = &class.id {
                                self.declare(&id.name, index);
                            }
                            self.statement.effects = !class_is_pure(class, self);
                        }
                        Statement::VariableDeclaration(variables) => {
                            for declarator in &variables.declarations {
                                for binding in declarator.id.get_binding_identifiers() {
                                    self.declare(&binding.name, index);
                                }
                            }
                            self.statement.effects = !variables.declarations.iter().all(|d| {
                                d.init
                                    .as_ref()
                                    .is_none_or(|init| expression_is_pure(init, self))
                            });
                        }
                        Statement::EmptyStatement(_) => {}
                        _ => self.statement.effects = true,
                    }
                    self.visit_statement(statement);
                }
            }
            if let Some(error) = self.error.take() {
                return Err(error);
            }
            statements.push(std::mem::take(&mut self.statement));
        }
        Ok(statements)
    }

    fn declare(&mut self, name: &str, statement: usize) {
        self.declared_by
            .entry(name.to_string())
            .or_default()
            .push(statement);
    }

    /// The top-level name a reference reads, if it reads one (not a local of a function, not a
    /// global).
    fn top_level(&self, reference: &IdentifierReference) -> Option<String> {
        let symbol = self
            .scoping
            .get_reference(reference.reference_id.get()?)
            .symbol_id()?;
        (self.scoping.symbol_scope_id(symbol) == self.scoping.root_scope_id())
            .then(|| self.scoping.symbol_name(symbol).to_string())
    }

    fn fail(&mut self, offset: u32, message: String) {
        if self.error.is_none() {
            let line = self.line(offset);
            self.error = Some(format!("{}:{line}: {message}", self.name));
        }
    }
}

impl<'a> Visit<'a> for Reader<'_, '_> {
    fn visit_identifier_reference(&mut self, reference: &IdentifierReference<'a>) {
        if let Some(name) = self.top_level(reference) {
            self.statement.uses.push(Use::Name(name));
        }
    }

    fn visit_static_member_expression(&mut self, expression: &StaticMemberExpression<'a>) {
        if let Expression::Identifier(object) = &expression.object
            && let Some(name) = self.top_level(object)
            && self.imports.contains_key(&name)
        {
            self.statement
                .uses
                .push(Use::Member(name, expression.property.name.to_string()));
            return;
        }
        walk::walk_static_member_expression(self, expression);
    }

    fn visit_simple_assignment_target(&mut self, target: &SimpleAssignmentTarget<'a>) {
        // Writing `name.property` uses `name` itself.
        if let SimpleAssignmentTarget::StaticMemberExpression(expression) = target
            && let Expression::Identifier(object) = &expression.object
            && let Some(name) = self.top_level(object)
        {
            self.statement.uses.push(Use::Name(name));
            return;
        }
        walk::walk_simple_assignment_target(self, target);
    }

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        match required(call, self.scoping) {
            Some(Ok(specifier)) => match (self.resolve)(&specifier, Kind::Require) {
                Ok(id) => self.statement.requires.push((call.span.start, id)),
                Err(error) => {
                    if self.error.is_none() {
                        self.error = Some(error);
                    }
                }
            },
            Some(Err(())) => self.fail(
                call.span.start,
                "`require` of something else than a string cannot be bundled".to_string(),
            ),
            None => walk::walk_call_expression(self, call),
        }
    }

    fn visit_import_expression(&mut self, import: &ImportExpression<'a>) {
        self.fail(
            import.span.start,
            "`import()` cannot be bundled into a Function: import the module".to_string(),
        );
    }
}

/// Whether running `expression` only computes a value, as esbuild judges it: no call but those
/// marked `/* @__PURE__ */` (as oxc marks the functions that build enums), no global that may
/// not exist, no spread.
fn expression_is_pure(expression: &Expression, reader: &Reader) -> bool {
    let pure = |expression: &Expression| expression_is_pure(expression, reader);
    match expression {
        Expression::BooleanLiteral(_)
        | Expression::NullLiteral(_)
        | Expression::NumericLiteral(_)
        | Expression::BigIntLiteral(_)
        | Expression::StringLiteral(_)
        | Expression::RegExpLiteral(_)
        | Expression::FunctionExpression(_)
        | Expression::ArrowFunctionExpression(_) => true,
        Expression::TemplateLiteral(template) => template.expressions.iter().all(pure),
        Expression::Identifier(reference) => {
            !is_global(reader.scoping, reference)
                || matches!(reference.name.as_str(), "undefined" | "NaN" | "Infinity")
        }
        Expression::ClassExpression(class) => class_is_pure(class, reader),
        Expression::ObjectExpression(object) => {
            object.properties.iter().all(|property| match property {
                ObjectPropertyKind::ObjectProperty(property) => {
                    key_is_pure(&property.key, reader) && pure(&property.value)
                }
                ObjectPropertyKind::SpreadProperty(_) => false,
            })
        }
        Expression::ArrayExpression(array) => array.elements.iter().all(|element| match element {
            ArrayExpressionElement::SpreadElement(_) => false,
            ArrayExpressionElement::Elision(_) => true,
            element => element.as_expression().is_some_and(pure),
        }),
        Expression::UnaryExpression(unary) => match unary.operator {
            UnaryOperator::Delete => false,
            // `typeof name` does not throw for a global that does not exist.
            UnaryOperator::Typeof if matches!(&unary.argument, Expression::Identifier(_)) => true,
            _ => pure(&unary.argument),
        },
        Expression::CallExpression(call) => {
            call.pure
                && call
                    .arguments
                    .iter()
                    .all(|argument| argument.as_expression().is_some_and(pure))
        }
        Expression::NewExpression(new) => {
            new.pure
                && new
                    .arguments
                    .iter()
                    .all(|argument| argument.as_expression().is_some_and(pure))
        }
        Expression::ParenthesizedExpression(inner) => pure(&inner.expression),
        Expression::SequenceExpression(sequence) => sequence.expressions.iter().all(pure),
        Expression::ConditionalExpression(conditional) => {
            pure(&conditional.test) && pure(&conditional.consequent) && pure(&conditional.alternate)
        }
        Expression::LogicalExpression(logical) => pure(&logical.left) && pure(&logical.right),
        _ => false,
    }
}

/// Whether a property key is computed without side effects: a name, a literal.
fn key_is_pure(key: &PropertyKey, reader: &Reader) -> bool {
    match key {
        PropertyKey::StaticIdentifier(_) | PropertyKey::PrivateIdentifier(_) => true,
        key => key.as_expression().is_some_and(|key| {
            matches!(
                key,
                Expression::StringLiteral(_) | Expression::NumericLiteral(_)
            ) || !matches!(key, Expression::Identifier(_)) && expression_is_pure(key, reader)
        }),
    }
}

fn class_is_pure(class: &Class, reader: &Reader) -> bool {
    let value_is_pure = |is_static: bool, value: Option<&Expression>| {
        !is_static || value.is_none_or(|value| expression_is_pure(value, reader))
    };
    class.decorators.is_empty()
        && class.heritage.as_ref().is_none_or(|heritage| {
            matches!(heritage.expression, Expression::Identifier(_))
                && expression_is_pure(&heritage.expression, reader)
        })
        && class.body.body.iter().all(|element| match element {
            ClassElement::StaticBlock(_) => false,
            ClassElement::MethodDefinition(method) => {
                method.decorators.is_empty() && key_is_pure(&method.key, reader)
            }
            ClassElement::PropertyDefinition(property) => {
                property.decorators.is_empty()
                    && key_is_pure(&property.key, reader)
                    && value_is_pure(property.r#static, property.value.as_ref())
            }
            ClassElement::AccessorProperty(accessor) => {
                accessor.decorators.is_empty()
                    && key_is_pure(&accessor.key, reader)
                    && value_is_pure(accessor.r#static, accessor.value.as_ref())
            }
            ClassElement::TSIndexSignature(_) => true,
        })
}

fn declaration_is_pure(declaration: &Declaration, reader: &Reader) -> bool {
    match declaration {
        Declaration::VariableDeclaration(variables) => {
            variables.declarations.iter().all(|declarator| {
                declarator
                    .init
                    .as_ref()
                    .is_none_or(|init| expression_is_pure(init, reader))
            })
        }
        Declaration::ClassDeclaration(class) => class_is_pure(class, reader),
        _ => true,
    }
}

/// Writes a module (second phase): its imports read where they are declared, its top-level names
/// renamed, enum members inlined, `require`s run.
struct Writer<'s, 'g> {
    graph: &'s Graph<'g>,
    layout: &'s Layout,
    module: usize,
    scoping: &'s Scoping,
    edits: Vec<(u32, u32, String)>,
    /// What each imported binding reads, and whether that is a plain name.
    reads: HashMap<SymbolId, (String, bool)>,
    /// The namespace imports (`* as name`), and the module of each.
    namespaces: HashMap<SymbolId, usize>,
    /// The top-level bindings renamed.
    renames: HashMap<SymbolId, String>,
    /// Imported bindings called (`f()`, f`…`): called as `(0, object.f)()` when what they read
    /// is a property, so that the call has no `this`.
    callees: BTreeSet<u32>,
    /// Names in shorthand properties and patterns (`{ f }`), written `{ f: renamed }`.
    shorthands: BTreeSet<u32>,
    /// Member expressions written to: never inlined.
    targets: BTreeSet<u32>,
    /// The modules of the module's `require` calls, by where the call starts.
    requires: &'s BTreeMap<u32, usize>,
}

impl<'s, 'g> Writer<'s, 'g> {
    fn new(graph: &'s Graph<'g>, module: usize, scoping: &'s Scoping, layout: &'s Layout) -> Self {
        Writer {
            graph,
            layout,
            module,
            scoping,
            edits: Vec::new(),
            reads: HashMap::new(),
            namespaces: HashMap::new(),
            renames: HashMap::new(),
            callees: BTreeSet::new(),
            shorthands: BTreeSet::new(),
            targets: BTreeSet::new(),
            requires: &graph.modules[module].requires,
        }
    }

    fn edit(&mut self, span: Span, replacement: impl Into<String>) {
        self.edits.push((span.start, span.end, replacement.into()));
    }

    fn symbol(&self, reference: &IdentifierReference) -> Option<SymbolId> {
        self.scoping
            .get_reference(reference.reference_id.get()?)
            .symbol_id()
    }
}

impl<'a> Visit<'a> for Writer<'_, '_> {
    fn visit_identifier_reference(&mut self, reference: &IdentifierReference<'a>) {
        let Some(symbol) = self.symbol(reference) else {
            return;
        };
        let span = reference.span;
        let (read, plain) = match (self.reads.get(&symbol), self.renames.get(&symbol)) {
            (Some(read), _) => read.clone(),
            (None, Some(renamed)) => (renamed.clone(), true),
            (None, None) => return,
        };
        let replacement = if self.shorthands.contains(&span.start) {
            format!("{}: {read}", reference.name)
        } else if !plain && self.callees.contains(&span.start) {
            format!("(0, {read})")
        } else {
            read
        };
        self.edit(span, replacement);
    }

    fn visit_binding_identifier(&mut self, binding: &BindingIdentifier<'a>) {
        let Some(renamed) = binding
            .symbol_id
            .get()
            .and_then(|symbol| self.renames.get(&symbol))
            .cloned()
        else {
            return;
        };
        let replacement = if self.shorthands.contains(&binding.span.start) {
            format!("{}: {renamed}", binding.name)
        } else {
            renamed
        };
        self.edit(binding.span, replacement);
    }

    fn visit_binding_property(&mut self, property: &BindingProperty<'a>) {
        if property.shorthand {
            self.shorthands.insert(property.key.span().start);
        }
        walk::walk_binding_property(self, property);
    }

    fn visit_assignment_target_property_identifier(
        &mut self,
        property: &AssignmentTargetPropertyIdentifier<'a>,
    ) {
        self.shorthands.insert(property.binding.span.start);
        walk::walk_assignment_target_property_identifier(self, property);
    }

    fn visit_object_property(&mut self, property: &ObjectProperty<'a>) {
        if property.shorthand
            && let Expression::Identifier(value) = &property.value
        {
            self.shorthands.insert(value.span.start);
        }
        walk::walk_object_property(self, property);
    }

    fn visit_call_expression(&mut self, call: &CallExpression<'a>) {
        if required(call, self.scoping).is_some() {
            let id = self.requires[&call.span.start];
            let call_text = format!("{}()", self.layout.wrappers[&id]);
            self.edit(call.span, call_text);
            return;
        }
        if let Expression::Identifier(callee) = &call.callee {
            self.callees.insert(callee.span.start);
        }
        walk::walk_call_expression(self, call);
    }

    fn visit_tagged_template_expression(&mut self, tagged: &TaggedTemplateExpression<'a>) {
        if let Expression::Identifier(tag) = &tagged.tag {
            self.callees.insert(tag.span.start);
        }
        walk::walk_tagged_template_expression(self, tagged);
    }

    fn visit_simple_assignment_target(&mut self, target: &SimpleAssignmentTarget<'a>) {
        if let SimpleAssignmentTarget::StaticMemberExpression(expression) = target {
            self.targets.insert(expression.span.start);
        }
        walk::walk_simple_assignment_target(self, target);
    }

    fn visit_static_member_expression(&mut self, expression: &StaticMemberExpression<'a>) {
        let property = &expression.property.name;
        if !self.targets.contains(&expression.span.start)
            && let Expression::Identifier(object) = &expression.object
            && let Some(symbol) = self.symbol(object)
        {
            if let Some(constant) =
                self.graph
                    .enum_member(self.module, self.scoping.symbol_name(symbol), property)
            {
                // `-Dir.Down` must not become `--1`.
                let replacement = if constant.starts_with('-') {
                    format!("({constant} /* {property} */)")
                } else {
                    format!("{constant} /* {property} */")
                };
                self.edit(expression.span, replacement);
                return;
            }
            // `namespace.name` reads `name` where it is declared, as esbuild does.
            if let Some(&from) = self.namespaces.get(&symbol)
                && let Some((read, _)) = self.layout.read(&self.graph.target(from, Some(property)))
            {
                self.edit(expression.span, read);
                return;
            }
        }
        if let Expression::StaticMemberExpression(object) = &expression.object
            && object.property.name == "env"
            && let Expression::Identifier(process) = &object.object
            && process.name == "process"
            && is_global(self.scoping, process)
            && let Some(value) = self.graph.env.get(expression.property.name.as_str())
        {
            let literal = quote(value);
            self.edit(expression.span, literal);
            return;
        }
        walk::walk_static_member_expression(self, expression);
    }
}

#[cfg(test)]
mod tests;
