//! Whether the module of a Function was built from its files as they are now: a module older
//! than its sources runs the old code. `lsf app function run` builds such a Function first, as
//! `shopify app function run` builds before it runs, and `lsf app check` fails on it unless
//! `--build`.
//!
//! Each build lsf makes (`app build`, `app function build`, `--build`, `--watch`) records what
//! the module was built from: a hash of each file of the Function's folder and of the app's
//! files every build may read (a workspace's manifest and lock file, a crate or a package the
//! Functions share, as the watch follows them), and of the module, in the user's cache folder,
//! never in the app. A lock file the build itself writes is taken as the build left it. A module that is the one recorded is stale when
//! a file changed since, whatever the times of the files say (a `git checkout` touches them all;
//! a build that compiles nothing leaves the module's time). A module lsf did not build (Shopify's
//! CLI, a build by hand, a fresh clone) is stale when a file is newer than it by more than
//! [`SLACK`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use lsf_app::config::FunctionExtension;
use serde_json::{Value as Json, json};

/// How much newer than a module lsf did not build a file must be to count as changed: a clone
/// writes a module and its sources in the same moment, in no set order.
const SLACK: Duration = Duration::from_secs(2);

/// The files a Function is built from, by their path in its folder, with a hash of each.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sources {
    files: BTreeMap<String, String>,
}

/// Whether a file of a Function's folder is not one it is built from: what the watch leaves
/// alone ([`crate::watch::ignored`]: dependencies, hidden files, `schema.graphql`, what the
/// build writes), and its tests (`tests/`, `*.test.ts`), which a run of `--save-input` adds to.
fn skipped(relative: &Path, outputs: &[PathBuf]) -> bool {
    if crate::watch::ignored(relative, outputs) || relative.starts_with("tests") {
        return true;
    }
    relative.file_name().is_some_and(|name| {
        let name = name.to_string_lossy();
        name.contains(".test.") || name.contains(".spec.")
    })
}

/// The prefix of the app's shared files among a Function's sources: `@app/Cargo.lock`.
const SHARED: &str = "@app/";

/// The lock files a build may write or update itself (`cargo build` writes `Cargo.lock`).
const LOCK_FILES: [&str; 7] = [
    "Cargo.lock",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
];

/// Whether a source, by its path, is a lock file.
fn is_lock_file(path: &str) -> bool {
    LOCK_FILES
        .iter()
        .any(|lock| path == *lock || path.ends_with(&format!("/{lock}")))
}

/// Each file `function`, of the app in `root`, is built from, with its time.
fn files(root: &Path, function: &FunctionExtension) -> Vec<(String, PathBuf, Option<SystemTime>)> {
    files_in(root, &function.dir, &crate::watch::outputs_of(function))
}

/// Each file of the folder `dir` of a Function whose build writes `outputs`, with its time,
/// then the files of the app in `root` that every build may read ([`SHARED`]).
fn files_in(
    root: &Path,
    dir: &Path,
    outputs: &[PathBuf],
) -> Vec<(String, PathBuf, Option<SystemTime>)> {
    let mut found = own_files(dir, outputs);
    found.extend(shared_files(root));
    found
}

/// The files of the app in `root`, out of `extensions/`, that the builds of its Functions may
/// read: what the watch builds every Function again for.
fn shared_files(root: &Path) -> Vec<(String, PathBuf, Option<SystemTime>)> {
    let mut found = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        if name == "extensions" || !crate::watch::shared_input(Path::new(&name), &path, root, None)
        {
            continue;
        }
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if !metadata.is_dir() {
            found.push((format!("{SHARED}{name}"), path, metadata.modified().ok()));
            continue;
        }
        crate::watch::walk(
            &path,
            &|relative| crate::watch::ignored(relative, &[]),
            &mut |relative, file, metadata| {
                found.push((
                    format!(
                        "{SHARED}{name}/{}",
                        relative.to_string_lossy().replace('\\', "/")
                    ),
                    file.to_path_buf(),
                    metadata.modified().ok(),
                ));
            },
            &mut |_| {},
        );
    }
    found
}

/// Each file of the Function's own folder `dir`, whose build writes `outputs`, with its time.
fn own_files(dir: &Path, outputs: &[PathBuf]) -> Vec<(String, PathBuf, Option<SystemTime>)> {
    let mut found = Vec::new();
    crate::watch::walk(
        dir,
        &|relative| skipped(relative, outputs),
        &mut |relative, path, metadata| {
            found.push((
                relative.to_string_lossy().replace('\\', "/"),
                path.to_path_buf(),
                metadata.modified().ok(),
            ));
        },
        &mut |_| {},
    );
    found
}

impl Sources {
    /// What a build read before it started (`self`), with what the build wrote itself, read
    /// once it is over (`after`): a file that was not there before, and the lock files it
    /// updates. A source edited during the build keeps its hash of before: it is not in the
    /// module.
    pub fn after_build(mut self, after: Sources) -> Sources {
        for (path, hash) in after.files {
            if is_lock_file(&path) || !self.files.contains_key(&path) {
                self.files.insert(path, hash);
            }
        }
        self
    }
}

/// What `function`, of the app in `root`, is built from, as it is now: read before a build,
/// recorded once it is over ([`Sources::after_build`]).
pub fn sources(root: &Path, function: &FunctionExtension) -> Sources {
    sources_in(root, &function.dir, &crate::watch::outputs_of(function))
}

/// [`sources`] of the Function in the folder `dir`, whose build writes `outputs`.
pub fn sources_in(root: &Path, dir: &Path, outputs: &[PathBuf]) -> Sources {
    Sources {
        files: files_in(root, dir, outputs)
            .into_iter()
            .filter_map(|(relative, path, _)| {
                let bytes = std::fs::read(path).ok()?;
                Some((relative, blake3::hash(&bytes).to_hex().to_string()))
            })
            .collect(),
    }
}

/// Where what the module of `handle`, of the app in `root`, was built from is recorded.
fn record_file(root: &Path, handle: &str) -> Option<PathBuf> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let key = blake3::hash(root.as_os_str().as_encoded_bytes()).to_hex();
    Some(
        lsf_core::util::user_cache_dir()?
            .join("lsf")
            .join("builds")
            .join(&key.as_str()[..16])
            .join(format!("{}.json", crate::checkout::encoded_handle(handle))),
    )
}

/// The hash of the module at `module`.
fn module_hash(module: &Path) -> Option<String> {
    std::fs::read(module)
        .ok()
        .map(|bytes| blake3::hash(&bytes).to_hex().to_string())
}

/// Records that the module at `module` of the Function `handle`, of the app in `root`, was
/// built from `sources`. Nothing is recorded where there is no cache folder.
pub fn record(root: &Path, handle: &str, module: &Path, sources: &Sources) {
    let (Some(file), Some(module)) = (record_file(root, handle), module_hash(module)) else {
        return;
    };
    let Some(parent) = file.parent() else {
        return;
    };
    let json = json!({"handle": handle, "module": module, "files": sources.files});
    let partial = file.with_extension(format!("{}.partial", std::process::id()));
    let written = std::fs::create_dir_all(parent).is_ok()
        && std::fs::write(&partial, json.to_string()).is_ok()
        && std::fs::rename(&partial, &file).is_ok();
    if !written {
        let _ = std::fs::remove_file(&partial);
    }
}

/// The sources the module of `function` was recorded to be built from, when it is that module.
fn recorded(root: &Path, function: &FunctionExtension) -> Option<Sources> {
    let json: Json =
        serde_json::from_slice(&std::fs::read(record_file(root, &function.handle)?).ok()?).ok()?;
    if json["handle"] != function.handle.as_str()
        || json["module"].as_str()? != module_hash(&function.wasm_path)?
    {
        return None;
    }
    Some(Sources {
        files: json["files"]
            .as_object()?
            .iter()
            .map(|(path, hash)| Some((path.clone(), hash.as_str()?.to_string())))
            .collect::<Option<_>>()?,
    })
}

/// A file of `function` that changed after its module was built, by its path in the Function's
/// folder (`src/run.ts`), or `None` when the module is up to date or missing (which is another
/// problem).
pub fn stale(root: &Path, function: &FunctionExtension) -> Option<String> {
    let built = std::fs::metadata(&function.wasm_path)
        .and_then(|metadata| metadata.modified())
        .ok()?;
    if let Some(recorded) = recorded(root, function) {
        let now = sources(root, function);
        return now
            .files
            .iter()
            .find(|(path, hash)| recorded.files.get(*path) != Some(*hash))
            .map(|(path, _)| path.clone())
            .or_else(|| {
                // A file removed since.
                recorded
                    .files
                    .keys()
                    .find(|path| !now.files.contains_key(*path))
                    .cloned()
            });
    }
    files(root, function)
        .into_iter()
        .filter_map(|(relative, _, modified)| Some((modified?, relative)))
        .filter(|(modified, _)| {
            modified
                .duration_since(built)
                .is_ok_and(|newer| newer > SLACK)
        })
        .max()
        .map(|(_, relative)| relative)
}

/// How a message names the file [`stale`] found: by its path in the app
/// (`extensions/discount/src/run.ts`, or `Cargo.lock` for a file the Functions share).
pub fn in_app(root: &Path, function: &FunctionExtension, file: &str) -> String {
    if let Some(shared) = file.strip_prefix(SHARED) {
        return shared.to_string();
    }
    let folder = function
        .dir
        .strip_prefix(root)
        .unwrap_or(&function.dir)
        .to_string_lossy()
        .replace('\\', "/");
    format!("{folder}/{file}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A JavaScript Function in a folder of its own, with a module.
    fn function(label: &str) -> (PathBuf, FunctionExtension) {
        let root =
            std::env::temp_dir().join(format!("lsf-freshness-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dir = root.join("extensions/discount");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("dist")).unwrap();
        std::fs::write(dir.join("src/run.ts"), "export function run() {}").unwrap();
        std::fs::write(dir.join("src/run.graphql"), "query Input { cart { cost } }").unwrap();
        std::fs::write(
            dir.join("shopify.extension.toml"),
            "api_version = \"2026-10\"",
        )
        .unwrap();
        std::fs::write(dir.join("dist/function.wasm"), b"\0asm").unwrap();
        let root = root.canonicalize().unwrap();
        let dir = root.join("extensions/discount");
        let function = FunctionExtension {
            handle: format!("discount-{label}"),
            name: None,
            dir: dir.clone(),
            api_version: Some("2026-10".to_string()),
            version: None,
            targets: Vec::new(),
            build_command: None,
            javascript: Some(dir.join("src/run.ts")),
            wasm_path: dir.join("dist/function.wasm"),
            input_variables: None,
        };
        (root, function)
    }

    fn age(path: &Path, seconds: u64) {
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(seconds))
            .unwrap();
    }

    #[test]
    fn a_module_lsf_built_is_stale_once_a_file_changes() {
        let (root, function) = function("recorded");
        let before = sources(&root, &function);
        record(&root, &function.handle, &function.wasm_path, &before);
        assert_eq!(stale(&root, &function), None);
        // Touched, not changed: a checkout of another branch and back.
        age(&function.wasm_path, 60);
        assert_eq!(stale(&root, &function), None);
        // A test fixture added next to it is not what it is built from.
        std::fs::create_dir_all(function.dir.join("tests/fixtures")).unwrap();
        std::fs::write(function.dir.join("tests/fixtures/a.json"), "{}").unwrap();
        assert_eq!(stale(&root, &function), None);
        std::fs::write(
            function.dir.join("src/run.graphql"),
            "query Input { cart { id } }",
        )
        .unwrap();
        assert_eq!(stale(&root, &function).as_deref(), Some("src/run.graphql"));
        assert_eq!(
            in_app(&root, &function, "src/run.graphql"),
            "extensions/discount/src/run.graphql"
        );
        // Built again: up to date.
        record(
            &root,
            &function.handle,
            &function.wasm_path,
            &sources(&root, &function),
        );
        assert_eq!(stale(&root, &function), None);
        std::fs::remove_file(function.dir.join("src/run.graphql")).unwrap();
        assert_eq!(stale(&root, &function).as_deref(), Some("src/run.graphql"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_module_lsf_did_not_build_is_stale_when_a_file_is_newer() {
        let (root, function) = function("times");
        // Written in the same moment, as a clone writes them: not stale.
        assert_eq!(stale(&root, &function), None);
        age(&function.wasm_path, 60);
        let found = stale(&root, &function);
        assert!(found.is_some(), "{found:?}");
        // What the build writes is not a source.
        age(&function.dir.join("src/run.ts"), 120);
        age(&function.dir.join("src/run.graphql"), 120);
        age(&function.dir.join("shopify.extension.toml"), 120);
        std::fs::write(function.dir.join("dist/function.js"), "").unwrap();
        assert_eq!(stale(&root, &function), None);
        // A module replaced by another build is no longer the one recorded.
        record(
            &root,
            &function.handle,
            &function.wasm_path,
            &sources(&root, &function),
        );
        std::fs::write(&function.wasm_path, b"\0asm\x01").unwrap();
        std::fs::write(
            function.dir.join("src/run.ts"),
            "export function run() { 1 }",
        )
        .unwrap();
        assert_eq!(stale(&root, &function), None);
        // No module: another problem.
        std::fs::remove_file(&function.wasm_path).unwrap();
        assert_eq!(stale(&root, &function), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
