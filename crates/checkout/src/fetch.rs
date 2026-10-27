//! Network access for Functions: the HTTP request a `.fetch` target asks for, and the response
//! the `.run` target reads as `fetchResult`.
//!
//! Shopify performs the request of a `fetch` target and hands the response to the `run` target
//! of the same Function (<https://shopify.dev/docs/apps/build/functions/network-access>). The
//! local checkout does the same from the machine it runs on, with the limits Shopify documents:
//!
//! - the methods are `GET` and `POST`, and the URL is HTTPS (`http` is allowed for `localhost`
//!   and loopback addresses only, so that a server of the app under development can answer);
//! - the read timeout, `policy.readTimeoutMs`, is between 100 and 2000 milliseconds;
//! - the headers and the body of the response weigh 100 KB at most;
//! - redirects are not followed, and the body has to be UTF-8.
//!
//! A request that cannot be made does not fail the Function: the `run` target reads a response
//! with a 5xx status, as it does on Shopify (504 for a timeout, 502 for a connection or TLS
//! error or a response past the limit, 501 for a charset other than UTF-8).
//!
//! What is not done: the JWT Shopify signs the request with (`x-shopify-request-jwt` needs the
//! secret of the app, which the local checkout does not have) and the cache of responses (every
//! run of the Function sends its request).

use std::io::Read;
use std::net::IpAddr;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

/// The smallest and the largest `policy.readTimeoutMs`.
pub const TIMEOUT_MS: std::ops::RangeInclusive<u64> = 100..=2000;

/// The headers and the body of a response weigh this much at most.
pub const MAX_RESPONSE_BYTES: usize = 100 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Method {
    Get,
    Post,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
        }
    }
}

/// The request of a `FunctionFetchResult`, checked: what a [`Fetcher`] sends.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub method: Method,
    pub url: String,
    /// In the order the Function gave them, with `content-type` added for a `jsonBody`.
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    pub read_timeout_ms: u64,
}

/// The `HttpResponse` the `run` target reads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub status: u16,
    /// Lower-case names, in the order of the response.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Response {
    /// A response of Shopify's own for a request that could not be made.
    pub fn failure(status: u16, body: &str) -> Response {
        Response {
            status,
            headers: Vec::new(),
            body: body.to_string(),
        }
    }

    /// A header by name, whatever the case.
    pub fn header(&self, name: &str) -> Option<&(String, String)> {
        self.headers
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
    }

    /// The body parsed as JSON, or as the string it is when it is not JSON (`jsonBody`).
    pub fn json_body(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|_| Value::String(self.body.clone()))
    }
}

/// What a `.fetch` target asked for and what came back, for the debug panel.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Exchange {
    pub request: Request,
    pub response: Response,
}

/// Sends the requests of the `.fetch` targets. The pipeline takes one so that tests need no
/// network: [`HttpFetcher`] is the real one.
pub trait Fetcher {
    /// Never fails: what goes wrong is a response with a 5xx status.
    fn send(&self, request: &Request) -> Response;
}

/// The request in the output of a `.fetch` target (`{"request": {...}}`). `None` when the
/// Function asks for nothing: the `run` target then has no `fetchResult`.
///
/// The error says what Shopify would refuse: a URL that is not HTTPS, a timeout out of range.
pub fn request_from_output(output: &Value) -> Result<Option<Request>, String> {
    let Some(request) = output.get("request").filter(|request| !request.is_null()) else {
        return Ok(None);
    };
    let text = |key: &str| request.get(key).and_then(Value::as_str);
    let method = match text("method") {
        Some("GET") => Method::Get,
        Some("POST") => Method::Post,
        other => {
            return Err(format!(
                "the method of the request must be GET or POST, got {}",
                other.map_or("none".to_string(), |method| format!("`{method}`"))
            ));
        }
    };
    let url = text("url").ok_or("the request has no url")?;
    check_url(url)?;
    let timeout = request
        .pointer("/policy/readTimeoutMs")
        .and_then(Value::as_u64)
        .ok_or("the request has no policy.readTimeoutMs")?;
    if !TIMEOUT_MS.contains(&timeout) {
        return Err(format!(
            "policy.readTimeoutMs must be between {} and {} milliseconds, got {timeout}",
            TIMEOUT_MS.start(),
            TIMEOUT_MS.end()
        ));
    }
    let mut headers = Vec::new();
    for header in request
        .get("headers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = header.get("name").and_then(Value::as_str).unwrap_or("");
        let value = header.get("value").and_then(Value::as_str).unwrap_or("");
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_graphic() && byte != b':')
        {
            return Err(format!("`{name}` is not a header name"));
        }
        if value.contains(['\r', '\n']) {
            return Err(format!("the value of the header `{name}` has a line break"));
        }
        headers.push((name.to_string(), value.to_string()));
    }
    // `body` is a plain string; `jsonBody` is JSON, and the `body` wins when both are given.
    let body = match (
        text("body"),
        request.get("jsonBody").filter(|v| !v.is_null()),
    ) {
        (Some(body), _) => Some(body.to_string()),
        (None, Some(json)) => {
            if !headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("content-type"))
            {
                headers.push(("content-type".to_string(), "application/json".to_string()));
            }
            Some(json.to_string())
        }
        (None, None) => None,
    };
    Ok(Some(Request {
        method,
        url: url.to_string(),
        headers,
        body,
        read_timeout_ms: timeout,
    }))
}

/// HTTPS, or `http` to this machine.
fn check_url(url: &str) -> Result<(), String> {
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err(format!("`{url}` is not an absolute URL"));
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = match host.strip_prefix('[') {
        Some(ipv6) => ipv6.split(']').next().unwrap_or(""),
        None => host.rsplit_once(':').map_or(host, |(host, _)| host),
    };
    if host.is_empty() {
        return Err(format!("`{url}` has no host"));
    }
    match scheme.to_ascii_lowercase().as_str() {
        "https" => Ok(()),
        "http" if is_local(host) => Ok(()),
        "http" => Err(format!(
            "the scheme of the url needs to be HTTPS (`http` is only accepted for localhost): {url}"
        )),
        other => Err(format!(
            "the scheme of the url needs to be HTTPS, got `{other}`"
        )),
    }
}

fn is_local(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// The real thing: sends the request over the network.
#[derive(Clone, Copy, Debug, Default)]
pub struct HttpFetcher;

const BAD_GATEWAY: u16 = 502;

impl Fetcher for HttpFetcher {
    fn send(&self, request: &Request) -> Response {
        send(request).unwrap_or_else(|failure| failure)
    }
}

fn send(request: &Request) -> Result<Response, Response> {
    let timeout = Duration::from_millis(request.read_timeout_ms);
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .max_redirects(0)
        .build()
        .into();
    let mut builder = ureq::http::Request::builder()
        .method(request.method.as_str())
        .uri(&request.url);
    for (name, value) in &request.headers {
        builder = builder.header(name, value);
    }
    let http_request = builder
        .body(request.body.clone().unwrap_or_default())
        .map_err(|_| connection_failed())?;
    let response = agent
        .run(http_request)
        .map_err(|error| failure_of(&error))?;
    let (parts, mut body) = response.into_parts();

    let headers: Vec<(String, String)> = parts
        .headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                String::from_utf8_lossy(value.as_bytes()).into_owned(),
            )
        })
        .collect();
    let header_bytes: usize = headers
        .iter()
        .map(|(name, value)| name.len() + value.len() + 4)
        .sum();
    let too_big = || {
        Response::failure(
            BAD_GATEWAY,
            "502 Bad Gateway (Headers and body exceed 100KB limit)",
        )
    };
    if header_bytes > MAX_RESPONSE_BYTES {
        return Err(too_big());
    }
    let charset_ok = parts
        .headers
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .and_then(charset_of)
        .is_none_or(|charset| matches!(charset.as_str(), "utf-8" | "utf8"));
    let mut bytes = Vec::new();
    // One byte more than the room that is left tells a body that fits from one that does not.
    let room = (MAX_RESPONSE_BYTES - header_bytes) as u64;
    body.as_reader()
        .take(room + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => gateway_timeout(),
            std::io::ErrorKind::InvalidData => {
                Response::failure(BAD_GATEWAY, "502 Bad Gateway (Data Compression Error)")
            }
            _ => failure_of(&ureq::Error::Io(error)),
        })?;
    if bytes.len() as u64 > room {
        return Err(too_big());
    }
    let text = match String::from_utf8(bytes) {
        Ok(text) if charset_ok => text,
        _ => return Err(Response::failure(501, ENCODING)),
    };
    Ok(Response {
        status: parts.status.as_u16(),
        headers,
        body: text,
    })
}

const ENCODING: &str = "501 Encoding charset not supported. Use 'UTF-8' encoding.";

fn charset_of(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|parameter| {
        let (name, value) = parameter.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches('"').to_ascii_lowercase())
    })
}

fn gateway_timeout() -> Response {
    Response::failure(504, "504 Gateway Timeout")
}

fn ssl_error() -> Response {
    Response::failure(BAD_GATEWAY, "502 Bad Gateway (SSL Error)")
}

fn connection_failed() -> Response {
    Response::failure(BAD_GATEWAY, "502 Bad Gateway (Connection Failed)")
}

/// The response Shopify gives the `run` target for a request that failed.
fn failure_of(error: &ureq::Error) -> Response {
    use ureq::Error;
    match error {
        Error::Timeout(_) => gateway_timeout(),
        Error::Io(io)
            if matches!(
                io.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) =>
        {
            gateway_timeout()
        }
        Error::Tls(_) | Error::Rustls(_) | Error::Pem(_) => ssl_error(),
        // The errors of the handshake come out of the socket as I/O errors that wrap them.
        Error::Io(io)
            if io
                .get_ref()
                .is_some_and(|inner| inner.downcast_ref::<rustls::Error>().is_some()) =>
        {
            ssl_error()
        }
        _ => connection_failed(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn output(request: Value) -> Value {
        json!({"request": request})
    }

    fn valid() -> Value {
        json!({
            "method": "POST",
            "url": "https://app.example/check",
            "headers": [{"name": "accept", "value": "application/json"}],
            "jsonBody": {"cart": {"total": "12.0"}},
            "policy": {"readTimeoutMs": 500},
        })
    }

    #[test]
    fn a_request_is_read_from_the_output() {
        let request = request_from_output(&output(valid())).unwrap().unwrap();
        assert_eq!(request.method, Method::Post);
        assert_eq!(request.url, "https://app.example/check");
        assert_eq!(request.read_timeout_ms, 500);
        assert_eq!(
            request.body.as_deref(),
            Some(r#"{"cart":{"total":"12.0"}}"#)
        );
        // A JSON body without a content type is sent as JSON.
        assert_eq!(
            request.headers,
            [
                ("accept".to_string(), "application/json".to_string()),
                ("content-type".to_string(), "application/json".to_string()),
            ]
        );
    }

    #[test]
    fn a_function_may_ask_for_nothing() {
        assert_eq!(request_from_output(&json!({})), Ok(None));
        assert_eq!(request_from_output(&json!({"request": null})), Ok(None));
    }

    #[test]
    fn body_wins_over_json_body_and_a_content_type_is_kept() {
        let mut request = valid();
        request["body"] = json!("plain");
        request["headers"] = json!([{"name": "Content-Type", "value": "text/plain"}]);
        let request = request_from_output(&output(request)).unwrap().unwrap();
        assert_eq!(request.body.as_deref(), Some("plain"));
        assert_eq!(
            request.headers,
            [("Content-Type".to_string(), "text/plain".to_string())]
        );
        let mut get = valid();
        get["method"] = json!("GET");
        get.as_object_mut().unwrap().remove("jsonBody");
        let get = request_from_output(&output(get)).unwrap().unwrap();
        assert_eq!((get.method, get.body), (Method::Get, None));
    }

    #[test]
    fn the_url_has_to_be_https_but_a_local_server_may_be_http() {
        let with_url = |url: &str| {
            let mut request = valid();
            request["url"] = json!(url);
            request_from_output(&output(request))
        };
        for url in [
            "https://app.example/x",
            "HTTPS://app.example:8443/x?y=1",
            "https://user@app.example/",
            "http://localhost:3000/x",
            "http://127.0.0.1:8080/",
            "http://127.1.2.3/",
            "http://[::1]:8080/x",
        ] {
            assert!(with_url(url).is_ok(), "{url}");
        }
        for url in [
            "http://app.example/x",
            "http://localhost.example.com/",
            "http://10.0.0.1/",
            "ftp://app.example/",
            "app.example/x",
            "https://",
            "https:///path",
        ] {
            let error = with_url(url).unwrap_err();
            assert!(!error.is_empty(), "{url}");
        }
        assert!(
            with_url("http://app.example/x")
                .unwrap_err()
                .contains("HTTPS")
        );
    }

    #[test]
    fn the_timeout_is_between_100_and_2000_milliseconds() {
        for (timeout, ok) in [
            (99, false),
            (100, true),
            (2000, true),
            (2001, false),
            (0, false),
        ] {
            let mut request = valid();
            request["policy"]["readTimeoutMs"] = json!(timeout);
            let read = request_from_output(&output(request));
            assert_eq!(read.is_ok(), ok, "{timeout}");
            if !ok {
                let error = read.unwrap_err();
                assert!(error.contains("between 100 and 2000"), "{error}");
            }
        }
        let mut request = valid();
        request.as_object_mut().unwrap().remove("policy");
        assert!(request_from_output(&output(request)).is_err());
    }

    #[test]
    fn only_get_and_post_are_allowed() {
        let mut request = valid();
        request["method"] = json!("DELETE");
        let error = request_from_output(&output(request)).unwrap_err();
        assert!(error.contains("GET or POST"), "{error}");
    }

    #[test]
    fn a_header_has_a_name_and_no_line_break() {
        for (name, value) in [("", "x"), ("bad name", "x"), ("a:b", "x"), ("x", "a\r\nb")] {
            let mut request = valid();
            request["headers"] = json!([{"name": name, "value": value}]);
            assert!(request_from_output(&output(request)).is_err(), "{name:?}");
        }
    }

    #[test]
    fn a_response_answers_its_headers_in_any_case_and_a_body_as_json_when_it_is() {
        let response = Response {
            status: 200,
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body: r#"{"allowed": true}"#.to_string(),
        };
        assert_eq!(
            response.header("Content-Type").unwrap().1,
            "application/json"
        );
        assert!(response.header("x-other").is_none());
        assert_eq!(response.json_body(), json!({"allowed": true}));
        let text = Response::failure(502, "502 Bad Gateway (Connection Failed)");
        assert_eq!(
            text.json_body(),
            json!("502 Bad Gateway (Connection Failed)")
        );
        assert_eq!(Response::failure(204, "").json_body(), json!(""));
    }

    #[test]
    fn the_charset_of_a_content_type() {
        assert_eq!(
            charset_of("text/plain; charset=UTF-8").as_deref(),
            Some("utf-8")
        );
        assert_eq!(
            charset_of(r#"text/html; boundary=x; charset="ISO-8859-1""#).as_deref(),
            Some("iso-8859-1")
        );
        assert_eq!(charset_of("application/json"), None);
    }
}
