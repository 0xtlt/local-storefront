//! `--watch`: the Functions of the app are built again when their files change, and the open
//! pages reload, as `shopify app dev` does.
//!
//! The system says when a file of the app changes (FSEvents, inotify, ReadDirectoryChangesW,
//! through `notify`). The watcher then walks the app and compares the size and the
//! modification time of its files with what it saw last, to know what changed and to leave
//! alone what is not looked at (the build outputs, `target/`): what the system says only wakes
//! it up. A walk of an app of 90 extensions takes about 3 ms, so polling every 100 ms would
//! keep 3 to 5 % of a core busy for nothing; with the events the watcher idles, and still walks
//! every few seconds in case an event was missed. Where the events cannot be had (a file system
//! that has none), it polls, less often.
//!
//! - A change in a Function that has a build command (`[extensions.build] command`) builds it
//!   again, in its folder. One build at a time per Function; what changes during a build
//!   builds it once more after it. Several Functions build at once, up to [`PARALLEL`].
//! - A change to the other files of the app (a Cargo workspace's `Cargo.toml` and
//!   `Cargo.lock`, a `package.json`, a crate the Functions share) builds every Function.
//! - The checkout never runs the module a build writes: it runs a copy of the last one that
//!   built ([`Modules`]), outside the app, replaced in one step once a build is over and its
//!   module is whole. A build that fails leaves that copy, and the pages say that the build
//!   failed, with its first error and the end of its output.
//! - Any other change (a checkout UI extension's bundle, an extension without a build,
//!   `shopify.app.toml`, an extension added or removed) reloads the pages.
//!
//! The pages reload through live reload: [`Builds::generation`] is part of its token. They
//! reload once the builds a change started are all over.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::hash::Hasher;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use lsf_checkout::app::{self, App, Selection};
use serde_json::{Value as Json, json};

/// How many builds run at once. Functions of one Cargo workspace wait for each other on its
/// lock anyway; more would only compete for the processor.
pub const PARALLEL: usize = 3;

/// How many lines of the first error of a build that fails are kept.
const FIRST_ERROR: usize = 40;

/// How many of the last lines of the output of a build that fails are kept.
const TAIL: usize = 20;

/// The builds of the app, as the pages show them, and the modules the checkout runs.
#[derive(Default)]
pub struct Builds {
    /// Moves on whenever the pages should reload: builds are over, or a file they show changed.
    generation: AtomicU64,
    /// The Functions whose last build failed, by handle.
    failures: Mutex<BTreeMap<String, Failure>>,
    /// With `--watch`: the copies of the modules the checkout runs.
    modules: Option<Modules>,
    /// The Functions the watcher builds, whose copy the checkout runs.
    owned: Mutex<BTreeSet<String>>,
}

/// Why a build failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// `build failed (exit status: 101)`, `build timed out after 600s`.
    pub reason: String,
    /// Its first error and the last lines it printed.
    pub output: String,
}

impl Builds {
    /// The builds of `--watch`, whose modules are published in `modules`.
    pub fn publishing(modules: Modules) -> Builds {
        Builds {
            modules: Some(modules),
            ..Builds::default()
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    pub(crate) fn bump(&self) {
        self.generation.fetch_add(1, Ordering::Relaxed);
    }

    /// The Functions whose last build failed, by handle.
    pub fn failures(&self) -> Vec<(String, Failure)> {
        let failures = self.failures.lock().expect("builds poisoned");
        failures
            .iter()
            .map(|(handle, failure)| (handle.clone(), failure.clone()))
            .collect()
    }

    pub fn failure(&self, handle: &str) -> Option<Failure> {
        let failures = self.failures.lock().expect("builds poisoned");
        failures.get(handle).cloned()
    }

    pub(crate) fn set(&self, handle: &str, failure: Option<Failure>) {
        let mut failures = self.failures.lock().expect("builds poisoned");
        match failure {
            Some(failure) => failures.insert(handle.to_string(), failure),
            None => failures.remove(handle),
        };
    }

    /// Forgets the Functions that are not in `handles` any more.
    fn retain(&self, handles: &BTreeSet<String>) {
        let mut failures = self.failures.lock().expect("builds poisoned");
        failures.retain(|handle, _| handles.contains(handle));
    }

    /// The builds that failed, as the checkout page reads them: `builds` of its state.
    pub fn to_json(&self) -> Json {
        Json::Array(
            self.failures()
                .into_iter()
                .map(|(handle, failure)| {
                    json!({"handle": handle, "reason": failure.reason, "output": failure.output})
                })
                .collect(),
        )
    }

    /// Makes the Functions the watcher builds run their published module: the checkout, the
    /// runs of the home page and the Functions dialog read it, never what a build is writing.
    /// Nothing changes outside `--watch`.
    pub fn apply(&self, app: &mut App) {
        let Some(modules) = &self.modules else {
            return;
        };
        let owned = self.owned.lock().expect("builds poisoned").clone();
        for handle in owned {
            app.use_module(&handle, modules.path(&handle));
        }
    }

    /// Where the module of `handle` is published, with `--watch`.
    #[cfg(test)]
    pub fn module_path(&self, handle: &str) -> Option<PathBuf> {
        self.modules.as_ref().map(|modules| modules.path(handle))
    }

    /// Removes the published modules: the server stops.
    pub fn close(&self) {
        if let Some(modules) = &self.modules {
            modules.remove();
        }
    }

    /// Has the watcher build `handles`, and publishes `module` for some: what the tests of the
    /// server set up.
    #[cfg(test)]
    pub(crate) fn own_for_tests(&self, handles: &[&str], published: &[(&str, &[u8])]) {
        self.set_owned(handles.iter().map(|handle| handle.to_string()).collect());
        for (handle, module) in published {
            self.modules().publish(handle, module).unwrap();
        }
    }

    fn modules(&self) -> &Modules {
        self.modules
            .as_ref()
            .expect("the builds of --watch publish their modules")
    }

    fn set_owned(&self, handles: BTreeSet<String>) {
        *self.owned.lock().expect("builds poisoned") = handles;
    }

    fn owns(&self, handle: &str) -> bool {
        self.owned.lock().expect("builds poisoned").contains(handle)
    }
}

/// The modules the checkout runs with `--watch`: a copy of the last module that built, for
/// each Function, in a folder of the system's temporary folder that is the server's own.
pub struct Modules {
    dir: PathBuf,
}

/// The folders of [`Modules`] start with this, then the process id.
const MODULES_PREFIX: &str = "lsf-watch-modules-";

impl Modules {
    /// A new folder for the modules. Those of servers that are gone without removing theirs are
    /// removed.
    pub fn create() -> std::io::Result<Modules> {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let base = std::env::temp_dir();
        remove_stale(&base);
        let dir = base.join(format!(
            "{MODULES_PREFIX}{}-{}",
            std::process::id(),
            COUNT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        Ok(Modules { dir })
    }

    /// The copy of the module of `handle`. It exists once a module of it was published.
    pub fn path(&self, handle: &str) -> PathBuf {
        self.dir.join(format!("{}.wasm", file_name(handle)))
    }

    /// Publishes a module that built, in one step: a request reads the old one or this one.
    fn publish(&self, handle: &str, bytes: &[u8]) -> std::io::Result<()> {
        let partial = self.dir.join(format!("{}.wasm.partial", file_name(handle)));
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(&partial, bytes)?;
        std::fs::rename(&partial, self.path(handle)).inspect_err(|_| {
            let _ = std::fs::remove_file(&partial);
        })
    }

    /// Publishes the module the build of `job` left, if it is a whole one. Returns whether it
    /// did.
    fn adopt(&self, job: &Job) -> bool {
        match std::fs::read(&job.module) {
            Ok(bytes) if is_module(&bytes) => self.publish(&job.handle, &bytes).is_ok(),
            _ => false,
        }
    }

    fn withdraw(&self, handle: &str) {
        let _ = std::fs::remove_file(self.path(handle));
    }

    fn remove(&self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Drop for Modules {
    fn drop(&mut self) {
        self.remove();
    }
}

/// A handle as a file name.
fn file_name(handle: &str) -> String {
    handle
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Removes the folders of [`Modules`] of the processes that are gone (killed before they could).
fn remove_stale(base: &Path) {
    #[cfg(unix)]
    for entry in std::fs::read_dir(base).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(pid) = name
            .strip_prefix(MODULES_PREFIX)
            .and_then(|rest| rest.split('-').next())
            .and_then(|pid| pid.parse::<i32>().ok())
        else {
            continue;
        };
        // SAFETY: signal 0 only asks whether the process exists.
        let gone = unsafe { libc::kill(pid, 0) } != 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        if gone {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
    #[cfg(not(unix))]
    let _ = base;
}

/// Whether `bytes` are a whole Wasm module: its header, then sections that end where the bytes
/// do. A module cut short (a build still writing it) is not.
pub fn is_module(bytes: &[u8]) -> bool {
    if bytes.len() < 8 || &bytes[..4] != b"\0asm" {
        return false;
    }
    let mut at = 8;
    while at < bytes.len() {
        // The section's id, then its size.
        at += 1;
        let mut size: usize = 0;
        let mut shift = 0;
        loop {
            let Some(&byte) = bytes.get(at) else {
                return false;
            };
            at += 1;
            size |= usize::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift > 28 {
                return false;
            }
        }
        at = match at.checked_add(size) {
            Some(end) if end <= bytes.len() => end,
            _ => return false,
        };
    }
    true
}

/// How the watcher looks at the files and runs the builds.
#[derive(Clone, Debug)]
pub struct Options {
    /// The extensions `--extension` selects: only those are built. All when empty.
    pub extensions: Vec<String>,
    /// Build every selected Function when the watcher starts (`--watch --build`). Without it,
    /// only the Functions that have no module, or whose files changed since the last watch.
    pub build_first: bool,
    /// The store data folder: the server reloads the pages when it changes, it builds nothing.
    /// `shopify-local/` of the app is left alone too.
    pub data: Option<PathBuf>,
    /// Where what the Functions were built from is recorded, from one watch to the next. Not
    /// recorded without it.
    pub record: Option<PathBuf>,
    /// How often the files are looked at while they change.
    pub poll: Duration,
    /// Follow what the system says of the files. Without it (or when the system cannot say),
    /// the files are polled every three `poll`.
    pub events: bool,
    /// Ask the system for each folder looked at, rather than for the app and all it holds:
    /// inotify watches a folder at a time, and the folders of the tools (`target/`,
    /// `node_modules/`) would use up what it allows.
    pub each_folder: bool,
    /// How often the files are looked at anyway while nothing changes, in case an event was
    /// missed.
    pub idle: Duration,
    /// How long the files must hold still after a change before it counts: a save writes more
    /// than one file (a formatter, an editor's backup), and builds once.
    pub quiet: Duration,
    /// Files that never hold still (a log written in an extension) do not hold a change back
    /// longer than this.
    pub max_wait: Duration,
    pub parallel: usize,
    /// A build that takes longer is stopped and fails.
    pub timeout: Duration,
}

impl Options {
    pub fn new(extensions: Vec<String>, build_first: bool) -> Options {
        Options {
            extensions,
            build_first,
            data: None,
            record: None,
            poll: Duration::from_millis(100),
            events: true,
            // FSEvents and ReadDirectoryChangesW follow a folder and all it holds at no cost.
            each_folder: !cfg!(any(target_os = "macos", windows)),
            idle: Duration::from_secs(3),
            quiet: Duration::from_millis(150),
            max_wait: Duration::from_secs(2),
            parallel: PARALLEL,
            timeout: Duration::from_secs(600),
        }
    }
}

/// Where what the Functions of the app in `root` were built from is recorded: in the user's
/// cache folder, never in the app.
pub fn record_file(root: &Path) -> Option<PathBuf> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut hasher = Fnv::default();
    hasher.write(root.as_os_str().as_encoded_bytes());
    Some(
        cache_dir()?
            .join("lsf")
            .join("watch")
            .join(format!("{:016x}.json", hasher.finish())),
    )
}

/// The user's cache folder.
fn cache_dir() -> Option<PathBuf> {
    let var = |name: &str| {
        std::env::var_os(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    if cfg!(windows) {
        var("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|home| home.join("Library").join("Caches"))
    } else {
        var("XDG_CACHE_HOME").or_else(|| var("HOME").map(|home| home.join(".cache")))
    }
}

/// Where the watcher says what it does: a line at a time.
pub type Log = Arc<dyn Fn(&str) + Send + Sync>;

/// The watcher of an app, running until it is stopped (or dropped).
pub struct Watcher {
    shared: Arc<Shared>,
    sender: mpsc::Sender<Event>,
    thread: Option<JoinHandle<()>>,
}

/// What wakes the watcher's thread up.
enum Event {
    /// A build is over: the Function, how it ended, and a hash of what it was built from.
    Finished(String, Outcome, u64),
    /// The system says that a file changed.
    Touched,
    Stop,
}

struct Shared {
    stop: AtomicBool,
    status: Mutex<Status>,
    changed: Condvar,
    /// The folders the system is asked about, for the tests.
    #[cfg(test)]
    followed: Mutex<Option<BTreeSet<PathBuf>>>,
}

/// What the watcher is doing, for the tests to wait on.
#[derive(Clone, Copy, Debug, Default)]
struct Status {
    /// How many looks at the files started, and how many are over.
    started: u64,
    looked: u64,
    /// A change waits to count, or the pages to reload.
    pending: bool,
    /// A build is queued or running.
    building: bool,
}

impl Watcher {
    /// Starts watching the app in `root`. `on_change` runs whenever the pages are told to
    /// reload, after `builds` changed. The modules that built before are published before it
    /// returns.
    pub fn start(
        root: &Path,
        builds: Arc<Builds>,
        options: Options,
        on_change: Box<dyn Fn() + Send>,
        log: Log,
    ) -> Watcher {
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            status: Mutex::new(Status {
                pending: true,
                ..Status::default()
            }),
            changed: Condvar::new(),
            #[cfg(test)]
            followed: Mutex::new(None),
        });
        let (sender, receiver) = mpsc::channel();
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let events = if options.events {
            match Follow::start(&root, options.each_folder, sender.clone()) {
                Ok(follow) => Some(follow),
                Err(error) => {
                    log(&format!(
                        "watch:  the system does not say when files change ({error}): they are polled"
                    ));
                    None
                }
            }
        } else {
            None
        };
        let data = options
            .data
            .as_ref()
            .map(|data| data.canonicalize().unwrap_or_else(|_| data.clone()));
        let mut coordinator = Coordinator {
            idle: if events.is_some() {
                options.idle
            } else {
                options.poll * 3
            },
            events,
            dirty: false,
            debounce: Debounce::new(options.quiet, options.max_wait),
            scheduler: Scheduler::new(options.parallel),
            selection: if options.extensions.is_empty() {
                Selection::all()
            } else {
                Selection::only(options.extensions.iter().cloned())
            },
            record: Record::read(options.record.clone(), &root),
            data,
            root,
            builds,
            options,
            on_change,
            log,
            shared: shared.clone(),
            rules: BTreeMap::new(),
            seen: Snapshot::default(),
            sender: sender.clone(),
            workers: Vec::new(),
            initial: None,
            reload_due: false,
            started: 0,
            looked: 0,
            shared_content: None,
        };
        let first = coordinator.prepare();
        let thread = std::thread::Builder::new()
            .name("lsf-watch".to_string())
            .spawn(move || coordinator.run(receiver, first))
            .expect("cannot start the watcher");
        Watcher {
            shared,
            sender,
            thread: Some(thread),
        }
    }

    /// Stops the builds that run and the watching.
    pub fn stop(mut self) {
        self.halt();
    }

    fn halt(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        let _ = self.sender.send(Event::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
impl Watcher {
    /// Waits until `done` says so of what the watcher does, for `timeout` at most. Returns
    /// whether it did.
    fn wait_for(&self, timeout: Duration, done: impl Fn(&Status) -> bool) -> bool {
        let status = self.shared.status.lock().expect("watcher poisoned");
        let (status, _) = self
            .shared
            .changed
            .wait_timeout_while(status, timeout, |status| !done(status))
            .expect("watcher poisoned");
        done(&status)
    }

    /// Waits until no change waits to count and no build is queued or running.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        self.wait_for(timeout, |status| !status.pending && !status.building)
    }

    /// Has the files looked at, as an event would, and waits until a look that started after
    /// this call is over and what it saw counted: with `builds`, until the builds are over too.
    fn look_and_wait(&self, timeout: Duration, builds: bool) -> bool {
        let started = self.shared.status.lock().expect("watcher poisoned").started;
        let _ = self.sender.send(Event::Touched);
        self.wait_for(timeout, |status| {
            status.looked > started && !status.pending && !(builds && status.building)
        })
    }

    /// The folders the system is asked about, when it says when files change.
    fn followed(&self) -> Option<BTreeSet<PathBuf>> {
        self.shared
            .followed
            .lock()
            .expect("watcher poisoned")
            .clone()
    }

    /// What the system would say when it lost track of the files.
    fn system_error(&self) {
        on_event(Err(notify::Error::generic("events lost")), &self.sender);
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.halt();
    }
}

/// What the system says of the files of the app.
struct Follow {
    watcher: notify::RecommendedWatcher,
    each_folder: bool,
    root: PathBuf,
    /// The folders asked for one at a time.
    watched: BTreeSet<PathBuf>,
    /// A folder could not be followed: said once.
    failed: bool,
}

impl Follow {
    /// Asks the system to say when a file of the app changes: for the app and all it holds, or
    /// (`each_folder`) for each folder the watcher looks at, which [`Follow::sync`] keeps up.
    fn start(
        root: &Path,
        each_folder: bool,
        sender: mpsc::Sender<Event>,
    ) -> notify::Result<Follow> {
        use notify::Watcher as _;
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                on_event(event, &sender)
            })?;
        if each_folder {
            watcher.watch(root, notify::RecursiveMode::NonRecursive)?;
        } else {
            watcher.watch(root, notify::RecursiveMode::Recursive)?;
        }
        Ok(Follow {
            watcher,
            each_folder,
            root: root.to_path_buf(),
            watched: BTreeSet::from([root.to_path_buf()]),
            failed: false,
        })
    }

    /// Follows the folders the watcher looked at (`folders`), and no others: with
    /// `each_folder`, all of them; otherwise, those out of the app (where a symbolic link
    /// leads).
    fn sync(&mut self, folders: &BTreeSet<PathBuf>, log: &Log) {
        use notify::Watcher as _;
        let wanted: BTreeSet<PathBuf> = folders
            .iter()
            .filter(|folder| self.each_folder || !folder.starts_with(&self.root))
            .cloned()
            .chain(std::iter::once(self.root.clone()))
            .collect();
        for gone in self.watched.difference(&wanted) {
            let _ = self.watcher.unwatch(gone);
        }
        for folder in wanted.difference(&self.watched) {
            if let Err(error) = self
                .watcher
                .watch(folder, notify::RecursiveMode::NonRecursive)
                && !self.failed
                && folder.exists()
            {
                self.failed = true;
                log(&format!(
                    "watch:  cannot follow {} ({error}): it is looked at every few seconds",
                    folder.display()
                ));
            }
        }
        self.watched = wanted;
    }
}

/// Wakes the watcher up for an event of the system that may be about a file it looks at.
fn on_event(event: notify::Result<notify::Event>, sender: &mpsc::Sender<Event>) {
    // A missed event is told as an error: look anyway.
    if event.as_ref().is_ok_and(|event| !worth_a_look(event)) {
        return;
    }
    let _ = sender.send(Event::Touched);
}

/// Whether an event may be about a file the watcher looks at: not a read, and not only in the
/// folders of the tools (a build writes a lot in `target/`).
fn worth_a_look(event: &notify::Event) -> bool {
    if matches!(event.kind, notify::EventKind::Access(_)) {
        return false;
    }
    event.paths.is_empty()
        || event.paths.iter().any(|path| {
            !path
                .iter()
                .any(|name| name == "target" || name == ".git" || name == "node_modules")
        })
}

// --- What changed --------------------------------------------------------------------------

/// Whether a file is left alone, by its path in the extension's folder (or in the app's):
/// version control, dependencies and build folders, editors' temporary files, the copy of the
/// Function API's schema a build writes (`schema.graphql`), and `outputs`, what the build of the
/// extension writes.
pub fn ignored(relative: &Path, outputs: &[PathBuf]) -> bool {
    let mut names = relative.iter().map(|name| name.to_string_lossy());
    if names.any(|name| name.starts_with('.') || name == "target" || name == "node_modules") {
        return true;
    }
    let name = relative
        .file_name()
        .map(|name| name.to_string_lossy())
        .unwrap_or_default();
    if name.ends_with('~') || name.ends_with(".swp") || name.ends_with(".swx") {
        return true;
    }
    relative == Path::new("schema.graphql")
        || outputs.iter().any(|output| relative.starts_with(output))
}

/// What the build of a Function writes, in its folder: `dist/`, and the module wherever
/// `[extensions.build] path` puts it.
fn outputs_of(function: &app::FunctionExtension) -> Vec<PathBuf> {
    let mut outputs = vec![PathBuf::from("dist")];
    if let Ok(module) = function.wasm_path.strip_prefix(&function.dir) {
        match module
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            Some(parent) => outputs.push(parent.to_path_buf()),
            None => outputs.push(module.to_path_buf()),
        }
    }
    outputs
}

/// How the watcher treats the folder of an extension.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Rules {
    /// What its build writes, which is not looked at.
    outputs: Vec<PathBuf>,
    /// Its build, for a selected Function that has one.
    build: Option<Job>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Job {
    handle: String,
    command: String,
    dir: PathBuf,
    module: PathBuf,
    outputs: Vec<PathBuf>,
}

/// The rules of each extension folder (by its name) of `app`.
fn rules_of(app: &App, selection: &Selection) -> BTreeMap<String, Rules> {
    let mut rules = BTreeMap::new();
    for function in &app.functions {
        let Some(folder) = function.dir.file_name() else {
            continue;
        };
        let outputs = if function.build_command.is_some() {
            outputs_of(function)
        } else {
            Vec::new()
        };
        rules.insert(
            folder.to_string_lossy().into_owned(),
            Rules {
                build: function
                    .build_command
                    .as_ref()
                    .filter(|_| selection.contains(&function.handle))
                    .map(|command| Job {
                        handle: function.handle.clone(),
                        command: command.clone(),
                        dir: function.dir.clone(),
                        module: function.wasm_path.clone(),
                        outputs: outputs.clone(),
                    }),
                outputs,
            },
        );
    }
    rules
}

/// Walks the files of `dir` that `skip` does not leave alone (by their path in `dir`): `file`
/// gets each one's path in `dir`, its path and its metadata, `folder` each folder walked. A
/// symbolic link is followed, to what it leads to, once; one that leads into `dir` or above it
/// is not (what is in `dir` is walked anyway).
fn walk(
    dir: &Path,
    skip: &dyn Fn(&Path) -> bool,
    file: &mut dyn FnMut(&Path, &Path, &std::fs::Metadata),
    folder: &mut dyn FnMut(&Path),
) {
    struct Links {
        /// `dir`, as the links lead to it.
        top: Option<PathBuf>,
        followed: BTreeSet<PathBuf>,
    }
    fn inner(
        dir: &Path,
        relative: &Path,
        skip: &dyn Fn(&Path) -> bool,
        file: &mut dyn FnMut(&Path, &Path, &std::fs::Metadata),
        folder: &mut dyn FnMut(&Path),
        links: &mut Links,
    ) {
        folder(dir);
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = relative.join(entry.file_name());
            if skip(&path) {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let mut target = entry.path();
            if kind.is_symlink() {
                // What it leads to is what is built.
                match target.canonicalize() {
                    Ok(real) => target = real,
                    Err(_) => continue,
                }
            }
            let Ok(metadata) = std::fs::metadata(&target) else {
                continue;
            };
            if metadata.is_dir() {
                if kind.is_symlink() {
                    let top = links.top.as_ref().map_or(Path::new(""), PathBuf::as_path);
                    if target.starts_with(top)
                        || top.starts_with(&target)
                        || !links.followed.insert(target.clone())
                    {
                        continue;
                    }
                }
                inner(&target, &path, skip, file, folder, links);
            } else {
                file(&path, &target, &metadata);
            }
        }
    }
    let mut links = Links {
        top: dir.canonicalize().ok(),
        followed: BTreeSet::new(),
    };
    inner(dir, Path::new(""), skip, file, folder, &mut links);
}

fn write_metadata(metadata: &std::fs::Metadata, hasher: &mut impl Hasher) {
    hasher.write_u64(metadata.len());
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos() as u64);
    hasher.write_u64(modified);
}

/// FNV-1a: a hash that stays the same from one version of lsf to the next, for the record.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

/// The files of the app as the watcher last saw them.
#[derive(Clone, Debug, Default)]
struct Snapshot {
    /// `shopify.app.toml` and the other `shopify.app.*.toml`.
    config: u64,
    /// The other files of the app, out of `extensions/`: what every build may read (a Cargo
    /// workspace's manifest and lock file, a crate the Functions share), by path in the app.
    shared: BTreeMap<PathBuf, u64>,
    /// Each folder of `extensions/`, by name.
    extensions: BTreeMap<String, u64>,
    /// The folders walked, to follow.
    folders: BTreeSet<PathBuf>,
}

impl Snapshot {
    fn take(root: &Path, rules: &BTreeMap<String, Rules>, data: Option<&Path>) -> Snapshot {
        let mut snapshot = Snapshot::default();
        let mut config = Fnv::default();
        snapshot.folders.insert(root.to_path_buf());
        let mut entries: Vec<_> = std::fs::read_dir(root)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if name.starts_with("shopify.app") && name.ends_with(".toml") {
                config.write(name.as_bytes());
                if let Ok(metadata) = std::fs::metadata(&path) {
                    write_metadata(&metadata, &mut config);
                }
                continue;
            }
            if name == "extensions" || !shared_input(Path::new(&name), &path, root, data) {
                continue;
            }
            let Ok(metadata) = std::fs::metadata(&path) else {
                continue;
            };
            if !metadata.is_dir() {
                let mut hasher = Fnv::default();
                write_metadata(&metadata, &mut hasher);
                snapshot
                    .shared
                    .insert(PathBuf::from(&name), hasher.finish());
                continue;
            }
            let base = PathBuf::from(&name);
            walk(
                &path,
                &|relative| {
                    ignored(relative, &[]) || data.is_some_and(|data| data == path.join(relative))
                },
                &mut |relative, _, metadata| {
                    let mut hasher = Fnv::default();
                    write_metadata(metadata, &mut hasher);
                    snapshot.shared.insert(base.join(relative), hasher.finish());
                },
                &mut |folder| {
                    snapshot.folders.insert(folder.to_path_buf());
                },
            );
        }
        snapshot.config = config.finish();

        let extensions = root.join("extensions");
        snapshot.folders.insert(extensions.clone());
        for entry in std::fs::read_dir(&extensions)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
        {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || !entry.path().is_dir() {
                continue;
            }
            let outputs = rules
                .get(&name)
                .map(|rules| rules.outputs.as_slice())
                .unwrap_or_default();
            let mut hasher = Fnv::default();
            walk(
                &entry.path(),
                &|relative| ignored(relative, outputs),
                &mut |relative, _, metadata| {
                    hasher.write(relative.as_os_str().as_encoded_bytes());
                    hasher.write_u8(0);
                    write_metadata(metadata, &mut hasher);
                },
                &mut |folder| {
                    snapshot.folders.insert(folder.to_path_buf());
                },
            );
            snapshot.extensions.insert(name, hasher.finish());
        }
        snapshot
    }

    /// What changed from `self` to `now`.
    fn diff(&self, now: &Snapshot) -> Changes {
        let mut extensions = BTreeSet::new();
        for (name, fingerprint) in &now.extensions {
            if self.extensions.get(name) != Some(fingerprint) {
                extensions.insert(name.clone());
            }
        }
        for name in self.extensions.keys() {
            if !now.extensions.contains_key(name) {
                extensions.insert(name.clone());
            }
        }
        let mut shared = BTreeSet::new();
        for (path, fingerprint) in &now.shared {
            if self.shared.get(path) != Some(fingerprint) {
                shared.insert(path.clone());
            }
        }
        for path in self.shared.keys() {
            if !now.shared.contains_key(path) {
                shared.insert(path.clone());
            }
        }
        Changes {
            config: self.config != now.config,
            shared,
            extensions,
        }
    }
}

/// Whether a file or folder of the app's own folder (`name`, at `path`) is one the builds may
/// read: not the extensions, the tools' folders, the store data, or a Markdown document.
fn shared_input(name: &Path, path: &Path, root: &Path, data: Option<&Path>) -> bool {
    if ignored(name, &[]) || name == Path::new(crate::app::DEFAULT_DATA_DIRECTORY) {
        return false;
    }
    if name.extension().is_some_and(|extension| extension == "md") {
        return false;
    }
    match data {
        Some(data) => data != path && data != root.join(name),
        None => true,
    }
}

/// The files that changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// `shopify.app.toml`.
    pub config: bool,
    /// The files of the app out of `extensions/` that every build may read, by path.
    pub shared: BTreeSet<PathBuf>,
    /// The extension folders (added, removed or changed), by name.
    pub extensions: BTreeSet<String>,
}

impl Changes {
    fn is_empty(&self) -> bool {
        !self.config && self.shared.is_empty() && self.extensions.is_empty()
    }

    fn merge(&mut self, other: Changes) {
        self.config |= other.config;
        self.shared.extend(other.shared);
        self.extensions.extend(other.extensions);
    }
}

/// Holds changes back until the files hold still: a burst of them counts once.
pub struct Debounce {
    quiet: Duration,
    max_wait: Duration,
    /// What changed, when the first and the last of it were seen.
    pending: Option<(Instant, Instant, Changes)>,
}

impl Debounce {
    pub fn new(quiet: Duration, max_wait: Duration) -> Debounce {
        Debounce {
            quiet,
            max_wait,
            pending: None,
        }
    }

    /// Notes what was seen to change at `now`.
    pub fn add(&mut self, changes: Changes, now: Instant) {
        if changes.is_empty() {
            return;
        }
        match &mut self.pending {
            Some((_, last, pending)) => {
                *last = now;
                pending.merge(changes);
            }
            None => self.pending = Some((now, now, changes)),
        }
    }

    /// What changed, once nothing did for `quiet` (or for `max_wait` since the first change).
    pub fn take(&mut self, now: Instant) -> Option<Changes> {
        let (first, last, _) = self.pending.as_ref()?;
        if now.duration_since(*last) >= self.quiet || now.duration_since(*first) >= self.max_wait {
            return self.pending.take().map(|(_, _, changes)| changes);
        }
        None
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
}

// --- The builds ----------------------------------------------------------------------------

/// Which builds run, and which wait: one at a time per Function, `parallel` at once, and a
/// change during a build makes one more build after it.
pub struct Scheduler {
    parallel: usize,
    /// The builds that run, by handle, and whether one more is due after it.
    running: BTreeMap<String, bool>,
    /// The builds that wait for a free slot, in the order they were asked for.
    queue: VecDeque<String>,
}

impl Scheduler {
    pub fn new(parallel: usize) -> Scheduler {
        Scheduler {
            parallel: parallel.max(1),
            running: BTreeMap::new(),
            queue: VecDeque::new(),
        }
    }

    /// Asks for a build of `handle`. Returns the builds to start now.
    pub fn request(&mut self, handle: &str) -> Vec<String> {
        if let Some(again) = self.running.get_mut(handle) {
            // It started before the change: it may not see it.
            *again = true;
        } else if !self.queue.iter().any(|queued| queued == handle) {
            // A build that waits has not started: it will see the change.
            self.queue.push_back(handle.to_string());
        }
        self.fill()
    }

    /// The build of `handle` is over. Returns the builds to start now.
    pub fn finished(&mut self, handle: &str) -> Vec<String> {
        if self.running.remove(handle) == Some(true) {
            self.queue.push_back(handle.to_string());
        }
        self.fill()
    }

    /// Drops the build of `handle` that waits (its extension is gone).
    pub fn forget(&mut self, handle: &str) {
        self.queue.retain(|queued| queued != handle);
    }

    pub fn is_running(&self, handle: &str) -> bool {
        self.running.contains_key(handle)
    }

    pub fn is_busy(&self) -> bool {
        !self.running.is_empty() || !self.queue.is_empty()
    }

    fn fill(&mut self) -> Vec<String> {
        let mut started = Vec::new();
        while self.running.len() < self.parallel {
            let Some(handle) = self.queue.pop_front() else {
                break;
            };
            self.running.insert(handle.clone(), false);
            started.push(handle);
        }
        started
    }
}

/// How a build ended.
#[derive(Debug)]
enum Outcome {
    Built(Duration),
    Failed(Failure),
    /// The watcher was stopped.
    Stopped,
}

/// What is kept of the output of a build: its first error, from its first line (`error: …`,
/// `error[E0425]: …`) to the end of its source context, and its last lines.
#[derive(Default)]
struct Output {
    /// How many lines it printed.
    lines: usize,
    /// The first error, while it is read: its first line, and the line after it.
    first: Vec<String>,
    reading_first: bool,
    first_start: usize,
    first_end: usize,
    /// The last lines, with their number.
    tail: VecDeque<(usize, String)>,
}

impl Output {
    fn push(&mut self, line: String) {
        let number = self.lines;
        self.lines += 1;
        if self.first.is_empty() && line.starts_with("error") {
            self.reading_first = true;
            self.first_start = number;
        } else if self.reading_first
            && (line.trim().is_empty()
                || line.starts_with("error")
                || line.starts_with("warning")
                || self.first.len() == FIRST_ERROR)
        {
            self.reading_first = false;
        }
        if self.reading_first {
            self.first.push(line.clone());
            self.first_end = number + 1;
        }
        if self.tail.len() == TAIL {
            self.tail.pop_front();
        }
        self.tail.push_back((number, line));
    }

    /// The first error, then what came after it of the last lines; all of it when the last
    /// lines hold the first error.
    fn text(&self) -> String {
        let tail_start = self.tail.front().map_or(0, |(number, _)| *number);
        let mut lines = Vec::new();
        if !self.first.is_empty() && self.first_start < tail_start {
            lines.extend(self.first.iter().cloned());
            if self.first_end < tail_start {
                lines.push("…".to_string());
            }
        }
        let shown = if lines.is_empty() { 0 } else { self.first_end };
        lines.extend(
            self.tail
                .iter()
                .filter(|(number, _)| *number >= shown)
                .map(|(_, line)| line.clone()),
        );
        lines.join("\n")
    }
}

/// Runs the build of a Function in its folder, as `--build` does, and publishes the module it
/// wrote when it built and the module is whole. The module of a build that fails is left as it
/// is: the checkout runs the copy of the last one that built.
fn build(job: &Job, timeout: Duration, stop: &AtomicBool, modules: &Modules) -> Outcome {
    let started = Instant::now();
    let mut command = crate::checkout::build_command(&job.command, &job.dir);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // A group of its own, to stop all it starts (`sh`, then `cargo`, then `rustc`) at once.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Outcome::Failed(Failure {
                reason: format!("cannot run `{}`: {error}", job.command),
                output: String::new(),
            });
        }
    };
    let output = Arc::new(Mutex::new(Output::default()));
    let (done, finished) = mpsc::channel();
    let streams: [Option<Box<dyn Read + Send>>; 2] = [
        child
            .stdout
            .take()
            .map(|out| Box::new(out) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|err| Box::new(err) as Box<dyn Read + Send>),
    ];
    for stream in streams.into_iter().flatten() {
        let output = output.clone();
        let done = done.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stream).split(b'\n').map_while(Result::ok) {
                let line = String::from_utf8_lossy(&line).trim_end().to_string();
                output.lock().expect("output poisoned").push(line);
            }
            let _ = done.send(());
        });
    }
    drop(done);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if stop.load(Ordering::Relaxed) {
            terminate(&mut child);
            return Outcome::Stopped;
        }
        if started.elapsed() >= timeout {
            terminate(&mut child);
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    // What it printed, once its output is closed. Something it left running may hold the
    // output open: that is not waited for long.
    while finished.recv_timeout(Duration::from_millis(500)).is_ok() {}
    let output = output.lock().expect("output poisoned").text();
    let reason = match status {
        Some(status) if status.success() => {
            let module = job.module.strip_prefix(&job.dir).unwrap_or(&job.module);
            match std::fs::read(&job.module) {
                Ok(bytes) if is_module(&bytes) => match modules.publish(&job.handle, &bytes) {
                    Ok(()) => return Outcome::Built(started.elapsed()),
                    Err(error) => format!("cannot copy the module it built: {error}"),
                },
                Ok(_) => format!(
                    "the build wrote {}, which is not a whole Wasm module",
                    module.display()
                ),
                Err(_) => format!("the build wrote no module at {}", module.display()),
            }
        }
        Some(status) => format!("build failed ({status})"),
        None if started.elapsed() >= timeout => {
            format!("build timed out after {}", seconds(timeout))
        }
        None => "build failed".to_string(),
    };
    Outcome::Failed(Failure { reason, output })
}

/// Stops a build and everything it started: politely, then not.
fn terminate(child: &mut Child) {
    #[cfg(unix)]
    {
        let group = -(child.id() as i32);
        // SAFETY: `kill` takes any process group id; the group is the build's own.
        unsafe { libc::kill(group, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = child.try_wait() {
                // What the build started may still run in its group.
                unsafe { libc::kill(group, libc::SIGKILL) };
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        unsafe { libc::kill(group, libc::SIGKILL) };
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

// --- What the last watch built -------------------------------------------------------------

/// What each Function was last built from, by handle: a hash of its files, of the files the
/// builds share, and of its build command. Kept from one watch to the next in the user's cache
/// folder, so that a watch builds at start only what changed since.
struct Record {
    path: Option<PathBuf>,
    root: PathBuf,
    /// `None`: nothing was recorded for the app.
    functions: Option<BTreeMap<String, String>>,
}

impl Record {
    fn read(path: Option<PathBuf>, root: &Path) -> Record {
        let functions = path
            .as_ref()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<Json>(&bytes).ok())
            .and_then(|json| serde_json::from_value(json.get("functions")?.clone()).ok());
        Record {
            path,
            root: root.to_path_buf(),
            functions,
        }
    }

    fn get(&self, handle: &str) -> Option<&str> {
        self.functions.as_ref()?.get(handle).map(String::as_str)
    }

    fn set(&mut self, handle: &str, inputs: u64) {
        self.functions
            .get_or_insert_default()
            .insert(handle.to_string(), format!("{inputs:016x}"));
    }

    fn write(&self) {
        let (Some(path), Some(functions)) = (&self.path, &self.functions) else {
            return;
        };
        let json = json!({"app": self.root.display().to_string(), "functions": functions});
        let Some(parent) = path.parent() else {
            return;
        };
        let partial = path.with_extension("json.partial");
        let written = std::fs::create_dir_all(parent).is_ok()
            && std::fs::write(&partial, json.to_string()).is_ok()
            && std::fs::rename(&partial, path).is_ok();
        if !written {
            let _ = std::fs::remove_file(&partial);
        }
    }
}

/// A hash of the content of the files under `dir` that `skip` does not leave alone.
fn content_hash(dir: &Path, skip: &dyn Fn(&Path) -> bool, hasher: &mut Fnv) {
    walk(
        dir,
        skip,
        &mut |relative, path, _| {
            hasher.write(relative.as_os_str().as_encoded_bytes());
            hasher.write_u8(0);
            if let Ok(bytes) = std::fs::read(path) {
                hasher.write_u64(bytes.len() as u64);
                hasher.write(&bytes);
            }
        },
        &mut |_| {},
    );
}

// --- The watcher's thread ------------------------------------------------------------------

struct Coordinator {
    root: PathBuf,
    data: Option<PathBuf>,
    builds: Arc<Builds>,
    options: Options,
    selection: Selection,
    on_change: Box<dyn Fn() + Send>,
    log: Log,
    shared: Arc<Shared>,
    rules: BTreeMap<String, Rules>,
    seen: Snapshot,
    debounce: Debounce,
    scheduler: Scheduler,
    sender: mpsc::Sender<Event>,
    /// What the system says of the files, while it is kept.
    events: Option<Follow>,
    /// How often the files are looked at while nothing changes.
    idle: Duration,
    /// The system said that a file changed since the last look.
    dirty: bool,
    workers: Vec<JoinHandle<()>>,
    /// The builds of the start, while they run: when they started, how many, how many failed.
    initial: Option<(Instant, usize, usize)>,
    /// The pages are to reload, once no build runs or waits.
    reload_due: bool,
    /// How many looks at the files started, and how many are over.
    started: u64,
    looked: u64,
    record: Record,
    /// A hash of the content of the shared files, by a hash of what the watcher saw of them.
    shared_content: Option<(u64, u64)>,
}

impl Coordinator {
    /// Reads the app, publishes the modules that built before, and returns the Functions to
    /// build at start.
    fn prepare(&mut self) -> Vec<String> {
        self.reload_app();
        self.seen = Snapshot::take(&self.root, &self.rules, self.data.as_deref());
        if let Some(events) = &mut self.events {
            events.sync(&self.seen.folders, &self.log);
            #[cfg(test)]
            {
                *self.shared.followed.lock().expect("watcher poisoned") =
                    Some(events.watched.clone());
            }
        }
        let jobs: Vec<Job> = self.jobs().cloned().collect();
        let builds = self.builds.clone();
        let modules = builds.modules();
        let mut first = Vec::new();
        let mut trusted = 0;
        for job in &jobs {
            if self.options.build_first || !modules.path(&job.handle).is_file() {
                first.push(job.handle.clone());
                continue;
            }
            let inputs = self.inputs(job);
            match self.record.get(&job.handle) {
                Some(recorded) if recorded == format!("{inputs:016x}") => {}
                Some(_) => first.push(job.handle.clone()),
                None => {
                    // Built before lsf watched it: it is taken to be built from these files.
                    self.record.set(&job.handle, inputs);
                    trusted += 1;
                }
            }
        }
        if trusted > 0 {
            self.record.write();
            (self.log)("watch:  using the existing builds; --build rebuilds them all at start");
        }
        first
    }

    fn run(mut self, receiver: mpsc::Receiver<Event>, first: Vec<String>) {
        if !first.is_empty() {
            (self.log)(&format!(
                "↻ building {}…",
                count(first.len(), "Function", "Functions")
            ));
            self.initial = Some((Instant::now(), first.len(), 0));
            for handle in first {
                self.request(&handle);
            }
        }
        self.update_status();
        let mut last_look = Instant::now();
        while !self.shared.stop.load(Ordering::Relaxed) {
            let wait = self.interval().saturating_sub(last_look.elapsed());
            match receiver.recv_timeout(wait) {
                Ok(Event::Finished(handle, outcome, inputs)) => {
                    self.finished(&handle, outcome, inputs)
                }
                Ok(Event::Touched) => self.dirty = true,
                Ok(Event::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            // A burst of events (a build writing its output) makes one look per `poll`.
            if last_look.elapsed() >= self.interval() {
                last_look = Instant::now();
                self.dirty = false;
                self.look();
            }
            self.settle();
            self.update_status();
        }
        // The builds see the stop and end theirs.
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        let mut status = self.shared.status.lock().expect("watcher poisoned");
        status.pending = false;
        status.building = false;
        self.shared.changed.notify_all();
    }

    /// How long until the next look at the files: soon while they change, otherwise when it is
    /// time to look anyway.
    fn interval(&self) -> Duration {
        if self.dirty || self.debounce.is_pending() {
            self.options.poll
        } else {
            self.idle
        }
    }

    fn update_status(&mut self) {
        self.workers.retain(|worker| !worker.is_finished());
        let mut status = self.shared.status.lock().expect("watcher poisoned");
        status.looked = self.looked;
        status.pending = self.dirty || self.debounce.is_pending() || self.reload_due;
        status.building = self.scheduler.is_busy();
        self.shared.changed.notify_all();
    }

    /// The pages reload once the builds are over: none runs or waits, and no change waits to
    /// count (it may start one).
    fn settle(&mut self) {
        if self.reload_due
            && !self.scheduler.is_busy()
            && !self.debounce.is_pending()
            && !self.dirty
        {
            self.reload_due = false;
            self.publish();
        }
    }

    /// The builds of the selected Functions that have one.
    fn jobs(&self) -> impl Iterator<Item = &Job> {
        self.rules.values().filter_map(|rules| rules.build.as_ref())
    }

    /// Reads the app again, for which folder is what. Returns whether that changed. The
    /// Functions it builds now run their published module; one that was built before it is
    /// watched is published as it is, when its module is whole.
    fn reload_app(&mut self) -> bool {
        let rules = match app::load(&self.root) {
            Ok(app) => rules_of(&app, &self.selection),
            // The app cannot be read: the pages say why. Its folders are watched as they were.
            Err(_) => return false,
        };
        let changed = rules != self.rules;
        self.rules = rules;
        let jobs: BTreeMap<String, Job> = self
            .jobs()
            .map(|job| (job.handle.clone(), job.clone()))
            .collect();
        let modules = self.builds.modules();
        let owned = self.builds.owned.lock().expect("builds poisoned").clone();
        for gone in owned.iter().filter(|handle| !jobs.contains_key(*handle)) {
            modules.withdraw(gone);
        }
        for (handle, job) in &jobs {
            if !owned.contains(handle) && !self.scheduler.is_running(handle) {
                modules.adopt(job);
            }
        }
        self.builds.set_owned(jobs.into_keys().collect());
        changed
    }

    /// Looks at the files, and acts on what changed once they hold still.
    fn look(&mut self) {
        self.started += 1;
        self.shared.status.lock().expect("watcher poisoned").started = self.started;
        let now = Snapshot::take(&self.root, &self.rules, self.data.as_deref());
        let changes = self.seen.diff(&now);
        if let Some(events) = &mut self.events {
            events.sync(&now.folders, &self.log);
            #[cfg(test)]
            {
                *self.shared.followed.lock().expect("watcher poisoned") =
                    Some(events.watched.clone());
            }
        }
        self.seen = now;
        self.debounce.add(changes, Instant::now());
        if let Some(changes) = self.debounce.take(Instant::now()) {
            self.apply(changes);
        }
        self.looked += 1;
    }

    fn apply(&mut self, changes: Changes) {
        if self.reload_app() {
            // What is looked at in a folder depends on what the app says of it.
            self.seen = Snapshot::take(&self.root, &self.rules, self.data.as_deref());
        }
        let handles: BTreeSet<String> = self.jobs().map(|job| job.handle.clone()).collect();
        let mut reload = false;
        // Each Function once, whatever asked for it.
        let mut to_build = BTreeSet::new();
        if changes.config {
            (self.log)("↻ shopify.app.toml changed");
            reload = true;
        }
        if let Some(first) = changes.shared.first()
            && !handles.is_empty()
        {
            let what = match changes.shared.len() {
                1 => first.display().to_string(),
                n => format!("{} and {} other files", first.display(), n - 1),
            };
            (self.log)(&format!(
                "↻ {what} changed: building {}",
                count(handles.len(), "Function", "Functions")
            ));
            to_build.extend(handles.iter().cloned());
        }
        for folder in &changes.extensions {
            match self.rules.get(folder).and_then(|rules| rules.build.clone()) {
                Some(job) => {
                    to_build.insert(job.handle);
                }
                None if self.root.join("extensions").join(folder).is_dir() => {
                    (self.log)(&format!("↻ {folder} changed"));
                    reload = true;
                }
                None => {
                    (self.log)(&format!("↻ {folder} removed"));
                    reload = true;
                }
            }
        }
        for handle in &to_build {
            self.request(handle);
        }
        // The Functions that are gone, or that do not build any more.
        let known: BTreeSet<String> = self.builds.failures().into_iter().map(|(h, _)| h).collect();
        if known.iter().any(|handle| !handles.contains(handle)) {
            self.builds.retain(&handles);
            reload = true;
        }
        for handle in known.difference(&handles) {
            self.scheduler.forget(handle);
        }
        self.reload_due |= reload;
    }

    /// Tells the pages to reload.
    fn publish(&self) {
        self.builds.bump();
        (self.on_change)();
    }

    fn request(&mut self, handle: &str) {
        for handle in self.scheduler.request(handle) {
            self.spawn(handle);
        }
    }

    /// A hash of what the build of `job` reads: the files of its folder, the files the builds
    /// share, and its command.
    fn inputs(&mut self, job: &Job) -> u64 {
        let shared = self.shared_content();
        let mut hasher = Fnv::default();
        hasher.write(job.command.as_bytes());
        hasher.write_u8(0);
        hasher.write(job.module.as_os_str().as_encoded_bytes());
        hasher.write_u8(0);
        hasher.write_u64(shared);
        content_hash(
            &job.dir,
            &|relative| ignored(relative, &job.outputs),
            &mut hasher,
        );
        hasher.finish()
    }

    /// A hash of the content of the files the builds share, read again only when they changed.
    fn shared_content(&mut self) -> u64 {
        let mut seen = Fnv::default();
        for (path, fingerprint) in &self.seen.shared {
            seen.write(path.as_os_str().as_encoded_bytes());
            seen.write_u64(*fingerprint);
        }
        let seen = seen.finish();
        if let Some((key, content)) = self.shared_content
            && key == seen
        {
            return content;
        }
        let mut hasher = Fnv::default();
        for path in self.seen.shared.keys() {
            hasher.write(path.as_os_str().as_encoded_bytes());
            hasher.write_u8(0);
            if let Ok(bytes) = std::fs::read(self.root.join(path)) {
                hasher.write_u64(bytes.len() as u64);
                hasher.write(&bytes);
            }
        }
        let content = hasher.finish();
        self.shared_content = Some((seen, content));
        content
    }

    fn spawn(&mut self, handle: String) {
        let Some(job) = self.jobs().find(|job| job.handle == handle).cloned() else {
            // Gone since it was asked for.
            for next in self.scheduler.finished(&handle) {
                self.spawn(next);
            }
            return;
        };
        if self.initial.is_none() {
            (self.log)(&format!("↻ building {handle}…"));
        }
        // What it builds from, as it starts: what changes later builds again.
        let inputs = self.inputs(&job);
        let sender = self.sender.clone();
        let shared = self.shared.clone();
        let builds = self.builds.clone();
        let timeout = self.options.timeout;
        let worker = std::thread::Builder::new()
            .name(format!("lsf-build-{handle}"))
            .spawn(move || {
                let outcome = build(&job, timeout, &shared.stop, builds.modules());
                let _ = sender.send(Event::Finished(handle, outcome, inputs));
            })
            .expect("cannot start a build");
        self.workers.push(worker);
    }

    fn finished(&mut self, handle: &str, outcome: Outcome, inputs: u64) {
        match outcome {
            Outcome::Built(took) => {
                self.builds.set(handle, None);
                if self.builds.owns(handle) {
                    self.record.set(handle, inputs);
                    self.record.write();
                } else {
                    // Gone while it built.
                    self.builds.modules().withdraw(handle);
                }
                if self.initial.is_none() {
                    (self.log)(&format!("✓ {handle} built in {:.1}s", took.as_secs_f64()));
                }
            }
            Outcome::Failed(failure) => {
                let mut message = format!("✗ {handle}: {}", failure.reason);
                for line in failure.output.lines() {
                    message.push_str("\n    ");
                    message.push_str(line);
                }
                (self.log)(&message);
                self.builds.set(handle, Some(failure));
                if let Some((_, _, failed)) = &mut self.initial {
                    *failed += 1;
                }
            }
            Outcome::Stopped => return,
        }
        self.reload_due = true;
        for next in self.scheduler.finished(handle) {
            self.spawn(next);
        }
        if let Some((started, total, failed)) = self.initial
            && !self.scheduler.is_busy()
        {
            let took = started.elapsed().as_secs_f64();
            if failed == 0 {
                (self.log)(&format!(
                    "✓ {} built in {took:.1}s",
                    count(total, "Function", "Functions")
                ));
            } else {
                (self.log)(&format!(
                    "✗ {failed} of {} failed to build ({took:.1}s); the checkout runs the last module that built",
                    count(total, "Function", "Functions")
                ));
            }
            self.initial = None;
        }
    }
}

/// `600s`, `0.5s`.
fn seconds(duration: Duration) -> String {
    if duration.subsec_millis() == 0 {
        format!("{}s", duration.as_secs())
    } else {
        format!("{:.1}s", duration.as_secs_f64())
    }
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

#[cfg(test)]
mod tests;
