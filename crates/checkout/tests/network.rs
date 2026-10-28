//! Network access for Functions through the pipeline: a `.fetch` target asks for an HTTP
//! request, the checkout sends it, and the `.run` target of the Function reads the response as
//! `fetchResult`.
//!
//! The Functions are Wasm modules that answer what the input builder gives them (see
//! `answering_module`), so the tests choose the output of each target, and either a fake
//! fetcher or a server of the test answers the requests.

mod common;

use std::cell::RefCell;

use common::*;
use lsf_checkout::app::Selection;
use lsf_checkout::fetch::{Fetcher, OfflineFetcher, Request, Response};
use lsf_checkout::input::{FetchInput, InputContext};
use lsf_checkout::pipeline::{
    RunOptions, compute_with, compute_with_fetcher, compute_with_options,
};
use lsf_checkout::{CheckoutResult, CheckoutState, DiscountClass, DiscountConfig};
use serde_json::{Value, json};

/// A module that exports each name, reads its input and answers it: what the input builder
/// returns is what the Function outputs.
fn answering_module(exports: &[&str]) -> String {
    let functions: String = exports
        .iter()
        .map(|export| {
            r#"
  (func (export "EXPORT")
    (i32.store (i32.const 0) (i32.const 256))
    (i32.store (i32.const 4) (i32.const 4096))
    (drop (call $fd_read (i32.const 0) (i32.const 0) (i32.const 1) (i32.const 8)))
    (i32.store (i32.const 4) (i32.load (i32.const 8)))
    (drop (call $fd_write (i32.const 1) (i32.const 0) (i32.const 1) (i32.const 12))))"#
                .replace("EXPORT", export)
        })
        .collect();
    format!(
        r#"
(module
  (import "wasi_snapshot_preview1" "fd_read" (func $fd_read (param i32 i32 i32 i32) (result i32)))
  (import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1){functions}
)
"#
    )
}

/// A Function of the fixture app, with a `.run` target and the `.fetch` target that goes with it.
#[derive(Clone, Copy)]
struct Kind {
    handle: &'static str,
    run: &'static str,
    fetch: &'static str,
    /// What the `.run` target answers when it has nothing to say.
    nothing: fn() -> Value,
}

const LINES_DISCOUNT: Kind = Kind {
    handle: "volume-discount",
    run: "cart.lines.discounts.generate.run",
    fetch: "cart.lines.discounts.generate.fetch",
    nothing: || json!({"operations": []}),
};

const DELIVERY_DISCOUNT: Kind = Kind {
    handle: "volume-discount",
    run: "cart.delivery-options.discounts.generate.run",
    fetch: "cart.delivery-options.discounts.generate.fetch",
    nothing: || json!({"operations": []}),
};

const VALIDATION: Kind = Kind {
    handle: "quantity-limit",
    run: "cart.validations.generate.run",
    fetch: "cart.validations.generate.fetch",
    nothing: || json!({"operations": []}),
};

/// What the `.run` target reads of the response.
const FETCH_RESULT: &str = r#"{ fetchResult { status body jsonBody
    header(name: "x-app") { value } } }"#;

/// A request to a server of the app.
fn request_to(url: &str) -> Value {
    json!({"request": {
        "method": "POST",
        "url": url,
        "headers": [{"name": "accept", "value": "application/json"}],
        "jsonBody": {"cart": {"lines": 1}},
        "policy": {"readTimeoutMs": 800},
    }})
}

/// A fetcher that records the requests and answers one response.
struct Fake {
    answer: Response,
    seen: RefCell<Vec<Request>>,
}

impl Fake {
    fn answering(status: u16, body: &str) -> Fake {
        Fake {
            answer: Response {
                status,
                headers: vec![("x-app".to_string(), "checks".to_string())],
                body: body.to_string(),
            },
            seen: RefCell::new(Vec::new()),
        }
    }
}

impl Fetcher for Fake {
    fn send(&self, request: &Request) -> Response {
        self.seen.borrow_mut().push(request.clone());
        self.answer.clone()
    }
}

/// The result of [`run`]: the checkout, and what the `.run` target read of `fetchResult`.
struct Outcome {
    result: CheckoutResult,
    fetch_results: Vec<Value>,
}

/// Runs the Function of `kind`, once its targets are the new ones and it has a `.fetch` target.
/// The `.fetch` target answers `request`; the `.run` target reads [`FETCH_RESULT`] and answers
/// nothing. `fetcher` sends the requests (the real one when `None`).
fn run(kind: Kind, request: Value, fetcher: Option<&dyn Fetcher>) -> Outcome {
    static COPIES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let copy = AppCopy::new(&format!(
        "network-{}-{}",
        kind.handle,
        COPIES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let (run_export, fetch_export) = (export_of(kind.run), export_of(kind.fetch));
    std::fs::write(
        copy.file(kind.handle, "dist/function.wasm"),
        answering_module(&[run_export.as_str(), fetch_export.as_str()]),
    )
    .unwrap();
    let file = copy.file(kind.handle, "shopify.extension.toml");
    let mut toml = std::fs::read_to_string(&file).unwrap();
    // The validation of the fixture app is a legacy target: use the current one.
    toml = toml.replace("purchase.validation.run", "cart.validations.generate.run");
    // The query file of the other target, for the fetch target to read.
    let query = if kind.handle == "quantity-limit" {
        "src/run.graphql"
    } else {
        "src/cart_lines_discounts_generate_run.graphql"
    };
    toml.push_str(&format!(
        "\n  [[extensions.targeting]]\n  target = \"{}\"\n  input_query = \"{query}\"\n  export = \"{fetch_export}\"\n",
        kind.fetch
    ));
    std::fs::write(file, toml).unwrap();
    // The run export of the volume discount is its own.
    if kind.run == "cart.validations.generate.run" {
        copy.replace(
            kind.handle,
            "shopify.extension.toml",
            "export = \"run\"",
            &format!("export = \"{run_export}\""),
        );
    }

    let app = copy.load();
    assert!(
        app.diagnostics
            .iter()
            .all(|d| d.severity != lsf_checkout::app::Severity::Error),
        "{:?}",
        app.diagnostics
    );
    let store = store();
    let session = session(&store, &[(TEE, 2)]);
    let mut config = config();
    config.discounts = vec![DiscountConfig {
        function: "volume-discount".to_string(),
        title: "Remote".to_string(),
        code: None,
        classes: vec![if kind.run.contains("delivery") {
            DiscountClass::Shipping
        } else {
            DiscountClass::Product
        }],
        metafields: settings(json!({})),
    }];
    config
        .function_metafields
        .insert("quantity-limit".to_string(), settings(json!({})));

    let fetch_results = RefCell::new(Vec::new());
    let build = |_: &str, _: Option<&Value>, ctx: &InputContext| match ctx.fetch {
        FetchInput::Unavailable => Ok(request.clone()),
        FetchInput::Response(_) => {
            fetch_results
                .borrow_mut()
                .push(lsf_checkout::input::build(FETCH_RESULT, None, ctx)?);
            Ok((kind.nothing)())
        }
    };
    let selection = Selection::only([kind.handle]);
    let state = CheckoutState::default();
    let result = match fetcher {
        Some(fetcher) => compute_with_fetcher(
            &app, &selection, &store, &session, &state, &config, &build, fetcher,
        ),
        None => compute_with(&app, &selection, &store, &session, &state, &config, &build),
    };
    Outcome {
        result,
        fetch_results: fetch_results.into_inner(),
    }
}

/// `cart.lines.discounts.generate.run` → `cart_lines_discounts_generate_run`.
fn export_of(target: &str) -> String {
    target.replace(['.', '-'], "_")
}

fn all_kinds() -> [Kind; 3] {
    [LINES_DISCOUNT, DELIVERY_DISCOUNT, VALIDATION]
}

#[test]
fn a_run_target_reads_the_response_to_the_request_of_its_fetch_target() {
    for kind in all_kinds() {
        let fake = Fake::answering(200, r#"{"allowed": true}"#);
        let outcome = run(kind, request_to("https://app.example/check"), Some(&fake));
        let name = kind.run;
        assert!(
            outcome.result.errors.is_empty(),
            "{name}: {:?}",
            outcome.result.errors
        );
        // The checkout sent what the Function asked for.
        let seen = fake.seen.borrow();
        assert_eq!(seen.len(), 1, "{name}");
        assert_eq!(seen[0].url, "https://app.example/check");
        assert_eq!(seen[0].method.as_str(), "POST");
        assert_eq!(seen[0].read_timeout_ms, 800);
        assert_eq!(seen[0].body.as_deref(), Some(r#"{"cart":{"lines":1}}"#));
        assert!(
            seen[0]
                .headers
                .contains(&("content-type".to_string(), "application/json".to_string()))
        );
        // The run target read the response.
        assert_eq!(
            outcome.fetch_results,
            [json!({"fetchResult": {
                "status": 200,
                "body": r#"{"allowed": true}"#,
                "jsonBody": {"allowed": true},
                "header": {"value": "checks"},
            }})],
            "{name}"
        );
        // Both targets ran, fetch first, and the exchange is on the run of the fetch target.
        let targets: Vec<&str> = outcome
            .result
            .runs
            .iter()
            .map(|run| run.target.as_str())
            .collect();
        assert_eq!(targets, [kind.fetch, kind.run], "{name}");
        let exchange = outcome.result.runs[0].exchange.as_ref().unwrap();
        assert_eq!(exchange.request.url, "https://app.example/check");
        assert_eq!(exchange.response.status, 200);
        assert!(outcome.result.runs[1].exchange.is_none());
    }
}

#[test]
fn a_response_that_is_a_failure_is_read_like_any_other() {
    for kind in all_kinds() {
        let fake = Fake::answering(504, "504 Gateway Timeout");
        let outcome = run(kind, request_to("https://app.example/slow"), Some(&fake));
        assert!(
            outcome.result.errors.is_empty(),
            "{:?}",
            outcome.result.errors
        );
        assert_eq!(outcome.fetch_results[0]["fetchResult"]["status"], 504);
        assert_eq!(
            outcome.fetch_results[0]["fetchResult"]["body"],
            "504 Gateway Timeout"
        );
    }
}

#[test]
fn a_fetch_target_that_asks_for_nothing_leaves_the_fetch_result_empty() {
    for kind in all_kinds() {
        let fake = Fake::answering(200, "unused");
        let outcome = run(kind, json!({}), Some(&fake));
        assert!(
            outcome.result.errors.is_empty(),
            "{:?}",
            outcome.result.errors
        );
        assert!(fake.seen.borrow().is_empty());
        assert_eq!(outcome.fetch_results, [json!({"fetchResult": null})]);
        assert!(outcome.result.runs[0].exchange.is_none());
    }
}

#[test]
fn a_request_shopify_would_refuse_fails_the_function_and_the_run_target_does_not_run() {
    let mut request = request_to("http://app.example/insecure");
    for (request, expected) in [
        (request.clone(), "HTTPS"),
        (
            {
                request["request"]["url"] = json!("https://app.example/x");
                request["request"]["policy"]["readTimeoutMs"] = json!(5000);
                request
            },
            "between 100 and 2000",
        ),
    ] {
        for kind in all_kinds() {
            let fake = Fake::answering(200, "unused");
            let outcome = run(kind, request.clone(), Some(&fake));
            assert!(fake.seen.borrow().is_empty(), "nothing is sent");
            assert!(
                outcome.fetch_results.is_empty(),
                "the run target does not run"
            );
            assert_eq!(outcome.result.runs.len(), 1);
            let error = outcome.result.runs[0].error.as_deref().unwrap();
            assert!(error.contains(expected), "{error}");
            assert_eq!(
                outcome.result.errors.len(),
                1,
                "{:?}",
                outcome.result.errors
            );
            assert!(outcome.result.errors[0].message.contains("fetch request"));
        }
    }
}

#[test]
fn the_real_fetcher_sends_the_request_to_a_server_on_this_machine() {
    let server = serve(vec![ok("application/json", r#"{"allowed": false}"#)]);
    let outcome = run(
        VALIDATION,
        request_to(&format!("http://127.0.0.1:{}/check", server.port)),
        None,
    );
    assert!(
        outcome.result.errors.is_empty(),
        "{:?}",
        outcome.result.errors
    );
    assert_eq!(
        outcome.fetch_results[0]["fetchResult"]["jsonBody"],
        json!({"allowed": false})
    );
    assert_eq!(
        outcome.fetch_results[0]["fetchResult"]["header"]["value"],
        "checks"
    );
    let received = server.received.lock().unwrap();
    assert_eq!(received[0].request_line, "POST /check HTTP/1.1");
    assert_eq!(received[0].body, r#"{"cart":{"lines":1}}"#);
    let exchange = outcome.result.runs[0].exchange.as_ref().unwrap();
    assert_eq!(exchange.response.status, 200);
}

#[test]
fn the_real_fetcher_gives_a_server_that_does_not_answer_a_gateway_timeout() {
    let server = serve(vec![(
        std::time::Duration::from_millis(1500),
        ok("text/plain", "late").1,
    )]);
    let mut request = request_to(&format!("http://127.0.0.1:{}/slow", server.port));
    request["request"]["policy"]["readTimeoutMs"] = json!(100);
    let outcome = run(DELIVERY_DISCOUNT, request, None);
    assert!(
        outcome.result.errors.is_empty(),
        "{:?}",
        outcome.result.errors
    );
    assert_eq!(outcome.fetch_results[0]["fetchResult"]["status"], 504);
}

#[test]
fn the_fetch_target_cannot_read_the_fetch_result() {
    // A query that asks for the response in the fetch target: the input does not build.
    let copy = AppCopy::new("network-fetch-query");
    std::fs::write(
        copy.file("quantity-limit", "dist/function.wasm"),
        answering_module(&["run", "fetch"]),
    )
    .unwrap();
    let file = copy.file("quantity-limit", "shopify.extension.toml");
    let toml = std::fs::read_to_string(&file)
        .unwrap()
        .replace("purchase.validation.run", "cart.validations.generate.run");
    std::fs::write(
        &file,
        format!(
            "{toml}\n  [[extensions.targeting]]\n  target = \"cart.validations.generate.fetch\"\n  input_query = \"src/fetch.graphql\"\n  export = \"fetch\"\n"
        ),
    )
    .unwrap();
    std::fs::write(
        copy.file("quantity-limit", "src/fetch.graphql"),
        "query Input { fetchResult { status } }",
    )
    .unwrap();
    let app = copy.load();
    let store = store();
    let session = session(&store, &[(TEE, 1)]);
    let mut config = config();
    config
        .function_metafields
        .insert("quantity-limit".to_string(), settings(json!({})));
    let fake = Fake::answering(200, "unused");
    let result = compute_with_fetcher(
        &app,
        &Selection::only(["quantity-limit"]),
        &store,
        &session,
        &CheckoutState::default(),
        &config,
        &lsf_checkout::input::build,
        &fake,
    );
    assert_eq!(result.runs.len(), 1, "{:?}", result.runs);
    assert_eq!(result.runs[0].target, "cart.validations.generate.fetch");
    let error = result.runs[0].error.as_deref().unwrap();
    assert!(
        error.contains("only available to the input query of a run target"),
        "{error}"
    );
    assert!(fake.seen.borrow().is_empty());
}

#[test]
fn a_function_without_a_fetch_target_reads_no_fetch_result() {
    let copy = AppCopy::new("network-run-only");
    std::fs::write(
        copy.file("quantity-limit", "dist/function.wasm"),
        answering_module(&["run"]),
    )
    .unwrap();
    copy.replace(
        "quantity-limit",
        "shopify.extension.toml",
        "purchase.validation.run",
        "cart.validations.generate.run",
    );
    let app = copy.load();
    let store = store();
    let session = session(&store, &[(TEE, 1)]);
    let mut config = config();
    config
        .function_metafields
        .insert("quantity-limit".to_string(), settings(json!({})));
    let fake = Fake::answering(200, "unused");
    let seen = RefCell::new(Vec::new());
    let build = |_: &str, _: Option<&Value>, ctx: &InputContext| {
        seen.borrow_mut()
            .push(lsf_checkout::input::build(FETCH_RESULT, None, ctx)?);
        Ok(json!({"operations": []}))
    };
    let result = compute_with_fetcher(
        &app,
        &Selection::only(["quantity-limit"]),
        &store,
        &session,
        &CheckoutState::default(),
        &config,
        &build,
        &fake,
    );
    assert!(result.errors.is_empty(), "{:?}", result.errors);
    assert_eq!(seen.into_inner(), [json!({"fetchResult": null})]);
    assert!(fake.seen.borrow().is_empty());
}

#[test]
fn without_network_a_request_is_checked_but_not_sent() {
    // A server of the test: no request of the test may reach it. Others may (a test that sends
    // to a port it found free, a tool that watches the ports): the URL has a path of its own.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let path = format!("/offline-{}", std::process::id());
    let url = format!("http://{}{path}", listener.local_addr().unwrap());
    for kind in all_kinds() {
        let outcome = run(kind, request_to(&url), Some(&OfflineFetcher));
        // The `.run` target reads what a request whose connection fails reads.
        assert_eq!(outcome.fetch_results.len(), 1, "{}", kind.run);
        let fetch_result = &outcome.fetch_results[0]["fetchResult"];
        assert_eq!(fetch_result["status"], 502, "{}", kind.run);
        assert_eq!(
            fetch_result["body"], "502 Bad Gateway (Connection Failed)",
            "{}",
            kind.run
        );
        let exchange = outcome.result.runs[0]
            .exchange
            .as_ref()
            .expect("the exchange");
        assert_eq!(exchange.request.url, url);
        assert_eq!(exchange.response.status, 502);

        // A request out of Shopify's limits still fails its `.fetch` target.
        let mut too_slow = request_to(&url);
        too_slow["request"]["policy"]["readTimeoutMs"] = json!(5000);
        let outcome = run(kind, too_slow, Some(&OfflineFetcher));
        assert!(outcome.fetch_results.is_empty(), "{}", kind.run);
        assert!(
            outcome
                .result
                .errors
                .iter()
                .any(|error| error.message.contains("fetch request")),
            "{:?}",
            outcome.result.errors
        );
    }
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                use std::io::BufRead;
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                    .unwrap();
                let mut line = String::new();
                let _ = std::io::BufReader::new(stream).read_line(&mut line);
                assert!(!line.contains(&path), "the request was sent: {line}");
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) => panic!("{error}"),
        }
    }
}

#[test]
fn past_the_deadline_no_function_runs_and_the_result_says_so() {
    let copy = AppCopy::new("network-deadline");
    std::fs::write(
        copy.file("quantity-limit", "dist/function.wasm"),
        answering_module(&["run"]),
    )
    .unwrap();
    copy.replace(
        "quantity-limit",
        "shopify.extension.toml",
        "purchase.validation.run",
        "cart.validations.generate.run",
    );
    let app = copy.load();
    let store = store();
    let session = session(&store, &[(TEE, 1)]);
    let mut config = config();
    config
        .function_metafields
        .insert("quantity-limit".to_string(), settings(json!({})));
    let build = |_: &str, _: Option<&Value>, _: &InputContext| Ok(json!({"operations": []}));
    let compute = |deadline| {
        compute_with_options(
            &app,
            &Selection::only(["quantity-limit"]),
            &store,
            &session,
            &CheckoutState::default(),
            &config,
            &build,
            RunOptions {
                fetcher: &OfflineFetcher,
                deadline,
            },
        )
    };
    let in_time = compute(Some(
        std::time::Instant::now() + std::time::Duration::from_secs(60),
    ));
    assert!(!in_time.interrupted);
    assert_eq!(in_time.runs.len(), 1);
    assert!(in_time.errors.is_empty(), "{:?}", in_time.errors);

    let late = compute(Some(std::time::Instant::now()));
    assert!(late.interrupted);
    assert!(late.runs.is_empty(), "{:?}", late.runs);
    // What did not run is not an error of the checkout.
    assert!(late.errors.is_empty(), "{:?}", late.errors);

    assert!(!compute(None).interrupted);
}
