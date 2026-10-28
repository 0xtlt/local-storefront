//! Runs the Wasm module of a Function with Shopify's own `function-runner` (the engine behind
//! `shopify app function run`): the WASI stdin/stdout ABI, the Shopify Function Wasm API
//! (`shopify_function_v1`/`v2` providers) and Javy (JS Functions) all work the way they do on
//! Shopify.
//!
//! Compiled modules are cached by path, modification time and size, so a rebuilt Function is
//! picked up by the next run.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use function_runner::engine::{
    FunctionRunParams, new_engine, run as run_module, uses_msgpack_provider,
};
use function_runner::{BytesContainer, BytesContainerType, Codec};
use indexmap::IndexMap;
use serde_json::Value;
use wasmtime::{Config, Engine, Module};

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

struct Cached {
    modified: Option<SystemTime>,
    size: u64,
    module: Module,
}

/// An engine and the modules compiled with it.
///
/// function-runner arms the engine's epoch deadline itself, at the next epoch, so the epoch
/// of an engine stops every run that uses it at the time. A slot is therefore used by one run at
/// a time: the watchdog of a run moves the epoch of its own engine only.
struct Slot {
    engine: Engine,
    /// The modules compiled, the least recently used first: at most [`MODULES_PER_ENGINE`], so
    /// that the modules of Functions gone from the app (or of other apps) do not stay forever.
    modules: IndexMap<PathBuf, Cached>,
    /// [`MODULES_PER_ENGINE`], or fewer in tests.
    capacity: usize,
}

/// How many compiled modules an engine keeps: more than an app has Functions (the playground
/// has 50), fewer than a long session of `--watch` and tests can leave behind.
const MODULES_PER_ENGINE: usize = 64;

/// How many engines stay in the pool when no run needs them.
const IDLE_ENGINES: usize = 2;

/// The engines of a runner.
struct Pool {
    idle: Vec<Slot>,
    /// Slots in use.
    busy: usize,
    /// Runs waiting for a slot.
    waiting: usize,
}

/// Runs Functions, concurrent runs included: each run has an engine of its own (from a pool, so
/// that the compiled modules are reused).
///
/// At most as many runs as the machine has CPUs go at once (a run is CPU-bound; every run holds
/// an engine and the memory of its instance): the others wait for one to finish. Past a small
/// number of idle engines, the engines of a burst are dropped, with the modules they compiled.
pub struct Runner {
    timeout: Duration,
    max_concurrent: usize,
    keep_idle: usize,
    pool: Mutex<Pool>,
    released: Condvar,
}

impl Default for Runner {
    fn default() -> Self {
        Runner::new()
    }
}

/// A slot taken from the pool: it goes back to it when the run is over, even if the run panics.
struct Lease<'r> {
    runner: &'r Runner,
    slot: Option<Slot>,
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Some(slot) = self.slot.take() {
            self.runner.release(slot);
        }
    }
}

impl Runner {
    pub fn new() -> Runner {
        Runner {
            timeout: DEFAULT_TIMEOUT,
            max_concurrent: std::thread::available_parallelism().map_or(4, |count| count.get()),
            keep_idle: IDLE_ENGINES,
            pool: Mutex::new(Pool {
                idle: Vec::new(),
                busy: 0,
                waiting: 0,
            }),
            released: Condvar::new(),
        }
    }

    /// A runner that runs at most `max_concurrent` Functions at once (at least one) and keeps
    /// at most `keep_idle` engines when it is idle.
    pub fn with_limits(mut self, max_concurrent: usize, keep_idle: usize) -> Runner {
        self.max_concurrent = max_concurrent.max(1);
        self.keep_idle = keep_idle;
        self
    }

    /// How many engines wait for a run.
    pub fn idle_engines(&self) -> usize {
        self.pool().idle.len()
    }

    fn pool(&self) -> std::sync::MutexGuard<'_, Pool> {
        self.pool.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// An idle slot, or a new one. Waits while `max_concurrent` runs are going.
    fn acquire(&self) -> Result<Lease<'_>, String> {
        let mut pool = self.pool();
        if pool.busy >= self.max_concurrent {
            pool.waiting += 1;
            while pool.busy >= self.max_concurrent {
                pool = self.released.wait(pool).unwrap_or_else(|e| e.into_inner());
            }
            pool.waiting -= 1;
        }
        pool.busy += 1;
        let idle = pool.idle.pop();
        drop(pool);
        let slot = match idle {
            Some(slot) => Ok(slot),
            None => new_slot(),
        };
        match slot {
            Ok(slot) => Ok(Lease {
                runner: self,
                slot: Some(slot),
            }),
            Err(error) => {
                self.give_back(None);
                Err(error)
            }
        }
    }

    fn release(&self, slot: Slot) {
        self.give_back(Some(slot));
    }

    /// Ends a run. Its slot stays in the pool if the pool is short of idle engines (or runs wait
    /// for one), and is dropped otherwise.
    fn give_back(&self, slot: Option<Slot>) {
        let mut pool = self.pool();
        pool.busy -= 1;
        let keep = self.keep_idle.max(pool.waiting);
        let dropped = match slot {
            Some(slot) if pool.idle.len() < keep => {
                pool.idle.push(slot);
                None
            }
            other => other,
        };
        drop(pool);
        self.released.notify_one();
        // An engine is torn down outside the lock.
        drop(dropped);
    }

    /// A runner that stops a Function after `timeout` (10 seconds by default).
    pub fn with_timeout(mut self, timeout: Duration) -> Runner {
        self.timeout = timeout;
        self
    }

    pub fn run(&self, wasm: &Path, export: &str, input: &Value) -> Execution {
        let mut lease = match self.acquire() {
            Ok(lease) => lease,
            Err(error) => return Execution::failed(error),
        };
        let Some(slot) = lease.slot.as_mut() else {
            return Execution::failed("no Wasm engine");
        };
        // The watchdog is over when `execute` returns, so the slot is clean for the next run.
        self.execute(slot, wasm, export, input)
    }

    fn execute(&self, slot: &mut Slot, wasm: &Path, export: &str, input: &Value) -> Execution {
        let module = match slot.module(wasm) {
            Ok(module) => module,
            Err(error) => return Execution::failed(error),
        };
        let engine = &slot.engine;

        if let Some(error) = untrampolined(&module) {
            return Execution::failed(error);
        }

        let codec = if uses_msgpack_provider(&module) {
            Codec::Messagepack
        } else {
            Codec::Json
        };
        let raw = match serde_json::to_vec(input) {
            Ok(raw) => raw,
            Err(error) => return Execution::failed(format!("cannot serialize the input: {error}")),
        };
        let input = match BytesContainer::new(BytesContainerType::Input, codec, raw) {
            Ok(input) => input,
            Err(error) => return Execution::failed(error.to_string()),
        };

        let watchdog = Watchdog::start(engine.clone(), self.timeout);
        let result = run_module(FunctionRunParams {
            function_path: wasm.to_path_buf(),
            input,
            export,
            profile_opts: None,
            scale_factor: 1.0,
            module,
            engine: engine.clone(),
        });
        let timed_out = watchdog.stop();

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
        if timed_out {
            execution.error = Some(format!(
                "the Function ran for more than {:?} and was stopped",
                self.timeout
            ));
            return execution;
        }
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
        if let Some(error) = &result.output.encoding_error {
            execution.error = Some(format!("the output is not valid: {error}"));
            return execution;
        }
        execution.output = result.output.json_value;
        if execution.output.is_none() {
            execution.error = Some("the Function produced no output".to_string());
        }
        execution
    }
}

/// A new engine. function-runner's engine keeps wasmtime's compilation cache on disk (a second
/// run of a Function does not compile it again). Where that cache cannot be set up, the same
/// configuration without it does.
fn new_slot() -> Result<Slot, String> {
    Ok(Slot {
        engine: engine()?,
        modules: IndexMap::new(),
        capacity: MODULES_PER_ENGINE,
    })
}

fn engine() -> Result<Engine, String> {
    new_engine()
        .or_else(|_| {
            let mut config = Config::new();
            config
                .wasm_multi_memory(true)
                .wasm_threads(false)
                .consume_fuel(true)
                .epoch_interruption(true);
            Engine::new(&config)
        })
        .map_err(|error| format!("cannot start the Wasm engine: {error}"))
}

/// Whether `bytes` are a module a run can load: compiled as a run compiles it, with the same
/// engine settings, and exporting a function. Says why not. What it compiles goes to the same
/// cache as the runs, so the first run of the module does not compile it again.
pub fn check_module(bytes: &[u8]) -> Result<(), String> {
    static ENGINE: OnceLock<Result<Engine, String>> = OnceLock::new();
    let engine = ENGINE.get_or_init(engine).as_ref().map_err(Clone::clone)?;
    let module = Module::new(engine, bytes).map_err(|error| format!("{error:#}"))?;
    if !module
        .exports()
        .any(|export| matches!(export.ty(), wasmtime::ExternType::Func(_)))
    {
        return Err("it exports no function".to_string());
    }
    Ok(())
}

impl Slot {
    fn module(&mut self, wasm: &Path) -> Result<Module, String> {
        let metadata = std::fs::metadata(wasm).map_err(|_| {
            format!(
                "no Wasm module at {}: build the Function (`shopify app build`)",
                wasm.display()
            )
        })?;
        let modified = metadata.modified().ok();
        let size = metadata.len();

        // Taken out, and put back last: the most recently used.
        if let Some(cached) = self.modules.shift_remove(wasm)
            && cached.modified == modified
            && cached.size == size
        {
            let module = cached.module.clone();
            self.modules.insert(wasm.to_path_buf(), cached);
            return Ok(module);
        }
        let module = Module::from_file(&self.engine, wasm)
            .map_err(|error| format!("cannot load {}: {error:#}", wasm.display()))?;
        // Past the capacity, the least recently used goes.
        while self.modules.len() >= self.capacity {
            self.modules.shift_remove_index(0);
        }
        self.modules.insert(
            wasm.to_path_buf(),
            Cached {
                modified,
                size,
                module: module.clone(),
            },
        );
        Ok(module)
    }
}

/// A Rust Function calls the Function Wasm API of the host by name; lsf (`--build`,
/// `lsf function build`) and Shopify's CLI link it to the host's provider with
/// `shopify-function-trampoline` after its build command ([`crate::trampoline`]). function-runner
/// only runs linked modules, and says so with an unhelpful "unknown import".
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
                 shopify-function-trampoline, which lsf (`lsf function build`, `--build`) and \
                 `shopify app build` run after compiling a Rust Function",
                import.module(),
                import.name()
            )
        })
}

/// function-runner appends why a run failed to what the Function logged. Splits the two: a trap
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

/// Stops a run that takes too long by moving the epoch of its engine forward (the run traps at
/// the next epoch). The engine is the run's own: no other run is stopped with it.
struct Watchdog {
    cancel: mpsc::Sender<()>,
    handle: std::thread::JoinHandle<bool>,
}

impl Watchdog {
    fn start(engine: Engine, timeout: Duration) -> Watchdog {
        let (cancel, cancelled) = mpsc::channel();
        let handle = std::thread::spawn(move || match cancelled.recv_timeout(timeout) {
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // function-runner sets the deadline one epoch after the current one when it
                // creates the store, and again around the initialization of the input: a single
                // tick that lands before one of those is absorbed, and an endless run never
                // stops. Keep ticking until the run is over.
                loop {
                    engine.increment_epoch();
                    match cancelled.recv_timeout(Duration::from_millis(10)) {
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        _ => break true,
                    }
                }
            }
            _ => false,
        });
        Watchdog { cancel, handle }
    }

    /// Whether the watchdog had to stop the run.
    fn stop(self) -> bool {
        let _ = self.cancel.send(());
        self.handle.join().unwrap_or(false)
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
    fn an_engine_keeps_the_modules_used_last() {
        let dir = std::env::temp_dir().join(format!("lsf-runner-lru-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = |name: &str| dir.join(name);
        for name in ["a.wasm", "b.wasm", "c.wasm"] {
            // The smallest module there is.
            std::fs::write(path(name), b"\0asm\x01\0\0\0").unwrap();
        }
        let mut slot = new_slot().unwrap();
        slot.capacity = 2;
        slot.module(&path("a.wasm")).unwrap();
        slot.module(&path("b.wasm")).unwrap();
        // `a` is used again: `b` is now the oldest, and goes for `c`.
        slot.module(&path("a.wasm")).unwrap();
        slot.module(&path("c.wasm")).unwrap();
        let kept: Vec<PathBuf> = slot.modules.keys().cloned().collect();
        assert_eq!(kept, [path("a.wasm"), path("c.wasm")]);
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
