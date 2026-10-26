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
