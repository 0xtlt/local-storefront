//! The HTTP side of network access for Functions, against a server of the test (no network
//! beyond the loopback): the limits Shopify documents and the responses it gives for a request
//! that cannot be made.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

mod common;

use common::*;
use lsf_checkout::fetch::{
    Fetcher, HttpFetcher, MAX_RESPONSE_BYTES, Method, Request, Response, request_from_output,
};
use serde_json::json;

fn request(server: &Server, method: Method, timeout: u64) -> Request {
    Request {
        method,
        url: format!("http://127.0.0.1:{}/check?x=1", server.port),
        headers: Vec::new(),
        body: None,
        read_timeout_ms: timeout,
    }
}

#[test]
fn a_get_returns_the_status_the_headers_and_the_body() {
    let server = serve(vec![ok("application/json", r#"{"allowed": true}"#)]);
    let mut get = request(&server, Method::Get, 2000);
    get.headers = vec![("Accept".to_string(), "application/json".to_string())];
    let response = HttpFetcher.send(&get);
    assert_eq!(response.status, 200);
    assert_eq!(response.body, r#"{"allowed": true}"#);
    assert_eq!(response.header("X-App").unwrap().1, "checks");
    assert_eq!(response.json_body(), json!({"allowed": true}));

    let received = server.received.lock().unwrap();
    assert_eq!(received[0].request_line, "GET /check?x=1 HTTP/1.1");
    assert!(
        received[0]
            .headers
            .contains(&("accept".to_string(), "application/json".to_string()))
    );
}

#[test]
fn a_post_sends_its_body_and_its_headers() {
    let server = serve(vec![ok("text/plain", "done")]);
    let output = json!({"request": {
        "method": "POST",
        "url": format!("http://localhost:{}/validate", server.port),
        "headers": [{"name": "x-token", "value": "abc"}],
        "jsonBody": {"email": "buyer@example.com", "total": "12.0"},
        "policy": {"readTimeoutMs": 1000},
    }});
    let post = request_from_output(&output).unwrap().unwrap();
    let response = HttpFetcher.send(&post);
    assert_eq!((response.status, response.body.as_str()), (200, "done"));

    let received = server.received.lock().unwrap();
    assert_eq!(received[0].request_line, "POST /validate HTTP/1.1");
    assert_eq!(
        received[0].body,
        r#"{"email":"buyer@example.com","total":"12.0"}"#
    );
    let header = |name: &str| {
        received[0]
            .headers
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.as_str())
    };
    assert_eq!(header("content-type"), Some("application/json"));
    assert_eq!(header("x-token"), Some("abc"));
}

#[test]
fn an_error_status_is_a_response_not_a_failure() {
    let server = serve(vec![
        raw(
            "500 Internal Server Error",
            &[("content-type", "text/plain")],
            b"boom",
        ),
        raw("404 Not Found", &[], b""),
    ]);
    let response = HttpFetcher.send(&request(&server, Method::Get, 2000));
    assert_eq!((response.status, response.body.as_str()), (500, "boom"));
    let response = HttpFetcher.send(&request(&server, Method::Get, 2000));
    assert_eq!((response.status, response.body.as_str()), (404, ""));
}

#[test]
fn a_redirect_is_not_followed() {
    let server = serve(vec![raw(
        "302 Found",
        &[("location", "http://127.0.0.1:1/elsewhere")],
        b"",
    )]);
    let response = HttpFetcher.send(&request(&server, Method::Get, 2000));
    assert_eq!(response.status, 302);
    assert_eq!(
        response.header("location").unwrap().1,
        "http://127.0.0.1:1/elsewhere"
    );
}

#[test]
fn a_server_that_is_too_slow_is_a_gateway_timeout() {
    let server = serve(vec![(
        Duration::from_millis(1500),
        ok("text/plain", "late").1,
    )]);
    let started = std::time::Instant::now();
    let response = HttpFetcher.send(&request(&server, Method::Get, 100));
    assert_eq!(response, Response::failure(504, "504 Gateway Timeout"));
    assert!(
        started.elapsed() < Duration::from_millis(1200),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn a_server_that_cannot_be_reached_is_a_bad_gateway() {
    // A port nobody listens to: bind it, then close it.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let closed = Request {
        method: Method::Get,
        url: format!("http://127.0.0.1:{port}/"),
        headers: Vec::new(),
        body: None,
        read_timeout_ms: 500,
    };
    assert_eq!(
        HttpFetcher.send(&closed),
        Response::failure(502, "502 Bad Gateway (Connection Failed)")
    );
    let unknown = Request {
        url: "http://no-such-host.invalid/".to_string(),
        ..closed.clone()
    };
    assert_eq!(
        HttpFetcher.send(&unknown),
        Response::failure(502, "502 Bad Gateway (Connection Failed)")
    );
}

#[test]
fn a_tls_handshake_that_fails_is_an_ssl_error() {
    // The server speaks plain HTTP where the request asks for HTTPS: it answers the first bytes
    // of the handshake with text.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut hello = [0u8; 512];
            let _ = stream.read(&mut hello);
            let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\nnot tls");
        }
    });
    let https = Request {
        method: Method::Get,
        url: format!("https://127.0.0.1:{port}/"),
        headers: Vec::new(),
        body: None,
        read_timeout_ms: 1000,
    };
    assert_eq!(
        HttpFetcher.send(&https),
        Response::failure(502, "502 Bad Gateway (SSL Error)")
    );
}

#[test]
fn a_response_past_100_kb_is_a_bad_gateway() {
    let limit = MAX_RESPONSE_BYTES;
    // Just inside: the headers and the body weigh a little under the limit.
    let inside = vec![b'a'; limit - 1024];
    // Just outside: the body alone weighs the limit.
    let outside = vec![b'a'; limit + 1];
    let server = serve(vec![
        raw("200 OK", &[("content-type", "text/plain")], &inside),
        raw("200 OK", &[("content-type", "text/plain")], &outside),
    ]);
    let response = HttpFetcher.send(&request(&server, Method::Get, 2000));
    assert_eq!((response.status, response.body.len()), (200, limit - 1024));
    assert_eq!(
        HttpFetcher.send(&request(&server, Method::Get, 2000)),
        Response::failure(502, "502 Bad Gateway (Headers and body exceed 100KB limit)")
    );
}

#[test]
fn the_headers_count_in_the_limit_of_the_response() {
    let body = vec![b'a'; MAX_RESPONSE_BYTES - 100];
    let big_header = "h".repeat(400);
    let server = serve(vec![raw(
        "200 OK",
        &[
            ("content-type", "text/plain"),
            ("x-big", big_header.as_str()),
        ],
        &body,
    )]);
    assert_eq!(
        HttpFetcher
            .send(&request(&server, Method::Get, 2000))
            .status,
        502
    );
}

#[test]
fn a_charset_other_than_utf_8_is_not_supported() {
    let server = serve(vec![
        ok("text/plain; charset=ISO-8859-1", "caf\u{e9}"),
        raw(
            "200 OK",
            &[("content-type", "text/plain")],
            &[0x63, 0x61, 0x66, 0xe9],
        ),
        ok("text/plain; charset=utf-8", "caf\u{e9}"),
    ]);
    let not_supported = Response::failure(
        501,
        "501 Encoding charset not supported. Use 'UTF-8' encoding.",
    );
    assert_eq!(
        HttpFetcher.send(&request(&server, Method::Get, 2000)),
        not_supported
    );
    // Bytes that are not UTF-8 are not either, whatever the response says.
    assert_eq!(
        HttpFetcher.send(&request(&server, Method::Get, 2000)),
        not_supported
    );
    let response = HttpFetcher.send(&request(&server, Method::Get, 2000));
    assert_eq!(response.body, "caf\u{e9}");
}
