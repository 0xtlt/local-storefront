//! The runner on hand-written modules: the WASI stdin/stdout ABI, exit codes, timeouts and
//! the module cache.

mod common;

use std::path::PathBuf;
use std::time::Duration;

use lsf_checkout::runner::{self, Runner};
use serde_json::json;

fn temp_dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("lsf-checkout-runner-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Reads the input from stdin, writes it back to stdout, logs "hi" to stderr.
const ECHO: &str = r#"
(module
  (import "wasi_snapshot_preview1" "fd_read" (func $fd_read (param i32 i32 i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 100) "hi")
  (func (export "_start")
    (i32.store (i32.const 0) (i32.const 256))
    (i32.store (i32.const 4) (i32.const 4096))
    (drop (call $fd_read (i32.const 0) (i32.const 0) (i32.const 1) (i32.const 8)))
    (i32.store (i32.const 4) (i32.load (i32.const 8)))
    (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 12)))
    (i32.store (i32.const 32) (i32.const 100))
    (i32.store (i32.const 36) (i32.const 2))
    (drop (call $fd_write (i32.const 2) (i32.const 32) (i32.const 1) (i32.const 12)))))
"#;

const EXIT: &str = r#"
(module
  (import "wasi_snapshot_preview1" "proc_exit" (func $exit (param i32)))
  (memory (export "memory") 1)
  (func (export "_start") (call $exit (i32.const 3))))
"#;

const FOREVER: &str = r#"
(module
  (memory (export "memory") 1)
  (func (export "_start") (loop (br 0))))
"#;

const NOT_JSON: &str = r#"
(module
  (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 100) "nope")
  (func (export "_start")
    (i32.store (i32.const 0) (i32.const 100))
    (i32.store (i32.const 4) (i32.const 4))
    (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 12)))))
"#;

const UNTRAMPOLINED: &str = r#"
(module
  (import "shopify_function_v2" "shopify_function_input_read_utf8_str" (func (param i32 i32 i32)))
  (memory (export "memory") 1)
  (func (export "_start")))
"#;

fn write(dir: &std::path::Path, name: &str, wat: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, wat).unwrap();
    path
}

#[test]
fn a_wasi_function_reads_stdin_and_writes_stdout() {
    let dir = temp_dir("echo");
    let wasm = write(&dir, "echo.wasm", ECHO);
    let input = json!({"cart": {"id": "gid://shopify/Cart/1", "lines": [1, 2.5, "x"]}});
    let execution = runner::run(&wasm, "_start", &input);
    assert_eq!(execution.error, None);
    assert_eq!(execution.output, Some(input));
    assert_eq!(execution.logs, "hi");
    assert!(execution.instructions > 0);
}

#[test]
fn an_exit_code_is_a_failure() {
    let dir = temp_dir("exit");
    let wasm = write(&dir, "exit.wasm", EXIT);
    let execution = runner::run(&wasm, "_start", &json!({}));
    assert_eq!(
        execution.error.as_deref(),
        Some("module exited with code: 3")
    );
    assert!(execution.output.is_none());
}

#[test]
fn a_missing_export_is_a_failure() {
    let dir = temp_dir("export");
    let wasm = write(&dir, "echo.wasm", ECHO);
    let execution = runner::run(&wasm, "run", &json!({}));
    assert!(execution.error.unwrap().contains("export `run`"));
}

#[test]
fn an_output_that_is_not_json_is_a_failure() {
    let dir = temp_dir("not-json");
    let wasm = write(&dir, "bad.wasm", NOT_JSON);
    let execution = runner::run(&wasm, "_start", &json!({}));
    assert!(execution.output.is_none());
    assert!(
        execution
            .error
            .unwrap()
            .starts_with("the output is not valid")
    );
}

#[test]
fn a_module_that_was_not_trampolined_says_so() {
    let dir = temp_dir("trampoline");
    let wasm = write(&dir, "raw.wasm", UNTRAMPOLINED);
    let execution = runner::run(&wasm, "_start", &json!({}));
    let error = execution.error.unwrap();
    assert!(error.contains("shopify-function-trampoline"), "{error}");
}

#[test]
fn an_endless_function_is_stopped() {
    let dir = temp_dir("forever");
    let wasm = write(&dir, "forever.wasm", FOREVER);
    let runner = Runner::new().with_timeout(Duration::from_millis(200));
    let started = std::time::Instant::now();
    let execution = runner.run(&wasm, "_start", &json!({}));
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(execution.error.unwrap().contains("was stopped"));
}

#[test]
fn only_the_overdue_run_is_stopped() {
    // Two endless runs overlap on one runner: the first one is overdue at 600 ms, the second
    // (started 300 ms later) must keep running until its own deadline.
    let timeout = Duration::from_millis(600);
    let dir = temp_dir("overlap");
    let wasm = write(&dir, "forever.wasm", FOREVER);
    let runner = Runner::new().with_timeout(timeout);
    // Compile the module before the clock starts.
    let _ = Runner::new()
        .with_timeout(Duration::from_millis(1))
        .run(&wasm, "_start", &json!({}));

    let origin = std::time::Instant::now();
    let (first, second) = std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            let execution = runner.run(&wasm, "_start", &json!({}));
            (origin.elapsed(), execution)
        });
        std::thread::sleep(Duration::from_millis(300));
        let second = scope.spawn(|| {
            let started = origin.elapsed();
            let execution = runner.run(&wasm, "_start", &json!({}));
            (origin.elapsed() - started, execution)
        });
        (first.join().unwrap(), second.join().unwrap())
    });

    assert!(first.1.error.unwrap().contains("was stopped"));
    assert!(first.0 >= timeout, "{:?}", first.0);
    // Stopped by its own watchdog, not by the one of the first run (that fired 300 ms in).
    assert!(second.1.error.unwrap().contains("was stopped"));
    assert!(second.0 >= timeout, "{:?}", second.0);
}

#[test]
fn a_run_survives_the_timeout_of_another_one() {
    let timeout = Duration::from_millis(500);
    let dir = temp_dir("survive");
    let endless = write(&dir, "forever.wasm", FOREVER);
    let echo = write(&dir, "echo.wasm", ECHO);
    let runner = Runner::new().with_timeout(timeout);
    let input = json!({"a": 1});
    // Compile the modules before the clock starts.
    assert_eq!(runner.run(&echo, "_start", &input).error, None);

    let (endless_run, echo_run) = std::thread::scope(|scope| {
        let endless_run = scope.spawn(|| runner.run(&endless, "_start", &json!({})));
        // Runs of an instant overlap the overdue one, before and after its deadline.
        let echo_run = scope.spawn(|| {
            let mut runs = Vec::new();
            for _ in 0..8 {
                runs.push(runner.run(&echo, "_start", &input));
                std::thread::sleep(Duration::from_millis(100));
            }
            runs
        });
        (endless_run.join().unwrap(), echo_run.join().unwrap())
    });

    assert!(endless_run.error.unwrap().contains("was stopped"));
    for run in echo_run {
        assert_eq!(run.error, None);
        assert_eq!(run.output, Some(input.clone()));
    }
}

#[test]
fn a_rebuilt_module_is_picked_up() {
    let dir = temp_dir("cache");
    let wasm = dir.join("function.wasm");
    let input = json!({"a": 1});

    std::fs::write(&wasm, EXIT).unwrap();
    let runner = Runner::new();
    assert_eq!(
        runner.run(&wasm, "_start", &input).error.as_deref(),
        Some("module exited with code: 3")
    );
    // Same path, another module (another size).
    std::fs::write(&wasm, ECHO).unwrap();
    let execution = runner.run(&wasm, "_start", &input);
    assert_eq!(execution.error, None);
    assert_eq!(execution.output, Some(input.clone()));
    // And back, to be sure it is not just the first replacement.
    std::fs::write(&wasm, format!("{EXIT}\n\n\n")).unwrap();
    assert!(runner.run(&wasm, "_start", &input).error.is_some());
}

#[test]
fn a_burst_of_runs_loads_each_module_once() {
    let dir = temp_dir("burst");
    let echo = write(&dir, "echo.wasm", ECHO);
    let exit = write(&dir, "exit.wasm", EXIT);
    let runner = Runner::new().with_max_concurrent(8);
    // Sixteen runs at once, of two modules not compiled yet: each module is loaded once, and every
    // run uses it.
    let barrier = std::sync::Barrier::new(16);
    std::thread::scope(|scope| {
        for number in 0..16 {
            let (runner, echo, exit, barrier) = (&runner, &echo, &exit, &barrier);
            scope.spawn(move || {
                let input = json!({"run": number});
                barrier.wait();
                if number % 2 == 0 {
                    let execution = runner.run(echo, "_start", &input);
                    assert_eq!(execution.error, None);
                    assert_eq!(execution.output, Some(input));
                } else {
                    let execution = runner.run(exit, "_start", &input);
                    assert_eq!(
                        execution.error.as_deref(),
                        Some("module exited with code: 3")
                    );
                }
            });
        }
    });
    assert_eq!(runner.modules_loaded(), 2);
    // The next runs use them too.
    assert_eq!(runner.run(&echo, "_start", &json!({})).error, None);
    assert_eq!(runner.modules_loaded(), 2);
}

#[test]
fn concurrent_runs_are_bounded_and_the_others_wait() {
    let dir = temp_dir("bounded");
    let wasm = write(&dir, "forever.wasm", FOREVER);
    let timeout = Duration::from_millis(300);
    // Two at a time: four runs that each last until the timeout take two rounds. The time a run
    // waits is not part of its timeout.
    let runner = Runner::new().with_timeout(timeout).with_max_concurrent(2);
    let started = std::time::Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let (runner, wasm) = (&runner, &wasm);
            scope.spawn(move || {
                let execution = runner.run(wasm, "_start", &json!({}));
                assert!(execution.error.unwrap().contains("was stopped"));
            });
        }
    });
    assert!(
        started.elapsed() >= timeout * 2,
        "four runs took {:?} with two at a time",
        started.elapsed()
    );
}

#[test]
fn a_deadline_that_passes_before_the_run_starts_still_stops_it() {
    // The timeout is over before the host has set up the store: the tick that moves the
    // epoch lands before the deadline exists, and a single tick would be lost (the run would
    // never stop). The run is on a thread of its own so that the test fails instead of hanging.
    let dir = temp_dir("early");
    let wasm = write(&dir, "forever.wasm", FOREVER);
    for _ in 0..20 {
        let (done, wait) = std::sync::mpsc::channel();
        let wasm = wasm.clone();
        std::thread::spawn(move || {
            let runner = Runner::new().with_timeout(Duration::ZERO);
            let _ = done.send(runner.run(&wasm, "_start", &json!({})));
        });
        let execution = wait
            .recv_timeout(Duration::from_secs(10))
            .expect("the run was never stopped");
        assert!(execution.error.unwrap().contains("was stopped"));
    }
}
