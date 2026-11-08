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

use lsf_app::checkout::BuyerJourneyStep;
use lsf_app::checkout::expect;
use lsf_app::checkout::fetch::OfflineFetcher;
use lsf_app::checkout::pipeline::RunOptions;
use lsf_app::config::App;
use lsf_core::Store;
use lsf_core::store::model::ScenarioStep;
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
    /// Why the run failed, as the first scenario it failed in says. A trap ends with the last
    /// message the Function logged: `the Function trapped: Error: boom`.
    pub error: String,
    /// What the Function logged in the first scenario it failed in.
    pub logs: String,
    pub scenarios: Vec<String>,
}

/// An expectation of the `expect` of a scenario that its checkout does not meet.
#[derive(Clone, Debug, PartialEq)]
pub struct Unmet {
    pub scenario: String,
    /// The file of the scenario, relative to the data directory.
    pub file: String,
    /// The step the Functions ran for: `completion` or `interaction`.
    pub step: &'static str,
    pub unmet: expect::Unmet,
}

/// What a check found.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub failures: Vec<Failure>,
    /// How many expectations of the `expect` of the scenarios were checked, met or not, and
    /// those that are not met. Checked by [`run_with_expectations`] only.
    pub expectations: usize,
    pub unmet: Vec<Unmet>,
    /// How many scenarios ran, out of how many could.
    pub checked: usize,
    pub total: usize,
    pub elapsed: Duration,
    /// The handles of the Functions that ran in at least one scenario, failing or not, in the
    /// order they first ran.
    pub ran: Vec<String>,
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
                "logs": failure.logs,
                "scenarios": failure.scenarios,
            })).collect::<Vec<_>>(),
        })
    }
}

/// What one scenario came to: the Functions that ran, the runs that failed and what its
/// `expect` found (when it has one and it was checked), or why it could not run.
type Outcome = Result<(Vec<String>, Vec<FailedRun>, Option<expect::Outcome>), String>;

/// A run that failed: the handle, the target, the error and the logs.
type FailedRun = (String, String, String, String);

/// Runs the Functions of each scenario of `store` that opens, until `budget` is spent. No
/// request of a `.fetch` target is sent. The budget is checked before each Function runs: a
/// scenario whose Functions did not all run in time is not checked (the runs of it that failed
/// before still count).
pub fn run(state: &ServerState, app: &App, store: &Store, budget: Duration) -> Report {
    check(state, app, store, budget, false)
}

/// [`run`], and the `expect` of each scenario that has one checked against its checkout: the
/// one the Functions compute at the step it names (`completion` by default, the Functions run
/// a second time then), which `lsf app check` fails on.
pub fn run_with_expectations(
    state: &ServerState,
    app: &App,
    store: &Store,
    budget: Duration,
) -> Report {
    check(state, app, store, budget, true)
}

fn check(
    state: &ServerState,
    app: &App,
    store: &Store,
    budget: Duration,
    expectations: bool,
) -> Report {
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
            scenario.problems.is_empty() && options.selection(app, Some(scenario)).is_ok()
        })
        .collect();
    let runs = RunOptions {
        fetcher: &OfflineFetcher,
        deadline: Some(deadline),
    };
    // What a scenario came to, and whether all its Functions ran.
    let one = |scenario: &lsf_core::store::CheckoutScenario| -> (Outcome, bool) {
        let at = |step| super::checkout::run_scenario(state, app, store, scenario, runs, step);
        // A panic in a run must not take the page down: the scenario is not checked.
        let ran = catch_unwind(AssertUnwindSafe(|| {
            // The checkout as it opens, then, for an `expect` of the completion, as the order is
            // placed: only a validation Function reads the difference.
            let mut results = vec![at(BuyerJourneyStep::Interaction)?];
            let mut found = None;
            if let Some(expected) = scenario.expect.as_ref().filter(|_| expectations)
                && !results[0].interrupted
            {
                if expected.step.unwrap_or_default() == ScenarioStep::Completion {
                    results.push(at(BuyerJourneyStep::Completion)?);
                }
                let last = results.last().expect("one result at least");
                if !last.interrupted {
                    found = Some(expect::evaluate(expected, last));
                }
            }
            Ok::<_, String>((results, found))
        }));
        match ran {
            Ok(Ok((results, found))) => {
                let complete = results.iter().all(|result| !result.interrupted);
                let mut ran = Vec::new();
                let mut failed = Vec::new();
                for result in results {
                    for run in result.runs {
                        if !ran.contains(&run.handle) {
                            ran.push(run.handle.clone());
                        }
                        if let Some(error) = run.error {
                            failed.push((
                                run.handle,
                                run.target,
                                with_message(error, &run.logs),
                                run.logs,
                            ));
                        }
                    }
                }
                (Ok((ran, failed, found)), complete)
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
        let scenario = runnable[index];
        let name = &scenario.name;
        let step = match scenario.expect.as_ref().and_then(|expect| expect.step) {
            Some(ScenarioStep::Interaction) => "interaction",
            _ => "completion",
        };
        let unmet = |unmet| Unmet {
            scenario: name.clone(),
            file: scenario.file.clone(),
            step,
            unmet,
        };
        // A scenario that could not run at all has errors of its own, which the page lists
        // anyway: it is not a failure. Its `expect` is not met, though: nothing checked it.
        let (ran, failed, found) = match outcome {
            Ok(outcome) => outcome,
            Err(problem) => {
                if expectations && scenario.expect.is_some() {
                    report.expectations += 1;
                    report.unmet.push(unmet(expect::Unmet {
                        assertion: "expect".to_string(),
                        expected: json!("the checkout of the scenario"),
                        actual: json!(problem),
                    }));
                }
                continue;
            }
        };
        if let Some(found) = found {
            report.expectations += found.checked;
            report.unmet.extend(found.unmet.into_iter().map(unmet));
        }
        for handle in ran {
            if !report.ran.contains(&handle) {
                report.ran.push(handle);
            }
        }
        for (handle, target, error, logs) in failed {
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
                    logs,
                    scenarios: vec![name.clone()],
                }),
            }
        }
    }
    report.elapsed = started.elapsed();
    report
}

/// A trap says nothing of why: what the Function logged last does (`Error: boom`, the message
/// of what a JavaScript Function threw, before the lines of its stack).
fn with_message(error: String, logs: &str) -> String {
    if error != "the Function trapped" {
        return error;
    }
    let message = logs
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("at "));
    match message {
        Some(message) => format!("{error}: {message}"),
        None => error,
    }
}

/// What a check was run on: the very store that was loaded, the app as its files are, and
/// whether the Admin API had the app uninstalled (none of its Functions runs then).
struct Key {
    store: Arc<Store>,
    app: u64,
    uninstalled: bool,
}

impl Key {
    fn of(app: &App, store: &Arc<Store>, uninstalled: bool) -> Key {
        Key {
            store: store.clone(),
            app: fingerprint(app),
            uninstalled,
        }
    }

    fn matches(&self, other: &Key) -> bool {
        Arc::ptr_eq(&self.store, &other.store)
            && self.app == other.app
            && self.uninstalled == other.uninstalled
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

    /// The last check, if it was run on this app and this store, the app installed or not as
    /// it is now (`uninstalled`).
    pub fn get(&self, app: &App, store: &Arc<Store>, uninstalled: bool) -> Option<Arc<Report>> {
        let key = Key::of(app, store, uninstalled);
        let last = self.last.lock().expect("check cache poisoned");
        last.as_ref()
            .filter(|(cached, _)| cached.matches(&key))
            .map(|(_, report)| report.clone())
    }

    /// The check of this app and this store: the last one, or a new one.
    pub fn get_or_run(&self, state: &ServerState, app: &App, store: &Arc<Store>) -> Arc<Report> {
        let uninstalled = state.app_uninstalled();
        self.get_or_run_with(app, store, uninstalled, || run(state, app, store, BUDGET))
    }

    /// [`Cache::get_or_run`], with `check` to run the check.
    pub(super) fn get_or_run_with(
        &self,
        app: &App,
        store: &Arc<Store>,
        uninstalled: bool,
        check: impl FnOnce() -> Report,
    ) -> Arc<Report> {
        let _running = self
            .running
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(report) = self.get(app, store, uninstalled) {
            return report;
        }
        // What the check runs on, taken before it runs: a module rebuilt while it runs makes the
        // key of after differ, and what it found is not kept for the new module.
        let before = Key::of(app, store, uninstalled);
        let report = Arc::new(check());
        // A check that ran out of time is not kept either: the next request tries again.
        if report.checked == report.total && before.matches(&Key::of(app, store, uninstalled)) {
            *self.last.lock().expect("check cache poisoned") = Some((before, report.clone()));
        }
        report
    }
}

#[cfg(test)]
mod tests {
    use super::with_message;

    #[test]
    fn a_trap_ends_with_the_last_message_the_function_logged() {
        let thrown = "checking\nError: boom\n    at run (function.mjs:3:9)\n    at main (function.mjs:9:2)\n";
        assert_eq!(
            with_message("the Function trapped".into(), thrown),
            "the Function trapped: Error: boom"
        );
        // Nothing logged: nothing to add.
        assert_eq!(
            with_message("the Function trapped".into(), ""),
            "the Function trapped"
        );
        // Another error says why already.
        assert_eq!(
            with_message("module exited with code: 1".into(), thrown),
            "module exited with code: 1"
        );
    }
}
