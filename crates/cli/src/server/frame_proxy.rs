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
//!   lsf's `/__lsf/cdn/` (Shopify's CDN online, lsf's copy offline);
//! - the backend's origin is the proxy's in the pages and the headers (`Location`, the
//!   `shopify-reload` of the bounce page, `X-Shopify-API-Request-Failure-Reauthorize-Url`), so
//!   that the frame stays on the proxy;
//! - the request reaches the backend as if it came to it: `Host`, `Origin` and `Referer` are
//!   the backend's.
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

/// The proxy of the app's frame.
pub struct FrameProxy {
    /// The backend's origin: `http://localhost:3000`.
    upstream: String,
    /// The proxy's: `http://127.0.0.1:52011`.
    origin: String,
    /// lsf's, where the admin and its App Bridge are: `http://127.0.0.1:9292`.
    lsf: String,
    agent: ureq::Agent,
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
        }
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
        let html = html
            .replace(
                CDN_APP_BRIDGE,
                &format!("{}/__lsf/admin/app-bridge.js", self.lsf),
            )
            .replace(
                CDN_POLARIS,
                &format!("{}/__lsf/cdn/shopifycloud/polaris", self.lsf),
            );
        self.rewrite_url(&html)
    }

    /// A header of the backend's answer, `None` when it goes.
    pub fn rewrite_header(&self, name: &str, value: &str) -> Option<String> {
        match name.to_ascii_lowercase().as_str() {
            "x-frame-options" => None,
            "content-security-policy" => without_frame_ancestors(value),
            "link" => Some(self.rewrite_page(value)),
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
        assert!(rewritten.contains(r#"src="http://127.0.0.1:9292/__lsf/admin/app-bridge.js""#));
        assert!(rewritten.contains(r#"window.open("http://127.0.0.1:52011/app/plans", "_top")"#));
        assert_eq!(
            proxy
                .rewrite_header(
                    "link",
                    "<https://cdn.shopify.com/shopifycloud/polaris.js>; rel=\"preload\"; as=\"script\""
                )
                .unwrap(),
            "<http://127.0.0.1:9292/__lsf/cdn/shopifycloud/polaris.js>; rel=\"preload\"; as=\"script\""
        );
    }

    #[test]
    fn the_request_reaches_the_backend_as_if_it_came_to_it() {
        let proxy = proxy();
        assert_eq!(proxy.request_header("Host", "127.0.0.1:52011"), None);
        assert_eq!(proxy.request_header("accept-encoding", "gzip"), None);
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
}
