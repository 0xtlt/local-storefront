//! Tests of `--watch`: what is looked at, when builds run, and what a build that fails leaves.
//!
//! The apps are made in a temporary folder, with a build command that is a shell script: it
//! "builds" a Function by copying `src/main.txt` to `dist/function.wasm`, counts its builds in
//! `count-<handle>` (in the app's folder, which is not watched), sleeps for as long as
//! `sleep-<handle>` says, and fails when `fail-<handle>` exists.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;

const BUILD: &str = r#"h=$(basename "$PWD")
echo build >> "../../count-$h"
echo "compiling $h"
if [ -f "../../sleep-$h" ]; then sleep "$(cat "../../sleep-$h")"; fi
mkdir -p dist
if [ -f "../../fail-$h" ]; then
  printf 'half-written' > dist/function.wasm
  echo "error: expected one of \`;\` or \`}\`" >&2
  exit 1
fi
cp src/main.txt dist/function.wasm
# As the playground's build does: a copy of the schema next to the Function.
echo "type Query" > schema.graphql
"#;

struct TempApp(PathBuf);

impl TempApp {
    fn new(label: &str) -> TempApp {
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("lsf-watch-{label}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(root.join("extensions")).unwrap();
        let app = TempApp(root.canonicalize().unwrap());
        app.write("shopify.app.toml", "name = \"Watch\"\nhandle = \"watch\"\n");
        app.write("build.sh", BUILD);
        app.write(
            "build2.sh",
            "mkdir -p dist\nprintf 'from build2' > dist/function.wasm\n",
        );
        app
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }

    fn write(&self, relative: &str, content: &str) {
        let path = self.path(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.path(relative)).unwrap_or_default()
    }

    /// A Function built by `command`, whose module is `v1`.
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
        self.write(&format!("extensions/{handle}/src/main.txt"), "v1");
        self.write(&format!("extensions/{handle}/dist/function.wasm"), "v1");
    }

    fn function(&self, handle: &str) {
        self.function_with(handle, "sh ../../build.sh");
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

    /// How many builds of `handle` started.
    fn builds(&self, handle: &str) -> usize {
        self.read(&format!("count-{handle}")).lines().count()
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

impl Watching {
    fn start(app: &TempApp, tune: impl FnOnce(&mut Options)) -> Watching {
        let mut options = Options::new(Vec::new(), false);
        options.poll = Duration::from_millis(10);
        // Polled, every 30 ms: the same on every system. One test follows the events.
        options.events = false;
        options.quiet = Duration::from_millis(80);
        tune(&mut options);
        let builds = Arc::new(Builds::default());
        let lines = Arc::new(Mutex::new(Vec::new()));
        let reloads = Arc::new(AtomicUsize::new(0));
        let said = lines.clone();
        let counted = reloads.clone();
        let watcher = Watcher::start(
            &app.0,
            builds.clone(),
            options,
            Box::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
            }),
            Arc::new(move |line: &str| said.lock().unwrap().push(line.to_string())),
        );
        // What the files are when it starts is what changes are measured from.
        assert!(watcher.wait_idle(Duration::from_secs(20)));
        std::thread::sleep(Duration::from_millis(30));
        Watching {
            watcher: Some(watcher),
            builds,
            lines,
            reloads,
        }
    }

    fn reloads(&self) -> usize {
        self.reloads.load(Ordering::SeqCst)
    }

    fn said(&self) -> String {
        self.lines.lock().unwrap().join("\n")
    }

    /// Waits for the changes to count and the builds to end.
    fn settle(&self) {
        // Long enough for a change to be seen and to hold still.
        std::thread::sleep(Duration::from_millis(150));
        assert!(
            self.watcher
                .as_ref()
                .unwrap()
                .wait_idle(Duration::from_secs(20)),
            "the builds did not end:\n{}",
            self.said()
        );
    }
}

fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
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
    ] {
        assert!(ignored(Path::new(left_alone), &built), "{left_alone}");
    }
}

#[test]
fn a_burst_of_changes_counts_once_when_the_files_hold_still() {
    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    let mut debounce = Debounce::new(Duration::from_millis(100), Duration::from_secs(1));
    let change = |name: &str| Changes {
        config: false,
        extensions: [name.to_string()].into(),
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
    // `d` is gone before its turn... it ran already; `e` is dropped while it waits.
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
    let watching = Watching::start(&app, |options| options.quiet = Duration::from_millis(150));
    let generation = watching.builds.generation();

    // An editor's save: several files, a few milliseconds apart.
    for (index, file) in [
        "src/main.txt",
        "src/lib.rs",
        "src/run.graphql",
        "src/main.txt",
    ]
    .iter()
    .enumerate()
    {
        app.write(&format!("extensions/fn-a/{file}"), &format!("v2 {index}"));
        std::thread::sleep(Duration::from_millis(15));
    }
    app.write("extensions/fn-a/src/main.txt", "v2");
    watching.settle();

    assert_eq!(app.builds("fn-a"), 1, "{}", watching.said());
    assert_eq!(app.read("extensions/fn-a/dist/function.wasm"), "v2");
    assert!(watching.builds.generation() > generation);
    assert_eq!(watching.reloads(), 1);
    let said = watching.said();
    assert!(said.contains("↻ building fn-a…"), "{said}");
    assert!(said.contains("✓ fn-a built in "), "{said}");
}

#[test]
fn a_change_during_a_build_builds_exactly_once_more() {
    let app = TempApp::new("during");
    app.function("fn-a");
    let watching = Watching::start(&app, |_| {});
    app.write("sleep-fn-a", "0.6");

    app.write("extensions/fn-a/src/main.txt", "v2");
    wait_for("the build to start", || app.builds("fn-a") == 1);
    // Saved three times while it builds.
    for version in ["v3", "v4", "v5"] {
        app.write("extensions/fn-a/src/main.txt", version);
        std::thread::sleep(Duration::from_millis(120));
    }
    watching.settle();

    assert_eq!(app.builds("fn-a"), 2, "{}", watching.said());
    assert_eq!(app.read("extensions/fn-a/dist/function.wasm"), "v5");
    // The pages reload once, after the last build.
    assert_eq!(watching.reloads(), 1);
}

#[test]
fn a_build_that_fails_keeps_the_last_module_and_says_why() {
    let app = TempApp::new("fail");
    app.function("fn-a");
    let watching = Watching::start(&app, |_| {});
    app.write("extensions/fn-a/src/main.txt", "v2");
    watching.settle();
    assert_eq!(app.read("extensions/fn-a/dist/function.wasm"), "v2");

    // It breaks: the build writes half a module and fails.
    app.write("fail-fn-a", "");
    app.write("extensions/fn-a/src/main.txt", "v3, broken");
    watching.settle();
    assert_eq!(app.builds("fn-a"), 2);
    assert_eq!(
        app.read("extensions/fn-a/dist/function.wasm"),
        "v2",
        "the last module that built"
    );
    let failure = watching.builds.failure("fn-a").expect("a failure");
    assert!(
        failure.reason.starts_with("build failed (exit"),
        "{failure:?}"
    );
    assert!(failure.output.contains("compiling fn-a"), "{failure:?}");
    assert!(failure.output.contains("expected one of"), "{failure:?}");
    assert_eq!(watching.builds.to_json()[0]["handle"], "fn-a");
    let said = watching.said();
    assert!(said.contains("✗ fn-a: build failed (exit"), "{said}");
    assert!(said.contains("\n    error: expected one of"), "{said}");
    assert_eq!(watching.reloads(), 2, "the pages show the error");

    // Fixed.
    std::fs::remove_file(app.path("fail-fn-a")).unwrap();
    app.write("extensions/fn-a/src/main.txt", "v4");
    watching.settle();
    assert_eq!(app.read("extensions/fn-a/dist/function.wasm"), "v4");
    assert_eq!(watching.builds.failure("fn-a"), None);
    assert_eq!(watching.reloads(), 3);
}

#[test]
fn what_a_build_writes_does_not_build_again() {
    let app = TempApp::new("outputs");
    app.function("fn-a");
    let watching = Watching::start(&app, |_| {});
    app.write("extensions/fn-a/src/main.txt", "v2");
    watching.settle();
    assert_eq!(app.builds("fn-a"), 1);
    let reloads = watching.reloads();

    // The build wrote `schema.graphql` and `dist/`; tools write in `target/` and `.git/`.
    app.write("extensions/fn-a/dist/function.wasm", "by hand");
    app.write("extensions/fn-a/schema.graphql", "type Query { a: Int }");
    app.write("extensions/fn-a/target/debug/x", "x");
    app.write("extensions/fn-a/node_modules/a/index.js", "x");
    app.write("extensions/fn-a/.git/HEAD", "x");
    app.write("build.sh", BUILD);
    app.write("count-other", "x");
    watching.settle();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(app.builds("fn-a"), 1, "{}", watching.said());
    assert_eq!(watching.reloads(), reloads);
}

#[test]
fn a_change_to_the_app_files_reloads_the_app() {
    let app = TempApp::new("toml");
    app.function("fn-a");
    let watching = Watching::start(&app, |_| {});

    // Another build command: the next build runs it.
    app.function_with("fn-a", "sh ../../build2.sh");
    watching.settle();
    assert_eq!(
        app.read("extensions/fn-a/dist/function.wasm"),
        "from build2"
    );
    assert_eq!(app.builds("fn-a"), 0, "the old command did not run");

    // A new Function is built; one that is removed is forgotten.
    app.function("fn-b");
    app.write("extensions/fn-b/src/main.txt", "b1");
    watching.settle();
    assert_eq!(app.builds("fn-b"), 1, "{}", watching.said());
    assert_eq!(app.read("extensions/fn-b/dist/function.wasm"), "b1");

    app.write("fail-fn-b", "");
    app.write("extensions/fn-b/src/main.txt", "b2");
    watching.settle();
    assert!(watching.builds.failure("fn-b").is_some());
    let reloads = watching.reloads();
    std::fs::remove_dir_all(app.path("extensions/fn-b")).unwrap();
    watching.settle();
    assert_eq!(watching.builds.failure("fn-b"), None);
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

    app.write("extensions/fn-a/src/main.txt", "a2");
    watching.settle();
    assert_eq!(app.builds("fn-a"), 0);
}

#[test]
fn the_first_builds_run_side_by_side_and_say_what_failed() {
    let app = TempApp::new("first");
    for handle in ["fn-a", "fn-b", "fn-c", "fn-d"] {
        app.function(handle);
        app.write(&format!("sleep-{handle}"), "0.5");
    }
    app.write("fail-fn-c", "");
    let started = Instant::now();
    let watching = Watching::start(&app, |options| {
        options.build_first = true;
        options.parallel = 2;
    });
    // Four builds of 0.5 s, two at a time.
    let took = started.elapsed();
    assert!(took >= Duration::from_millis(1000), "{took:?}");
    assert!(took < Duration::from_millis(1800), "{took:?}");
    let said = watching.said();
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
    assert_eq!(app.read("extensions/fn-c/dist/function.wasm"), "v1");
}

#[test]
fn a_build_that_hangs_is_stopped() {
    let app = TempApp::new("hang");
    app.function("fn-a");
    app.function("fn-b");
    let watching = Watching::start(&app, |options| {
        options.timeout = Duration::from_millis(500);
    });
    app.write("sleep-fn-a", "30");
    app.write("extensions/fn-a/src/main.txt", "a2");
    wait_for("the build to start", || app.builds("fn-a") == 1);

    // Another Function builds meanwhile.
    app.write("extensions/fn-b/src/main.txt", "b2");
    wait_for("fn-b to build", || {
        app.read("extensions/fn-b/dist/function.wasm") == "b2"
    });
    watching.settle();
    let failure = watching.builds.failure("fn-a").expect("timed out");
    assert_eq!(failure.reason, "build timed out after 0.5s");
    assert!(failure.output.contains("compiling fn-a"), "{failure:?}");
    assert_eq!(app.read("extensions/fn-a/dist/function.wasm"), "v1");
}

#[test]
fn stopping_stops_the_builds_that_run() {
    let app = TempApp::new("stop");
    app.function("fn-a");
    let mut watching = Watching::start(&app, |_| {});
    app.write("sleep-fn-a", "30");
    app.write("extensions/fn-a/src/main.txt", "a2");
    wait_for("the build to start", || app.builds("fn-a") == 1);

    let stopping = Instant::now();
    watching.watcher.take().unwrap().stop();
    assert!(
        stopping.elapsed() < Duration::from_secs(3),
        "{:?}",
        stopping.elapsed()
    );
    // The `sleep` it started is gone with it: nothing writes the module any more.
    std::fs::remove_file(app.path("sleep-fn-a")).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(app.read("extensions/fn-a/dist/function.wasm"), "v1");
}

#[test]
fn a_build_stopped_halfway_puts_the_module_back() {
    let app = TempApp::new("halfway");
    // It removes the module first, as the playground's build does, then takes its time.
    app.function_with("fn-a", "rm dist/function.wasm && sleep 30");
    let mut watching = Watching::start(&app, |_| {});
    app.write("extensions/fn-a/src/main.txt", "a2");
    wait_for("the build to remove the module", || {
        !app.path("extensions/fn-a/dist/function.wasm").exists()
    });
    watching.watcher.take().unwrap().stop();
    assert_eq!(app.read("extensions/fn-a/dist/function.wasm"), "v1");
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
    app.write("extensions/fn-a/src/main.txt", "v2");
    wait_for("the build", || {
        app.read("extensions/fn-a/dist/function.wasm") == "v2"
    });
    watching.settle();
    assert_eq!(app.builds("fn-a"), 1);

    // A build writes a lot where nobody looks: no build again.
    app.write("extensions/fn-a/target/debug/x", "x");
    app.write("extensions/fn-a/dist/function.wasm", "v2 again");
    watching.settle();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(app.builds("fn-a"), 1);
}
