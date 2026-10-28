//! `--watch`: the Functions of the app are built again when their files change, and the open
//! pages reload, as `shopify app dev` does.
//!
//! The system says when a file of the app changes (FSEvents, inotify, ReadDirectoryChangesW,
//! through `notify`). The watcher then walks the extension folders and compares the size and
//! the modification time of their files with what it saw last, to know which extension changed
//! and to leave alone what is not looked at (the build outputs, `target/`): what the system says
//! only wakes it up. A walk of an app of 90 extensions takes about 3 ms, so polling every 100 ms
//! would keep 3 to 5 % of a core busy for nothing; with the events the watcher idles, and still
//! walks every few seconds in case an event was missed. Where the events cannot be had (a file
//! system that has none), it polls, less often.
//!
//! - A change in a Function that has a build command (`[extensions.build] command`) builds it
//!   again, in its folder. One build at a time per Function; what changes during a build
//!   builds it once more after it. Several Functions build at once, up to [`PARALLEL`].
//! - A build that fails keeps the last module that built: the checkout runs it, and the pages
//!   say that the build failed, with the end of its output.
//! - Any other change (a checkout UI extension's bundle, an extension without a build,
//!   `shopify.app.toml`, an extension added or removed) reloads the pages at once.
//!
//! The pages reload through live reload: [`Builds::generation`] is part of its token.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::hash::Hasher;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use lsf_checkout::app::{self, App, Selection};
use serde_json::{Value as Json, json};

/// How many builds run at once. Functions of one Cargo workspace wait for each other on its
/// lock anyway; more would only compete for the processor.
pub const PARALLEL: usize = 3;

/// How many lines of the output of a build that fails are kept, for the terminal and the pages.
const TAIL: usize = 20;

/// The builds of the app, as the pages show them.
#[derive(Default)]
pub struct Builds {
    /// Moves on whenever the pages should reload: a build is over, or a file they show changed.
    generation: AtomicU64,
    /// The Functions whose last build failed, by handle.
    failures: Mutex<BTreeMap<String, Failure>>,
}

/// Why a build failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// `build failed (exit status: 101)`, `build timed out after 600s`.
    pub reason: String,
    /// The last lines it printed.
    pub output: String,
}

impl Builds {
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
}

/// How the watcher looks at the files and runs the builds.
#[derive(Clone, Debug)]
pub struct Options {
    /// The extensions `--extension` selects: only those are built. All when empty.
    pub extensions: Vec<String>,
    /// Build every selected Function when the watcher starts, as `--build` does.
    pub build_first: bool,
    /// How often the files are looked at while they change.
    pub poll: Duration,
    /// Follow what the system says of the files. Without it (or when the system cannot say),
    /// the files are polled every three `poll`.
    pub events: bool,
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
            poll: Duration::from_millis(100),
            events: true,
            idle: Duration::from_secs(3),
            quiet: Duration::from_millis(150),
            max_wait: Duration::from_secs(2),
            parallel: PARALLEL,
            timeout: Duration::from_secs(600),
        }
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
    /// A build is over.
    Finished(String, Outcome),
    /// The system says that a file changed.
    Touched,
    Stop,
}

struct Shared {
    stop: AtomicBool,
    /// Whether a change waits to count or a build is queued or running.
    busy: Mutex<bool>,
    idle: Condvar,
}

impl Watcher {
    /// Starts watching the app in `root`. `on_change` runs whenever the pages are told to
    /// reload, after `builds` changed.
    pub fn start(
        root: &Path,
        builds: Arc<Builds>,
        options: Options,
        on_change: Box<dyn Fn() + Send>,
        log: Log,
    ) -> Watcher {
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            busy: Mutex::new(true),
            idle: Condvar::new(),
        });
        let (sender, receiver) = mpsc::channel();
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let events = if options.events {
            match follow(&root, sender.clone()) {
                Ok(events) => Some(events),
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
        let coordinator = Coordinator {
            idle: if events.is_some() {
                options.idle
            } else {
                options.poll * 3
            },
            _events: events,
            dirty: false,
            debounce: Debounce::new(options.quiet, options.max_wait),
            scheduler: Scheduler::new(options.parallel),
            selection: if options.extensions.is_empty() {
                Selection::all()
            } else {
                Selection::only(options.extensions.iter().cloned())
            },
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
        };
        let thread = std::thread::Builder::new()
            .name("lsf-watch".to_string())
            .spawn(move || coordinator.run(receiver))
            .expect("cannot start the watcher");
        Watcher {
            shared,
            sender,
            thread: Some(thread),
        }
    }

    #[cfg(test)]
    /// Waits until no change waits to count and no build is queued or running, for `timeout`
    /// at most. Returns whether it is so.
    pub fn wait_idle(&self, timeout: Duration) -> bool {
        let busy = self.shared.busy.lock().expect("watcher poisoned");
        let (busy, _) = self
            .shared
            .idle
            .wait_timeout_while(busy, timeout, |busy| *busy)
            .expect("watcher poisoned");
        !*busy
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

impl Drop for Watcher {
    fn drop(&mut self) {
        self.halt();
    }
}

/// Asks the system to say when a file of the app changes: in `extensions/`, and next to
/// `shopify.app.toml`.
fn follow(root: &Path, sender: mpsc::Sender<Event>) -> notify::Result<notify::RecommendedWatcher> {
    use notify::Watcher as _;
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        // A missed event is told as an error: look anyway.
        if event.as_ref().is_ok_and(|event| !worth_a_look(event)) {
            return;
        }
        let _ = sender.send(Event::Touched);
    })?;
    watcher.watch(root, notify::RecursiveMode::NonRecursive)?;
    let extensions = root.join("extensions");
    if extensions.is_dir() {
        watcher.watch(&extensions, notify::RecursiveMode::Recursive)?;
    }
    Ok(watcher)
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

/// Whether a file of an extension is left alone, by its path in the extension's folder:
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
}

/// The rules of each extension folder (by its name) of `app`.
fn rules_of(app: &App, selection: &Selection) -> BTreeMap<String, Rules> {
    let mut rules = BTreeMap::new();
    for function in &app.functions {
        let Some(folder) = function.dir.file_name() else {
            continue;
        };
        let has_build = function.build_command.is_some();
        rules.insert(
            folder.to_string_lossy().into_owned(),
            Rules {
                outputs: if has_build {
                    outputs_of(function)
                } else {
                    Vec::new()
                },
                build: function
                    .build_command
                    .as_ref()
                    .filter(|_| selection.contains(&function.handle))
                    .map(|command| Job {
                        handle: function.handle.clone(),
                        command: command.clone(),
                        dir: function.dir.clone(),
                        module: function.wasm_path.clone(),
                    }),
            },
        );
    }
    rules
}

/// A fingerprint of the files of a folder the watcher looks at: their paths, sizes and
/// modification times.
fn fingerprint(dir: &Path, outputs: &[PathBuf]) -> u64 {
    fn walk(
        dir: &Path,
        relative: &Path,
        outputs: &[PathBuf],
        hasher: &mut std::collections::hash_map::DefaultHasher,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = relative.join(entry.file_name());
            if ignored(&path, outputs) {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                walk(&entry.path(), &path, outputs, hasher);
            } else {
                hasher.write(path.as_os_str().as_encoded_bytes());
                hasher.write_u8(0);
                write_metadata(&metadata, hasher);
            }
        }
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    walk(dir, Path::new(""), outputs, &mut hasher);
    hasher.finish()
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

/// The files of the app as the watcher last saw them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Snapshot {
    /// `shopify.app.toml` and the other `shopify.app.*.toml`.
    config: u64,
    /// Each folder of `extensions/`, by name.
    extensions: BTreeMap<String, u64>,
}

impl Snapshot {
    fn take(root: &Path, rules: &BTreeMap<String, Rules>) -> Snapshot {
        let mut config = std::collections::hash_map::DefaultHasher::new();
        let mut files: Vec<_> = std::fs::read_dir(root)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.starts_with("shopify.app") && name.ends_with(".toml")
            })
            .collect();
        files.sort_by_key(|entry| entry.file_name());
        for file in files {
            config.write(file.file_name().as_encoded_bytes());
            if let Ok(metadata) = file.metadata() {
                write_metadata(&metadata, &mut config);
            }
        }
        let mut extensions = BTreeMap::new();
        for entry in std::fs::read_dir(root.join("extensions"))
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
            extensions.insert(name, fingerprint(&entry.path(), outputs));
        }
        Snapshot {
            config: config.finish(),
            extensions,
        }
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
        Changes {
            config: self.config != now.config,
            extensions,
        }
    }
}

/// The files that changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// `shopify.app.toml`.
    pub config: bool,
    /// The extension folders (added, removed or changed), by name.
    pub extensions: BTreeSet<String>,
}

impl Changes {
    fn is_empty(&self) -> bool {
        !self.config && self.extensions.is_empty()
    }

    fn merge(&mut self, other: Changes) {
        self.config |= other.config;
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

/// The last lines of an output.
#[derive(Default)]
struct Tail(VecDeque<String>);

impl Tail {
    fn push(&mut self, line: String) {
        if self.0.len() == TAIL {
            self.0.pop_front();
        }
        self.0.push_back(line);
    }
}

/// Runs the build of a Function in its folder, as `--build` does, and keeps the module it had
/// when the build fails.
fn build(job: &Job, timeout: Duration, stop: &AtomicBool) -> Outcome {
    let started = Instant::now();
    // The last module that built: the one in place, since a build that fails puts it back.
    let good = std::fs::read(&job.module).ok();
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
    let tail = Arc::new(Mutex::new(Tail::default()));
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
        let tail = tail.clone();
        let done = done.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stream).split(b'\n').map_while(Result::ok) {
                let line = String::from_utf8_lossy(&line).trim_end().to_string();
                tail.lock().expect("tail poisoned").push(line);
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
            // Stopped halfway, it may have removed the module: the next start runs the last one.
            restore(&job.module, good.as_deref());
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
    let output = Vec::from(std::mem::take(&mut tail.lock().expect("tail poisoned").0)).join("\n");
    match status {
        Some(status) if status.success() => Outcome::Built(started.elapsed()),
        status => {
            restore(&job.module, good.as_deref());
            Outcome::Failed(Failure {
                reason: match status {
                    Some(status) => format!("build failed ({status})"),
                    None if started.elapsed() >= timeout => {
                        format!("build timed out after {}", seconds(timeout))
                    }
                    None => "build failed".to_string(),
                },
                output,
            })
        }
    }
}

/// Puts the module that built last back, when the build that failed changed it.
fn restore(module: &Path, good: Option<&[u8]>) {
    let Some(good) = good else {
        return;
    };
    if std::fs::read(module).ok().as_deref() == Some(good) {
        return;
    }
    if let Some(parent) = module.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // In one step: a request may load the module meanwhile.
    let partial = module.with_extension("lsf-restore");
    if std::fs::write(&partial, good).is_ok() && std::fs::rename(&partial, module).is_err() {
        let _ = std::fs::remove_file(&partial);
    }
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

// --- The watcher's thread ------------------------------------------------------------------

struct Coordinator {
    root: PathBuf,
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
    _events: Option<notify::RecommendedWatcher>,
    /// How often the files are looked at while nothing changes.
    idle: Duration,
    /// The system said that a file changed since the last look.
    dirty: bool,
    workers: Vec<JoinHandle<()>>,
    /// The builds of the start, while they run: when they started, how many, how many failed.
    initial: Option<(Instant, usize, usize)>,
}

impl Coordinator {
    fn run(mut self, receiver: mpsc::Receiver<Event>) {
        self.reload_app();
        self.seen = Snapshot::take(&self.root, &self.rules);
        if self.options.build_first {
            let handles: Vec<String> = self
                .rules
                .values()
                .filter_map(|rules| rules.build.as_ref())
                .map(|job| job.handle.clone())
                .collect();
            if !handles.is_empty() {
                (self.log)(&format!(
                    "↻ building {}…",
                    count(handles.len(), "Function", "Functions")
                ));
                self.initial = Some((Instant::now(), handles.len(), 0));
                for handle in handles {
                    self.request(&handle);
                }
            }
        }
        self.update_busy();
        let mut last_look = Instant::now();
        while !self.shared.stop.load(Ordering::Relaxed) {
            let wait = self.interval().saturating_sub(last_look.elapsed());
            match receiver.recv_timeout(wait) {
                Ok(Event::Finished(handle, outcome)) => self.finished(&handle, outcome),
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
            self.update_busy();
        }
        // The builds see the stop and end theirs.
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        *self.shared.busy.lock().expect("watcher poisoned") = false;
        self.shared.idle.notify_all();
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

    fn update_busy(&mut self) {
        self.workers.retain(|worker| !worker.is_finished());
        let busy = self.dirty || self.debounce.is_pending() || self.scheduler.is_busy();
        *self.shared.busy.lock().expect("watcher poisoned") = busy;
        if !busy {
            self.shared.idle.notify_all();
        }
    }

    /// Reads the app again, for which folder is what. Returns whether that changed.
    fn reload_app(&mut self) -> bool {
        let rules = match app::load(&self.root) {
            Ok(app) => rules_of(&app, &self.selection),
            // The app cannot be read: the pages say why. Its folders are watched as they were.
            Err(_) => return false,
        };
        let changed = rules != self.rules;
        self.rules = rules;
        changed
    }

    /// Looks at the files, and acts on what changed once they hold still.
    fn look(&mut self) {
        let now = Snapshot::take(&self.root, &self.rules);
        let changes = self.seen.diff(&now);
        self.seen = now;
        self.debounce.add(changes, Instant::now());
        if let Some(changes) = self.debounce.take(Instant::now()) {
            self.apply(changes);
        }
    }

    fn apply(&mut self, changes: Changes) {
        if self.reload_app() {
            // What is looked at in a folder depends on what the app says of it.
            self.seen = Snapshot::take(&self.root, &self.rules);
        }
        let handles: BTreeSet<String> = self
            .rules
            .values()
            .filter_map(|rules| rules.build.as_ref())
            .map(|job| job.handle.clone())
            .collect();
        let mut reload = false;
        if changes.config {
            (self.log)("↻ shopify.app.toml changed");
            reload = true;
        }
        for folder in &changes.extensions {
            match self.rules.get(folder).and_then(|rules| rules.build.clone()) {
                Some(job) => self.request(&job.handle),
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
        // The Functions that are gone, or that do not build any more.
        let known: BTreeSet<String> = self.builds.failures().into_iter().map(|(h, _)| h).collect();
        if known.iter().any(|handle| !handles.contains(handle)) {
            self.builds.retain(&handles);
            reload = true;
        }
        for handle in known.difference(&handles) {
            self.scheduler.forget(handle);
        }
        if reload {
            self.publish();
        }
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

    fn spawn(&mut self, handle: String) {
        let job = self
            .rules
            .values()
            .filter_map(|rules| rules.build.as_ref())
            .find(|job| job.handle == handle)
            .cloned();
        let Some(job) = job else {
            // Gone since it was asked for.
            for next in self.scheduler.finished(&handle) {
                self.spawn(next);
            }
            return;
        };
        if self.initial.is_none() {
            (self.log)(&format!("↻ building {handle}…"));
        }
        let sender = self.sender.clone();
        let shared = self.shared.clone();
        let timeout = self.options.timeout;
        let worker = std::thread::Builder::new()
            .name(format!("lsf-build-{handle}"))
            .spawn(move || {
                let outcome = build(&job, timeout, &shared.stop);
                let _ = sender.send(Event::Finished(handle, outcome));
            })
            .expect("cannot start a build");
        self.workers.push(worker);
    }

    fn finished(&mut self, handle: &str, outcome: Outcome) {
        match outcome {
            Outcome::Built(took) => {
                self.builds.set(handle, None);
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
        for next in self.scheduler.finished(handle) {
            self.spawn(next);
        }
        match self.initial {
            Some((started, total, failed)) => {
                if !self.scheduler.is_busy() {
                    let took = started.elapsed().as_secs_f64();
                    if failed == 0 {
                        (self.log)(&format!(
                            "✓ {} built in {took:.1}s",
                            count(total, "Function", "Functions")
                        ));
                    } else {
                        (self.log)(&format!(
                            "✗ {failed} of {} failed to build ({took:.1}s); the checkout runs their last module",
                            count(total, "Function", "Functions")
                        ));
                    }
                    self.initial = None;
                    self.publish();
                }
            }
            // One more build of it is due: the pages reload after that one.
            None if self.scheduler.is_running(handle) => {}
            None => self.publish(),
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
