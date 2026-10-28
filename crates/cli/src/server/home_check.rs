//! The Functions that fail when the scenarios run: the home page runs the Functions of every
//! scenario that opens, as its checkout would when it opens (`lsf function run --scenario`),
//! and lists each run that fails (a trap, too many instructions, an output that does not fit
//! the schema, a `.fetch` that cannot be sent) with the scenarios it fails in.
//!
//! Running them all takes a while (a Function that runs past the instruction limit, `.fetch`
//! targets that wait for a server), so the page does not wait for it: it asks
//! `/__lsf/checkouts/check` once it is shown. What a check found is kept until the store data
//! or the app changes.

use std::hash::{Hash, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use lsf_checkout::app::App;
use lsf_core::Store;
use serde_json::{Value as Json, json};

use super::ServerState;

/// How long a check may take: the scenarios not run by then are said to be unchecked.
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
    /// Scenarios that could not be run at all, and why (they have errors of their own, which
    /// the page lists anyway, so they are not failures).
    pub skipped: Vec<(String, String)>,
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

/// Runs the Functions of each scenario of `store` that opens, until `budget` is spent.
pub fn run(state: &ServerState, app: &App, store: &Store, budget: Duration) -> Report {
    let started = Instant::now();
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
    let one = |scenario: &lsf_core::store::CheckoutScenario| -> Outcome {
        // A panic in a run must not take the page down: the scenario is not checked.
        match catch_unwind(AssertUnwindSafe(|| {
            super::checkout::run_scenario(state, app, store, scenario)
        })) {
            Ok(Ok(result)) => Ok(result
                .runs
                .into_iter()
                .filter_map(|run| run.error.map(|error| (run.handle, run.target, error)))
                .collect()),
            Ok(Err(problem)) => Err(problem),
            Err(_) => Err("running its Functions panicked".to_string()),
        }
    };
    // One after the other: the runner keeps a couple of engines with their compiled modules,
    // and more runs at once would compile every module again in new engines.
    let mut outcomes: Vec<(usize, Outcome)> = Vec::new();
    for (index, scenario) in runnable.iter().enumerate() {
        if started.elapsed() > budget {
            break;
        }
        outcomes.push((index, one(scenario)));
    }

    let mut report = Report {
        total: runnable.len(),
        checked: outcomes.len(),
        ..Report::default()
    };
    // By (handle, target), in the order of the scenarios.
    for (index, outcome) in outcomes {
        let name = &runnable[index].name;
        let failed = match outcome {
            Ok(failed) => failed,
            Err(problem) => {
                report.skipped.push((name.clone(), problem));
                continue;
            }
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
        let _running = self
            .running
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(report) = self.get(app, store) {
            return report;
        }
        let report = Arc::new(run(state, app, store, BUDGET));
        // A check that ran out of time is not kept: the next request tries again.
        if report.checked == report.total {
            *self.last.lock().expect("check cache poisoned") =
                Some((Key::of(app, store), report.clone()));
        }
        report
    }
}
