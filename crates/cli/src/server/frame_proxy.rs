//! The app's frame in lsf's admin: a proxy, on a port of its own, between the frame and the
//! backend of the app.
//!
//! Shopify's libraries let only Shopify's admin frame the app: `@shopify/shopify-app-react-router`
//! answers every page with `Content-Security-Policy: frame-ancestors https://<shop>
//! https://admin.shopify.com …`, the pages it writes itself included (the bounce page
//! `/auth/session-token`, the page of `redirect(url, {target: "_top"})`, the exit-iframe page),
//! which a Response thrown from a loader carries past `entry.server`. No local origin is
//! `https://<shop>`, so a browser refuses all of them in lsf's frame. The proxy changes what
//! the backend answers, and nothing else:
//!
//! - `frame-ancestors` leaves `Content-Security-Policy`, and `X-Frame-Options` goes: lsf's admin
//!   may frame every page;
//! - Shopify's App Bridge (`https://cdn.shopify.com/shopifycloud/app-bridge.js`) is lsf's in the
//!   pages and in `Link`, and Polaris (`https://cdn.shopify.com/shopifycloud/polaris…`) is
//!   lsf's `/__lsf/cdn/` (Shopify's CDN online, lsf's copy offline). Each URL it changes ends
//!   with [`CDN_MARK`]: lsf's App Bridge writes the CDN's URL back in the `src` of those
//!   scripts once they have started loading, before React hydrates the page, so that React
//!   finds the `src` it rendered (no "Prop `src` did not match"). So each directive of the
//!   policy that lets the page load from Shopify's CDN (`script-src`, `style-src`, `connect-src`,
//!   `font-src`, `img-src`, their `-elem` forms, `default-src`) lets it load from lsf too, and
//!   nothing else of the policy changes;
//! - the backend's origin is the proxy's in the pages and the headers (`Location`, the
//!   `shopify-reload` of the bounce page, `X-Shopify-API-Request-Failure-Reauthorize-Url`), so
//!   that the frame stays on the proxy;
//! - a URL of Shopify's admin in `Location` or in
//!   `X-Shopify-API-Request-Failure-Reauthorize-Url` (`https://<shop>/admin/charges/…`, the
//!   `confirmationUrl` of a charge) is lsf's admin's, for that shop: a backend that redirects the
//!   merchant there plainly reaches lsf, not Shopify;
//! - the request reaches the backend as if it came to it: `Host`, `Origin` and `Referer` are
//!   the backend's; and a `User-Agent` of headless Chromium (`HeadlessChrome`, which the
//!   template's `isbot` answers `410 Gone`) is Chrome's.
//!
//! A request with a session token that fails, while lsf has received no token exchange and no
//! Admin API call with a token issued to the backend since lsf started (or since the backend could
//! not be reached), is reported to the [`BackendWatch`]: the backend may send them to Shopify (no
//! `LSF_URL`), and lsf's admin and its terminal say so, with an "if".
//!
//! The proxy forwards plain HTTP: a WebSocket (Vite's HMR on the app's own port) does not pass,
//! and the template's HMR has a port of its own. `LSF_ADMIN_FRAME=direct` opens the backend in
//! the frame without the proxy.

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderName, HeaderValue, Request as HttpRequest, Response, StatusCode};
use lsf_app::admin::api::BackendWatch;

/// What an answer of the backend may weigh.
const MAX_ANSWER: u64 = 256 << 20;

/// What a request to the backend may weigh.
const MAX_REQUEST: usize = 64 << 20;

/// How long the backend may take: a dev server compiles a page the first time it is asked.
const TIMEOUT: Duration = Duration::from_secs(120);

/// The headers of a connection, which a proxy does not pass on.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "content-length",
];

/// Shopify's App Bridge, which the pages load from the CDN.
const CDN_APP_BRIDGE: &str = "https://cdn.shopify.com/shopifycloud/app-bridge.js";

/// Polaris on the CDN: `polaris.js`, `polaris-2.js`.
const CDN_POLARIS: &str = "https://cdn.shopify.com/shopifycloud/polaris";

/// The fragment that ends a URL of the CDN the proxy pointed at lsf: lsf's App Bridge puts the
/// CDN's URL back in the `src` of such a script once it loads (`app-bridge.js`), so that React
/// hydrates the page without a mismatch. A fragment never reaches a server.
pub const CDN_MARK: &str = "#lsf-cdn";

/// The proxy of the app's frame.
pub struct FrameProxy {
    /// The backend's origin: `http://localhost:3000`.
    upstream: String,
    /// The proxy's: `http://127.0.0.1:52011`.
    origin: String,
    /// lsf's, where the admin and its App Bridge are: `http://127.0.0.1:9292`.
    lsf: String,
    agent: ureq::Agent,
    /// Where the failures of the backend go, when lsf's admin watches it.
    watch: Option<Arc<BackendWatch>>,
}

impl FrameProxy {
    pub fn new(upstream: &str, origin: &str, lsf: &str) -> FrameProxy {
        FrameProxy {
            upstream: upstream.trim_end_matches('/').to_string(),
            origin: origin.trim_end_matches('/').to_string(),
            lsf: lsf.trim_end_matches('/').to_string(),
            agent: ureq::Agent::config_builder()
                .timeout_global(Some(TIMEOUT))
                .http_status_as_error(false)
                .max_redirects(0)
                .allow_non_standard_methods(true)
                .build()
                .into(),
            watch: None,
        }
    }

    /// Reports to `watch` the requests with a session token that fail.
    pub fn with_watch(mut self, watch: Arc<BackendWatch>) -> FrameProxy {
        self.watch = Some(watch);
        self
    }

    /// A URL of the backend as the frame sees it: the backend's origin, raw or in a query
    /// (`shopify-reload=http%3A%2F%2Flocalhost%3A3000%2Fapp`), is the proxy's.
    pub fn rewrite_url(&self, text: &str) -> String {
        let mut text = text.replace(&self.upstream, &self.origin);
        for (from, to) in [
            (encode(&self.upstream), encode(&self.origin)),
            (encode(&self.upstream).to_lowercase(), encode(&self.origin)),
        ] {
            text = text.replace(&from, &to);
        }
        text
    }

    /// A page of the backend: lsf's App Bridge and Polaris, the proxy's origin.
    pub fn rewrite_page(&self, html: &str) -> String {
        let html = self.point_at_lsf(html);
        self.rewrite_url(&html)
    }

    /// `text` where Shopify's App Bridge and Polaris on the CDN are lsf's, each URL ending with
    /// [`CDN_MARK`] (unless it has a fragment already).
    fn point_at_lsf(&self, text: &str) -> String {
        let app_bridge = format!("{}/__lsf/admin/app-bridge.js", self.lsf);
        let polaris = format!("{}/__lsf/cdn/shopifycloud/polaris", self.lsf);
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        loop {
            let found = [(CDN_APP_BRIDGE, &app_bridge), (CDN_POLARIS, &polaris)]
                .into_iter()
                .filter_map(|(cdn, local)| rest.find(cdn).map(|at| (at, cdn, local)))
                .min_by_key(|(at, ..)| *at);
            let Some((at, cdn, local)) = found else {
                out.push_str(rest);
                return out;
            };
            out.push_str(&rest[..at]);
            let after = &rest[at + cdn.len()..];
            // The rest of the URL: up to a quote, a bracket, a space or an escape.
            let end = after
                .find(|c: char| {
                    c.is_whitespace()
                        || matches!(c, '"' | '\'' | '`' | '<' | '>' | '(' | ')' | '\\')
                })
                .unwrap_or(after.len());
            let tail = &after[..end];
            out.push_str(local);
            out.push_str(tail);
            if !tail.contains('#') {
                out.push_str(CDN_MARK);
            }
            rest = &after[end..];
        }
    }

    /// A header of the backend's answer, `None` when it goes.
    pub fn rewrite_header(&self, name: &str, value: &str) -> Option<String> {
        match name.to_ascii_lowercase().as_str() {
            "x-frame-options" => None,
            "content-security-policy" => {
                without_frame_ancestors(value).map(|policy| allowing_lsf(&policy, &self.lsf))
            }
            "content-security-policy-report-only" => Some(allowing_lsf(value, &self.lsf)),
            "link" => Some(self.rewrite_page(value)),
            "location" | "x-shopify-api-request-failure-reauthorize-url" => {
                let value = self.rewrite_url(value);
                Some(shopify_admin_on_lsf(&value, &self.lsf).unwrap_or(value))
            }
            _ => Some(self.rewrite_url(value)),
        }
    }

    /// A header of the frame's request, as the backend would have it directly; `None` when it
    /// does not go.
    fn request_header(&self, name: &str, value: &str) -> Option<String> {
        let name = name.to_ascii_lowercase();
        if HOP_BY_HOP.contains(&name.as_str()) || name == "host" || name == "accept-encoding" {
            return None;
        }
        Some(match name.as_str() {
            "origin" | "referer" => value.replace(&self.origin, &self.upstream),
            // Playwright's headless Chromium says `HeadlessChrome`, which the template's `isbot`
            // answers `410 Gone`: in lsf's frame, it is the merchant's browser.
            "user-agent" => value.replace("HeadlessChrome", "Chrome"),
            _ => value.to_string(),
        })
    }

    /// Sends a request of the frame to the backend, and answers what the frame gets.
    fn forward(
        &self,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: Vec<u8>,
    ) -> (u16, Vec<(String, String)>, Vec<u8>) {
        let url = format!("{}{path}", self.upstream);
        let authenticated = has_session_token(path, headers);
        let mut builder = ureq::http::Request::builder().method(method).uri(&url);
        for (name, value) in headers {
            if let Some(value) = self.request_header(name, value) {
                builder = builder.header(name.as_str(), value);
            }
        }
        let sent = if body.is_empty() && !matches!(method, "POST" | "PUT" | "PATCH") {
            builder
                .body(())
                .map_err(ureq::Error::Http)
                .and_then(|request| self.agent.run(request))
        } else {
            builder
                .body(body)
                .map_err(ureq::Error::Http)
                .and_then(|request| self.agent.run(request))
        };
        let response = match sent {
            Ok(response) => response,
            Err(error) => {
                // Refused or cut: the backend is not there (a timeout is a slow page).
                if let Some(watch) = &self.watch
                    && matches!(error, ureq::Error::ConnectionFailed | ureq::Error::Io(_))
                {
                    watch.unreachable();
                }
                let message = format!(
                    "lsf's admin frame could not reach the app's backend at {url}: {error}. Is it running, and is --app-url where it listens?"
                );
                return (
                    502,
                    vec![
                        ("content-type".into(), "text/plain; charset=utf-8".into()),
                        ("x-lsf-reason".into(), message.clone()),
                    ],
                    message.into_bytes(),
                );
            }
        };
        let status = response.status().as_u16();
        if authenticated
            && let Some(watch) = &self.watch
            && let Some(hint) = watch.failed(path, status, &self.lsf)
        {
            eprintln!("admin:  {hint}");
        }
        let mut html = false;
        let mut out = Vec::new();
        for (name, value) in response.headers() {
            let name = name.as_str().to_ascii_lowercase();
            let Ok(value) = value.to_str() else {
                continue;
            };
            if HOP_BY_HOP.contains(&name.as_str()) {
                continue;
            }
            if name == "content-type" && value.to_ascii_lowercase().starts_with("text/html") {
                html = true;
            }
            if let Some(value) = self.rewrite_header(&name, value) {
                out.push((name, value));
            }
        }
        let mut body = Vec::new();
        let _ = response
            .into_body()
            .as_reader()
            .take(MAX_ANSWER)
            .read_to_end(&mut body);
        let encoded = out
            .iter()
            .any(|(name, value)| name == "content-encoding" && value != "identity");
        if html && !encoded {
            body = self
                .rewrite_page(&String::from_utf8_lossy(&body))
                .into_bytes();
        }
        (status, out, body)
    }
}

/// Whether a request of the frame carries a session token: the `id_token` of a launch or a
/// bounce, or the `Authorization: Bearer` App Bridge adds to `fetch()`.
fn has_session_token(path: &str, headers: &[(String, String)]) -> bool {
    let query = path.split_once('?').map_or("", |(_, query)| query);
    form_urlencoded::parse(query.as_bytes())
        .any(|(name, value)| name == "id_token" && !value.is_empty())
        || headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("authorization")
                && value
                    .get(..7)
                    .is_some_and(|scheme| scheme.eq_ignore_ascii_case("bearer "))
        })
}

/// `Content-Security-Policy` without its `frame-ancestors` directive; `None` when nothing else
/// is left.
pub fn without_frame_ancestors(policy: &str) -> Option<String> {
    let kept: Vec<&str> = policy
        .split(';')
        .map(str::trim)
        .filter(|directive| !directive.is_empty())
        .filter(|directive| {
            !directive
                .split_whitespace()
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case("frame-ancestors"))
        })
        .collect();
    (!kept.is_empty()).then(|| kept.join("; "))
}

/// The fetch directives whose sources the proxy's rewrites of a page touch: lsf's App Bridge is a
/// script, Polaris is scripts, styles, fonts and icons, and lsf's App Bridge fetches from lsf.
const LSF_DIRECTIVES: &[&str] = &[
    "default-src",
    "script-src",
    "script-src-elem",
    "style-src",
    "style-src-elem",
    "connect-src",
    "font-src",
    "img-src",
];

/// `policy` where each directive of [`LSF_DIRECTIVES`] that allows Shopify's CDN allows `lsf`
/// too, since the proxy points the page's CDN URLs at lsf. Every other directive, and every
/// directive that does not allow the CDN, stays as it is.
pub fn allowing_lsf(policy: &str, lsf: &str) -> String {
    policy
        .split(';')
        .map(str::trim)
        .filter(|directive| !directive.is_empty())
        .map(|directive| {
            let mut tokens = directive.split_whitespace();
            let name = tokens.next().unwrap_or_default().to_ascii_lowercase();
            let sources: Vec<&str> = tokens.collect();
            let allows_cdn = sources.iter().any(|source| allows_shopify_cdn(source));
            let allows_lsf = sources
                .iter()
                .any(|source| *source == "*" || source.trim_end_matches('/') == lsf);
            if LSF_DIRECTIVES.contains(&name.as_str()) && allows_cdn && !allows_lsf {
                format!("{directive} {lsf}")
            } else {
                directive.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Whether a source of a policy allows `https://cdn.shopify.com/shopifycloud/…`: `https:`, `*`,
/// the CDN's host (or `*.shopify.com`), or a path of it the rewritten URLs are under.
fn allows_shopify_cdn(source: &str) -> bool {
    let source = source.to_ascii_lowercase();
    if source == "https:" || source == "*" {
        return true;
    }
    if source.contains("://") && !source.starts_with("https://") {
        return false;
    }
    let rest = source.strip_prefix("https://").unwrap_or(&source);
    let (host, path) = rest
        .split_once('/')
        .map_or((rest, ""), |(host, path)| (host, path));
    if host != "cdn.shopify.com" && host != "*.shopify.com" {
        return false;
    }
    let path = format!("/{path}");
    path == "/" || "/shopifycloud/".starts_with(&path) || path.starts_with("/shopifycloud/")
}

/// A URL of Shopify's admin, `https://<shop>.myshopify.com/admin/…` or
/// `https://admin.shopify.com/store/<store>/…`, as lsf's admin's (`<lsf>/admin/…?shop=<shop>`), as
/// lsf's admin takes such a URL in its page; `None` for another URL.
pub fn shopify_admin_on_lsf(url: &str, lsf: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let (host, rest) = rest
        .split_once('/')
        .map_or((rest, ""), |(host, rest)| (host, rest));
    let host = host.to_ascii_lowercase();
    let (shop, path) = if host == "admin.shopify.com" {
        let rest = rest.strip_prefix("store/")?;
        let (store, path) = rest
            .split_once('/')
            .map_or((rest, ""), |(store, path)| (store, path));
        let store = store.split(['?', '#']).next().unwrap_or_default();
        if store.is_empty() {
            return None;
        }
        (
            format!("{}.myshopify.com", store.to_ascii_lowercase()),
            path,
        )
    } else if host.ends_with(".myshopify.com") && !host.contains(':') {
        let path = rest.strip_prefix("admin")?;
        if !(path.is_empty() || path.starts_with(['/', '?', '#'])) {
            return None;
        }
        (host, path.trim_start_matches('/'))
    } else {
        return None;
    };
    let (path, fragment) = path
        .split_once('#')
        .map_or((path, None), |(path, fragment)| (path, Some(fragment)));
    let (path, query) = path.split_once('?').unwrap_or((path, ""));
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (name, value) in form_urlencoded::parse(query.as_bytes()) {
        if name != "shop" {
            serializer.append_pair(&name, &value);
        }
    }
    serializer.append_pair("shop", &shop);
    let mut local = format!(
        "{lsf}/admin{}{path}?{}",
        if path.is_empty() { "" } else { "/" },
        serializer.finish()
    );
    if let Some(fragment) = fragment {
        local.push('#');
        local.push_str(fragment);
    }
    Some(local)
}

/// A text as `encodeURIComponent` writes it, for the characters of an origin.
fn encode(text: &str) -> String {
    text.replace(':', "%3A").replace('/', "%2F")
}

/// Serves the proxy on `listener` for as long as the runtime runs.
pub async fn serve(proxy: Arc<FrameProxy>, listener: tokio::net::TcpListener) {
    let router = axum::Router::new().fallback(handle).with_state(proxy);
    let _ = axum::serve(listener, router).await;
}

async fn handle(
    State(proxy): State<Arc<FrameProxy>>,
    request: HttpRequest<Body>,
) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let headers: Vec<(String, String)> = parts
        .headers
        .iter()
        .filter_map(|(name, value)| {
            Some((name.as_str().to_string(), value.to_str().ok()?.to_string()))
        })
        .collect();
    let body = match axum::body::to_bytes(body, MAX_REQUEST).await {
        Ok(bytes) => bytes.to_vec(),
        Err(error) => {
            return Response::builder()
                .status(StatusCode::PAYLOAD_TOO_LARGE)
                .body(Body::from(format!(
                    "the request could not be read: {error}"
                )))
                .unwrap_or_else(|_| Response::new(Body::empty()));
        }
    };
    let method = parts.method.as_str().to_string();
    let path = parts
        .uri
        .path_and_query()
        .map_or_else(|| "/".to_string(), |path| path.as_str().to_string());
    let forwarded = proxy.clone();
    let (status, headers, body) =
        tokio::task::spawn_blocking(move || forwarded.forward(&method, &path, &headers, body))
            .await
            .unwrap_or_else(|error| {
                (
                    500,
                    Vec::new(),
                    format!("internal error: {error}").into_bytes(),
                )
            });
    let mut response =
        Response::builder().status(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY));
    for (name, value) in headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::try_from(name.as_str()),
            HeaderValue::try_from(value.as_str()),
        ) {
            response = response.header(name, value);
        }
    }
    response
        .body(Body::from(body))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proxy() -> FrameProxy {
        FrameProxy::new(
            "http://localhost:3000",
            "http://127.0.0.1:52011",
            "http://127.0.0.1:9292",
        )
    }

    #[test]
    fn the_policy_of_the_library_lets_lsf_frame_the_page() {
        let library = "frame-ancestors https://local-supply-co.myshopify.com https://admin.shopify.com https://*.spin.dev https://admin.myshopify.io https://admin.shop.dev;";
        assert_eq!(without_frame_ancestors(library), None);
        assert_eq!(
            without_frame_ancestors("default-src 'self'; frame-ancestors 'none'; img-src *")
                .as_deref(),
            Some("default-src 'self'; img-src *")
        );
        assert_eq!(proxy().rewrite_header("X-Frame-Options", "DENY"), None);
    }

    #[test]
    fn the_bounce_page_and_its_reload_stay_on_the_proxy() {
        let proxy = proxy();
        let location = "/auth/session-token?shop=a.myshopify.com&shopify-reload=http%3A%2F%2Flocalhost%3A3000%2Fapp%3Fshop%3Da.myshopify.com";
        assert_eq!(
            proxy.rewrite_header("location", location).unwrap(),
            "/auth/session-token?shop=a.myshopify.com&shopify-reload=http%3A%2F%2F127.0.0.1%3A52011%2Fapp%3Fshop%3Da.myshopify.com"
        );
        let page = r#"<script data-api-key="k" src="https://cdn.shopify.com/shopifycloud/app-bridge.js"></script>
      <script>window.open("http://localhost:3000/app/plans", "_top")</script>"#;
        let rewritten = proxy.rewrite_page(page);
        assert!(
            rewritten.contains(r#"src="http://127.0.0.1:9292/__lsf/admin/app-bridge.js#lsf-cdn""#)
        );
        assert!(rewritten.contains(r#"window.open("http://127.0.0.1:52011/app/plans", "_top")"#));
        assert_eq!(
            proxy
                .rewrite_header(
                    "link",
                    "<https://cdn.shopify.com/shopifycloud/polaris.js>; rel=\"preload\"; as=\"script\""
                )
                .unwrap(),
            "<http://127.0.0.1:9292/__lsf/cdn/shopifycloud/polaris.js#lsf-cdn>; rel=\"preload\"; as=\"script\""
        );
    }

    #[test]
    fn each_cdn_url_pointed_at_lsf_ends_with_the_mark() {
        let proxy = proxy();
        let page = r#"<script src="https://cdn.shopify.com/shopifycloud/polaris-2.js?v=2"></script><script src='https://cdn.shopify.com/shopifycloud/app-bridge.js'></script>{"u":"https://cdn.shopify.com/shopifycloud/polaris.js\"}<link href="https://cdn.shopify.com/shopifycloud/polaris.js#x"><img src="https://cdn.shopify.com/s/files/a.png">"#;
        assert_eq!(
            proxy.rewrite_page(page),
            r#"<script src="http://127.0.0.1:9292/__lsf/cdn/shopifycloud/polaris-2.js?v=2#lsf-cdn"></script><script src='http://127.0.0.1:9292/__lsf/admin/app-bridge.js#lsf-cdn'></script>{"u":"http://127.0.0.1:9292/__lsf/cdn/shopifycloud/polaris.js#lsf-cdn\"}<link href="http://127.0.0.1:9292/__lsf/cdn/shopifycloud/polaris.js#x"><img src="https://cdn.shopify.com/s/files/a.png">"#
        );
    }

    #[test]
    fn a_session_token_is_in_the_query_or_the_authorization() {
        let none: [(String, String); 0] = [];
        assert!(has_session_token("/app?embedded=1&id_token=a.b.c", &none));
        assert!(!has_session_token("/app?embedded=1&id_token=", &none));
        assert!(!has_session_token("/app?embedded=1", &none));
        assert!(has_session_token(
            "/app.data",
            &[("Authorization".to_string(), "Bearer a.b.c".to_string())]
        ));
        assert!(!has_session_token(
            "/app.data",
            &[("authorization".to_string(), "Basic abc".to_string())]
        ));
    }

    #[test]
    fn the_request_reaches_the_backend_as_if_it_came_to_it() {
        let proxy = proxy();
        assert_eq!(proxy.request_header("Host", "127.0.0.1:52011"), None);
        assert_eq!(proxy.request_header("accept-encoding", "gzip"), None);
        assert_eq!(
            proxy
                .request_header(
                    "user-agent",
                    "Mozilla/5.0 HeadlessChrome/141.0.0.0 Safari/537.36"
                )
                .as_deref(),
            Some("Mozilla/5.0 Chrome/141.0.0.0 Safari/537.36")
        );
        assert_eq!(
            proxy
                .request_header("origin", "http://127.0.0.1:52011")
                .as_deref(),
            Some("http://localhost:3000")
        );
        assert_eq!(
            proxy
                .request_header("referer", "http://127.0.0.1:52011/app?x=1")
                .as_deref(),
            Some("http://localhost:3000/app?x=1")
        );
    }

    #[test]
    fn a_policy_that_allows_shopifys_cdn_allows_lsf_where_it_did() {
        let lsf = "http://127.0.0.1:9292";
        assert_eq!(
            allowing_lsf(
                "default-src 'self'; script-src 'self' https://cdn.shopify.com; style-src 'self' 'unsafe-inline' https://cdn.shopify.com/shopifycloud/; img-src 'self' data:; connect-src 'self' https://*.shopify.com; font-src https:; base-uri 'self'; form-action https://cdn.shopify.com",
                lsf
            ),
            "default-src 'self'; script-src 'self' https://cdn.shopify.com http://127.0.0.1:9292; style-src 'self' 'unsafe-inline' https://cdn.shopify.com/shopifycloud/ http://127.0.0.1:9292; img-src 'self' data:; connect-src 'self' https://*.shopify.com http://127.0.0.1:9292; font-src https: http://127.0.0.1:9292; base-uri 'self'; form-action https://cdn.shopify.com"
        );
        // `script-src-elem` and `default-src` too; a source that already allows lsf, `*`, another
        // path of the CDN, plain http and `'none'` change nothing.
        assert_eq!(
            allowing_lsf(
                "default-src cdn.shopify.com; script-src-elem https://cdn.shopify.com/shopifycloud/app-bridge.js; style-src *; img-src https://cdn.shopify.com/s/files/; connect-src http://cdn.shopify.com; font-src 'none'; script-src https://cdn.shopify.com http://127.0.0.1:9292",
                lsf
            ),
            "default-src cdn.shopify.com http://127.0.0.1:9292; script-src-elem https://cdn.shopify.com/shopifycloud/app-bridge.js http://127.0.0.1:9292; style-src *; img-src https://cdn.shopify.com/s/files/; connect-src http://cdn.shopify.com; font-src 'none'; script-src https://cdn.shopify.com http://127.0.0.1:9292"
        );
        // Through the header: `frame-ancestors` goes, the rest is adapted.
        assert_eq!(
            proxy()
                .rewrite_header(
                    "content-security-policy",
                    "frame-ancestors https://a.myshopify.com; script-src 'self' https://cdn.shopify.com"
                )
                .as_deref(),
            Some("script-src 'self' https://cdn.shopify.com http://127.0.0.1:9292")
        );
    }

    #[test]
    fn a_url_of_shopifys_admin_in_a_redirect_is_lsfs_admin() {
        let proxy = proxy();
        assert_eq!(
            proxy
                .rewrite_header(
                    "location",
                    "https://b.myshopify.com/admin/charges/1/2/RecurringApplicationCharge/confirm_recurring_application_charge?signature=abc"
                )
                .as_deref(),
            Some("http://127.0.0.1:9292/admin/charges/1/2/RecurringApplicationCharge/confirm_recurring_application_charge?signature=abc&shop=b.myshopify.com")
        );
        assert_eq!(
            proxy
                .rewrite_header(
                    "X-Shopify-API-Request-Failure-Reauthorize-Url",
                    "https://admin.shopify.com/store/b/charges/my-app/pricing_plans"
                )
                .as_deref(),
            Some("http://127.0.0.1:9292/admin/charges/my-app/pricing_plans?shop=b.myshopify.com")
        );
        // The storefront, another host, and the backend's own URLs are not the admin.
        for url in [
            "https://b.myshopify.com/products/x",
            "https://b.myshopify.com/administrator",
            "https://example.com/admin/charges/1",
        ] {
            assert_eq!(proxy.rewrite_header("location", url).as_deref(), Some(url));
        }
        assert_eq!(
            proxy
                .rewrite_header("location", "http://localhost:3000/app")
                .as_deref(),
            Some("http://127.0.0.1:52011/app")
        );
    }

    /// A backend on a socket of the test: a page with a strict policy and Shopify's tags, a
    /// redirect to the confirmation URL of a charge, and the `401` of a billing request.
    async fn backend() -> String {
        use axum::http::header;
        use axum::response::IntoResponse;
        let router = axum::Router::new()
            .route(
                "/app",
                axum::routing::get(|| async {
                    (
                        [
                            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                            (
                                header::CONTENT_SECURITY_POLICY,
                                "frame-ancestors https://a.myshopify.com https://admin.shopify.com; default-src 'self'; script-src 'self' https://cdn.shopify.com; style-src 'self' https://cdn.shopify.com",
                            ),
                        ],
                        r#"<script src="https://cdn.shopify.com/shopifycloud/app-bridge.js" data-api-key="k"></script><script src="https://cdn.shopify.com/shopifycloud/polaris.js"></script>"#,
                    )
                        .into_response()
                }),
            )
            .route(
                "/billing",
                axum::routing::get(|| async {
                    (
                        StatusCode::FOUND,
                        [(
                            header::LOCATION,
                            "https://a.myshopify.com/admin/charges/7/1/ApplicationCharge/confirm_application_charge?signature=s",
                        )],
                    )
                        .into_response()
                }),
            )
            .route(
                "/broken",
                axum::routing::get(|| async { StatusCode::INTERNAL_SERVER_ERROR.into_response() }),
            )
            .route(
                "/api/billing",
                axum::routing::post(|| async {
                    (
                        StatusCode::UNAUTHORIZED,
                        [(
                            "x-shopify-api-request-failure-reauthorize-url",
                            "https://a.myshopify.com/admin/charges/7/2/RecurringApplicationCharge/confirm_recurring_application_charge?signature=s",
                        )],
                    )
                        .into_response()
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        origin
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_proxy_lets_the_pages_load_what_it_points_at_lsf() {
        let upstream = backend().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let lsf = "http://127.0.0.1:9292";
        tokio::spawn(serve(
            Arc::new(FrameProxy::new(&upstream, &origin, lsf)),
            listener,
        ));
        let answers = tokio::task::spawn_blocking(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .http_status_as_error(false)
                .max_redirects(0)
                .build()
                .into();
            let mut page = agent.get(format!("{origin}/app")).call().unwrap();
            let policy = page
                .headers()
                .get("content-security-policy")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            let html = page.body_mut().read_to_string().unwrap();
            let redirect = agent.get(format!("{origin}/billing")).call().unwrap();
            let location = redirect
                .headers()
                .get("location")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            let unauthorized = agent
                .post(format!("{origin}/api/billing"))
                .send_empty()
                .unwrap();
            let reauthorize = unauthorized
                .headers()
                .get("x-shopify-api-request-failure-reauthorize-url")
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();
            (
                policy,
                html,
                redirect.status().as_u16(),
                location,
                unauthorized.status().as_u16(),
                reauthorize,
            )
        })
        .await
        .unwrap();
        let (policy, html, redirect, location, unauthorized, reauthorize) = answers;
        assert_eq!(
            policy,
            "default-src 'self'; script-src 'self' https://cdn.shopify.com http://127.0.0.1:9292; style-src 'self' https://cdn.shopify.com http://127.0.0.1:9292"
        );
        assert!(
            html.contains(r#"src="http://127.0.0.1:9292/__lsf/admin/app-bridge.js#lsf-cdn""#),
            "{html}"
        );
        assert!(
            html.contains(
                r#"src="http://127.0.0.1:9292/__lsf/cdn/shopifycloud/polaris.js#lsf-cdn""#
            ),
            "{html}"
        );
        assert_eq!(redirect, 302);
        assert_eq!(
            location,
            "http://127.0.0.1:9292/admin/charges/7/1/ApplicationCharge/confirm_application_charge?signature=s&shop=a.myshopify.com"
        );
        assert_eq!(unauthorized, 401);
        assert_eq!(
            reauthorize,
            "http://127.0.0.1:9292/admin/charges/7/2/RecurringApplicationCharge/confirm_recurring_application_charge?signature=s&shop=a.myshopify.com"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failure_with_a_session_token_before_any_call_of_the_backend_is_reported() {
        let upstream = backend().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let watch = Arc::new(BackendWatch::default());
        tokio::spawn(serve(
            Arc::new(
                FrameProxy::new(&upstream, &origin, "http://127.0.0.1:9292")
                    .with_watch(watch.clone()),
            ),
            listener,
        ));
        let get = move |path: &str| {
            let url = format!("{origin}{path}");
            async move {
                tokio::task::spawn_blocking(move || {
                    let agent: ureq::Agent = ureq::Agent::config_builder()
                        .http_status_as_error(false)
                        .build()
                        .into();
                    agent.get(url).call().unwrap().status().as_u16()
                })
                .await
                .unwrap()
            }
        };
        // Without a session token, or answered, nothing is said.
        assert_eq!(get("/broken").await, 500);
        assert_eq!(get("/app?id_token=a.b.c").await, 200);
        assert_eq!(watch.hint(), None);
        assert_eq!(get("/broken?embedded=1&id_token=a.b.c").await, 500);
        let hint = watch.hint().expect("a hint");
        assert!(hint.contains("answered 500 to /broken,"), "{hint}");
        assert!(hint.contains("LSF_URL=http://127.0.0.1:9292"), "{hint}");
    }

    /// A backend the frame cannot reach went away: what it did before tells nothing of the
    /// process started next.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_backend_gone_away_starts_over() {
        // A port nothing listens on.
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let upstream = format!("http://{}", closed.local_addr().unwrap());
        drop(closed);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let watch = Arc::new(BackendWatch::default());
        watch.called();
        tokio::spawn(serve(
            Arc::new(
                FrameProxy::new(&upstream, &origin, "http://127.0.0.1:9292")
                    .with_watch(watch.clone()),
            ),
            listener,
        ));
        let status = tokio::task::spawn_blocking(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .http_status_as_error(false)
                .build()
                .into();
            agent
                .get(format!("{origin}/app"))
                .call()
                .unwrap()
                .status()
                .as_u16()
        })
        .await
        .unwrap();
        assert_eq!(status, 502);
        assert_eq!(watch.calls(), 0);
    }
}
