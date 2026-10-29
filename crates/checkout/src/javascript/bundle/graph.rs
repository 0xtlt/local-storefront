//! The modules of a bundle: what each one is, from a first reading of it, and how a name it
//! exports leads to where it is declared.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::path::{Path, PathBuf};

use oxc::allocator::Allocator;
use oxc::parser::Parser;
use oxc::semantic::SemanticBuilder;
use oxc::span::SourceType;

use crate::javascript::resolve::{Kind, Resolver};

use super::*;

/// Where a module comes from.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(super) enum Source {
    /// The entry Shopify's CLI generates.
    Entry,
    File(PathBuf),
}

pub(super) struct Graph<'a> {
    pub(super) root: &'a Path,
    pub(super) env: &'a BTreeMap<String, String>,
    pub(super) resolver: &'a Resolver,
    pub(super) jsx_in_js: bool,
    pub(super) entry: &'a str,
    pub(super) modules: Vec<Module>,
    pub(super) ids: HashMap<Source, usize>,
    pub(super) queue: VecDeque<usize>,
    /// The `jsxImportSource` of the `tsconfig.json` that rules each folder.
    pub(super) jsx_sources: HashMap<PathBuf, String>,
    /// What each module exports ([`Graph::exports_of`]), once every module is read.
    pub(super) exported: Vec<Option<BTreeSet<String>>>,
}

/// What a module is, from its first reading.
pub(super) struct Module {
    pub(super) source: Source,
    /// The path relative to the Function's folder, for messages.
    pub(super) name: String,
    /// The JavaScript, types removed; for a JSON file, the JSON.
    pub(super) javascript: String,
    pub(super) form: Form,
    /// The modules its `import`s and `export … from`s name, in order, each once.
    pub(super) requests: Vec<(String, usize)>,
    /// Its top-level statements, in order (an ES module's).
    pub(super) statements: Vec<StatementUse>,
    /// What each imported binding reads.
    pub(super) imports: HashMap<String, Import>,
    /// The statements that declare each other top-level name.
    pub(super) declared_by: HashMap<String, Vec<usize>>,
    pub(super) exports: BTreeMap<String, Export>,
    /// The modules of its `export * from`.
    pub(super) stars: Vec<usize>,
    /// The constant members of the enums it declares, written as JavaScript, by enum.
    pub(super) enums: Enums,
    /// The modules its `require` calls name, by where the call starts in `javascript`.
    pub(super) requires: BTreeMap<u32, usize>,
    /// The names it imports from other modules, to check they are exported.
    pub(super) named: Vec<NamedImport>,
    /// Every name it declares, in any scope: a top-level name of another module must not be
    /// one of them, or the module could not read it.
    pub(super) symbols: BTreeSet<String>,
    /// The globals it reads, which no top-level name may hide.
    pub(super) globals: BTreeSet<String>,
}

/// The constant members of enums, written as JavaScript, by member, by enum.
pub(super) type Enums = HashMap<String, HashMap<String, String>>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Form {
    Esm,
    CommonJs,
    Json,
}

#[derive(Default)]
pub(super) struct StatementUse {
    /// Whether running it can do something else than declare what it declares.
    pub(super) effects: bool,
    /// The top-level names it reads.
    pub(super) uses: Vec<Use>,
    /// The modules it `require`s, by where the call starts.
    pub(super) requires: Vec<(u32, usize)>,
}

pub(super) enum Use {
    Name(String),
    /// `name.property`, where `name` is imported: an enum member, or a name of a namespace.
    Member(String, String),
}

#[derive(Clone)]
pub(super) struct Import {
    pub(super) from: usize,
    /// The name imported; `None` for a namespace (`* as name`).
    pub(super) name: Option<String>,
}

#[derive(Clone)]
pub(super) enum Export {
    Local(String),
    From(usize, String),
    Namespace(usize),
}

pub(super) struct NamedImport {
    pub(super) module: usize,
    pub(super) from: usize,
    pub(super) name: String,
    pub(super) line: usize,
    /// A re-export in TypeScript (`export { Foo } from`): a name its module does not export is a
    /// type, which esbuild leaves out.
    pub(super) maybe_type: bool,
}

/// What an import reads.
pub(super) enum Target {
    /// A top-level name of an ES module.
    Local(usize, String),
    /// The namespace object of an ES module.
    Namespace(usize),
    /// A CommonJS module's (or JSON file's) view as an ES module, or a name of it.
    CommonJs(usize, Option<String>),
}

impl Graph<'_> {
    /// The id of a module, read later if it is new.
    pub(super) fn id(&mut self, source: Source) -> usize {
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
    pub(super) fn analyze(&mut self, id: usize) -> Result<(), String> {
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
    pub(super) fn jsx_source(&mut self, dir: &Path) -> String {
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
    pub(super) fn exports_of(&self, id: usize) -> Option<&BTreeSet<String>> {
        self.exported[id].as_ref()
    }

    pub(super) fn compute_exports_of(&self, id: usize) -> Option<BTreeSet<String>> {
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
    pub(super) fn check_imports(&self) -> Result<(), String> {
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
    pub(super) fn check_requires(&self) -> Result<(), String> {
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
    pub(super) fn target(&self, id: usize, name: Option<&str>) -> Target {
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
    pub(super) fn enum_member(&self, id: usize, imported: &str, property: &str) -> Option<&str> {
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
}
