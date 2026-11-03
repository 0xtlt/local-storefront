//! Where an `import` or a `require` of an extension's JavaScript leads, as esbuild resolves it
//! when Shopify's CLI bundles a Function or a checkout UI extension (its default `browser`
//! platform): aliases, relative paths with the extensions the build tries, `index` files,
//! packages in `node_modules` with their `exports`, `module` or `main`. A package no
//! `node_modules` of the app has comes from lsf's own copy ([`super::packages`]), when it is one
//! of those.

use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use super::packages;

/// The extensions esbuild tries after a path that names no file, for a Function (its default).
pub const FUNCTION_EXTENSIONS: &[&str] = &[".tsx", ".ts", ".jsx", ".js", ".css", ".json"];

/// The extensions Shopify's CLI has esbuild try for a checkout UI extension (no `.jsx`).
pub const UI_EXTENSIONS: &[&str] = &[".tsx", ".ts", ".js", ".json", ".esnext", ".mjs", ".ejs"];

/// Whether the import is an `import` (or `export … from`) or a `require`: the condition
/// `exports` maps of packages are read with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Import,
    Require,
}

/// How a build resolves imports.
#[derive(Clone, Debug)]
pub struct Resolver {
    /// Specifiers that stand for a file, as esbuild's `alias`: `user-function` is the entry of a
    /// Function.
    pub aliases: Vec<(String, PathBuf)>,
    /// The extensions tried after a path that names no file, in order.
    pub extensions: &'static [&'static str],
}

impl Resolver {
    /// The file `specifier`, imported from a file of `from_dir`, leads to. Paths are as found,
    /// not canonicalized: two paths to one file are one module only if they are written the
    /// same.
    pub fn resolve(&self, specifier: &str, from_dir: &Path, kind: Kind) -> Result<PathBuf, String> {
        if let Some((_, path)) = self.aliases.iter().find(|(alias, _)| alias == specifier) {
            return self
                .file_or_directory(path)
                .ok_or_else(|| format!("cannot find `{}` (`{specifier}`)", path.display()));
        }
        if specifier.starts_with("./")
            || specifier.starts_with("../")
            || specifier == "."
            || specifier == ".."
            || Path::new(specifier).is_absolute()
        {
            let path = normalize(&from_dir.join(specifier));
            return self
                .file_or_directory(&path)
                .ok_or_else(|| format!("cannot find `{specifier}`"));
        }
        let (name, subpath) = split_package(specifier)
            .ok_or_else(|| format!("`{specifier}` is not a path nor a package name"))?;
        for dir in from_dir.ancestors() {
            let root = dir.join("node_modules").join(name);
            if root.is_dir() {
                return self.package(specifier, &root, name, subpath, kind);
            }
        }
        // A package the app does not have, but lsf does.
        let ours = packages::root()?.join(name);
        if ours.is_dir() {
            return self.package(specifier, &ours, name, subpath, kind);
        }
        if is_builtin(specifier) {
            return Err(format!(
                "cannot import `{specifier}`: it is a module of Node, and an extension does not run on Node"
            ));
        }
        Err(format!(
            "cannot find the package of `{specifier}` in a node_modules folder: install it"
        ))
    }
}

impl Resolver {
    /// The file `path` names, with one of the extensions, or the `index` of the directory.
    fn file_or_directory(&self, path: &Path) -> Option<PathBuf> {
        self.file(path).or_else(|| self.directory(path))
    }

    fn file(&self, path: &Path) -> Option<PathBuf> {
        if path.is_file() {
            return Some(path.to_path_buf());
        }
        let text = path.to_str()?;
        for extension in self.extensions {
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

    fn directory(&self, path: &Path) -> Option<PathBuf> {
        if !path.is_dir() {
            return None;
        }
        if let Some(manifest) = read_manifest(&path.join("package.json"))
            && let Some(main) = main_field(&manifest)
            && let Some(found) = self.file_or_directory(&normalize(&path.join(main)))
        {
            return Some(found);
        }
        self.file(&path.join("index"))
    }

    /// The file of `subpath` in the package at `root`: through its `exports` when it has them.
    fn package(
        &self,
        specifier: &str,
        root: &Path,
        name: &str,
        subpath: &str,
        kind: Kind,
    ) -> Result<PathBuf, String> {
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
            return self.file(&path).ok_or_else(|| {
                format!(
                    "`{specifier}` leads to {}, which is missing",
                    path.display()
                )
            });
        }
        let found = if subpath.is_empty() {
            self.directory(root)
        } else {
            self.file_or_directory(&normalize(&root.join(subpath)))
        };
        found.ok_or_else(|| format!("cannot find `{specifier}` in {}", root.display()))
    }
}

/// The entry a package's `browser` (when a path), `module` or `main` names, in esbuild's order
/// for the browser.
fn main_field(manifest: &Value) -> Option<&str> {
    ["browser", "module", "main"]
        .into_iter()
        .find_map(|field| manifest.get(field).and_then(Value::as_str))
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
