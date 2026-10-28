//! Tests of `--watch`: what is looked at, when builds run, what the checkout runs meanwhile,
//! and what a build that fails leaves.
//!
//! Each test makes a temporary folder with the app in `app/` and what the tests steer the
//! builds with in `ctl/`, out of the app (which is all watched). The build command is a shell
//! script: it "builds" a Function by copying `src/main.wasm` (a module that exports a function,
//! and whose last section says its version) to `dist/function.wasm`. It counts its builds in `ctl/count-<handle>`, says it
//! runs with `ctl/running-<handle>-<n>` while it does, and notes in `ctl/concurrency` how many
//! ran when it started. Steered by files of `ctl/`:
//!
//! - `gated-<handle>`: build `n` waits for `go-<handle>-<n>`;
//! - `partial-<handle>`: it removes the module and writes half of one first, as a compiler
//!   writing its output does (before the gate);
//! - `early-<handle>`: it writes that file as its module first (before the gate);
//! - `fail-<handle>`: it fails, after writing half a module;
//! - `version-<handle>-<n>`: build `n` writes that file as its module, not `src/main.wasm`.
//!
//! The tests wait for what they can see (a file, what the watcher says it did), never for a
//! guessed time.

use std::io::Write as _;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;

const BUILD: &str = r#"h=$(basename "$PWD")
ctl=../../../ctl
echo build >> "$ctl/count-$h"
n=$(wc -l < "$ctl/count-$h" | tr -d ' ')
: > "$ctl/running-$h-$n"
ls "$ctl" | grep -c '^running-' >> "$ctl/concurrency"
echo $$ > "$ctl/pid-$h"
echo "compiling $h"
mkdir -p dist
if [ -f "$ctl/partial-$h" ]; then
  rm -f dist/function.wasm
  printf 'half-written' > dist/function.wasm
fi
if [ -f "$ctl/early-$h" ]; then
  cp "$ctl/early-$h" dist/function.wasm
fi
if [ -f "$ctl/gated-$h" ]; then
  while [ ! -f "$ctl/go-$h-$n" ]; do sleep 0.01; done
fi
if [ -f "$ctl/fail-$h" ]; then
  printf 'half-written' > dist/function.wasm
  echo "error: expected one of \`;\` or \`}\`" >&2
  rm -f "$ctl/running-$h-$n"
  exit 1
fi
if [ -f "$ctl/version-$h-$n" ]; then
  cp "$ctl/version-$h-$n" dist/function.wasm
else
  cp src/main.wasm dist/function.wasm
fi
# As the playground's build does: a copy of the schema next to the Function.
echo "type Query" > schema.graphql
rm -f "$ctl/running-$h-$n"
"#;

/// A build for the tests of the write lock: build `n` waits for `go-<handle>-<n>`, and writes
/// `version-<handle>-<n>` (or `src/main.wasm`) as its module, after the wait, or before it with
/// `write-first-<handle>-<n>`. It gives up once `ctl/` is gone (the test is over) or after 30 s:
/// a build no watcher stops (its server "killed") ends on its own.
const ORDER: &str = r#"h=$(basename "$PWD")
ctl=../../../ctl
echo build >> "$ctl/count-$h"
n=$(wc -l < "$ctl/count-$h" | tr -d ' ')
: > "$ctl/running-$h-$n"
echo "compiling $h"
mkdir -p dist
made=src/main.wasm
if [ -f "$ctl/version-$h-$n" ]; then made="$ctl/version-$h-$n"; fi
if [ -f "$ctl/write-first-$h-$n" ]; then cp "$made" dist/function.wasm; fi
i=0
while [ ! -f "$ctl/go-$h-$n" ] && [ -d "$ctl" ] && [ $i -lt 3000 ]; do sleep 0.01; i=$((i + 1)); done
if [ ! -f "$ctl/write-first-$h-$n" ]; then cp "$made" dist/function.wasm; fi
rm -f "$ctl/running-$h-$n"
"#;

/// What a build that waits for the write lock of its Function says.
const WAITING: &str = "waiting for another build of it to end";

/// A module that exports a function, `f`, that does nothing.
const EXPORTS_F: &[u8] = b"\0asm\x01\0\0\0\
    \x01\x04\x01\x60\0\0\
    \x03\x02\x01\0\
    \x07\x05\x01\x01f\0\0\
    \x0a\x04\x01\x02\0\x0b";

/// A Wasm module that exports a function, and whose last section (a custom one) holds `text`.
fn module(text: &str) -> Vec<u8> {
    assert!(text.len() < 120);
    let mut bytes = EXPORTS_F.to_vec();
    bytes.push(0);
    bytes.push(text.len() as u8 + 1);
    bytes.push(0);
    bytes.extend_from_slice(text.as_bytes());
    bytes
}

/// What [`module`] made `bytes` from: `missing` without bytes, `invalid` for what a run cannot
/// load.
fn version(bytes: Option<Vec<u8>>) -> String {
    match bytes {
        None => "missing".to_string(),
        Some(bytes) if check_module(&bytes).is_ok() && bytes.starts_with(EXPORTS_F) => {
            String::from_utf8_lossy(&bytes[EXPORTS_F.len() + 3..]).into_owned()
        }
        Some(_) => "invalid".to_string(),
    }
}

struct TempApp(PathBuf);

impl TempApp {
    fn new(label: &str) -> TempApp {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base =
            std::env::temp_dir().join(format!("lsf-watch-{label}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(base.join("app/extensions")).unwrap();
        std::fs::create_dir_all(base.join("ctl")).unwrap();
        let app = TempApp(base.canonicalize().unwrap());
        app.write("shopify.app.toml", "name = \"Watch\"\nhandle = \"watch\"\n");
        app.ctl_write("build.sh", BUILD);
        app.ctl_write("order.sh", ORDER);
        app.ctl_write(
            "build2.sh",
            "mkdir -p dist\ncp src/other.wasm dist/function.wasm\n",
        );
        app
    }

    fn root(&self) -> PathBuf {
        self.0.join("app")
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root().join(relative)
    }

    fn write(&self, relative: &str, content: &str) {
        self.write_bytes(relative, content.as_bytes());
    }

    fn write_bytes(&self, relative: &str, content: &[u8]) {
        let path = self.path(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn ctl(&self, name: &str) -> PathBuf {
        self.0.join("ctl").join(name)
    }

    fn ctl_write(&self, name: &str, content: &str) {
        std::fs::write(self.ctl(name), content).unwrap();
    }

    /// Lets build `n` of `handle` go on.
    fn go(&self, handle: &str, n: usize) {
        self.ctl_write(&format!("go-{handle}-{n}"), "");
    }

    /// Whether build `n` of `handle` runs.
    fn running(&self, handle: &str, n: usize) -> bool {
        self.ctl(&format!("running-{handle}-{n}")).exists()
    }

    /// A Function built by `command`, whose source and module are `v1`.
    fn function_with(&self, handle: &str, command: &str) {
        self.write(
            &format!("extensions/{handle}/shopify.extension.toml"),
            &format!(
                r#"api_version = "2026-10"

[[extensions]]
name = "{handle}"
handle = "{handle}"
type = "function"

  [[extensions.targeting]]
  target = "cart.lines.discounts.generate.run"
  input_query = "src/run.graphql"
  export = "cart_lines_discounts_generate_run"

  [extensions.build]
  command = "{command}"
  path = "dist/function.wasm"
"#
            ),
        );
        self.write(
            &format!("extensions/{handle}/src/run.graphql"),
            "query { cart { cost { subtotalAmount { amount } } } }\n",
        );
        self.source(handle, "v1");
        self.write_bytes(
            &format!("extensions/{handle}/dist/function.wasm"),
            &module("v1"),
        );
    }

    fn function(&self, handle: &str) {
        self.function_with(handle, "sh ../../../ctl/build.sh");
    }

    /// Saves the source of `handle`: what its next build makes.
    fn source(&self, handle: &str, version: &str) {
        self.write_bytes(
            &format!("extensions/{handle}/src/main.wasm"),
            &module(version),
        );
    }

    fn ui_extension(&self, handle: &str) {
        self.write(
            &format!("extensions/{handle}/shopify.extension.toml"),
            &format!(
                r#"api_version = "2026-10"

[[extensions]]
name = "{handle}"
handle = "{handle}"
type = "ui_extension"

  [[extensions.targeting]]
  target = "purchase.checkout.block.render"
  module = "./src/Checkout.jsx"
"#
            ),
        );
        self.write(&format!("extensions/{handle}/dist/{handle}.js"), "// v1\n");
    }

    /// Shopify's CLI, as the app's own (`node_modules/.bin/shopify`): `app function build
    /// --path DIR` "builds" the Function in `DIR` as `build.sh` does (and writes the types of
    /// its typegen); `app build --path ROOT` bundles each checkout UI extension that has a
    /// `src/Checkout.jsx` into `dist/<folder>.js` (the source, under a comment). It notes each
    /// call in `ctl/cli` (its folder, then its arguments), fails with `ctl/fail-cli`, and with
    /// `ctl/gated-cli` waits for `ctl/go-cli` before `app build`.
    fn shopify_cli(&self) {
        let ctl = self.0.join("ctl");
        let script = format!(
            r#"#!/bin/sh
ctl='{}'
echo "$PWD $*" >> "$ctl/cli"
if [ -f "$ctl/fail-cli" ]; then
  echo "error: [targeting.0.module]: Required" >&2
  exit 1
fi
if [ "$2" = function ]; then
  dir="$5"
  mkdir -p "$dir/dist" "$dir/generated"
  echo "export type Run = {{}};" > "$dir/generated/api.ts"
  cp "$dir/src/main.wasm" "$dir/dist/function.wasm"
  exit 0
fi
if [ -f "$ctl/gated-cli" ]; then
  while [ ! -f "$ctl/go-cli" ]; do sleep 0.01; done
fi
for source in "$4"/extensions/*/src/Checkout.jsx; do
  [ -f "$source" ] || continue
  extension=$(dirname "$(dirname "$source")")
  mkdir -p "$extension/dist"
  {{ echo "// built"; cat "$source"; }} > "$extension/dist/$(basename "$extension").js"
done
"#,
            ctl.display()
        );
        let path = self.path("node_modules/.bin/shopify");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, script).unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// The calls of the fake [`TempApp::shopify_cli`]: its folder, then its arguments.
    fn cli_calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.ctl("cli"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// A Function written in TypeScript as Shopify's CLI generates it: no build command, an
    /// `src/index.ts`, a typegen in its `package.json`, and no module yet.
    fn javascript_function(&self, handle: &str) {
        self.function_with(handle, "");
        self.write(
            &format!("extensions/{handle}/package.json"),
            r#"{ "codegen": { "schema": "schema.graphql", "documents": "src/*.graphql",
  "generates": { "./generated/api.ts": { "plugins": ["typescript", "typescript-operations"] } } } }"#,
        );
        self.write(
            &format!("extensions/{handle}/src/index.ts"),
            "export * from './run';\n",
        );
        std::fs::remove_file(self.path(&format!("extensions/{handle}/dist/function.wasm")))
            .unwrap();
    }

    /// A checkout UI extension built from its source, `src/Checkout.jsx`, which says `text`.
    fn ui_source(&self, handle: &str, text: &str) {
        self.write(
            &format!("extensions/{handle}/src/Checkout.jsx"),
            &format!("export default () => '{text}';\n"),
        );
    }

    /// The bundle of the checkout UI extension `handle`.
    fn bundle(&self, handle: &str) -> String {
        std::fs::read_to_string(self.path(&format!("extensions/{handle}/dist/{handle}.js")))
            .unwrap_or_default()
    }

    /// How many builds of `handle` started.
    fn builds(&self, handle: &str) -> usize {
        std::fs::read_to_string(self.ctl(&format!("count-{handle}")))
            .unwrap_or_default()
            .lines()
            .count()
    }

    /// What the build output of `handle` is.
    fn dist(&self, handle: &str) -> String {
        version(std::fs::read(self.path(&format!("extensions/{handle}/dist/function.wasm"))).ok())
    }

    /// The hash of the module of `handle` in its `dist/`.
    fn dist_hash(&self, handle: &str) -> String {
        module_hash(
            &std::fs::read(self.path(&format!("extensions/{handle}/dist/function.wasm"))).unwrap(),
        )
    }

    /// What the record in `ctl/record/app` says of `handle`: what its last build was made
    /// from, and the hash of what it made. Empty when it says nothing.
    fn recorded(&self, handle: &str) -> (String, String) {
        let path = self.ctl(&format!("record/app/inputs/{handle}.json"));
        let json: Json = std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let text = |key: &str| json[key].as_str().unwrap_or_default().to_string();
        (text("inputs"), text("output"))
    }

    /// How many builds ran at once, at most.
    fn concurrency(&self) -> usize {
        std::fs::read_to_string(self.ctl("concurrency"))
            .unwrap_or_default()
            .lines()
            .filter_map(|line| line.trim().parse().ok())
            .max()
            .unwrap_or(0)
    }

    /// The app as the checkout reads it with these builds.
    fn load(&self, builds: &Builds) -> App {
        let mut app = app::load(&self.root()).unwrap();
        builds.apply(&mut app);
        app
    }
}

impl Drop for TempApp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A watcher of `app` that looks often, with what it says and how often it told the pages.
struct Watching {
    watcher: Option<Watcher>,
    builds: Arc<Builds>,
    lines: Arc<Mutex<Vec<String>>>,
    reloads: Arc<AtomicUsize>,
}

const LONG: Duration = Duration::from_secs(30);

impl Watching {
    fn start(app: &TempApp, tune: impl FnOnce(&mut Options)) -> Watching {
        let watching = Watching::launch(app, tune);
        assert!(
            watching.watcher().wait_idle(LONG),
            "the first builds did not end:\n{}",
            watching.said()
        );
        watching
    }

    /// A watcher of `app`, whose first builds may still run.
    fn launch(app: &TempApp, tune: impl FnOnce(&mut Options)) -> Watching {
        let mut options = Options::new(Vec::new(), false);
        options.poll = Duration::from_millis(10);
        // Polled, every 30 ms: the same on every system. Some tests follow the events.
        options.events = false;
        options.quiet = Duration::from_millis(60);
        tune(&mut options);
        let builds = Arc::new(Builds::publishing(Modules::create().unwrap()));
        let lines = Arc::new(Mutex::new(Vec::new()));
        let reloads = Arc::new(AtomicUsize::new(0));
        let said = lines.clone();
        let counted = reloads.clone();
        let watcher = Watcher::start(
            &app.root(),
            builds.clone(),
            options,
            Box::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
            }),
            Arc::new(move |line: &str| said.lock().unwrap().push(line.to_string())),
        );
        Watching {
            watcher: Some(watcher),
            builds,
            lines,
            reloads,
        }
    }

    fn watcher(&self) -> &Watcher {
        self.watcher.as_ref().unwrap()
    }

    fn reloads(&self) -> usize {
        self.reloads.load(Ordering::SeqCst)
    }

    fn said(&self) -> String {
        self.lines.lock().unwrap().join("\n")
    }

    /// What the checkout runs for `handle`.
    fn served(&self, handle: &str) -> String {
        version(std::fs::read(self.builds.module_path(handle).unwrap()).ok())
    }

    /// Waits until the watcher looked at the files after what the test changed, and what it
    /// saw counted: the builds it started are over, and the pages told.
    fn settle(&self) {
        assert!(
            self.watcher().look_and_wait(LONG, true),
            "the builds did not end:\n{}",
            self.said()
        );
    }

    /// Waits until the watcher saw what the test changed: the builds it asks for may run.
    fn seen(&self) {
        assert!(
            self.watcher().look_and_wait(LONG, false),
            "the change was not seen:\n{}",
            self.said()
        );
    }

    fn stop(&mut self) {
        self.watcher.take().unwrap().stop();
    }
}

fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + LONG;
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

// --- The rules, on their own ---------------------------------------------------------------

#[test]
fn the_build_outputs_and_the_tools_folders_are_not_watched() {
    let none: &[PathBuf] = &[];
    let built = [PathBuf::from("dist"), PathBuf::from("out")];
    for watched in [
        "src/main.rs",
        "src/run.graphql",
        "input.graphql",
        "shopify.extension.toml",
        "locales/en.default.json",
        "Cargo.toml",
        "src/lib/schema.graphql",
    ] {
        assert!(!ignored(Path::new(watched), &built), "{watched}");
    }
    // The bundle of an extension that has no build is what it is made of.
    assert!(!ignored(Path::new("dist/ui-notice.js"), none));
    for left_alone in [
        "dist/function.wasm",
        "out/volume.wasm",
        "schema.graphql",
        "target/release/x.wasm",
        "node_modules/react/index.js",
        ".git/HEAD",
        "src/.main.rs.swp",
        "src/main.rs~",
        ".DS_Store",
        "README.md",
        "docs/README.md",
    ] {
        assert!(ignored(Path::new(left_alone), &built), "{left_alone}");
    }
}

#[test]
fn a_javascript_function_is_built_by_shopify_s_cli_from_the_app_s_folder() {
    let app = TempApp::new("javascript");
    app.shopify_cli();
    app.javascript_function("bulk");
    app.ui_extension("ui-a");
    app.ui_source("ui-a", "a");
    app.ui_extension("ui-b");
    app.ui_source("ui-b", "b");
    let shopify_app = app::load(&app.root()).unwrap();
    let rules = rules_of(
        &shopify_app,
        &Selection::only(["bulk".to_string(), "ui-a".to_string()]),
    );
    let bulk = &rules["bulk"];
    let job = bulk.build.as_ref().expect("a build");
    let command = job.command.as_ref().unwrap();
    assert!(
        command.starts_with(&format!(
            "'{}' 'app' 'function' 'build' '--path' ",
            app.path("node_modules/.bin/shopify").display()
        )),
        "{command}"
    );
    assert!(command.ends_with(" '--no-input'"), "{command}");
    assert_eq!(job.cwd, app.root());
    assert!(
        ignored(Path::new("generated/api.ts"), &bulk.outputs),
        "{:?}",
        bulk.outputs
    );
    assert!(ignored(Path::new("dist/function.js"), &bulk.outputs));
    assert!(!ignored(Path::new("src/index.ts"), &bulk.outputs));
    assert!(!ignored(Path::new("package.json"), &bulk.outputs));
    // Shopify's CLI bundles the selected checkout UI extension; what it writes in the other's
    // `dist/` is not looked at either.
    assert_eq!(rules["ui-a"].ui.as_deref(), Some("ui-a"));
    assert_eq!(rules["ui-b"].ui, None);
    assert!(ignored(Path::new("dist/ui-b.js"), &rules["ui-b"].outputs));
}

#[test]
fn what_every_build_may_read_is_the_rest_of_the_app() {
    let root = Path::new("/app");
    let data = Path::new("/app/fixtures");
    let shared = |name: &str| shared_input(Path::new(name), &root.join(name), root, Some(data));
    for name in [
        "Cargo.toml",
        "Cargo.lock",
        "package.json",
        "package-lock.json",
        "pnpm-lock.yaml",
        "crates",
        "tools",
    ] {
        assert!(shared(name), "{name}");
    }
    for name in [
        "shopify-local",
        "fixtures",
        "target",
        "node_modules",
        ".git",
        "README.md",
    ] {
        assert!(!shared(name), "{name}");
    }
}

#[test]
fn a_module_is_one_a_run_can_load() {
    assert_eq!(check_module(&module("v1")), Ok(()));
    let whole = module("a longer section");
    for cut in 1..whole.len() {
        // Without its custom section, it is a whole module.
        if cut != EXPORTS_F.len() {
            assert!(check_module(&whole[..cut]).is_err(), "cut at {cut}");
        }
    }
    // Section boundaries that hold, around a section whose content is cut short.
    let error = check_module(b"\0asm\x01\0\0\0\x01\x01\xff").unwrap_err();
    assert!(error.contains("unexpected end"), "{error}");
    // Another version of the format.
    let mut other = module("v1");
    other[4] = 2;
    assert!(check_module(&other).is_err());
    // A whole module, with nothing to run.
    assert_eq!(
        check_module(b"\0asm\x01\0\0\0"),
        Err("it exports no function".to_string())
    );
    assert!(check_module(b"half-written").is_err());
    assert!(check_module(b"").is_err());
}

#[test]
fn the_output_kept_is_the_first_error_and_the_end() {
    let mut output = Output::default();
    for n in 0..30 {
        output.push(format!("   Compiling crate-{n}"));
    }
    for line in [
        "error[E0425]: cannot find value `x` in this scope",
        " --> src/main.rs:3:5",
        "  |",
        "3 |     x",
        "  |     ^ not found in this scope",
        "",
        "error: aborting due to 1 previous error",
    ] {
        output.push(line.to_string());
    }
    for n in 0..30 {
        output.push(format!("note {n}"));
    }
    let text = output.text();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[..6],
        [
            "error[E0425]: cannot find value `x` in this scope",
            " --> src/main.rs:3:5",
            "  |",
            "3 |     x",
            "  |     ^ not found in this scope",
            "…",
        ]
    );
    assert_eq!(lines.len(), 6 + TAIL, "{text}");
    assert_eq!(lines.last(), Some(&"note 29"));
    assert!(!text.contains("Compiling"), "{text}");

    // Without an error: the end. A short output is kept whole.
    let mut output = Output::default();
    for n in 0..30 {
        output.push(format!("line {n}"));
    }
    let text = output.text();
    assert_eq!(text.lines().count(), TAIL);
    assert!(text.ends_with("line 29"));
    let mut output = Output::default();
    output.push("Compiling".to_string());
    output.push("error: boom".to_string());
    output.push("  context".to_string());
    assert_eq!(output.text(), "Compiling\nerror: boom\n  context");

    // A long error is cut.
    let mut output = Output::default();
    output.push("error: long".to_string());
    for n in 0..100 {
        output.push(format!("  context {n}"));
    }
    let text = output.text();
    assert!(text.lines().count() <= FIRST_ERROR + 1 + TAIL, "{text}");
    assert!(text.contains("\n…\n"), "{text}");
}

#[test]
fn a_burst_of_changes_counts_once_when_the_files_hold_still() {
    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    let mut debounce = Debounce::new(Duration::from_millis(100), Duration::from_secs(1));
    let change = |name: &str| Changes {
        extensions: [name.to_string()].into(),
        ..Changes::default()
    };
    debounce.add(Changes::default(), at(0));
    assert!(!debounce.is_pending(), "nothing changed");
    debounce.add(change("a"), at(0));
    debounce.add(change("b"), at(50));
    assert_eq!(debounce.take(at(120)), None, "b changed 70 ms ago");
    debounce.add(change("a"), at(120));
    assert_eq!(debounce.take(at(219)), None);
    let taken = debounce.take(at(220)).expect("held still for 100 ms");
    assert_eq!(
        taken.extensions,
        BTreeSet::from(["a".to_string(), "b".to_string()])
    );
    assert_eq!(debounce.take(at(400)), None, "once");

    // Files that never hold still are let through after `max_wait`.
    for ms in (1000..2100).step_by(50) {
        debounce.add(change("log"), at(ms));
        if let Some(taken) = debounce.take(at(ms)) {
            assert!(ms >= 2000, "{ms}");
            assert!(taken.extensions.contains("log"));
            return;
        }
    }
    panic!("never let through");
}

#[test]
fn one_build_at_a_time_per_function_and_one_more_after_a_change_during_it() {
    let mut scheduler = Scheduler::new(2);
    assert_eq!(scheduler.request("a"), ["a"]);
    // A burst during the build: one more build, after it.
    assert!(scheduler.request("a").is_empty());
    assert!(scheduler.request("a").is_empty());
    assert!(scheduler.is_running("a"));
    assert_eq!(scheduler.finished("a"), ["a"]);
    // Nothing changed during that one: no other.
    assert!(scheduler.finished("a").is_empty());
    assert!(!scheduler.is_busy());

    // At most two at once; the others wait their turn, once each.
    assert_eq!(scheduler.request("a"), ["a"]);
    assert_eq!(scheduler.request("b"), ["b"]);
    assert!(scheduler.request("c").is_empty());
    assert!(scheduler.request("c").is_empty());
    assert!(scheduler.request("d").is_empty());
    assert_eq!(scheduler.finished("b"), ["c"]);
    // A change to `a` during its build queues it behind `d`.
    assert!(scheduler.request("a").is_empty());
    assert_eq!(scheduler.finished("a"), ["d"]);
    assert_eq!(scheduler.finished("c"), ["a"]);
    // `e` is dropped while it waits.
    assert!(scheduler.request("e").is_empty());
    scheduler.forget("e");
    assert!(scheduler.finished("d").is_empty());
    assert!(scheduler.finished("a").is_empty());
    assert!(!scheduler.is_busy());
}

// --- The watcher, on an app ----------------------------------------------------------------

#[test]
fn a_save_builds_the_function_once_and_tells_the_pages_after_the_build() {
    let app = TempApp::new("burst");
    app.function("fn-a");
    let watching = Watching::start(&app, |options| options.quiet = Duration::from_millis(300));
    assert_eq!(app.builds("fn-a"), 0, "the module that is there is used");
    assert_eq!(watching.served("fn-a"), "v1");
    let generation = watching.builds.generation();

    // An editor's save: several files at once.
    app.source("fn-a", "v2 draft");
    app.write("extensions/fn-a/src/lib.rs", "// v2");
    app.write("extensions/fn-a/src/run.graphql", "query { cart { note } }");
    app.source("fn-a", "v2");
    watching.settle();

    assert_eq!(app.builds("fn-a"), 1, "{}", watching.said());
    assert_eq!(watching.served("fn-a"), "v2");
    assert!(watching.builds.generation() > generation);
    assert_eq!(watching.reloads(), 1);
    let said = watching.said();
    assert!(said.contains("↻ building fn-a…"), "{said}");
    assert!(said.contains("✓ fn-a built in "), "{said}");
}

#[test]
fn the_checkout_runs_the_last_module_that_built_while_a_build_writes_its_own() {
    let app = TempApp::new("published");
    app.function("fn-a");
    let watching = Watching::start(&app, |_| {});
    let published = watching.builds.module_path("fn-a").unwrap();
    assert!(!published.starts_with(app.root()), "out of the app");
    let function = app.load(&watching.builds);
    let function = function.function("fn-a").unwrap();
    assert_eq!(function.wasm_path, published);
    assert!(function.built());

    // The build removes the module, writes half of one, and holds there.
    app.ctl_write("partial-fn-a", "");
    app.ctl_write("gated-fn-a", "");
    app.source("fn-a", "v2");
    wait_for("the half-written module", || {
        app.running("fn-a", 1) && app.dist("fn-a") == "invalid"
    });
    let during = app.load(&watching.builds);
    let function = during.function("fn-a").unwrap();
    assert_eq!(function.wasm_path, published);
    assert!(function.built());
    assert_eq!(watching.served("fn-a"), "v1", "the last module that built");
    assert!(
        !during
            .diagnostics
            .iter()
            .any(|d| d.message.contains("no Wasm module")),
        "{:?}",
        during.diagnostics
    );

    app.go("fn-a", 1);
    watching.settle();
    assert_eq!(watching.served("fn-a"), "v2");
    assert_eq!(app.dist("fn-a"), "v2", "the build's own output, untouched");
    assert_eq!(watching.reloads(), 1);

    // Out of the watch, the app is what is on disk.
    let plain = app.load(&Builds::default());
    assert!(
        plain
            .function("fn-a")
            .unwrap()
            .wasm_path
            .ends_with("dist/function.wasm")
    );
}

#[test]
fn a_change_during_a_build_builds_exactly_once_more() {
    let app = TempApp::new("during");
    app.function("fn-a");
    let watching = Watching::start(&app, |_| {});
    app.ctl_write("gated-fn-a", "");

    app.source("fn-a", "v2");
    wait_for("the build to start", || app.running("fn-a", 1));
    // Saved three times while it builds.
    for version in ["v3", "v4", "v5"] {
        app.source("fn-a", version);
        watching.seen();
    }
    app.go("fn-a", 1);
    app.go("fn-a", 2);
    watching.settle();

    assert_eq!(app.builds("fn-a"), 2, "{}", watching.said());
    assert_eq!(watching.served("fn-a"), "v5");
    // The pages reload once, after the last build.
    assert_eq!(watching.reloads(), 1);
}

#[test]
fn the_pages_reload_once_every_build_a_change_started_is_over() {
    let app = TempApp::new("wave");
    let handles = ["fn-a", "fn-b", "fn-c", "fn-d"];
    for handle in handles {
        app.function(handle);
        app.ctl_write(&format!("gated-{handle}"), "");
    }
    let watching = Watching::start(&app, |options| options.parallel = 3);
    let generation = watching.builds.generation();

    // A, B and C build, D waits.
    for handle in handles {
        app.source(handle, "v2");
    }
    watching.seen();
    wait_for("three builds", || {
        app.running("fn-a", 1) && app.running("fn-b", 1) && app.running("fn-c", 1)
    });
    assert_eq!(app.builds("fn-d"), 0);
    // A changes again during its build.
    app.source("fn-a", "v3");
    watching.seen();

    // A is over: D starts, and A waits behind it. The pages do not reload yet.
    app.go("fn-a", 1);
    wait_for("D to start", || app.running("fn-d", 1));
    assert_eq!(app.builds("fn-a"), 1);
    assert_eq!(watching.reloads(), 0, "{}", watching.said());
    assert_eq!(watching.builds.generation(), generation);

    // B and C are over: A builds again. Still no reload.
    app.go("fn-b", 1);
    app.go("fn-c", 1);
    wait_for("A to build again", || app.running("fn-a", 2));
    assert_eq!(watching.reloads(), 0, "{}", watching.said());

    app.go("fn-d", 1);
    app.go("fn-a", 2);
    watching.settle();
    assert_eq!(watching.reloads(), 1, "{}", watching.said());
    assert_eq!(watching.served("fn-a"), "v3");
    assert_eq!(watching.served("fn-d"), "v2");
    assert_eq!(app.builds("fn-a"), 2);
}

#[test]
fn a_build_that_fails_keeps_the_last_module_and_says_why() {
    let app = TempApp::new("fail");
    app.function("fn-a");
    let watching = Watching::start(&app, |_| {});
    app.source("fn-a", "v2");
    watching.settle();
    assert_eq!(watching.served("fn-a"), "v2");

    // It breaks: the build writes half a module and fails.
    app.ctl_write("fail-fn-a", "");
    app.source("fn-a", "v3, broken");
    watching.settle();
    assert_eq!(app.builds("fn-a"), 2);
    assert_eq!(watching.served("fn-a"), "v2", "the last module that built");
    assert_eq!(
        app.dist("fn-a"),
        "invalid",
        "what the build left, left alone"
    );
    let failure = watching.builds.failure("fn-a").expect("a failure");
    assert!(
        failure.reason.starts_with("build failed (exit"),
        "{failure:?}"
    );
    assert!(failure.output.contains("compiling fn-a"), "{failure:?}");
    assert!(failure.output.contains("expected one of"), "{failure:?}");
    assert_eq!(watching.builds.to_json()[0]["handle"], "fn-a");
    assert_eq!(watching.builds.to_json()[0]["built"], true);
    let said = watching.said();
    assert!(said.contains("✗ fn-a: build failed (exit"), "{said}");
    assert!(said.contains("\n    error: expected one of"), "{said}");
    assert_eq!(watching.reloads(), 2, "the pages show the error");

    // Fixed.
    std::fs::remove_file(app.ctl("fail-fn-a")).unwrap();
    app.source("fn-a", "v4");
    watching.settle();
    assert_eq!(watching.served("fn-a"), "v4");
    assert_eq!(watching.builds.failure("fn-a"), None);
    assert_eq!(watching.reloads(), 3);
}

#[test]
fn a_build_that_writes_no_whole_module_fails() {
    let app = TempApp::new("nomodule");
    app.function_with("fn-a", "printf 'half' > dist/function.wasm");
    app.function_with("fn-b", "rm dist/function.wasm");
    let watching = Watching::start(&app, |_| {});
    app.source("fn-a", "v2");
    app.source("fn-b", "v2");
    watching.settle();
    let failure = watching.builds.failure("fn-a").expect("a failure");
    assert!(
        failure
            .reason
            .starts_with("the build wrote an invalid module: "),
        "{failure:?}"
    );
    assert_eq!(
        watching.builds.failure("fn-b").unwrap().reason,
        "the build wrote no module at dist/function.wasm"
    );
    assert_eq!(watching.served("fn-a"), "v1");
    assert_eq!(watching.served("fn-b"), "v1");
}

#[test]
fn a_build_that_writes_an_invalid_module_keeps_the_last_one() {
    let app = TempApp::new("invalid");
    // The header of a module, then a section whose content is cut short.
    std::fs::write(app.ctl("truncated.wasm"), b"\0asm\x01\0\0\0\x01\x01\xff").unwrap();
    // A whole module, with nothing to run.
    std::fs::write(app.ctl("empty.wasm"), b"\0asm\x01\0\0\0").unwrap();
    app.function_with("fn-a", "cp ../../../ctl/truncated.wasm dist/function.wasm");
    app.function_with("fn-b", "cp ../../../ctl/empty.wasm dist/function.wasm");
    let watching = Watching::start(&app, |_| {});
    assert_eq!(watching.served("fn-a"), "v1");
    app.source("fn-a", "v2");
    app.source("fn-b", "v2");
    watching.settle();

    let failure = watching.builds.failure("fn-a").expect("a failure");
    assert!(
        failure
            .reason
            .starts_with("the build wrote an invalid module: ")
            && failure.reason.contains("unexpected end"),
        "{failure:?}"
    );
    assert_eq!(
        watching.builds.failure("fn-b").unwrap().reason,
        "the build wrote an invalid module: it exports no function"
    );
    assert_eq!(watching.served("fn-a"), "v1", "the last module that built");
    assert_eq!(watching.served("fn-b"), "v1");
    assert_eq!(app.dist("fn-a"), "invalid");
    let said = watching.said();
    assert!(
        said.contains("✗ fn-a: the build wrote an invalid module: "),
        "{said}"
    );
}

#[test]
fn a_function_that_never_built_is_not_built() {
    let app = TempApp::new("never");
    app.function("fn-a");
    // What a build stopped halfway left.
    app.write("extensions/fn-a/dist/function.wasm", "half-written");
    app.ctl_write("fail-fn-a", "");
    let watching = Watching::start(&app, |_| {});
    assert_eq!(app.builds("fn-a"), 1, "built at start: it has no module");
    assert_eq!(watching.served("fn-a"), "missing");
    assert!(watching.builds.failure("fn-a").is_some());
    assert_eq!(watching.builds.to_json()[0]["built"], false);
    let app_now = app.load(&watching.builds);
    assert!(!app_now.function("fn-a").unwrap().built());
    assert!(
        app_now
            .diagnostics
            .iter()
            .any(|d| d.extension.as_deref() == Some("fn-a")
                && d.message
                    == "no Wasm module yet: the watch builds it with `sh ../../../ctl/build.sh`"),
        "{:?}",
        app_now.diagnostics
    );
    assert_eq!(watching.reloads(), 1, "the pages show the error");
}

#[test]
fn what_a_build_writes_does_not_build_again() {
    let app = TempApp::new("outputs");
    app.function("fn-a");
    let watching = Watching::start(&app, |_| {});
    app.source("fn-a", "v2");
    watching.settle();
    assert_eq!(app.builds("fn-a"), 1);
    let reloads = watching.reloads();

    // The build wrote `schema.graphql` and `dist/`; tools write in `target/` and `.git/`.
    app.write("extensions/fn-a/dist/function.wasm", "by hand");
    app.write("extensions/fn-a/schema.graphql", "type Query { a: Int }");
    app.write("extensions/fn-a/target/debug/x", "x");
    app.write("extensions/fn-a/node_modules/a/index.js", "x");
    app.write("extensions/fn-a/.git/HEAD", "x");
    app.write("target/release/x", "x");
    app.write(".git/index", "x");
    watching.settle();
    watching.settle();
    assert_eq!(app.builds("fn-a"), 1, "{}", watching.said());
    assert_eq!(watching.reloads(), reloads);
}

#[test]
fn a_change_to_the_files_the_builds_share_builds_every_function() {
    let app = TempApp::new("shared");
    app.write("Cargo.toml", "[workspace]\nmembers = [\"extensions/*\"]\n");
    app.write("crates/common/src/lib.rs", "pub fn x() {}");
    app.write("shopify-local/store.json", "{}");
    app.write("README.md", "# App");
    app.function("fn-a");
    app.function("fn-b");
    app.ui_extension("ui-a");
    let watching = Watching::start(&app, |_| {});

    // The root manifest, with a change to one Function at the same time: one build each.
    app.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"extensions/*\"]\nresolver = \"2\"\n",
    );
    app.source("fn-a", "a2");
    watching.settle();
    assert_eq!(app.builds("fn-a"), 1, "{}", watching.said());
    assert_eq!(app.builds("fn-b"), 1, "{}", watching.said());
    assert_eq!(watching.served("fn-a"), "a2");
    assert_eq!(watching.reloads(), 1);
    assert!(
        watching
            .said()
            .contains("↻ Cargo.toml changed: building 2 Functions"),
        "{}",
        watching.said()
    );

    // A crate they share, and a lock file.
    app.write("crates/common/src/lib.rs", "pub fn x() -> u8 { 1 }");
    app.write("Cargo.lock", "# lock");
    watching.settle();
    assert_eq!(app.builds("fn-a"), 2);
    assert_eq!(app.builds("fn-b"), 2);

    // The store data and a document build nothing.
    app.write("shopify-local/store.json", "{\"name\": \"x\"}");
    app.write("README.md", "# App, again");
    watching.settle();
    assert_eq!(app.builds("fn-a"), 2, "{}", watching.said());
}

#[test]
fn the_data_folder_named_by_data_builds_nothing() {
    let app = TempApp::new("data");
    app.write("fixtures/store.json", "{}");
    app.function("fn-a");
    let data = app.path("fixtures");
    let watching = Watching::start(&app, |options| options.data = Some(data));
    app.write("fixtures/store.json", "{\"name\": \"x\"}");
    watching.settle();
    assert_eq!(app.builds("fn-a"), 0, "{}", watching.said());
}

#[test]
fn a_change_to_the_app_files_reloads_the_app() {
    let app = TempApp::new("toml");
    app.function("fn-a");
    let watching = Watching::start(&app, |_| {});

    // Another build command: the next build runs it.
    app.write_bytes("extensions/fn-a/src/other.wasm", &module("from build2"));
    app.function_with("fn-a", "sh ../../../ctl/build2.sh");
    watching.settle();
    assert_eq!(
        watching.served("fn-a"),
        "from build2",
        "{}",
        watching.said()
    );
    assert_eq!(app.builds("fn-a"), 0, "the old command did not run");

    // A new Function is built; one that is removed is forgotten.
    app.function("fn-b");
    app.source("fn-b", "b1");
    watching.settle();
    assert_eq!(app.builds("fn-b"), 1, "{}", watching.said());
    assert_eq!(watching.served("fn-b"), "b1");

    app.ctl_write("fail-fn-b", "");
    app.source("fn-b", "b2");
    watching.settle();
    assert!(watching.builds.failure("fn-b").is_some());
    let reloads = watching.reloads();
    std::fs::remove_dir_all(app.path("extensions/fn-b")).unwrap();
    watching.settle();
    assert_eq!(watching.builds.failure("fn-b"), None);
    assert_eq!(
        watching.served("fn-b"),
        "missing",
        "its module is withdrawn"
    );
    assert!(
        watching.said().contains("↻ fn-b removed"),
        "{}",
        watching.said()
    );
    assert_eq!(watching.reloads(), reloads + 1);

    // `shopify.app.toml` reloads the pages.
    app.write("shopify.app.toml", "name = \"Watch 2\"\n");
    watching.settle();
    assert_eq!(watching.reloads(), reloads + 2);
    assert!(watching.said().contains("↻ shopify.app.toml changed"));
}

#[test]
fn a_change_to_an_extension_without_a_build_reloads_the_pages_at_once() {
    let app = TempApp::new("ui");
    app.function("fn-a");
    app.ui_extension("ui-a");
    let watching = Watching::start(&app, |_| {});
    let generation = watching.builds.generation();

    app.write("extensions/ui-a/dist/ui-a.js", "// v2\n");
    watching.settle();
    assert!(watching.builds.generation() > generation);
    assert_eq!(watching.reloads(), 1);
    assert_eq!(app.builds("fn-a"), 0);
    assert!(
        watching.said().contains("↻ ui-a changed"),
        "{}",
        watching.said()
    );
}

#[test]
fn only_the_selected_functions_build() {
    let app = TempApp::new("selected");
    app.function("fn-a");
    app.function("fn-b");
    let watching = Watching::start(&app, |options| {
        options.extensions = vec!["fn-b".to_string()];
        options.build_first = true;
    });
    assert_eq!(app.builds("fn-a"), 0);
    assert_eq!(app.builds("fn-b"), 1);
    assert!(
        watching.said().contains("✓ 1 Function built in "),
        "{}",
        watching.said()
    );
    assert_eq!(
        watching.reloads(),
        1,
        "once, when the first builds are over"
    );
    // The other one runs its build output, as without the watch.
    let loaded = app.load(&watching.builds);
    assert!(
        loaded
            .function("fn-a")
            .unwrap()
            .wasm_path
            .starts_with(app.root())
    );

    app.source("fn-a", "a2");
    watching.settle();
    assert_eq!(app.builds("fn-a"), 0);
}

#[test]
fn shopify_s_cli_builds_a_javascript_function_and_bundles_the_checkout_ui_extensions() {
    let app = TempApp::new("shopify-cli");
    app.shopify_cli();
    app.javascript_function("bulk");
    app.ui_extension("ui-a");
    // Written after its bundle: the bundle is older than its source, and built at start.
    app.ui_source("ui-a", "first");
    let watching = Watching::start(&app, |_| {});
    assert_eq!(watching.served("bulk"), "v1", "{}", watching.said());
    assert!(app.bundle("ui-a").contains("first"), "{}", watching.said());
    let calls = app.cli_calls();
    assert_eq!(calls.len(), 2, "{calls:?}");
    // From the app's folder, the Functions first: `shopify app build` builds them too.
    let root = app.root().display().to_string();
    assert_eq!(
        calls[0],
        format!("{root} app function build --path {root}/extensions/bulk --no-input")
    );
    assert_eq!(
        calls[1],
        format!("{root} app build --path {root} --skip-dependencies-installation --no-input")
    );
    assert!(
        watching.said().contains("✓ ui-a built in"),
        "{}",
        watching.said()
    );

    let reloads = watching.reloads();
    app.ui_source("ui-a", "second");
    watching.settle();
    assert!(app.bundle("ui-a").contains("second"), "{}", watching.said());
    assert!(watching.reloads() > reloads);
    assert!(
        watching.said().contains("↻ ui-a changed: building ui-a"),
        "{}",
        watching.said()
    );
    // What it writes (bundles, modules, types) builds nothing again.
    assert_eq!(app.cli_calls().len(), 3, "{:?}", app.cli_calls());

    // A Function that changes while Shopify's CLI builds the app builds once it is done.
    app.ctl_write("gated-cli", "");
    app.ui_source("ui-a", "third");
    wait_for("the app build", || app.cli_calls().len() == 4);
    app.source("bulk", "v2");
    watching.seen();
    assert_eq!(app.cli_calls().len(), 4, "{:?}", app.cli_calls());
    app.ctl_write("go-cli", "");
    watching.settle();
    let calls = app.cli_calls();
    assert_eq!(calls.len(), 5, "{calls:?}");
    assert!(calls[4].contains(" app function build "), "{calls:?}");
    assert_eq!(watching.served("bulk"), "v2", "{}", watching.said());

    // A build that fails: the pages say so, and the last bundle stays.
    app.ctl_write("fail-cli", "");
    app.ui_source("ui-a", "broken");
    watching.settle();
    let failure = watching.builds.failure("ui-a").expect("a failure");
    assert!(
        failure.reason.contains("build failed"),
        "{}",
        failure.reason
    );
    assert!(
        failure.output.contains("[targeting.0.module]: Required"),
        "{}",
        failure.output
    );
    assert!(
        app.bundle("ui-a").contains("third"),
        "the last bundle stays"
    );
}

#[test]
fn a_watch_builds_at_start_what_has_no_module_or_changed_since_the_last() {
    let app = TempApp::new("record");
    app.function("fn-a");
    app.function("fn-b");
    std::fs::remove_file(app.path("extensions/fn-b/dist/function.wasm")).unwrap();
    let record = app.ctl("record/app");
    let options = |options: &mut Options| options.record = Some(app.ctl("record/app"));

    // Nothing recorded: the modules there are used, the missing one is built.
    let mut watching = Watching::start(&app, options);
    assert_eq!(app.builds("fn-a"), 0);
    assert_eq!(app.builds("fn-b"), 1);
    assert!(
        watching
            .said()
            .contains("watch:  using the existing builds; --build rebuilds them all at start"),
        "{}",
        watching.said()
    );
    // What each was built from and what it is, the one used as the one built.
    assert_eq!(app.recorded("fn-a").1, module_hash(&module("v1")));
    assert_eq!(app.recorded("fn-b").1, app.dist_hash("fn-b"));
    assert!(!app.recorded("fn-a").0.is_empty());
    watching.stop();

    // Nothing changed since: nothing builds.
    let mut watching = Watching::start(&app, options);
    assert_eq!(app.builds("fn-a") + app.builds("fn-b"), 1);
    assert!(!watching.said().contains("using the existing builds"));
    assert_eq!(watching.reloads(), 0);
    watching.stop();

    // Changed while nothing watched: built at start.
    app.source("fn-a", "a2");
    let mut watching = Watching::start(&app, options);
    assert_eq!(app.builds("fn-a"), 1);
    assert_eq!(app.builds("fn-b"), 1);
    assert_eq!(watching.served("fn-a"), "a2");
    watching.stop();

    // A build that failed is built again at the next start.
    app.ctl_write("fail-fn-b", "");
    app.source("fn-b", "b2");
    let mut watching = Watching::start(&app, options);
    watching.settle();
    assert_eq!(app.builds("fn-b"), 2);
    watching.stop();
    std::fs::remove_file(app.ctl("fail-fn-b")).unwrap();
    let mut watching = Watching::start(&app, options);
    assert_eq!(app.builds("fn-b"), 3);
    assert_eq!(watching.served("fn-b"), "b2");
    watching.stop();

    // `--build`: all of them.
    let watching = Watching::start(&app, |options| {
        options.record = Some(record.clone());
        options.build_first = true;
    });
    assert_eq!(app.builds("fn-a"), 2);
    assert_eq!(app.builds("fn-b"), 4);
    assert!(watching.said().starts_with("↻ building 2 Functions…"));
}

#[test]
fn the_first_builds_run_side_by_side_and_say_what_failed() {
    let app = TempApp::new("first");
    let handles = ["fn-a", "fn-b", "fn-c", "fn-d"];
    for handle in handles {
        app.function(handle);
        app.ctl_write(&format!("gated-{handle}"), "");
    }
    app.ctl_write("fail-fn-c", "");
    let lines = Arc::new(Mutex::new(Vec::new()));
    let said = lines.clone();
    let builds = Arc::new(Builds::publishing(Modules::create().unwrap()));
    let mut options = Options::new(Vec::new(), true);
    options.events = false;
    options.poll = Duration::from_millis(10);
    options.parallel = 2;
    let watcher = Watcher::start(
        &app.root(),
        builds.clone(),
        options,
        Box::new(|| {}),
        Arc::new(move |line: &str| said.lock().unwrap().push(line.to_string())),
    );
    wait_for("two builds", || {
        handles.iter().filter(|h| app.running(h, 1)).count() == 2
    });
    for handle in handles {
        app.go(handle, 1);
    }
    assert!(watcher.wait_idle(LONG));
    // Two at once, never more.
    assert_eq!(app.concurrency(), 2);
    let said = lines.lock().unwrap().join("\n");
    assert!(said.starts_with("↻ building 4 Functions…"), "{said}");
    assert!(said.contains("✗ fn-c: build failed"), "{said}");
    assert!(
        said.contains("✗ 1 of 4 Functions failed to build"),
        "{said}"
    );
    assert!(
        !said.contains("✓ fn-a"),
        "no line per build at the start: {said}"
    );
    assert_eq!(
        version(std::fs::read(builds.module_path("fn-c").unwrap()).ok()),
        "v1"
    );
}

#[test]
fn a_build_that_hangs_is_stopped() {
    let app = TempApp::new("hang");
    app.function("fn-a");
    app.function("fn-b");
    let watching = Watching::start(&app, |options| {
        options.timeout = Duration::from_millis(500);
    });
    // It waits for a go that never comes.
    app.ctl_write("gated-fn-a", "");
    app.source("fn-a", "a2");
    wait_for("the build to start", || app.running("fn-a", 1));

    // Another Function builds meanwhile.
    app.source("fn-b", "b2");
    wait_for("fn-b to build", || watching.served("fn-b") == "b2");
    watching.settle();
    let failure = watching.builds.failure("fn-a").expect("timed out");
    assert_eq!(failure.reason, "build timed out after 0.5s");
    assert!(failure.output.contains("compiling fn-a"), "{failure:?}");
    assert_eq!(watching.served("fn-a"), "v1");
}

#[test]
fn stopping_stops_the_builds_that_run() {
    let app = TempApp::new("stop");
    app.function("fn-a");
    let mut watching = Watching::start(&app, |_| {});
    app.ctl_write("gated-fn-a", "");
    app.source("fn-a", "a2");
    wait_for("the build to start", || {
        app.running("fn-a", 1) && app.ctl("pid-fn-a").is_file()
    });
    let pid: i32 = std::fs::read_to_string(app.ctl("pid-fn-a"))
        .unwrap()
        .trim()
        .parse()
        .unwrap();

    let stopping = Instant::now();
    watching.stop();
    assert!(
        stopping.elapsed() < Duration::from_secs(5),
        "{:?}",
        stopping.elapsed()
    );
    // The build is gone: nothing writes the module any more.
    wait_for("the build to end", || unsafe { libc::kill(pid, 0) } != 0);
    assert_eq!(app.dist("fn-a"), "v1");
}

#[test]
fn a_build_stopped_halfway_builds_again_at_the_next_start() {
    let app = TempApp::new("halfway");
    app.function("fn-a");
    let options = |options: &mut Options| options.record = Some(app.ctl("record/app"));
    // What is there is recorded.
    let mut watching = Watching::start(&app, options);
    watching.stop();

    // `--watch --build`, nothing changed: the build writes a whole module that is not what it
    // builds, and the watch is stopped before it is over.
    app.ctl_write("gated-fn-a", "");
    std::fs::write(app.ctl("early-fn-a"), module("stale")).unwrap();
    let mut forced = Options::new(Vec::new(), true);
    forced.events = false;
    forced.poll = Duration::from_millis(10);
    forced.record = Some(app.ctl("record/app"));
    let watcher = Watcher::start(
        &app.root(),
        Arc::new(Builds::publishing(Modules::create().unwrap())),
        forced,
        Box::new(|| {}),
        Arc::new(|_: &str| {}),
    );
    wait_for("the stale module", || {
        app.running("fn-a", 1) && app.dist("fn-a") == "stale"
    });
    watcher.stop();
    std::fs::remove_file(app.ctl("gated-fn-a")).unwrap();
    std::fs::remove_file(app.ctl("early-fn-a")).unwrap();

    // Its files are the recorded ones, but the module is not the one recorded: built again.
    let mut watching = Watching::start(&app, options);
    assert_eq!(app.builds("fn-a"), 2, "{}", watching.said());
    assert_eq!(watching.served("fn-a"), "v1");
    watching.stop();

    // A build that removes the module and writes half of one, as the playground's does.
    let mut watching = Watching::start(&app, options);
    app.ctl_write("partial-fn-a", "");
    app.ctl_write("gated-fn-a", "");
    app.source("fn-a", "a2");
    wait_for("the half-written module", || app.dist("fn-a") == "invalid");
    watching.stop();
    assert_eq!(app.dist("fn-a"), "invalid", "the build's folder is its own");
    std::fs::remove_file(app.ctl("partial-fn-a")).unwrap();
    std::fs::remove_file(app.ctl("gated-fn-a")).unwrap();
    let mut watching = Watching::start(&app, options);
    assert_eq!(app.builds("fn-a"), 4);
    assert_eq!(watching.served("fn-a"), "a2");
    watching.stop();

    // A module that a run cannot load (its header only) is not used, whatever the record says.
    app.write_bytes("extensions/fn-a/dist/function.wasm", b"\0asm\x01\0\0\0");
    let watching = Watching::start(&app, options);
    assert_eq!(app.builds("fn-a"), 5);
    assert_eq!(watching.served("fn-a"), "a2");
}

#[test]
fn a_module_written_since_the_record_is_built_again() {
    let app = TempApp::new("written-since");
    app.function("fn-a");
    let options = |options: &mut Options| options.record = Some(app.ctl("record/app"));
    let mut watching = Watching::start(&app, options);
    watching.stop();

    // A whole module, from the same files, that no build of a watch published.
    app.write_bytes("extensions/fn-a/dist/function.wasm", &module("stale"));
    let mut watching = Watching::start(&app, options);
    assert_eq!(app.builds("fn-a"), 1, "{}", watching.said());
    assert_eq!(watching.served("fn-a"), "v1");
    assert_eq!(app.recorded("fn-a").1, app.dist_hash("fn-a"));
    watching.stop();

    // What it built is trusted at the next start.
    let watching = Watching::start(&app, options);
    assert_eq!(app.builds("fn-a"), 1, "{}", watching.said());
}

#[test]
fn a_module_a_build_left_running_writes_later_is_built_again() {
    let app = TempApp::new("orphan");
    app.function("fn-a");
    let options = |options: &mut Options| options.record = Some(app.ctl("record/app"));
    let mut watching = Watching::start(&app, options);
    app.source("fn-a", "a2");
    watching.settle();
    assert_eq!(watching.served("fn-a"), "a2");
    assert_eq!(app.recorded("fn-a").1, app.dist_hash("fn-a"));

    // The build of a server killed before, still running, writes the module of the files it
    // read then, once this one recorded its own.
    app.write_bytes("extensions/fn-a/dist/function.wasm", &module("v1"));
    watching.seen();
    // What this server runs is the copy it published.
    assert_eq!(watching.served("fn-a"), "a2");
    assert_eq!(app.builds("fn-a"), 1);
    watching.stop();

    // The next one does not trust it: its files are the recorded ones, its bytes are not.
    let watching = Watching::start(&app, options);
    assert_eq!(app.builds("fn-a"), 2, "{}", watching.said());
    assert_eq!(watching.served("fn-a"), "a2");
    assert_eq!(app.dist("fn-a"), "a2");
}

#[test]
fn a_watch_that_finds_what_it_recorded_writes_nothing() {
    let app = TempApp::new("unchanged");
    app.function("fn-a");
    let options = |options: &mut Options| options.record = Some(app.ctl("record/app"));
    let mut watching = Watching::start(&app, options);
    app.source("fn-a", "a2");
    watching.settle();
    watching.stop();
    let dist = app.path("extensions/fn-a/dist/function.wasm");
    let before = (
        std::fs::read(&dist).unwrap(),
        std::fs::metadata(&dist).unwrap().modified().unwrap(),
    );
    let recorded = app.recorded("fn-a");

    let watching = Watching::start(&app, options);
    assert_eq!(app.builds("fn-a"), 1);
    assert_eq!(watching.served("fn-a"), "a2");
    assert!(!watching.said().contains("using the existing builds"));
    assert_eq!(watching.reloads(), 0);
    let after = (
        std::fs::read(&dist).unwrap(),
        std::fs::metadata(&dist).unwrap().modified().unwrap(),
    );
    assert_eq!(after, before, "the module is not written");
    assert_eq!(app.recorded("fn-a"), recorded);
}

#[test]
fn two_watches_of_one_app_build_a_function_one_after_the_other() {
    let app = TempApp::new("two-watches");
    app.function("fn-a");
    let options = |options: &mut Options| options.record = Some(app.ctl("record/app"));
    let mut watching = Watching::start(&app, options);
    watching.stop();

    // A builds fn-a, and waits. B starts meanwhile, and builds it too. From the same files,
    // each makes other bytes.
    app.ctl_write("gated-fn-a", "");
    std::fs::write(app.ctl("version-fn-a-1"), module("one")).unwrap();
    std::fs::write(app.ctl("version-fn-a-2"), module("two")).unwrap();
    let mut a = Watching::start(&app, options);
    app.source("fn-a", "a2");
    wait_for("the build of A", || app.running("fn-a", 1));
    let mut b = Watching::launch(&app, options);

    // B's build waits for A's to be over, and for A to record what it built.
    wait_for("B to wait", || b.said().contains(WAITING));
    assert!(!app.running("fn-a", 2));
    assert_eq!(app.builds("fn-a"), 1);
    app.go("fn-a", 1);
    wait_for("the build of B", || app.running("fn-a", 2));
    assert_eq!(app.recorded("fn-a").1, module_hash(&module("one")));
    app.go("fn-a", 2);
    a.settle();
    assert!(b.watcher().wait_idle(LONG), "{}", b.said());
    assert_eq!(a.served("fn-a"), "one");
    assert_eq!(b.served("fn-a"), "two");
    assert_eq!(app.recorded("fn-a").1, module_hash(&module("two")));
    a.stop();
    b.stop();
    std::fs::remove_file(app.ctl("gated-fn-a")).unwrap();
    assert_eq!(app.dist("fn-a"), "two");

    // The module is the one last recorded: trusted.
    let mut watching = Watching::start(&app, options);
    assert_eq!(app.builds("fn-a"), 2, "{}", watching.said());
    assert_eq!(watching.served("fn-a"), "two");
    watching.stop();

    // The module the other built, from the same files, is not: built again.
    app.write_bytes("extensions/fn-a/dist/function.wasm", &module("one"));
    let watching = Watching::start(&app, options);
    assert_eq!(app.builds("fn-a"), 3, "{}", watching.said());
    assert_eq!(watching.served("fn-a"), "a2");
}

/// The finding of the 7th review, as it was reproduced with three servers: A's build, from the
/// files of then, is left running when A is killed; the files change, and B builds them; A's
/// build overwrites `dist` before B's is over. B must not record what A's build wrote as what
/// its own built, or C trusts it. B's build waits for A's to end, as the lock A's build holds
/// says.
#[test]
fn a_build_left_running_by_a_killed_server_holds_the_lock_until_it_is_over() {
    let app = TempApp::new("orphan-lock");
    app.function_with("fn-a", "sh ../../../ctl/order.sh");
    let options = |options: &mut Options| options.record = Some(app.ctl("record/app"));
    // What is there is recorded.
    let mut watching = Watching::start(&app, options);
    watching.stop();

    // A builds from the files of then: its build writes `old`, once let go.
    std::fs::write(app.ctl("version-fn-a-1"), module("old")).unwrap();
    let mut a = Watching::start(&app, options);
    app.source("fn-a", "a2");
    wait_for("A's build", || app.running("fn-a", 1));
    // A is killed: its build goes on.
    a.watcher.take().unwrap().abandon();
    assert!(app.running("fn-a", 1));

    // The files change; B builds them. Its build writes their module, then waits.
    app.source("fn-a", "a3");
    app.ctl_write("write-first-fn-a-2", "");
    let mut b = Watching::launch(&app, options);
    wait_for("B's build to wait or to start", || {
        b.said().contains(WAITING) || app.running("fn-a", 2)
    });
    let waited = !app.running("fn-a", 2) && app.builds("fn-a") == 1;

    // A's build is over: it writes what it built from the files of then.
    app.go("fn-a", 1);
    wait_for("A's build to end", || !app.running("fn-a", 1));
    // B's build runs then, and is over.
    wait_for("B's build", || app.running("fn-a", 2));
    app.go("fn-a", 2);
    assert!(b.watcher().wait_idle(LONG), "{}", b.said());
    assert_eq!(b.served("fn-a"), "a3", "{}", b.said());
    assert_eq!(app.dist("fn-a"), "a3");
    assert_eq!(app.recorded("fn-a").1, module_hash(&module("a3")));
    assert!(waited, "B's build did not wait for A's:\n{}", b.said());
    b.stop();

    // The next start trusts what B built.
    let watching = Watching::start(&app, options);
    assert_eq!(app.builds("fn-a"), 2, "{}", watching.said());
    assert_eq!(watching.served("fn-a"), "a3");
}

#[test]
fn a_watch_does_not_trust_a_module_while_a_build_of_it_holds_its_lock() {
    let app = TempApp::new("held-at-start");
    app.function("fn-a");
    let options = |options: &mut Options| options.record = Some(app.ctl("record/app"));
    let mut watching = Watching::start(&app, options);
    watching.stop();
    assert_eq!(app.builds("fn-a"), 0);

    // Another server's build of it holds the lock: its files and module are the recorded
    // ones, but what the build writes is not known yet.
    let locks = Locks::new(Some(&app.ctl("record/app")));
    let held = locks.try_take("fn-a").unwrap().expect("a lock");
    assert!(locks.try_take("fn-a").is_err());
    let watching = Watching::launch(&app, options);
    wait_for("its build to wait", || watching.said().contains(WAITING));
    assert_eq!(app.builds("fn-a"), 0);
    // Meanwhile the checkout runs what `dist` holds.
    assert_eq!(watching.served("fn-a"), "v1");
    drop(held);
    assert!(watching.watcher().wait_idle(LONG), "{}", watching.said());
    assert_eq!(app.builds("fn-a"), 1);
    drop(watching);

    // Let go of, the same files and module are trusted.
    let watching = Watching::start(&app, options);
    assert_eq!(app.builds("fn-a"), 1, "{}", watching.said());
}

#[test]
fn stopping_stops_a_build_that_waits_for_the_lock() {
    let app = TempApp::new("stop-waiting");
    app.function("fn-a");
    let options = |options: &mut Options| options.record = Some(app.ctl("record/app"));
    let mut watching = Watching::start(&app, options);
    let held = Locks::new(Some(&app.ctl("record/app")))
        .try_take("fn-a")
        .unwrap()
        .expect("a lock");
    app.source("fn-a", "a2");
    wait_for("the build to wait", || watching.said().contains(WAITING));

    let stopping = Instant::now();
    watching.stop();
    assert!(
        stopping.elapsed() < Duration::from_secs(2),
        "{:?}",
        stopping.elapsed()
    );
    drop(held);
    assert_eq!(app.builds("fn-a"), 0);
}

#[test]
fn a_lock_only_what_a_build_started_still_holds_is_not_waited_for() {
    let app = TempApp::new("helper-lock");
    app.function("fn-a");
    let record = app.ctl("record/app");
    let locks = Locks::new(Some(&record));
    let lock_file = record.join("locks/fn-a.lock");
    // What a build started, and that outlives it (a compiler cache's server), holds the lock:
    // a process that holds the same open file.
    let helper = |said: &str| {
        let lock = locks.try_take("fn-a").unwrap().expect("a lock");
        std::fs::OpenOptions::new()
            .append(true)
            .open(&lock_file)
            .unwrap()
            .write_all(said.as_bytes())
            .unwrap();
        let file = lock.share();
        (lock, file)
    };

    // Its server let go of it.
    let (lock, kept) = helper("");
    drop(lock);
    let lock = locks.try_take("fn-a").unwrap().expect("not waited for");
    drop(lock);
    drop(kept);

    // Its server is gone, and so is the build's process group.
    let mut gone = std::process::Command::new("true").spawn().unwrap();
    let pid = gone.id();
    gone.wait().unwrap();
    let (lock, kept) = helper(&format!("group {pid}\n"));
    lock.abandon();
    let lock = locks.try_take("fn-a").unwrap().expect("not waited for");
    drop(lock);
    drop(kept);

    // The build's process group is still there: waited for.
    let mut build = std::process::Command::new("sleep")
        .arg("30")
        .process_group(0)
        .spawn()
        .unwrap();
    let (lock, kept) = helper(&format!("group {}\n", build.id()));
    lock.abandon();
    assert!(locks.try_take("fn-a").is_err());
    let _ = build.kill();
    let _ = build.wait();
    // Once it is gone, what is left is not.
    assert!(locks.try_take("fn-a").unwrap().is_some());
    drop(kept);
}

#[test]
fn a_record_that_cannot_be_read_builds_again() {
    let app = TempApp::new("unreadable");
    app.function("fn-a");
    let record = app.ctl("record/app");
    let options = |options: &mut Options| options.record = Some(app.ctl("record/app"));
    let mut watching = Watching::start(&app, options);
    watching.stop();
    let inputs = app.recorded("fn-a").0;

    // Cut short, or without the hash of the module.
    for (n, text) in [
        "{\"handle\":".to_string(),
        json!({ "handle": "fn-a", "inputs": inputs }).to_string(),
    ]
    .into_iter()
    .enumerate()
    {
        std::fs::write(record.join("inputs/fn-a.json"), text).unwrap();
        let mut watching = Watching::start(&app, options);
        assert_eq!(app.builds("fn-a"), n + 1, "{}", watching.said());
        assert!(!watching.said().contains("using the existing builds"));
        assert_eq!(app.recorded("fn-a").1, app.dist_hash("fn-a"));
        watching.stop();
    }

    // What earlier versions left next to it is removed, and changes nothing.
    std::fs::write(record.with_extension("json"), "{}").unwrap();
    std::fs::create_dir_all(record.join("building")).unwrap();
    std::fs::write(record.join("building/fn-a.old"), "").unwrap();
    let _watching = Watching::start(&app, options);
    assert_eq!(app.builds("fn-a"), 2);
    assert!(!record.with_extension("json").exists());
    assert!(!record.join("building").exists());
}

#[test]
fn markdown_documents_build_nothing_wherever_they_are() {
    let app = TempApp::new("markdown");
    app.function("fn-a");
    app.write("crates/shared/src/lib.rs", "pub fn shared() {}\n");
    let watching = Watching::start(&app, |_| {});
    let reloads = watching.reloads();

    app.write("README.md", "# App\n");
    app.write("docs/README.md", "# Docs\n");
    app.write("docs/guides/setup.md", "# Setup\n");
    app.write("crates/shared/README.md", "# Shared\n");
    app.write("extensions/fn-a/README.md", "# fn-a\n");
    watching.settle();
    watching.settle();
    assert_eq!(app.builds("fn-a"), 0, "{}", watching.said());
    assert_eq!(watching.reloads(), reloads);

    // What else is in those folders is watched.
    app.write("docs/schema.json", "{}\n");
    watching.settle();
    assert_eq!(app.builds("fn-a"), 1, "{}", watching.said());
}

#[test]
fn what_the_system_says_wakes_the_watcher_up() {
    let app = TempApp::new("events");
    app.function("fn-a");
    let watching = Watching::start(&app, |options| {
        options.events = true;
        // Only the events can tell it in time.
        options.idle = Duration::from_secs(60);
    });
    assert!(!watching.said().contains("polled"), "{}", watching.said());
    app.source("fn-a", "v2");
    wait_for("the build", || watching.served("fn-a") == "v2");
    watching.settle();
    assert_eq!(app.builds("fn-a"), 1);

    // A save that writes a file aside, then renames it over the source.
    app.write_bytes("extensions/fn-a/src/.main.wasm.tmp", &module("v3"));
    std::fs::rename(
        app.path("extensions/fn-a/src/.main.wasm.tmp"),
        app.path("extensions/fn-a/src/main.wasm"),
    )
    .unwrap();
    wait_for("the build after the rename", || {
        watching.served("fn-a") == "v3"
    });
    watching.settle();
    assert_eq!(app.builds("fn-a"), 2);

    // A build writes a lot where nobody looks: no build again.
    app.write("extensions/fn-a/target/debug/x", "x");
    app.write("extensions/fn-a/dist/function.wasm", "v2 again");
    watching.settle();
    assert_eq!(app.builds("fn-a"), 2);
}

#[test]
fn folder_by_folder_new_folders_are_followed_and_the_tools_ones_are_not() {
    let app = TempApp::new("folders");
    app.function("fn-a");
    app.write("extensions/fn-a/target/debug/x", "x");
    app.write("node_modules/a/index.js", "x");
    let watching = Watching::start(&app, |options| {
        options.events = true;
        options.each_folder = true;
        options.idle = Duration::from_secs(60);
    });
    let followed = watching
        .watcher()
        .followed()
        .expect("the system says when files change");
    assert!(followed.contains(&app.root()));
    assert!(followed.contains(&app.path("extensions/fn-a/src")));
    assert!(
        !followed
            .iter()
            .any(|folder| folder.ends_with("target") || folder.ends_with("node_modules")),
        "{followed:?}"
    );

    // A new folder: followed once seen, and a change in it builds.
    app.write("extensions/fn-a/src/deep/x.rs", "// 1");
    wait_for("the first build", || app.builds("fn-a") == 1);
    watching.settle();
    assert!(
        watching
            .watcher()
            .followed()
            .unwrap()
            .contains(&app.path("extensions/fn-a/src/deep"))
    );
    app.write("extensions/fn-a/src/deep/x.rs", "// 2, longer");
    wait_for("the build of the change in the new folder", || {
        app.builds("fn-a") == 2
    });
    watching.settle();
}

#[test]
fn the_watcher_goes_on_when_the_system_loses_track() {
    let app = TempApp::new("lost");
    app.function("fn-a");
    let watching = Watching::start(&app, |options| {
        options.events = true;
        options.idle = Duration::from_secs(60);
    });
    watching.watcher().system_error();
    watching.settle();
    app.source("fn-a", "v2");
    wait_for("the build", || watching.served("fn-a") == "v2");
    watching.settle();
    assert_eq!(app.builds("fn-a"), 1);
}

#[test]
fn a_symbolic_link_is_watched_where_it_leads() {
    #[cfg(unix)]
    {
        let app = TempApp::new("links");
        app.function("fn-a");
        let shared = app.0.join("shared");
        std::fs::create_dir_all(shared.join("common")).unwrap();
        std::fs::write(shared.join("lib.rs"), "// 1").unwrap();
        std::fs::write(shared.join("common/mod.rs"), "// 1").unwrap();
        std::os::unix::fs::symlink(
            shared.join("lib.rs"),
            app.path("extensions/fn-a/src/lib.rs"),
        )
        .unwrap();
        std::os::unix::fs::symlink(shared.join("common"), app.path("extensions/fn-a/common"))
            .unwrap();
        // A link that leads in a circle stops.
        std::os::unix::fs::symlink(
            app.path("extensions/fn-a"),
            app.path("extensions/fn-a/loop"),
        )
        .unwrap();
        let watching = Watching::start(&app, |_| {});

        std::fs::write(shared.join("lib.rs"), "// 2, longer").unwrap();
        watching.settle();
        assert_eq!(app.builds("fn-a"), 1, "{}", watching.said());
        std::fs::write(shared.join("common/mod.rs"), "// 2, longer").unwrap();
        watching.settle();
        assert_eq!(app.builds("fn-a"), 2, "{}", watching.said());
    }
}

#[test]
fn handles_that_differ_have_files_that_differ() {
    assert_eq!(encoded_handle("discount-codes"), "discount-codes");
    let names = ["fn.a", "fn_a", "fn a", "Fn-a", "fn/a", "fné"].map(encoded_handle);
    let unique: BTreeSet<&String> = names.iter().collect();
    assert_eq!(unique.len(), names.len(), "{names:?}");
    assert!(
        names
            .iter()
            .all(|name| !name.contains('.') && !name.contains('/'))
    );

    // What is recorded of one is not what is recorded of the other.
    let app = TempApp::new("alike");
    let record = Record::open(Some(app.ctl("record/app")), &app.root());
    let built = Built::new(1, module_hash(b"one"));
    record.set("fn.a", &built);
    assert_eq!(record.get("fn.a"), Some(built));
    assert!(!record.has("fn_a"));
    assert_eq!(record.get("fn_a"), None);
}
