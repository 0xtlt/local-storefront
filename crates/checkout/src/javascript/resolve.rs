//! Where an `import` or a `require` of a Function's JavaScript leads, as esbuild resolves it when
//! Shopify's CLI bundles a Function (its default `browser` platform): relative paths with the
//! extensions it tries, `index` files, packages in `node_modules` with their `exports`, `module`
//! or `main`. `@shopify/shopify_function/run`, the library that reads the input and writes the
//! output, needs no `node_modules`: lsf has a copy of it.

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

/// The extensions tried after a path that names no file, in esbuild's order.
const EXTENSIONS: [&str; 5] = [".tsx", ".ts", ".jsx", ".js", ".json"];

/// The library a Function's entry imports to read its input and write its output. lsf uses the
/// one in `node_modules` when there is one, as esbuild does, and its own copy otherwise.
pub const RUN: &str = "@shopify/shopify_function/run";

/// `run.ts` of `@shopify/shopify_function` 2.0.1 (Apache-2.0, Shopify), which esbuild bundles
/// into every JavaScript Function.
pub const RUN_SOURCE: &str = r#"export type UserFunction<Input extends {}, Output extends {}> = (
  input: Input
) => Output;

interface ShopifyFunction {
  readInput(): any;
  writeOutput(val: any);
}

declare global {
  const ShopifyFunction: ShopifyFunction;
}

export default function <I extends {}, O extends {}>(
  userfunction: UserFunction<I, O>
) {
  try {
    ShopifyFunction;
  } catch (e) {
    throw new Error(
      "ShopifyFunction is not defined. Please rebuild your function using the latest version of Shopify CLI."
    );
  }

  const input_obj = ShopifyFunction.readInput();
  const output_obj = userfunction(input_obj);
  ShopifyFunction.writeOutput(output_obj);
}
"#;

/// What an import leads to.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Resolved {
    /// A file, its path as found (not canonicalized: two paths to one file are one module only
    /// if they are written the same).
    File(PathBuf),
    /// lsf's copy of `@shopify/shopify_function/run`.
    Run,
}

/// Whether the import is an `import` (or `export … from`) or a `require`: the condition
/// `exports` maps of packages are read with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Import,
    Require,
}

/// Where `specifier`, imported from a file of `from_dir`, leads.
pub fn resolve(specifier: &str, from_dir: &Path, kind: Kind) -> Result<Resolved, String> {
    if specifier.starts_with("./")
        || specifier.starts_with("../")
        || specifier == "."
        || specifier == ".."
        || Path::new(specifier).is_absolute()
    {
        let path = normalize(&from_dir.join(specifier));
        return file_or_directory(&path)
            .map(Resolved::File)
            .ok_or_else(|| format!("cannot find `{specifier}`"));
    }
    match package(specifier, from_dir, kind)? {
        Some(path) => Ok(Resolved::File(path)),
        None if specifier == RUN || specifier == "@shopify/shopify_function/run.ts" => {
            Ok(Resolved::Run)
        }
        // A package of npm by the same name comes first (`events`, `buffer`), as with esbuild.
        None if is_builtin(specifier) => Err(format!(
            "cannot import `{specifier}`: it is a module of Node, and a Function does not run on Node"
        )),
        None => Err(format!(
            "cannot find the package of `{specifier}` in a node_modules folder: install it"
        )),
    }
}

/// The file `path` names, with one of [`EXTENSIONS`], or the `index` of the directory.
fn file_or_directory(path: &Path) -> Option<PathBuf> {
    file(path).or_else(|| directory(path))
}

fn file(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    let text = path.to_str()?;
    for extension in EXTENSIONS {
        let candidate = PathBuf::from(format!("{text}{extension}"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    // TypeScript imports `./a.js` for `./a.ts`: esbuild tries the TypeScript file.
    let rewrites: [(&str, &[&str]); 4] = [
        (".js", &[".ts", ".tsx"]),
        (".jsx", &[".ts", ".tsx"]),
        (".mjs", &[".mts"]),
        (".cjs", &[".cts"]),
    ];
    for (javascript, typescript) in rewrites {
        if let Some(stem) = text.strip_suffix(javascript) {
            for extension in typescript {
                let candidate = PathBuf::from(format!("{stem}{extension}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

fn directory(path: &Path) -> Option<PathBuf> {
    if !path.is_dir() {
        return None;
    }
    if let Some(manifest) = read_manifest(&path.join("package.json"))
        && let Some(main) = main_field(&manifest)
        && let Some(found) = file_or_directory(&normalize(&path.join(main)))
    {
        return Some(found);
    }
    file(&path.join("index"))
}

/// The entry a package's `browser` (when a path), `module` or `main` names, in esbuild's order
/// for the browser.
fn main_field(manifest: &Value) -> Option<&str> {
    ["browser", "module", "main"]
        .into_iter()
        .find_map(|field| manifest.get(field).and_then(Value::as_str))
}

/// The file of a bare specifier (`lodash/fp`, `@scope/name/sub`) in the closest `node_modules`
/// that has the package. `None` when no `node_modules` has it.
fn package(specifier: &str, from_dir: &Path, kind: Kind) -> Result<Option<PathBuf>, String> {
    let (name, subpath) = split_package(specifier)
        .ok_or_else(|| format!("`{specifier}` is not a path nor a package name"))?;
    for dir in from_dir.ancestors() {
        let root = dir.join("node_modules").join(name);
        if !root.is_dir() {
            continue;
        }
        let manifest = read_manifest(&root.join("package.json"));
        if let Some(exports) = manifest
            .as_ref()
            .and_then(|manifest| manifest.get("exports"))
        {
            let wanted = if subpath.is_empty() {
                ".".to_string()
            } else {
                format!("./{subpath}")
            };
            let target = exported(exports, &wanted, kind).ok_or_else(|| {
                format!("the package `{name}` does not export `{wanted}` (its `exports`)")
            })?;
            let path = normalize(&root.join(target));
            return file(&path).map(Some).ok_or_else(|| {
                format!(
                    "`{specifier}` leads to {}, which is missing",
                    path.display()
                )
            });
        }
        let found = if subpath.is_empty() {
            directory(&root)
        } else {
            file_or_directory(&normalize(&root.join(subpath)))
        };
        return found
            .map(Some)
            .ok_or_else(|| format!("cannot find `{specifier}` in {}", root.display()));
    }
    Ok(None)
}

/// `name` and the rest: `@scope/name/sub` is `@scope/name` and `sub`.
fn split_package(specifier: &str) -> Option<(&str, &str)> {
    let end = if specifier.starts_with('@') {
        let slash = specifier.find('/')?;
        specifier[slash + 1..]
            .find('/')
            .map_or(specifier.len(), |next| slash + 1 + next)
    } else {
        specifier.find('/').unwrap_or(specifier.len())
    };
    let name = &specifier[..end];
    let subpath = specifier[end..].trim_start_matches('/');
    (!name.is_empty() && name != "@").then_some((name, subpath))
}

/// The target `exports` gives `subpath` (`.` or `./sub`), with the conditions esbuild matches
/// for the browser.
fn exported(exports: &Value, subpath: &str, kind: Kind) -> Option<String> {
    let map = match exports {
        Value::Object(map) if map.keys().any(|key| key.starts_with('.')) => map,
        // A string, an array or conditions alone: the export of `.`.
        _ => return (subpath == ".").then(|| condition(exports, kind)).flatten(),
    };
    if let Some(target) = map.get(subpath) {
        return condition(target, kind);
    }
    // `./*` patterns: the longest prefix wins.
    let mut best: Option<(&str, &str, &Value)> = None;
    for (key, target) in map {
        let Some((prefix, suffix)) = key.split_once('*') else {
            continue;
        };
        if subpath.starts_with(prefix)
            && subpath.ends_with(suffix)
            && subpath.len() >= prefix.len() + suffix.len()
            && best.is_none_or(|(best_prefix, _, _)| prefix.len() > best_prefix.len())
        {
            best = Some((prefix, suffix, target));
        }
    }
    let (prefix, suffix, target) = best?;
    let matched = &subpath[prefix.len()..subpath.len() - suffix.len()];
    condition(target, kind).map(|target| target.replace('*', matched))
}

/// The path conditions lead to: the first key of an object that is an active condition, the
/// first entry of an array that leads somewhere.
fn condition(target: &Value, kind: Kind) -> Option<String> {
    match target {
        Value::String(path) => Some(path.clone()),
        Value::Array(targets) => targets.iter().find_map(|target| condition(target, kind)),
        Value::Object(conditions) => conditions.iter().find_map(|(name, target)| {
            let active = match name.as_str() {
                "default" | "browser" | "module" => true,
                "import" => kind == Kind::Import,
                "require" => kind == Kind::Require,
                _ => false,
            };
            active.then(|| condition(target, kind)).flatten()
        }),
        _ => None,
    }
}

fn read_manifest(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// The modules Node has and a browser does not.
fn is_builtin(specifier: &str) -> bool {
    const BUILTINS: [&str; 20] = [
        "assert",
        "buffer",
        "child_process",
        "crypto",
        "dns",
        "events",
        "fs",
        "http",
        "https",
        "net",
        "os",
        "path",
        "process",
        "querystring",
        "stream",
        "string_decoder",
        "timers",
        "tls",
        "url",
        "util",
    ];
    let name = specifier.strip_prefix("node:").unwrap_or(specifier);
    specifier.starts_with("node:") || BUILTINS.contains(&name.split('/').next().unwrap_or(name))
}

/// `path` without `.` and with each `..` applied, without asking the file system.
pub fn normalize(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normal.pop() {
                    normal.push("..");
                }
            }
            other => normal.push(other),
        }
    }
    normal
}

#[cfg(test)]
mod tests;
