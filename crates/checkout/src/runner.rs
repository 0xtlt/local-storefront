//! Runs the Wasm module of a Function as Shopify's own `function-runner` (the engine behind
//! `shopify app function run`) runs it ([`host`]): the WASI stdin/stdout ABI, the Shopify
//! Function Wasm API (`shopify_function_v1`/`v2` providers) and Javy (JS Functions) all work the
//! way they do on Shopify.
//!
//! Compiled modules are cached by path, modification time and size, so a rebuilt Function is
//! picked up by the next run.

mod host;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use indexmap::IndexMap;
use serde_json::Value;
use wasmtime::{Engine, Module};

/// Shopify stops a Function after 11 million Wasm instructions.
pub const INSTRUCTIONS_LIMIT: u64 = 11_000_000;

/// A Function that runs longer than this is stopped. The runner itself counts instructions but
/// never stops a Function: an endless loop would hang the checkout.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// What came out of one run.
#[derive(Clone, Debug, Default)]
pub struct Execution {
    /// The Function's output, if it produced one that parses.
    pub output: Option<Value>,
    /// What the Function logged (stderr).
    pub logs: String,
    /// Why the run failed: no module, a trap, an output that does not parse.
    pub error: Option<String>,
    /// Wasm instructions executed.
    pub instructions: u64,
}

impl Execution {
    fn failed(error: impl Into<String>) -> Execution {
        Execution {
            error: Some(error.into()),
            ..Execution::default()
        }
    }
}

/// Runs the Function at `wasm`, calling `export`, with `input` as its input.
pub fn run(wasm: &Path, export: &str, input: &Value) -> Execution {
    shared().run(wasm, export, input)
}

fn shared() -> &'static Runner {
    static RUNNER: OnceLock<Runner> = OnceLock::new();
    RUNNER.get_or_init(Runner::new)
}

/// A module compiled from a file, as the file was then. The runs of a module that is not compiled
/// yet wait for one compilation.
struct Compiled {
    modified: Option<SystemTime>,
    size: u64,
    module: OnceLock<Result<Module, String>>,
}

/// How many compiled modules a runner keeps: more than an app has Functions (the playground
/// has 50), fewer than a long session of `--watch` and tests can leave behind.
const MODULES_KEPT: usize = 64;

/// Runs Functions, concurrent runs included, on one engine: a module is compiled once, and
/// every run uses it in a store of its own.
///
/// At most as many runs as the machine has CPUs go at once (a run is CPU-bound, and holds the
/// memory of its instance): the others wait for one to finish.
pub struct Runner {
    timeout: Duration,
    max_concurrent: usize,
    engine: Result<Engine, String>,
    /// The modules compiled, the least recently used first: at most `capacity`, so that the
    /// modules of Functions gone from the app (or of other apps) do not stay forever.
    modules: Mutex<IndexMap<PathBuf, Arc<Compiled>>>,
    /// [`MODULES_KEPT`], or fewer in tests.
    capacity: usize,
    /// The providers compiled, by name: a handful.
    providers: Mutex<HashMap<String, Module>>,
    /// How many times a module was loaded: compiled, or read from wasmtime's cache on disk.
    loads: AtomicUsize,
    /// Runs going.
    busy: Mutex<usize>,
    released: Condvar,
}

impl Default for Runner {
    fn default() -> Self {
        Runner::new()
    }
}

/// A run going: it stops counting when the permit goes, even if the run panics.
struct Permit<'r>(&'r Runner);

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        *lock(&self.0.busy) -= 1;
        self.0.released.notify_one();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

impl Runner {
    /// A runner with an engine of its own. The engine keeps wasmtime's compilation cache on disk,
    /// as function-runner's does (a second run of a Function does not compile it again). Where
    /// that cache cannot be set up, the same configuration without it does.
    pub fn new() -> Runner {
        Runner {
            timeout: DEFAULT_TIMEOUT,
            max_concurrent: std::thread::available_parallelism().map_or(4, |count| count.get()),
            engine: engine(),
            modules: Mutex::new(IndexMap::new()),
            capacity: MODULES_KEPT,
            providers: Mutex::new(HashMap::new()),
            loads: AtomicUsize::new(0),
            busy: Mutex::new(0),
            released: Condvar::new(),
        }
    }

    /// A runner that runs at most `max_concurrent` Functions at once (at least one).
    pub fn with_max_concurrent(mut self, max_concurrent: usize) -> Runner {
        self.max_concurrent = max_concurrent.max(1);
        self
    }

    /// How many times a module was loaded (compiled, or read from wasmtime's cache on disk):
    /// once per module and version of it, however many runs use it.
    pub fn modules_loaded(&self) -> usize {
        self.loads.load(Ordering::Relaxed)
    }

    /// Waits while `max_concurrent` runs are going.
    fn permit(&self) -> Permit<'_> {
        let mut busy = lock(&self.busy);
        while *busy >= self.max_concurrent {
            busy = self.released.wait(busy).unwrap_or_else(|e| e.into_inner());
        }
        *busy += 1;
        Permit(self)
    }

    /// A runner that stops a Function after `timeout` (10 seconds by default).
    pub fn with_timeout(mut self, timeout: Duration) -> Runner {
        self.timeout = timeout;
        self
    }

    pub fn run(&self, wasm: &Path, export: &str, input: &Value) -> Execution {
        let engine = match &self.engine {
            Ok(engine) => engine,
            Err(error) => return Execution::failed(error.clone()),
        };
        let _permit = self.permit();
        self.execute(engine, wasm, export, input)
    }

    fn execute(&self, engine: &Engine, wasm: &Path, export: &str, input: &Value) -> Execution {
        let module = match self.module(engine, wasm) {
            Ok(module) => module,
            Err(error) => return Execution::failed(error),
        };
        if let Some(error) = untrampolined(&module) {
            return Execution::failed(error);
        }

        let msgpack = host::uses_msgpack_provider(&module);
        let encoded = match host::encode_input(input, msgpack) {
            Ok(encoded) => encoded,
            Err(error) => return Execution::failed(error),
        };

        let stopped = Arc::new(AtomicBool::new(false));
        let deadline = host::Deadline {
            at: Instant::now() + self.timeout,
            stopped: stopped.clone(),
        };
        let watchdog = Watchdog::start(engine.clone(), self.timeout);
        let result = host::run(
            engine,
            &module,
            &self.providers,
            export,
            encoded,
            msgpack,
            deadline,
        );
        // The watchdog is over when `run` returns: the next runs are not ticked for.
        watchdog.stop();

        if stopped.load(Ordering::Relaxed) {
            let (logs, instructions) = match &result {
                Ok(result) => (result.logs.clone(), result.instructions),
                Err(_) => (String::new(), 0),
            };
            return Execution {
                output: None,
                logs,
                error: Some(format!(
                    "the Function ran for more than {:?} and was stopped",
                    self.timeout
                )),
                instructions,
            };
        }
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                // The export does not exist, the module cannot be instantiated...
                return Execution::failed(format!("{error:#}"));
            }
        };

        let mut execution = Execution {
            output: None,
            logs: result.logs.clone(),
            error: None,
            instructions: result.instructions,
        };
        if !result.success {
            // The trap or exit code is appended to the logs after what the Function printed.
            let (logs, error) = split_failure(&result.logs);
            execution.error = Some(match panic_message(&logs) {
                Some(message) => format!("the Function panicked: {message}"),
                None => error,
            });
            execution.logs = logs;
            return execution;
        }
        match result.output {
            Ok(output) => execution.output = Some(output),
            Err(error) => execution.error = Some(format!("the output is not valid: {error}")),
        }
        execution
    }

    /// The module at `wasm`, compiled once per version of the file.
    fn module(&self, engine: &Engine, wasm: &Path) -> Result<Module, String> {
        let metadata = std::fs::metadata(wasm).map_err(|_| {
            format!(
                "no Wasm module at {}: build the Function (`shopify app build`)",
                wasm.display()
            )
        })?;
        let modified = metadata.modified().ok();
        let size = metadata.len();

        let compiled = {
            let mut modules = lock(&self.modules);
            // Taken out, and put back last: the most recently used.
            let compiled = match modules.shift_remove(wasm) {
                Some(compiled) if compiled.modified == modified && compiled.size == size => {
                    compiled
                }
                _ => Arc::new(Compiled {
                    modified,
                    size,
                    module: OnceLock::new(),
                }),
            };
            // Past the capacity, the least recently used goes.
            while modules.len() >= self.capacity.max(1) {
                modules.shift_remove_index(0);
            }
            modules.insert(wasm.to_path_buf(), compiled.clone());
            compiled
        };
        // Outside the lock: the runs of other modules do not wait for this compilation.
        let loaded = compiled.module.get_or_init(|| {
            self.loads.fetch_add(1, Ordering::Relaxed);
            Module::from_file(engine, wasm)
                .map_err(|error| format!("cannot load {}: {error:#}", wasm.display()))
        });
        match loaded {
            Ok(module) => Ok(module.clone()),
            Err(error) => {
                // Not kept: the next run tries again.
                let mut modules = lock(&self.modules);
                if modules
                    .get(wasm)
                    .is_some_and(|kept| Arc::ptr_eq(kept, &compiled))
                {
                    modules.shift_remove(wasm);
                }
                Err(error.clone())
            }
        }
    }
}

fn engine() -> Result<Engine, String> {
    host::engine()
        .or_else(|_| Engine::new(&host::config()))
        .map_err(|error| format!("cannot start the Wasm engine: {error}"))
}

/// Whether `bytes` are a module a run can load: compiled as a run compiles it, with the same
/// engine settings, and exporting a function. Says why not. What it compiles goes to the same
/// cache as the runs, so the first run of the module does not compile it again.
pub fn check_module(bytes: &[u8]) -> Result<(), String> {
    let engine = shared().engine.as_ref().map_err(Clone::clone)?;
    let module = Module::new(engine, bytes).map_err(|error| format!("{error:#}"))?;
    if !module
        .exports()
        .any(|export| matches!(export.ty(), wasmtime::ExternType::Func(_)))
    {
        return Err("it exports no function".to_string());
    }
    Ok(())
}

/// A Rust Function calls the Function Wasm API of the host by name; lsf (`--build`,
/// `lsf app function build`) and Shopify's CLI link it to the host's provider with
/// `shopify-function-trampoline` after its build command ([`crate::trampoline`]). function-runner
/// and the host only run linked modules, and say so with an unhelpful "unknown import".
fn untrampolined(module: &Module) -> Option<String> {
    module
        .imports()
        .find(|import| {
            import.module().starts_with("shopify_function_v")
                && import.name() != "memory"
                && !import.name().starts_with('_')
        })
        .map(|import| {
            format!(
                "the module calls `{}::{}` directly: it was not processed by \
                 shopify-function-trampoline, which lsf (`lsf app function build`, `--build`) and \
                 `shopify app build` run after compiling a Rust Function",
                import.module(),
                import.name()
            )
        })
}

/// The host appends why a run failed to what the Function logged. Splits the two: a trap
/// is reported as `error while executing at wasm backtrace: ...`, an exit as `module exited with
/// code: N`; other failures are one line at the end.
fn split_failure(logs: &str) -> (String, String) {
    for marker in [
        "error while executing at wasm backtrace",
        "module exited with code",
    ] {
        if let Some(at) = logs.find(marker) {
            let (before, after) = logs.split_at(at);
            return (before.to_string(), summarize_failure(after));
        }
    }
    match logs.trim_end().rsplit_once('\n') {
        Some((before, last)) => (format!("{before}\n"), last.to_string()),
        None => (String::new(), logs.trim().to_string()),
    }
}

/// The message of a Rust panic in the logs: `panicked at src/lib.rs:1:1:\nmessage`.
fn panic_message(logs: &str) -> Option<&str> {
    let (_, after) = logs.split_once("panicked at ")?;
    let (_, message) = after.split_once('\n')?;
    message.lines().next().filter(|line| !line.is_empty())
}

/// A trap prints a backtrace of Wasm functions: the Function's own panic message (in the logs)
/// says more, so the error is the trap's reason.
fn summarize_failure(failure: &str) -> String {
    let reason = failure.lines().rev().map(str::trim).find(|line| {
        !line.is_empty()
            && !line.starts_with(|c: char| c.is_ascii_digit())
            && !line.starts_with("0x")
    });
    match reason {
        Some(reason) if reason != "error while executing at wasm backtrace:" => reason.to_string(),
        _ => "the Function trapped".to_string(),
    }
}

/// Moves the epoch of the engine forward once a run is overdue, and every 10 ms until the run is
/// over. Every run on the engine reaches its epoch deadline then: the deadline of each
/// ([`host::Deadline`]) stops the overdue one only, and the others go on.
struct Watchdog {
    cancel: mpsc::Sender<()>,
    handle: std::thread::JoinHandle<()>,
}

impl Watchdog {
    fn start(engine: Engine, timeout: Duration) -> Watchdog {
        let (cancel, cancelled) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            if let Err(mpsc::RecvTimeoutError::Timeout) = cancelled.recv_timeout(timeout) {
                // The host sets the deadline one epoch after the current one when it
                // creates the store, and again around the initialization of the input: a single
                // tick that lands before one of those is absorbed, and an endless run never
                // stops. Keep ticking until the run is over.
                engine.increment_epoch();
                while let Err(mpsc::RecvTimeoutError::Timeout) =
                    cancelled.recv_timeout(Duration::from_millis(10))
                {
                    engine.increment_epoch();
                }
            }
        });
        Watchdog { cancel, handle }
    }

    fn stop(self) {
        let _ = self.cancel.send(());
        let _ = self.handle.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_a_trap_from_the_logs() {
        let logs = "hello\nwasm trap reason\nerror while executing at wasm backtrace:\n    0: 0x1a - <unknown>!x\n";
        let (before, error) = split_failure(logs);
        assert_eq!(before, "hello\nwasm trap reason\n");
        assert_eq!(error, "the Function trapped");
    }

    #[test]
    fn splits_an_exit_code() {
        let (before, error) = split_failure("oops\nmodule exited with code: 1");
        assert_eq!(before, "oops\n");
        assert_eq!(error, "module exited with code: 1");
    }

    #[test]
    fn a_runner_keeps_the_modules_used_last() {
        let dir = std::env::temp_dir().join(format!("lsf-runner-lru-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = |name: &str| dir.join(name);
        for name in ["a.wasm", "b.wasm", "c.wasm"] {
            // The smallest module there is.
            std::fs::write(path(name), b"\0asm\x01\0\0\0").unwrap();
        }
        let mut runner = Runner::new();
        runner.capacity = 2;
        let engine = runner.engine.clone().unwrap();
        runner.module(&engine, &path("a.wasm")).unwrap();
        runner.module(&engine, &path("b.wasm")).unwrap();
        // `a` is used again: `b` is now the oldest, and goes for `c`.
        runner.module(&engine, &path("a.wasm")).unwrap();
        runner.module(&engine, &path("c.wasm")).unwrap();
        let kept: Vec<PathBuf> = lock(&runner.modules).keys().cloned().collect();
        assert_eq!(kept, [path("a.wasm"), path("c.wasm")]);
        assert_eq!(runner.modules_loaded(), 3);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_module_is_an_error() {
        let execution = run(
            Path::new("/nowhere/function.wasm"),
            "run",
            &serde_json::json!({}),
        );
        assert!(execution.output.is_none());
        assert!(execution.error.unwrap().contains("no Wasm module"));
    }
}
