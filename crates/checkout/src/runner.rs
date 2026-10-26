//! Runs the Wasm module of a Function with Shopify's own `function-runner` (the engine behind
//! `shopify app function run`): the WASI stdin/stdout ABI, the Shopify Function Wasm API
//! (`shopify_function_v1`/`v2` providers) and Javy (JS Functions) all work the way they do on
//! Shopify.
//!
//! Compiled modules are cached by path, modification time and size, so a rebuilt Function is
//! picked up by the next run.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use function_runner::engine::{
    FunctionRunParams, new_engine, run as run_module, uses_msgpack_provider,
};
use function_runner::{BytesContainer, BytesContainerType, Codec};
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
pub struct Runner {
    engine: Result<Engine, String>,
    timeout: Duration,
    modules: Mutex<HashMap<PathBuf, Cached>>,
}

impl Default for Runner {
    fn default() -> Self {
        Runner::new()
    }
}

impl Runner {
    pub fn new() -> Runner {
        // function-runner's engine keeps wasmtime's compilation cache on disk (a second run of a
        // Function does not compile it again). Where that cache cannot be set up, the same
        // configuration without it does.
        let engine = new_engine().or_else(|_| {
            let mut config = Config::new();
            config
                .wasm_multi_memory(true)
                .wasm_threads(false)
                .consume_fuel(true)
                .epoch_interruption(true);
            Engine::new(&config)
        });
        Runner {
            engine: engine.map_err(|error| format!("cannot start the Wasm engine: {error}")),
            timeout: DEFAULT_TIMEOUT,
            modules: Mutex::new(HashMap::new()),
        }
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
        let module = match self.module(engine, wasm) {
            Ok(module) => module,
            Err(error) => return Execution::failed(error),
        };

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

    fn module(&self, engine: &Engine, wasm: &Path) -> Result<Module, String> {
        let metadata = std::fs::metadata(wasm).map_err(|_| {
            format!(
                "no Wasm module at {}: build the Function (`shopify app build`)",
                wasm.display()
            )
        })?;
        let modified = metadata.modified().ok();
        let size = metadata.len();

        let mut modules = self.modules.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(cached) = modules.get(wasm)
            && cached.modified == modified
            && cached.size == size
        {
            return Ok(cached.module.clone());
        }
        let module = Module::from_file(engine, wasm)
            .map_err(|error| format!("cannot load {}: {error:#}", wasm.display()))?;
        modules.insert(
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

/// A Rust Function calls the Function Wasm API of the host by name; the CLI links it to the
/// host's provider (with `shopify-function-trampoline`) when it builds the app. function-runner
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
                 shopify-function-trampoline, which `shopify app build` runs after compiling a Rust Function",
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

/// Stops a run that takes too long by moving the engine's epoch forward (the engine is
/// configured to trap at the next epoch).
struct Watchdog {
    cancel: mpsc::Sender<()>,
    handle: std::thread::JoinHandle<bool>,
}

impl Watchdog {
    fn start(engine: Engine, timeout: Duration) -> Watchdog {
        let (cancel, cancelled) = mpsc::channel();
        let handle = std::thread::spawn(move || match cancelled.recv_timeout(timeout) {
            Err(mpsc::RecvTimeoutError::Timeout) => {
                engine.increment_epoch();
                true
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
