use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

/// A folder of `files` (path, content) in the temporary folder, removed with the value.
struct Folder(PathBuf);

impl Folder {
    fn new(files: &[(&str, &str)]) -> Folder {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "lsf-resolve-{}-{}",
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

    /// Where `specifier`, imported from `src/`, leads, relative to the folder.
    fn resolve(&self, specifier: &str, kind: Kind) -> Result<String, String> {
        resolve(specifier, &self.0.join("src"), kind).map(|resolved| match resolved {
            Resolved::File(path) => path
                .strip_prefix(&self.0)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/"),
            Resolved::Run => "(lsf's run)".to_string(),
        })
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn relative_paths_try_extensions_and_index_files() {
    let folder = Folder::new(&[
        ("src/a.ts", ""),
        ("src/b.js", ""),
        ("src/c.ts", ""),
        ("src/lib/index.ts", ""),
        ("src/data.json", "{}"),
        ("generated/api.ts", ""),
    ]);
    assert_eq!(folder.resolve("./a", Kind::Import).unwrap(), "src/a.ts");
    assert_eq!(folder.resolve("./b", Kind::Import).unwrap(), "src/b.js");
    // TypeScript imports `./c.js` for `./c.ts`.
    assert_eq!(folder.resolve("./c.js", Kind::Import).unwrap(), "src/c.ts");
    assert_eq!(
        folder.resolve("./lib", Kind::Import).unwrap(),
        "src/lib/index.ts"
    );
    assert_eq!(
        folder.resolve("./data.json", Kind::Import).unwrap(),
        "src/data.json"
    );
    assert_eq!(
        folder.resolve("../generated/api", Kind::Import).unwrap(),
        "generated/api.ts"
    );
    assert_eq!(
        folder.resolve("./missing", Kind::Import).unwrap_err(),
        "cannot find `./missing`"
    );
}

#[test]
fn packages_are_found_in_the_closest_node_modules() {
    let folder = Folder::new(&[
        (
            "node_modules/plain/package.json",
            r#"{ "main": "lib/main.js" }"#,
        ),
        ("node_modules/plain/lib/main.js", ""),
        ("node_modules/plain/extra.js", ""),
        (
            "node_modules/both/package.json",
            r#"{ "main": "main.cjs", "module": "module.mjs" }"#,
        ),
        ("node_modules/both/main.cjs", ""),
        ("node_modules/both/module.mjs", ""),
        ("node_modules/@scope/name/index.js", ""),
        ("src/node_modules/plain/index.js", ""),
    ]);
    // `src/node_modules` is closer than `node_modules`.
    assert_eq!(
        folder.resolve("plain", Kind::Import).unwrap(),
        "src/node_modules/plain/index.js"
    );
    assert_eq!(
        folder.resolve("both", Kind::Import).unwrap(),
        "node_modules/both/module.mjs"
    );
    assert_eq!(
        folder.resolve("@scope/name", Kind::Import).unwrap(),
        "node_modules/@scope/name/index.js"
    );
}

#[test]
fn exports_maps_are_read_with_the_conditions_of_the_import() {
    let folder = Folder::new(&[
        (
            "node_modules/pkg/package.json",
            r#"{
  "exports": {
    ".": { "node": "./node.js", "import": "./esm.js", "require": "./cjs.js" },
    "./utils/*": { "default": "./dist/utils/*.js" },
    "./utils/internal/*": null
  }
}"#,
        ),
        ("node_modules/pkg/esm.js", ""),
        ("node_modules/pkg/cjs.js", ""),
        ("node_modules/pkg/dist/utils/format.js", ""),
    ]);
    assert_eq!(
        folder.resolve("pkg", Kind::Import).unwrap(),
        "node_modules/pkg/esm.js"
    );
    assert_eq!(
        folder.resolve("pkg", Kind::Require).unwrap(),
        "node_modules/pkg/cjs.js"
    );
    assert_eq!(
        folder.resolve("pkg/utils/format", Kind::Import).unwrap(),
        "node_modules/pkg/dist/utils/format.js"
    );
    assert_eq!(
        folder
            .resolve("pkg/utils/internal/secret", Kind::Import)
            .unwrap_err(),
        "the package `pkg` does not export `./utils/internal/secret` (its `exports`)"
    );
    assert_eq!(
        folder.resolve("pkg/other", Kind::Import).unwrap_err(),
        "the package `pkg` does not export `./other` (its `exports`)"
    );
}

#[test]
fn the_run_library_is_lsfs_copy_without_node_modules() {
    let folder = Folder::new(&[("src/index.ts", "")]);
    assert_eq!(folder.resolve(RUN, Kind::Import).unwrap(), "(lsf's run)");
}

#[test]
fn node_modules_come_before_node_builtins() {
    let folder = Folder::new(&[("node_modules/events/index.js", "")]);
    assert_eq!(
        folder.resolve("events", Kind::Import).unwrap(),
        "node_modules/events/index.js"
    );
    assert_eq!(
        folder.resolve("node:fs", Kind::Import).unwrap_err(),
        "cannot import `node:fs`: it is a module of Node, and a Function does not run on Node"
    );
}
