//! Shopify's CLI, which lsf asks to build what Shopify's CLI builds itself: a Function written in
//! JavaScript or TypeScript without a build command (`shopify app function build`), and the
//! checkout UI extensions built from their sources (`shopify app build`). lsf builds none of it
//! itself: the modules and bundles are the CLI's, to the byte. It takes Node and Shopify's CLI,
//! as `shopify app dev` does.
//!
//! The CLI is, in this order: the command `LSF_SHOPIFY_CLI` names (`npx --yes @shopify/cli`), the
//! app's own (`node_modules/.bin/shopify`, which Shopify's app templates install), or `shopify` on
//! the `PATH`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// How to run Shopify's CLI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShopifyCli {
    program: PathBuf,
    /// What comes before the command (`--yes @shopify/cli` after `npx`).
    args: Vec<String>,
}

impl ShopifyCli {
    /// The CLI of the app at `root`.
    pub fn find(root: &Path) -> Result<ShopifyCli, String> {
        let configured = std::env::var("LSF_SHOPIFY_CLI").ok();
        Self::find_in(
            root,
            configured.as_deref(),
            std::env::var_os("PATH").as_deref(),
        )
    }

    /// The CLI of the app at `root`, with `LSF_SHOPIFY_CLI` and `PATH` as given.
    pub fn find_in(
        root: &Path,
        configured: Option<&str>,
        path: Option<&OsStr>,
    ) -> Result<ShopifyCli, String> {
        if let Some(configured) = configured.filter(|text| !text.trim().is_empty()) {
            let mut words = configured.split_whitespace().map(str::to_string);
            let program = PathBuf::from(words.next().expect("a word"));
            return Ok(ShopifyCli {
                program,
                args: words.collect(),
            });
        }
        let names: &[&str] = if cfg!(windows) {
            &["shopify.cmd", "shopify.exe", "shopify"]
        } else {
            &["shopify"]
        };
        let mut folders = vec![root.join("node_modules").join(".bin")];
        folders.extend(path.map(std::env::split_paths).into_iter().flatten());
        folders
            .iter()
            .flat_map(|folder| names.iter().map(move |name| folder.join(name)))
            .find(|candidate| candidate.is_file())
            .map(|program| ShopifyCli {
                program,
                args: Vec::new(),
            })
            .ok_or_else(|| {
                "this build takes Shopify's CLI, which lsf did not find (in LSF_SHOPIFY_CLI, the \
                 app's node_modules/.bin, or the PATH): install it (`npm install --global \
                 @shopify/cli`), or name one (`LSF_SHOPIFY_CLI=\"npx --yes @shopify/cli\"`)"
                    .to_string()
            })
    }

    /// `shopify app function build` for the Function in `dir`, to run from the app's folder, as
    /// `shopify app build` runs it (esbuild names the modules of a JavaScript Function from
    /// there).
    pub fn function_build(&self, root: &Path, dir: &Path) -> Command {
        self.command(root, &function_build_args(dir))
    }

    /// `shopify app build`: every extension of the app at `root`, its dependencies as they are.
    pub fn app_build(&self, root: &Path) -> Command {
        self.command(root, &app_build_args(root))
    }

    /// [`ShopifyCli::function_build`] as a command line, for the shell `--watch` runs builds
    /// in, from the app's folder.
    pub fn function_build_line(&self, dir: &Path) -> String {
        self.line(&function_build_args(dir))
    }

    /// [`ShopifyCli::app_build`] as a command line, for the shell `--watch` runs builds in.
    pub fn app_build_line(&self, root: &Path) -> String {
        self.line(&app_build_args(root))
    }

    fn line(&self, args: &[String]) -> String {
        std::iter::once(self.program.to_string_lossy().into_owned())
            .chain(self.args.iter().cloned())
            .chain(args.iter().cloned())
            .map(|word| crate::checkout::shell_quote(&word))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// How lsf runs it, for the messages: `/usr/local/bin/shopify`, `npx --yes @shopify/cli`.
    pub fn describe(&self) -> String {
        let mut words = vec![self.program.to_string_lossy().into_owned()];
        words.extend(self.args.iter().cloned());
        words.join(" ")
    }

    fn command(&self, root: &Path, args: &[String]) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .args(args)
            .current_dir(root)
            .stdin(Stdio::null());
        command
    }
}

fn function_build_args(dir: &Path) -> Vec<String> {
    let path = dir.to_string_lossy();
    ["app", "function", "build", "--path", &path, "--no-input"]
        .map(str::to_string)
        .to_vec()
}

fn app_build_args(root: &Path) -> Vec<String> {
    let path = root.to_string_lossy();
    [
        "app",
        "build",
        "--path",
        &path,
        "--skip-dependencies-installation",
        "--no-input",
    ]
    .map(str::to_string)
    .to_vec()
}

/// Runs a command of the CLI, its output on the terminal; an error says what failed.
pub fn run(mut command: Command, what: &str) -> Result<(), String> {
    let status = command
        .status()
        .map_err(|error| format!("cannot run Shopify's CLI for {what}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("Shopify's CLI failed to build {what} ({status})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lsf-shopify-cli-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn executable(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "#!/bin/sh\n").unwrap();
    }

    #[test]
    fn the_cli_is_the_one_named_then_the_app_s_then_the_path_s() {
        let app = folder("app");
        let bin = folder("bin");
        executable(&bin.join("shopify"));
        let path = std::env::join_paths([&bin]).unwrap();
        let found = |configured: Option<&str>, path: Option<&OsStr>| {
            ShopifyCli::find_in(&app, configured, path)
        };
        assert_eq!(
            found(None, Some(&path)).unwrap().program,
            bin.join("shopify")
        );
        executable(&app.join("node_modules/.bin/shopify"));
        assert_eq!(
            found(None, Some(&path)).unwrap().program,
            app.join("node_modules/.bin/shopify")
        );
        let npx = found(Some("npx --yes @shopify/cli@5.0.0"), Some(&path)).unwrap();
        assert_eq!(npx.program, PathBuf::from("npx"));
        assert_eq!(npx.args, ["--yes", "@shopify/cli@5.0.0"]);
        std::fs::remove_dir_all(app.join("node_modules")).unwrap();
        let missing = found(None, None).unwrap_err();
        assert!(missing.contains("lsf did not find"), "{missing}");
        assert!(
            missing.contains("npm install --global @shopify/cli"),
            "{missing}"
        );
    }

    #[test]
    fn the_commands_build_from_the_app_s_folder() {
        let cli = ShopifyCli {
            program: PathBuf::from("npx"),
            args: vec!["--yes".to_string(), "@shopify/cli".to_string()],
        };
        let root = Path::new("/apps/my app");
        let function = cli.function_build(root, &root.join("extensions/discount"));
        assert_eq!(function.get_current_dir(), Some(root));
        let args: Vec<String> = function
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "--yes",
                "@shopify/cli",
                "app",
                "function",
                "build",
                "--path",
                "/apps/my app/extensions/discount",
                "--no-input"
            ]
        );
        let app: Vec<String> = cli
            .app_build(root)
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            &app[2..],
            [
                "app",
                "build",
                "--path",
                "/apps/my app",
                "--skip-dependencies-installation",
                "--no-input"
            ]
        );
        assert_eq!(cli.describe(), "npx --yes @shopify/cli");
        if cfg!(unix) {
            assert_eq!(
                cli.function_build_line(&root.join("extensions/discount")),
                "'npx' '--yes' '@shopify/cli' 'app' 'function' 'build' '--path' \
                 '/apps/my app/extensions/discount' '--no-input'"
            );
        }
    }

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), &target).unwrap();
            }
        }
    }

    /// lsf's builds with Shopify's CLI write what Shopify's CLI writes on its own, to the byte:
    /// the modules of the Functions written in JavaScript and TypeScript, and the bundles of the
    /// checkout UI extensions built from their sources, as `tests/fixtures/shopify-cli/expected`
    /// holds them (Shopify's CLI 5.0.0, run from the root of the same app). Needs Node, npm, the
    /// network, and the CLI to compare with: `mise run shopify-cli:test`.
    #[test]
    #[ignore = "needs Node, npm, the network and Shopify's CLI: mise run shopify-cli:test"]
    fn lsf_s_builds_are_shopify_s_cli_s_to_the_byte() {
        use lsf_checkout::app::{self, Selection};
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let fixtures = manifest.join("tests/fixtures/shopify-cli");
        let schemas = manifest.join("../checkout/schemas/2026-10");
        let root = folder("byte").join("app");
        copy_dir(&fixtures.join("app"), &root);
        // What `shopify app function schema` writes, which takes a Partner account.
        for (handle, api) in [
            ("discount-typescript", "discount"),
            ("validation-javascript", "cart_checkout_validation"),
        ] {
            std::fs::copy(
                schemas.join(format!("{api}.graphql")),
                root.join("extensions").join(handle).join("schema.graphql"),
            )
            .unwrap();
        }
        for entry in std::fs::read_dir(root.join("extensions")).unwrap() {
            let status = Command::new("npm")
                .args(["install", "--no-audit", "--no-fund", "--loglevel=error"])
                .current_dir(entry.unwrap().path())
                .status()
                .expect("npm");
            assert!(status.success(), "npm install failed");
        }
        let compare = |names: &[(&str, &str)]| {
            for (name, built) in names {
                let built = std::fs::read(root.join(built)).unwrap_or_default();
                let expected = std::fs::read(fixtures.join("expected").join(name)).unwrap();
                assert!(
                    built == expected,
                    "{name}: {} bytes, Shopify's CLI on its own wrote {}",
                    built.len(),
                    expected.len()
                );
            }
        };
        let functions = [
            (
                "discount-typescript.wasm",
                "extensions/discount-typescript/dist/function.wasm",
            ),
            (
                "validation-javascript.wasm",
                "extensions/validation-javascript/dist/function.wasm",
            ),
        ];
        let shopify_app = app::load(&root).unwrap();
        // `lsf function build`: `shopify app function build`, for each Function.
        crate::checkout::build_functions(&shopify_app, &Selection::all()).unwrap();
        compare(&functions);
        // `lsf checkout build`: `shopify app build`, the Functions too.
        for (_, built) in functions {
            std::fs::remove_file(root.join(built)).unwrap();
        }
        crate::checkout::build(&shopify_app, &Selection::all()).unwrap();
        compare(&functions);
        compare(&[
            ("ui-preact.js", "extensions/ui-preact/dist/ui-preact.js"),
            (
                "ui-preact-typescript.js",
                "extensions/ui-preact-typescript/dist/ui-preact-typescript.js",
            ),
        ]);
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }
}
