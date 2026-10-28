//! Builds what Shopify's CLI builds with Node, without it:
//!
//! - a Function written in JavaScript or TypeScript, into its Wasm module, as
//!   `shopify app function build` does: the GraphQL typegen ([`typegen`]), one JavaScript file
//!   for the Function and what it imports ([`bundle`]), then the module that runs it on
//!   Shopify's Javy plugin ([`javy`]);
//! - a checkout UI extension (JavaScript, TypeScript, JSX with Preact or another runtime), into
//!   the script the page runs, as `shopify app build` bundles it ([`build_extension`]).
//!
//! The npm packages they import come from the app's `node_modules`, or from lsf's own copy of
//! those Shopify's templates use ([`packages`]).
//!
//! It writes what the CLI writes: the types of the `codegen` section of `package.json`
//! (`generated/api.ts`), `dist/function.js`, and the module at `[extensions.build] path`. Unlike
//! the CLI, it needs no `node_modules` (lsf has its own copy of what `@shopify/shopify_function`
//! puts in a Function) and no `schema.graphql`: without one, the typegen reads lsf's schema of the
//! Function's API version.

pub mod bundle;
pub mod javy;
pub mod packages;
pub mod resolve;
pub mod typegen;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::app::{FunctionExtension, UiExtension};

/// The entry of a JavaScript Function in `dir`, as Shopify's CLI finds it: `src/index.js`, else
/// `src/index.ts`.
pub fn entry(dir: &Path) -> Option<PathBuf> {
    ["src/index.js", "src/index.ts"]
        .into_iter()
        .map(|entry| dir.join(entry))
        .find(|entry| entry.is_file())
}

/// What a build wrote.
#[derive(Clone, Debug, PartialEq)]
pub struct Built {
    /// The module, at `[extensions.build] path`.
    pub module: PathBuf,
    /// `dist/function.js`, what Javy compiled.
    pub javascript: PathBuf,
    /// The types of the typegen, when `package.json` has a `codegen` section.
    pub types: Option<PathBuf>,
}

/// Builds the JavaScript Function `function` of the app in `app_dir`.
pub fn build(function: &FunctionExtension, app_dir: &Path) -> Result<Built, String> {
    let dir = &function.dir;
    let entry = function
        .javascript
        .clone()
        .or_else(|| entry(dir))
        .ok_or_else(|| "a JavaScript Function has `src/index.js` or `src/index.ts`".to_string())?;
    let package = read_package(dir)?;
    library(dir, &package)?;
    let exports = exports(function)?;

    let types = match plan(&package)? {
        Some(plan) => Some(types(function, &plan)?),
        None => None,
    };

    let env = environment(app_dir);
    let resolver = resolve::Resolver {
        aliases: vec![("user-function".to_string(), entry)],
        extensions: resolve::FUNCTION_EXTENSIONS,
    };
    let javascript = bundle::bundle(&bundle::Options {
        root: dir,
        entry: &function_entry(&exports),
        resolver: &resolver,
        env: &env,
        jsx_in_js: false,
        format: bundle::Format::Module,
        minify: false,
    })?;
    let exports: Vec<&str> = exports.iter().map(String::as_str).collect();
    let module = javy::module(&javascript, &exports)?;

    let javascript_path = dir.join("dist/function.js");
    write(&javascript_path, javascript.as_bytes())?;
    write(&function.wasm_path, &module)?;
    Ok(Built {
        module: function.wasm_path.clone(),
        javascript: javascript_path,
        types,
    })
}

/// Builds the checkout UI extension `extension` of the app in `app_dir`, as `shopify app build`
/// bundles it: `dist/<handle>.js`, a minified script. Returns where it wrote it.
///
/// What is not built as Shopify's CLI builds it: the script is not lowered to ES2015 (the CLI's
/// target; the page runs in a browser that needs none of it), a `.graphql` import is an error,
/// and a target's `should_render` module is not built (the page does not run it).
pub fn build_extension(extension: &UiExtension, app_dir: &Path) -> Result<PathBuf, String> {
    let modules: Vec<(&str, &str)> = extension
        .targets
        .iter()
        .filter_map(|target| Some((target.target.as_str(), target.module.as_deref()?)))
        .collect();
    if modules.is_empty() {
        return Err("no target names its source (`module`): there is nothing to build".to_string());
    }
    let mut env = environment_of(app_dir);
    // What `shopify app build` gives: a build for production.
    env.entry("NODE_ENV".to_string())
        .or_insert_with(|| "production".to_string());
    let resolver = resolve::Resolver {
        aliases: Vec::new(),
        extensions: resolve::UI_EXTENSIONS,
    };
    let script = bundle::bundle(&bundle::Options {
        root: &extension.dir,
        entry: &extension_entry(extension.api_version.as_deref(), &modules),
        resolver: &resolver,
        env: &env,
        jsx_in_js: true,
        format: bundle::Format::Script,
        minify: true,
    })?;
    write(&extension.bundle, script.as_bytes())?;
    Ok(extension.bundle.clone())
}

/// The entry Shopify's CLI generates for a checkout UI extension: from API version 2025-10, each
/// target's module is imported and its default export registered for the target with
/// `shopify.extend`; before, each module is imported, and registers itself.
pub fn extension_entry(api_version: Option<&str>, modules: &[(&str, &str)]) -> String {
    let registers = api_version
        .and_then(|version| {
            let (year, month) = version.split_once('-')?;
            Some((year.parse::<u32>().ok()?, month.parse::<u32>().ok()?))
        })
        .is_some_and(|(year, month)| year > 2025 || year == 2025 && month >= 10);
    let lines: Vec<String> = modules
        .iter()
        .enumerate()
        .map(|(index, (target, module))| {
            if registers {
                format!(
                    "import Target_{index} from '{module}';shopify.extend('{target}', (...args) => Target_{index}(...args));"
                )
            } else {
                format!("import '{module}';")
            }
        })
        .collect();
    lines.join("\n")
}

/// The entry Shopify's CLI generates for a Function and hands esbuild as `stdin`: for each export
/// of its targets (`cart-lines-discounts-generate-run`), a function of that name in camelCase
/// that hands the user's function of the same name to `@shopify/shopify_function/run`.
pub fn function_entry(exports: &[String]) -> String {
    let functions: Vec<String> = exports
        .iter()
        .map(|export| {
            let name = javy::camel_case(export);
            let run = javy::camel_case(&format!("run-{export}"));
            format!(
                "\nimport {{ {name} as {run} }} from \"user-function\"\nexport function {name}() {{ return __runFunction({run}) }}"
            )
        })
        .collect();
    format!(
        "\nimport __runFunction from \"@shopify/shopify_function/run\"\n{}",
        functions.join("\n")
    )
}

/// What the build of the Function in `dir` writes besides `dist/`, relative to `dir`: the file of
/// its typegen. The watch does not build again when they change.
pub fn outputs(dir: &Path) -> Vec<PathBuf> {
    let Ok(package) = read_package(dir) else {
        return Vec::new();
    };
    match plan(&package) {
        Ok(Some(plan)) => vec![resolve::normalize(Path::new(&plan.output))],
        _ => Vec::new(),
    }
}

/// The typegen of the `codegen` section of `package.json`; `None` without one.
fn plan(package: &Value) -> Result<Option<typegen::Plan>, String> {
    if package.get("codegen").is_none() {
        return Ok(None);
    }
    typegen::Plan::from_package_json(package)
}

/// `package.json` of the Function, `null` when it has none.
fn read_package(dir: &Path) -> Result<Value, String> {
    let path = dir.join("package.json");
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|error| format!("package.json is not valid JSON: {error}")),
        Err(_) => Ok(Value::Null),
    }
}

/// The library a JavaScript Function is written against: `@shopify/shopify_function` 2.x, the
/// one Shopify's CLI builds with Javy's plugin version 4. Its version is the installed one, or
/// the one `package.json` asks for.
fn library(dir: &Path, package: &Value) -> Result<(), String> {
    const NAME: &str = "@shopify/shopify_function";
    let installed = dir.ancestors().find_map(|dir| {
        let text =
            std::fs::read_to_string(dir.join("node_modules").join(NAME).join("package.json"))
                .ok()?;
        let manifest: Value = serde_json::from_str(&text).ok()?;
        manifest["version"].as_str().map(str::to_string)
    });
    let version = installed.or_else(|| {
        ["dependencies", "devDependencies"]
            .iter()
            .find_map(|field| package[field][NAME].as_str().map(str::to_string))
    });
    let Some(version) = version else {
        return Err(format!(
            "add `\"{NAME}\": \"2.0.1\"` to the dependencies of package.json (lsf does not need it installed)"
        ));
    };
    let major: Option<u64> = version
        .trim_start_matches(|c: char| !c.is_ascii_digit())
        .split(|c: char| !c.is_ascii_digit())
        .next()
        .and_then(|major| major.parse().ok());
    match major {
        Some(2) => Ok(()),
        _ => Err(format!(
            "lsf builds Functions written with {NAME} 2.x, and this one asks for `{version}`: \
             update it, or build it with `shopify app build`"
        )),
    }
}

/// The exports of the Function's targets, in order, each once: kebab-case, as Shopify's CLI
/// requires.
fn exports(function: &FunctionExtension) -> Result<Vec<String>, String> {
    let mut exports: Vec<String> = Vec::new();
    for target in &function.targets {
        let export = &target.export;
        let kebab = !export.is_empty()
            && export
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
        if !kebab {
            return Err(format!(
                "the export `{export}` of {} is not kebab-case (lowercase letters, digits and \
                 hyphens), as Shopify's CLI requires of a JavaScript Function",
                target.target
            ));
        }
        if !exports.contains(export) {
            exports.push(export.clone());
        }
    }
    if exports.is_empty() {
        return Err("the Function has no target".to_string());
    }
    Ok(exports)
}

/// Writes the types of the typegen, when they changed: the Function's `schema.graphql`, or lsf's
/// schema of its API version, and the documents of the plan.
fn types(function: &FunctionExtension, plan: &typegen::Plan) -> Result<PathBuf, String> {
    let dir = &function.dir;
    let schema = match std::fs::read_to_string(dir.join(&plan.schema)) {
        Ok(schema) => schema,
        Err(_) => {
            let api = function
                .targets
                .iter()
                .find_map(|target| target.spec())
                .map(|spec| spec.api);
            match (function.version, api) {
                (Some(version), Some(api)) => version.schema_source(api).to_string(),
                _ => {
                    return Err(format!(
                        "no {} for the typegen: run `shopify app function schema`",
                        plan.schema
                    ));
                }
            }
        }
    };
    let mut documents = Vec::new();
    for path in documents_of(dir, &plan.documents)? {
        let text = std::fs::read_to_string(dir.join(&path))
            .map_err(|error| format!("cannot read {path}: {error}"))?;
        documents.push((path, text));
    }
    let types = typegen::generate(&schema, &documents, &plan.options)?;
    let path = resolve::normalize(&dir.join(&plan.output));
    if std::fs::read_to_string(&path).ok().as_deref() != Some(types.as_str()) {
        write(&path, types.as_bytes())?;
    }
    Ok(path)
}

/// The files graphql-codegen's `documents` globs name, relative to `dir`: those the patterns
/// match, but those a `!pattern` matches. `{a,b}` is either; other glob syntax is an error
/// rather than a different set of files.
pub fn documents_of(dir: &Path, patterns: &[String]) -> Result<Vec<String>, String> {
    let mut included = std::collections::BTreeSet::new();
    let mut excluded = std::collections::BTreeSet::new();
    for pattern in patterns {
        let (set, pattern) = match pattern.strip_prefix('!') {
            Some(pattern) => (&mut excluded, pattern),
            None => (&mut included, pattern.as_str()),
        };
        if pattern.contains(['[', ']', '(', ')']) {
            return Err(format!(
                "the documents glob `{pattern}` uses a syntax lsf does not read (only `*`, `**`, \
                 `?`, `{{a,b}}` and `!`)"
            ));
        }
        for pattern in braces(pattern) {
            set.extend(glob(dir, &pattern));
        }
    }
    Ok(included.difference(&excluded).cloned().collect())
}

/// The patterns `{a,b}` stands for: `src/{a,b}.graphql` is `src/a.graphql` and `src/b.graphql`.
fn braces(pattern: &str) -> Vec<String> {
    let Some(open) = pattern.find('{') else {
        return vec![pattern.to_string()];
    };
    let Some(close) = pattern[open..].find('}').map(|close| open + close) else {
        return vec![pattern.to_string()];
    };
    let (head, tail) = (&pattern[..open], &pattern[close + 1..]);
    pattern[open + 1..close]
        .split(',')
        .flat_map(|choice| braces(&format!("{head}{choice}{tail}")))
        .collect()
}

/// The files of `dir` a glob matches (`src/*.graphql`, `src/**/*.graphql`, `../shared/*.graphql`),
/// relative to `dir` with `/`, sorted. `node_modules` and hidden folders are not searched.
pub fn glob(dir: &Path, pattern: &str) -> Vec<String> {
    let pattern: Vec<&str> = pattern
        .trim_start_matches("./")
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    let mut found = Vec::new();
    walk(dir, String::new(), &pattern, &mut found);
    found.sort();
    found.dedup();
    found
}

fn walk(dir: &Path, prefix: String, pattern: &[&str], found: &mut Vec<String>) {
    let Some((first, rest)) = pattern.split_first() else {
        return;
    };
    if *first == ".." {
        walk(&dir.join(".."), format!("{prefix}../"), rest, found);
        return;
    }
    if *first == "**" {
        // Zero folders, then one more.
        walk(dir, prefix.clone(), rest, found);
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let path = format!("{prefix}{name}");
        let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        if *first == "**" {
            if is_dir && !name.starts_with('.') && name != "node_modules" {
                walk(&entry.path(), format!("{path}/"), pattern, found);
            }
            continue;
        }
        if !matches(first, &name) {
            continue;
        }
        if rest.is_empty() {
            if !is_dir {
                found.push(path);
            }
        } else if is_dir {
            walk(&entry.path(), format!("{path}/"), rest, found);
        }
    }
}

/// Whether `name` matches one segment of a glob: `*` any characters, `?` one.
fn matches(pattern: &str, name: &str) -> bool {
    fn from(pattern: &[char], name: &[char]) -> bool {
        match pattern.split_first() {
            None => name.is_empty(),
            Some(('*', rest)) => (0..=name.len()).any(|skip| from(rest, &name[skip..])),
            Some(('?', rest)) => !name.is_empty() && from(rest, &name[1..]),
            Some((c, rest)) => name.first() == Some(c) && from(rest, &name[1..]),
        }
    }
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();
    from(&pattern, &name)
}

/// The values of `process.env` in a Function, as Shopify's CLI gives them to esbuild: those of
/// [`environment_of`], and `NODE_ENV` `development` when neither sets it (esbuild's default).
fn environment(app_dir: &Path) -> BTreeMap<String, String> {
    let mut env = environment_of(app_dir);
    env.entry("NODE_ENV".to_string())
        .or_insert_with(|| "development".to_string());
    env
}

/// The app's `.env`, then the environment (variables named as identifiers), as Shopify's CLI
/// gives them to esbuild.
fn environment_of(app_dir: &Path) -> BTreeMap<String, String> {
    let mut env = std::fs::read_to_string(app_dir.join(".env"))
        .map(|text| dotenv(&text))
        .unwrap_or_default();
    for (name, value) in std::env::vars() {
        let identifier = name.chars().enumerate().all(|(index, c)| {
            c == '_' || c == '$' || c.is_ascii_alphabetic() || (index > 0 && c.is_ascii_digit())
        });
        if identifier && !name.is_empty() {
            env.insert(name, value);
        }
    }
    env
}

/// The variables of a `.env` file, as the dotenv library of Shopify's CLI reads them: `NAME=value`
/// lines, `export` before the name allowed, `#` comments. A value in quotes is what the quotes
/// hold (`\n` is a new line in double quotes); one without ends at a `#`.
fn dotenv(text: &str) -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        let quoted = value
            .chars()
            .next()
            .filter(|c| matches!(c, '"' | '\'' | '`'));
        let value =
            match quoted.and_then(|quote| closing(&value[1..], quote).map(|end| (quote, end))) {
                Some(('"', end)) => value[1..1 + end].replace("\\n", "\n").replace("\\r", "\r"),
                Some((_, end)) => value[1..1 + end].to_string(),
                None => value
                    .split('#')
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_string(),
            };
        env.insert(name.trim().to_string(), value);
    }
    env
}

/// Where the quote that closes a value is, in what follows the opening one: the first not after
/// a backslash.
fn closing(text: &str, quote: char) -> Option<usize> {
    let mut escaped = false;
    for (index, c) in text.char_indices() {
        match c {
            '\\' if !escaped => escaped = true,
            c if c == quote && !escaped => return Some(index),
            _ => escaped = false,
        }
    }
    None
}

/// Writes a file whole: to a file next to it, renamed over it, so that nothing reads it half
/// written.
fn write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let failed = |error: std::io::Error| format!("cannot write {}: {error}", path.display());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(failed)?;
    }
    let mut partial = path.as_os_str().to_owned();
    partial.push(".partial");
    let partial = PathBuf::from(partial);
    std::fs::write(&partial, bytes).map_err(failed)?;
    std::fs::rename(&partial, path).map_err(failed)
}

#[cfg(test)]
mod tests;
