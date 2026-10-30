//! The Functions that fail when the scenarios run: the home page runs the Functions of every
//! scenario that opens, as its checkout would when it opens (`lsf app function run --scenario`),
//! and lists each run that fails (a trap, too many instructions, an output that does not fit
//! the schema, a `.fetch` that cannot be sent) with the scenarios it fails in.
//!
//! Opening the home page sends no HTTP request: the check runs the `.fetch` targets, and checks
//! the request each one asks for against Shopify's limits (a request out of them fails the
//! target), but sends none ([`OfflineFetcher`]): the `.run` target reads what a request whose
//! connection fails reads, a `502`.
//!
//! Running them all takes a while (a Function that runs past the instruction limit), so the
//! page does not wait for it: it asks `/__lsf/checkouts/check` once it is shown. A check stops
//! at its time budget, between two Function runs, and says how many scenarios it checked. What
//! a complete check found is kept until the store data or the app changes.

use std::hash::{Hash, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use lsf_checkout::app::App;
use lsf_checkout::fetch::OfflineFetcher;
use lsf_checkout::pipeline::RunOptions;
use lsf_core::Store;
use serde_json::{Value as Json, json};

use super::ServerState;

/// How long a check may take: the scenarios not run by then are said to be unchecked. The
/// page waits 30 s for the answer (`CHECK_TIMEOUT` in `home.js`).
pub const BUDGET: Duration = Duration::from_secs(20);

/// A Function target that fails, and the scenarios it fails in.
#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub handle: String,
    pub target: String,
    /// Why the run failed, as the first scenario it failed in says.
    pub error: String,
    pub scenarios: Vec<String>,
}

/// What a check found.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub failures: Vec<Failure>,
    /// How many scenarios ran, out of how many could.
    pub checked: usize,
    pub total: usize,
    pub elapsed: Duration,
}

impl Report {
    /// The handles of the Functions that fail in `scenario`.
    pub fn failing_in(&self, scenario: &str) -> Vec<&str> {
        let mut handles: Vec<&str> = Vec::new();
        for failure in &self.failures {
            if failure.scenarios.iter().any(|name| name == scenario)
                && !handles.contains(&failure.handle.as_str())
            {
                handles.push(&failure.handle);
            }
        }
        handles
    }

    pub fn to_json(&self) -> Json {
        json!({
            "state": if self.checked < self.total { "partial" } else { "done" },
            "checked": self.checked,
            "total": self.total,
            "elapsedMs": self.elapsed.as_millis() as u64,
            "failures": self.failures.iter().map(|failure| json!({
                "handle": failure.handle,
                "target": failure.target,
                "error": failure.error,
                "scenarios": failure.scenarios,
            })).collect::<Vec<_>>(),
        })
    }
}

/// What one scenario came to: its runs that failed, or why it could not run.
type Outcome = Result<Vec<(String, String, String)>, String>;

/// Runs the Functions of each scenario of `store` that opens, until `budget` is spent. No
/// request of a `.fetch` target is sent. The budget is checked before each Function runs: a
/// scenario whose Functions did not all run in time is not checked (the runs of it that failed
/// before still count).
pub fn run(state: &ServerState, app: &App, store: &Store, budget: Duration) -> Report {
    let started = Instant::now();
    let deadline = started + budget;
    let options = state
        .options
        .checkout
        .as_ref()
        .expect("the home page needs --app");
    let runnable: Vec<_> = store
        .scenarios
        .iter()
        .filter(|scenario| {
            scenario.problems.is_empty()
                && crate::checkout::selection_for(app, &options.extensions, Some(scenario)).is_ok()
        })
        .collect();
    let runs = RunOptions {
        fetcher: &OfflineFetcher,
        deadline: Some(deadline),
    };
    // What a scenario came to, and whether all its Functions ran.
    let one = |scenario: &lsf_core::store::CheckoutScenario| -> (Outcome, bool) {
        // A panic in a run must not take the page down: the scenario is not checked.
        match catch_unwind(AssertUnwindSafe(|| {
            super::checkout::run_scenario(state, app, store, scenario, runs)
        })) {
            Ok(Ok(result)) => {
                let complete = !result.interrupted;
                let failed = result
                    .runs
                    .into_iter()
                    .filter_map(|run| run.error.map(|error| (run.handle, run.target, error)))
                    .collect();
                (Ok(failed), complete)
            }
            Ok(Err(problem)) => (Err(problem), true),
            Err(_) => (Err("running its Functions panicked".to_string()), true),
        }
    };
    // One after the other: the runner keeps a couple of engines with their compiled modules,
    // and more runs at once would compile every module again in new engines.
    let mut outcomes: Vec<(usize, Outcome)> = Vec::new();
    let mut checked = 0;
    for (index, scenario) in runnable.iter().enumerate() {
        if Instant::now() >= deadline {
            break;
        }
        let (outcome, complete) = one(scenario);
        outcomes.push((index, outcome));
        if !complete {
            break;
        }
        checked += 1;
    }

    let mut report = Report {
        total: runnable.len(),
        checked,
        ..Report::default()
    };
    // By (handle, target), in the order of the scenarios.
    for (index, outcome) in outcomes {
        let name = &runnable[index].name;
        // A scenario that could not run at all has errors of its own, which the page lists
        // anyway: it is not a failure.
        let Ok(failed) = outcome else {
            continue;
        };
        for (handle, target, error) in failed {
            match report
                .failures
                .iter_mut()
                .find(|failure| failure.handle == handle && failure.target == target)
            {
                Some(failure) => {
                    if !failure.scenarios.contains(name) {
                        failure.scenarios.push(name.clone());
                    }
                }
                None => report.failures.push(Failure {
                    handle,
                    target,
                    error,
                    scenarios: vec![name.clone()],
                }),
            }
        }
    }
    report.elapsed = started.elapsed();
    report
}

/// What a check was run on: the very store that was loaded, and the app as its files are.
struct Key {
    store: Arc<Store>,
    app: u64,
}

impl Key {
    fn of(app: &App, store: &Arc<Store>) -> Key {
        Key {
            store: store.clone(),
            app: fingerprint(app),
        }
    }

    fn matches(&self, other: &Key) -> bool {
        Arc::ptr_eq(&self.store, &other.store) && self.app == other.app
    }
}

/// A fingerprint of what the runs of the Functions depend on in the app: the Functions as
/// their files declare them (targets, input queries, API versions), the handles a scenario may
/// name, and the Wasm modules (their size and modification time).
fn fingerprint(app: &App) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format!("{:?}", app.functions).hash(&mut hasher);
    app.handles().hash(&mut hasher);
    for function in &app.functions {
        let metadata = std::fs::metadata(&function.wasm_path).ok();
        metadata
            .as_ref()
            .map(|metadata| metadata.len())
            .hash(&mut hasher);
        metadata
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
            .hash(&mut hasher);
    }
    hasher.finish()
}

/// The last check, kept while what it ran on stays the same.
#[derive(Default)]
pub struct Cache {
    last: Mutex<Option<(Key, Arc<Report>)>>,
    /// Held while a check runs, so that two requests do not run it twice.
    running: Mutex<()>,
}

impl Cache {
    /// Forgets the last check: a build of `--watch` is over.
    pub fn clear(&self) {
        *self.last.lock().expect("check cache poisoned") = None;
    }

    /// The last check, if it was run on this app and this store.
    pub fn get(&self, app: &App, store: &Arc<Store>) -> Option<Arc<Report>> {
        let key = Key::of(app, store);
        let last = self.last.lock().expect("check cache poisoned");
        last.as_ref()
            .filter(|(cached, _)| cached.matches(&key))
            .map(|(_, report)| report.clone())
    }

    /// The check of this app and this store: the last one, or a new one.
    pub fn get_or_run(&self, state: &ServerState, app: &App, store: &Arc<Store>) -> Arc<Report> {
        self.get_or_run_with(app, store, || run(state, app, store, BUDGET))
    }

    /// [`Cache::get_or_run`], with `check` to run the check.
    pub(super) fn get_or_run_with(
        &self,
        app: &App,
        store: &Arc<Store>,
        check: impl FnOnce() -> Report,
    ) -> Arc<Report> {
        let _running = self
            .running
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(report) = self.get(app, store) {
            return report;
        }
        // What the check runs on, taken before it runs: a module rebuilt while it runs makes the
        // key of after differ, and what it found is not kept for the new module.
        let before = Key::of(app, store);
        let report = Arc::new(check());
        // A check that ran out of time is not kept either: the next request tries again.
        if report.checked == report.total && before.matches(&Key::of(app, store)) {
            *self.last.lock().expect("check cache poisoned") = Some((before, report.clone()));
        }
        report
    }
}
