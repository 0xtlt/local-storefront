//! The app proxy of the app of `--app`: a storefront request to `/<prefix>/<subpath>` or below is
//! forwarded to the backend of the app, signed as Shopify signs it, and the backend's answer goes
//! back to the browser, rendered in the theme when it is Liquid. A backend that serves storefront
//! pages or JSON through its app proxy (reviews, loyalty pages, wishlists) is tested locally.
//!
//! What Shopify does, and where it says so:
//!
//! - shopify.dev, "About app proxies and dynamic data": the `prefix` (`a`, `apps`, `community`,
//!   `tools`) and the `subpath` (letters, digits, `_` and `-`, up to 30, not `admin`,
//!   `services`, `password` or `login`); a relative `url` joins the app URL; `Content-Type:
//!   application/liquid` is rendered "in the context of the shop using the shop's theme",
//!   anything else is "returned directly to the client"; the 19 headers stripped from the
//!   answers ([`DISALLOWED_RESPONSE_HEADERS`]).
//! - shopify.dev, "Authenticate app proxies": the parameters added to the query (`shop`,
//!   `logged_in_customer_id`, empty when logged out, `path_prefix`, `timestamp`, `signature`),
//!   after those of the request; the `Cookie` header stripped, `X-Forwarded-For` and
//!   `X-Forwarded-Host` added; the method and the body forwarded.
//! - `@shopify/shopify-api` (`lib/utils/hmac-validator.ts`, `validateHmac` with `signator:
//!   'appProxy'`), which `authenticate.public.appProxy` of the app templates calls: the
//!   signature, and a `timestamp` within 90 seconds of the backend's clock. lsf signs with the
//!   system clock, whatever time the store data fixes for the storefront.

use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use lsf_admin::webhooks::{AppUrls, Destination, resolve_uri};
use lsf_checkout::app::AppConfig;
use lsf_core::Store;
use lsf_core::render::Target;
use lsf_core::render::page::{Page, Resource};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};

use super::reply::{Reply, cache};
use super::storefront::{Visit, reply_from};
use super::{Incoming, SESSION_COOKIE, SESSION_HEADER, ServerState};

/// The first segment of the proxied path, as shopify.dev lists the values a `prefix` may take.
const PREFIXES: [&str; 4] = ["a", "apps", "community", "tools"];

/// The `subpath`s Shopify refuses ("About app proxies").
const RESERVED_SUBPATHS: [&str; 4] = ["admin", "services", "password", "login"];

/// How long the backend has to answer. Shopify does not document the timeout of its app
/// proxies; lsf waits as long as shopify.dev says Shopify waits for its own APIs ("Shopify waits
/// up to 10 seconds for a response", the `504 Gateway Timeout` of "API response status and error
/// codes").
const TIMEOUT: Duration = Duration::from_secs(10);

/// The most of an answer that is read: the bodies of the requests the server reads are capped
/// at the same.
const MAX_ANSWER: u64 = 32 * 1024 * 1024;

/// The headers Shopify strips from the answers of app proxies, "due to security concerns"
/// (shopify.dev, "About app proxies", "Disallowed headers"), lowercase.
const DISALLOWED_RESPONSE_HEADERS: [&str; 19] = [
    "accept-patch",
    "accept-ranges",
    "connection",
    "cookie",
    "date",
    "delta-base",
    "im",
    "p3p",
    "pragma",
    "preference-applied",
    "proxy-authenticate",
    "public-key-pins",
    "server",
    "service-worker-allowed",
    "set-cookie",
    "trailer",
    "tk",
    "warning",
    "x-powered-by",
];

/// The headers that belong to one connection (RFC 9110 §7.6.1), which no proxy passes on, and
/// those the HTTP client writes itself (`Host`, `Content-Length`, and `Accept-Encoding`, since
/// what comes back is decoded before it is rendered).
const CONNECTION_HEADERS: [&str; 11] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authorization",
    "proxy-authenticate",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "host",
    "content-length",
];

/// What a path segment of the forwarded URL must not carry as is.
const PATH_SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}');

/// The app proxy of the app: `[app_proxy]` of `shopify.app.toml`, and where it forwards to.
pub struct AppProxy {
    /// `/<prefix>/<subpath>`: the storefront path that is proxied, and the `path_prefix` the
    /// backend is given. Shopify lets a merchant change both; lsf serves the app's.
    path_prefix: String,
    /// `url` of `[app_proxy]`, as written.
    url: String,
    /// Where the requests go: `url` resolved as the `uri` of a webhook (a path joins the app
    /// URL; on the origin of `application_url`, it moves to `--app-url`), or why it cannot be.
    target: Destination,
    /// Why the proxy is not served: an `[app_proxy]` Shopify would not take.
    problem: Option<String>,
    /// How long the backend has to answer: [`TIMEOUT`], shorter in tests.
    timeout: Duration,
    agent: ureq::Agent,
}

impl AppProxy {
    /// The app proxy of the app in `app_dir`, `None` when it has no `[app_proxy]` (or cannot be
    /// read: the checkout says why).
    pub fn of_app(app_dir: &Path, app_url: Option<&str>) -> Option<AppProxy> {
        let app = lsf_checkout::app::load(app_dir).ok()?;
        AppProxy::new(&app.config, app_url)
    }

    /// The app proxy of a `shopify.app.toml`, with the backend on `app_url` (`--app-url`).
    fn new(config: &AppConfig, app_url: Option<&str>) -> Option<AppProxy> {
        let proxy = config.app_proxy.as_ref()?;
        let prefix = proxy.prefix.trim().trim_matches('/');
        let subpath = proxy.subpath.trim().trim_matches('/');
        let problem = if !PREFIXES.contains(&prefix) {
            Some(format!(
                "[app_proxy] prefix `{prefix}` is not one of {}",
                PREFIXES.join(", ")
            ))
        } else if subpath.is_empty()
            || subpath.len() > 30
            || !subpath
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            Some(format!(
                "[app_proxy] subpath `{subpath}` must be 1 to 30 letters, digits, `_` and `-`"
            ))
        } else if RESERVED_SUBPATHS.contains(&subpath.to_ascii_lowercase().as_str()) {
            Some(format!(
                "[app_proxy] subpath `{subpath}` is one Shopify refuses ({})",
                RESERVED_SUBPATHS.join(", ")
            ))
        } else if proxy.url.trim().is_empty() {
            Some("[app_proxy] has no url".to_string())
        } else {
            None
        };
        let urls = AppUrls {
            app_url: app_url.map(str::to_string),
            application_url: config.application_url.clone(),
        };
        Some(AppProxy {
            path_prefix: format!("/{prefix}/{subpath}"),
            url: proxy.url.clone(),
            target: resolve_uri(&proxy.url, &urls),
            problem,
            timeout: TIMEOUT,
            agent: agent(TIMEOUT),
        })
    }

    /// The same proxy, waiting `timeout` for the backend.
    #[cfg(test)]
    fn with_timeout(mut self, timeout: Duration) -> AppProxy {
        self.timeout = timeout;
        self.agent = agent(timeout);
        self
    }

    /// What the server prints of the proxy when it starts, under `ready:`.
    pub fn banner(&self, origin: &str) -> String {
        if let Some(problem) = &self.problem {
            return format!("proxy:  not served: {problem}");
        }
        match &self.target {
            Destination::Http { url } => {
                format!("proxy:  {origin}{} → {url}", self.path_prefix)
            }
            Destination::Undeliverable { reason } => {
                format!("proxy:  {origin}{} → nowhere: {reason}", self.path_prefix)
            }
        }
    }

    /// The part of a storefront path (its locale prefix removed) below the proxied path: `""`
    /// for the proxied path itself, `/extra/path` below it, `None` elsewhere.
    fn rest_of<'a>(&self, path: &'a str) -> Option<&'a str> {
        let rest = path.strip_prefix(&self.path_prefix)?;
        (rest.is_empty() || rest.starts_with('/')).then_some(rest)
    }
}

/// The HTTP client of the proxy: a redirect is the backend's answer, which goes back to the
/// browser, and so is a status out of `2xx`.
fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .max_redirects(0)
        .allow_non_standard_methods(true)
        .build()
        .into()
}

/// The answer of the app proxy to a request, `None` when the path is not below the proxied one.
/// Routed after lsf's own paths (`/__lsf`, `/cdn`) and before the theme's pages, a locale prefix
/// included (`/fr/apps/reviews`): Shopify proxies localized URLs, with the language in
/// `Accept-Language` (Shopify's staff on the Shopify Community, "Localisation with App Proxy",
/// 2020).
pub fn route(state: &ServerState, incoming: &Incoming) -> Option<Reply> {
    let proxy = state
        .app_proxy
        .as_ref()
        .filter(|proxy| proxy.problem.is_none())?;
    let store = state.store_of(incoming);
    let (locale, root, path) = state.localize(&store, &incoming.path);
    let rest = proxy.rest_of(&path)?.to_string();
    let (session_id, is_new) = state.session_id(incoming);
    let locale = (!root.is_empty()).then_some(locale);
    let reply = forward(state, proxy, incoming, &session_id, &rest, locale);
    Some(if is_new {
        reply.header(
            "set-cookie",
            format!("{SESSION_COOKIE}={session_id}; Path=/; HttpOnly; SameSite=Lax"),
        )
    } else {
        reply
    })
}

struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn forward(
    state: &ServerState,
    proxy: &AppProxy,
    incoming: &Incoming,
    session_id: &str,
    rest: &str,
    locale: Option<String>,
) -> Reply {
    let (session, store) = state.snapshot(session_id);
    let base = match &proxy.target {
        Destination::Http { url } => url,
        Destination::Undeliverable { reason } => {
            return unreachable_page(state, 502, &proxy.url, reason);
        }
    };
    let (secret, shop) = match &state.admin {
        Some(admin) => (admin.secret.clone(), admin.api.default_shop()),
        None => return Reply::not_found(),
    };
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let query = signed_query(
        &incoming.query,
        &Shopify {
            shop: &shop,
            customer_id: session.customer_id,
            path_prefix: &proxy.path_prefix,
            timestamp,
            secret: &secret,
        },
    );
    let url = target_url(base, rest, &query);
    // The language of a localized URL first, then the shop's own, as in the example of
    // Shopify's staff (`es;q=1.0, en;q=0.5`).
    let language = locale.map(|locale| {
        let primary = &store.primary_language().iso_code;
        format!("{locale};q=1.0, {primary};q=0.5")
    });
    let headers = forwarded_headers(incoming, language.as_deref());
    match send(
        &proxy.agent,
        &incoming.method,
        &url,
        &headers,
        &incoming.body,
    ) {
        Ok(answer) => answered(state, incoming, session_id, store, answer),
        Err(error) => {
            let (status, reason) = describe(&error, base, proxy.timeout);
            unreachable_page(state, status, base, &reason)
        }
    }
}

/// What Shopify adds to the query of a proxied request.
struct Shopify<'a> {
    /// `{shop}.myshopify.com`.
    shop: &'a str,
    /// The customer the session is logged in as.
    customer_id: Option<u64>,
    path_prefix: &'a str,
    /// Seconds since 1970.
    timestamp: u64,
    /// The client secret the signature is keyed with.
    secret: &'a str,
}

/// The query the backend receives: the request's, as given, then `shop`,
/// `logged_in_customer_id` (empty when no customer is logged in), `path_prefix`, `timestamp`
/// and `signature`, in the order of shopify.dev's example. The signature is the one of
/// `lsf_admin::auth::app_proxy_signature`, over every other parameter.
fn signed_query(query: &[(String, String)], shopify: &Shopify<'_>) -> Vec<(String, String)> {
    let mut signed = query.to_vec();
    signed.push(("shop".to_string(), shopify.shop.to_string()));
    signed.push((
        "logged_in_customer_id".to_string(),
        shopify
            .customer_id
            .map(|id| id.to_string())
            .unwrap_or_default(),
    ));
    signed.push(("path_prefix".to_string(), shopify.path_prefix.to_string()));
    signed.push(("timestamp".to_string(), shopify.timestamp.to_string()));
    let signature = lsf_admin::auth::app_proxy_signature(shopify.secret, signed.iter().cloned());
    signed.push(("signature".to_string(), signature));
    signed
}

/// The URL a request goes to: the proxy's URL, the rest of the path after the proxied one, and
/// the query, form-encoded (`path_prefix=%2Fapps%2Freviews`, as in shopify.dev's example).
fn target_url(base: &str, rest: &str, query: &[(String, String)]) -> String {
    let rest: String = rest
        .split('/')
        .map(|segment| utf8_percent_encode(segment, PATH_SEGMENT).to_string())
        .collect::<Vec<_>>()
        .join("/");
    let base = if rest.is_empty() {
        base
    } else {
        base.trim_end_matches('/')
    };
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (name, value) in query {
        serializer.append_pair(name, value);
    }
    format!("{base}{rest}?{}", serializer.finish())
}

/// The headers of a request as Shopify forwards them: the browser's, without `Cookie` and what
/// belongs to the connection, with `X-Forwarded-For` (the client's address) and
/// `X-Forwarded-Host` (the host the browser asked for). `language` replaces `Accept-Language`
/// for a localized URL. lsf's own session header is not passed on.
fn forwarded_headers(incoming: &Incoming, language: Option<&str>) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = incoming
        .headers
        .iter()
        .filter(|(name, _)| {
            let name = name.to_ascii_lowercase();
            !(CONNECTION_HEADERS.contains(&name.as_str())
                || matches!(
                    name.as_str(),
                    "cookie" | "accept-encoding" | "x-forwarded-for" | "x-forwarded-host"
                )
                || name == SESSION_HEADER
                || (language.is_some() && name == "accept-language"))
        })
        .map(|(name, value)| (name.to_ascii_lowercase(), value.clone()))
        .collect();
    // Behind a tunnel, the client is the first address it forwarded for; else this machine.
    let client = incoming
        .header("x-forwarded-for")
        .and_then(|list| list.split(',').next())
        .map(str::trim)
        .filter(|address| !address.is_empty())
        .unwrap_or("127.0.0.1");
    headers.push(("x-forwarded-for".to_string(), client.to_string()));
    headers.push(("x-forwarded-host".to_string(), incoming.host.clone()));
    if let Some(language) = language {
        headers.push(("accept-language".to_string(), language.to_string()));
    }
    headers.sort();
    headers
}

fn send(
    agent: &ureq::Agent,
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<Answer, ureq::Error> {
    let mut builder = ureq::http::Request::builder().method(method).uri(url);
    for (name, value) in headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    // A `GET` goes without a body, as browsers send it; a `POST` with an empty one.
    let response = if body.is_empty() && !matches!(method, "POST" | "PUT" | "PATCH") {
        agent.run(builder.body(()).map_err(ureq::Error::Http)?)?
    } else {
        agent.run(builder.body(body.to_vec()).map_err(ureq::Error::Http)?)?
    };
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            Some((name.as_str().to_string(), value.to_str().ok()?.to_string()))
        })
        .collect();
    let mut body = Vec::new();
    response
        .into_body()
        .as_reader()
        .take(MAX_ANSWER)
        .read_to_end(&mut body)?;
    Ok(Answer {
        status,
        headers,
        body,
    })
}

/// Whether a header of the backend's answer reaches the browser: not one Shopify strips, nor one
/// of the connection.
fn kept(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    !(DISALLOWED_RESPONSE_HEADERS.contains(&name.as_str())
        || CONNECTION_HEADERS.contains(&name.as_str()))
}

/// Whether an answer is Liquid for the theme: `Content-Type: application/liquid`, parameters
/// aside.
fn is_liquid(headers: &[(String, String)]) -> bool {
    headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("content-type")
            && value
                .split(';')
                .next()
                .is_some_and(|kind| kind.trim().eq_ignore_ascii_case("application/liquid"))
    })
}

/// The backend's answer, for the browser. Liquid is rendered in the theme as the template of a
/// page, with the objects of the request (shop, customer, cart, …), inside `layout/theme.liquid`
/// unless it says `{% layout none %}`; without a theme (`lsf app dev`), there is no layout to go
/// in. Shopify does not document what `template` and `request.page_type` are on such a page:
/// lsf leaves them empty, so that no condition of the theme on a page type holds. Anything else
/// is passed on as it came, a redirect included, without the headers Shopify strips.
fn answered(
    state: &ServerState,
    incoming: &Incoming,
    session_id: &str,
    store: Arc<Store>,
    answer: Answer,
) -> Reply {
    let headers: Vec<(String, String)> = answer
        .headers
        .into_iter()
        .filter(|(name, _)| kept(name))
        .collect();
    if !is_liquid(&headers) {
        return Reply {
            status: answer.status,
            headers,
            body: answer.body,
        };
    }
    let request =
        state.storefront_request(&store, incoming, &incoming.path, incoming.query.clone());
    let mut page = Page::new("", Resource::Index);
    page.status = answer.status;
    page.canonical_path = request.path.clone();
    let visit = Visit {
        state,
        incoming,
        session_id: session_id.to_string(),
        store,
        request,
    };
    let source = String::from_utf8_lossy(&answer.body);
    let mut rendered = state
        .app
        .renderer
        .render_source(&visit.site(), page, &source, "app proxy");
    rendered.template = "app_proxy".to_string();
    let mut reply = reply_from(&visit, rendered, &Target::Page);
    for (name, value) in headers {
        if !name.eq_ignore_ascii_case("content-type") {
            reply.headers.push((name, value));
        }
    }
    if reply.has_header("cache-control") {
        reply
    } else {
        reply.cached(cache::RENDERED)
    }
}

/// The status of a request that got no answer, and why, in words a developer acts on.
fn describe(error: &ureq::Error, url: &str, timeout: Duration) -> (u16, String) {
    use ureq::Error;
    let timed_out = matches!(error, Error::Timeout(_))
        || matches!(error, Error::Io(io) if matches!(
            io.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        ));
    if timed_out {
        return (
            504,
            format!(
                "the backend of the app did not answer within {} s",
                timeout.as_secs_f64()
            ),
        );
    }
    let reason = match error {
        Error::Io(io) if io.kind() == std::io::ErrorKind::ConnectionRefused => {
            format!("connection refused: nothing listens on {url}")
        }
        Error::ConnectionFailed => format!("connection failed: nothing answers on {url}"),
        Error::HostNotFound => format!("the host of {url} was not found"),
        other => other.to_string(),
    };
    (502, reason)
}

/// The page of a request the backend did not answer: Shopify shows an error page of its own;
/// lsf's says what failed and what to do about it.
fn unreachable_page(state: &ServerState, status: u16, url: &str, reason: &str) -> Reply {
    if !state.options.quiet {
        eprintln!("    app proxy: {reason}");
    }
    let escape = lsf_liquid::filters::escape_html;
    let body = format!(
        "<!doctype html>\n<html lang=\"en\">\n<head><meta charset=\"utf-8\"><title>The app proxy got no answer</title></head>\n<body>\n<h1>The app proxy got no answer</h1>\n<p>lsf forwards this page to the backend of the app, at <code>{url}</code>, and {reason}.</p>\n<p>Start the backend there, or tell lsf where it listens with <code>--app-url</code> (<code>LSF_APP_URL</code>): the <code>url</code> of <code>[app_proxy]</code> in <code>shopify.app.toml</code> moves there when it is on the origin of <code>application_url</code>, and joins it when it is a path.</p>\n</body>\n</html>\n",
        url = escape(url),
        reason = escape(reason),
    );
    Reply::html(status, body)
        .header("x-lsf-reason", header_text(reason))
        .cached(cache::PRIVATE)
}

/// A reason as a header value: printable ASCII only.
fn header_text(reason: &str) -> String {
    reason
        .chars()
        .map(|c| {
            if c.is_ascii() && !c.is_ascii_control() {
                c
            } else {
                '?'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::path::PathBuf;
    use std::sync::mpsc;

    use hmac::{Hmac, KeyInit, Mac};
    use lsf_core::theme::Revalidate;
    use sha2::Sha256;

    use super::*;
    use crate::app::App;
    use crate::server::admin::AdminSettings;
    use crate::server::{ServeOptions, ServerState};

    const SECRET: &str = "hush";

    /// `[app_proxy]` on the origin of `application_url`, as `shopify app dev` leaves it.
    const PROXY: &str = "url = \"https://example.trycloudflare.com/proxy\"\nsubpath = \"reviews\"\nprefix = \"apps\"";

    /// A request as the backend received it.
    struct Seen {
        method: String,
        /// The path and the query, as sent.
        target: String,
        headers: HashMap<String, String>,
        body: Vec<u8>,
    }

    impl Seen {
        fn path(&self) -> &str {
            self.target.split('?').next().unwrap_or_default()
        }

        /// The query, decoded as `URLSearchParams` decodes it, in order.
        fn query(&self) -> Vec<(String, String)> {
            let query = self.target.split_once('?').map_or("", |(_, query)| query);
            form_urlencoded::parse(query.as_bytes())
                .into_owned()
                .collect()
        }

        fn param(&self, name: &str) -> Option<String> {
            self.query()
                .into_iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value)
        }
    }

    /// A backend on a port of its own: each request is answered with the raw HTTP `answer`
    /// writes for it, then sent to the channel.
    fn backend(
        answer: impl Fn(&Seen) -> String + Send + 'static,
    ) -> (String, mpsc::Receiver<Seen>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (sender, received) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let mut parts = line.split(' ');
                let method = parts.next().unwrap_or_default().to_string();
                let target = parts.next().unwrap_or_default().to_string();
                let mut headers = HashMap::new();
                loop {
                    line.clear();
                    if reader.read_line(&mut line).is_err() {
                        break;
                    }
                    match line.trim_end().split_once(':') {
                        Some((name, value)) => {
                            headers
                                .insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
                        }
                        None => break,
                    }
                }
                let length = headers
                    .get("content-length")
                    .and_then(|length| length.parse().ok())
                    .unwrap_or(0);
                let mut body = vec![0; length];
                let _ = reader.read_exact(&mut body);
                let seen = Seen {
                    method,
                    target,
                    headers,
                    body,
                };
                let _ = stream.write_all(answer(&seen).as_bytes());
                if sender.send(seen).is_err() {
                    break;
                }
            }
        });
        (url, received)
    }

    /// A raw HTTP answer.
    fn http(status: &str, headers: &[(&str, &str)], body: &str) -> String {
        let mut answer = format!(
            "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n",
            body.len()
        );
        for (name, value) in headers {
            answer.push_str(&format!("{name}: {value}\r\n"));
        }
        answer.push_str("\r\n");
        answer.push_str(body);
        answer
    }

    /// A Liquid answer, as `liquid()` of the app templates sends it.
    fn liquid(body: &str) -> String {
        http("200 OK", &[("content-type", "application/liquid")], body)
    }

    /// An app whose `shopify.app.toml` has `proxy` (TOML lines) as its `[app_proxy]`.
    fn app(label: &str, proxy: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lsf-app-proxy-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("shopify.app.toml"),
            format!(
                "client_id = \"proxy-test\"\nname = \"Proxy test\"\napplication_url = \"https://example.trycloudflare.com\"\n\n[app_proxy]\n{proxy}\n"
            ),
        )
        .unwrap();
        dir
    }

    /// A server of the fixture theme (none when `standalone`, as `lsf app dev` without one),
    /// with the app in `app_dir`, its backend on `app_url`, signing with [`SECRET`].
    fn server(app_dir: PathBuf, app_url: &str, standalone: bool) -> ServerState {
        let app = if standalone {
            App::open_without_theme(Path::new("."), None, &[]).expect("no theme")
        } else {
            let theme = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures/theme");
            App::open(&theme, None, Revalidate::Never, &[]).expect("fixture theme")
        };
        ServerState::new(
            app,
            ServeOptions {
                admin: Some(AdminSettings {
                    client_secret: Some(SECRET.to_string()),
                    app_url: Some(app_url.to_string()),
                    ..Default::default()
                }),
                live_reload: false,
                watch: false,
                quiet: true,
                compress: false,
                minify: false,
                throttle: Default::default(),
                timings: false,
                customer: None,
                checkout: Some(crate::checkout::CheckoutOptions {
                    app_dir,
                    extensions: Vec::new(),
                    variant: crate::checkout::Variant::Standard,
                    standalone,
                }),
            },
        )
        .0
    }

    /// A request of the session `session`, from a browser on `shop.test`.
    fn request(method: &str, path_and_query: &str, session: &str) -> Incoming {
        let (path, query) = path_and_query
            .split_once('?')
            .unwrap_or((path_and_query, ""));
        let mut headers = HashMap::new();
        headers.insert(SESSION_HEADER.to_string(), session.to_string());
        headers.insert("host".to_string(), "shop.test".to_string());
        Incoming {
            method: method.to_string(),
            path: path.to_string(),
            query: crate::server::params::query_pairs(query),
            headers,
            params: serde_json::json!({}),
            body: Default::default(),
            host: "shop.test".to_string(),
        }
    }

    fn get(state: &ServerState, path: &str, session: &str) -> Reply {
        state.dispatch(&request("GET", path, session))
    }

    fn text(reply: &Reply) -> String {
        String::from_utf8_lossy(&reply.body).into_owned()
    }

    fn header<'a>(reply: &'a Reply, name: &str) -> Option<&'a str> {
        reply
            .headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn received(requests: &mpsc::Receiver<Seen>) -> Seen {
        requests
            .recv_timeout(Duration::from_secs(5))
            .expect("the backend got the request")
    }

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn hex_hmac(secret: &str, message: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(message.as_bytes());
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// `validateHmac(searchParams, {signator: 'appProxy'})` of `@shopify/shopify-api`
    /// (`lib/utils/hmac-validator.ts`), rewritten: a repeated name has its values joined with
    /// commas (`normalizeQuery`), and `shop`, `signature` and `timestamp` must not repeat;
    /// `hmac` and `signature` are left out, the rest sorted by name (`localeCompare`, which
    /// agrees with byte order on these lowercase names) and written `name=value` with nothing
    /// between; the hex HMAC-SHA256 must be `signature`, and `timestamp` within 90 seconds.
    fn shopify_api_validates(secret: &str, query: &[(String, String)], now: u64) -> bool {
        let mut normalized: Vec<(String, String)> = Vec::new();
        for (name, value) in query {
            match normalized.iter_mut().find(|(key, _)| key == name) {
                Some(_) if ["hmac", "shop", "signature", "timestamp"].contains(&name.as_str()) => {
                    return false;
                }
                Some((_, existing)) => {
                    existing.push(',');
                    existing.push_str(value);
                }
                None => normalized.push((name.clone(), value.clone())),
            }
        }
        let find = |name: &str| {
            normalized
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        let (Some(signature), Some(timestamp)) = (find("signature"), find("timestamp")) else {
            return false;
        };
        let Ok(timestamp) = timestamp.parse::<i64>() else {
            return false;
        };
        if (now as i64 - timestamp).abs() > 90 {
            return false;
        }
        let mut signed: Vec<&(String, String)> = normalized
            .iter()
            .filter(|(name, _)| name != "hmac" && name != "signature")
            .collect();
        signed.sort_by(|a, b| a.0.cmp(&b.0));
        let message: String = signed
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect();
        hex_hmac(secret, &message) == signature
    }

    /// `calculated_signature` of the `shopify_app` gem (`ShopifyApp::AppProxyVerification`),
    /// rewritten: `"#{k}=#{Array(v).join(',')}"` for each name but `signature`, sorted,
    /// joined with nothing, as shopify.dev's Ruby example does.
    fn shopify_app_gem_signature(secret: &str, query: &[(String, String)]) -> String {
        let mut names: Vec<&String> = Vec::new();
        for (name, _) in query {
            if name != "signature" && !names.contains(&name) {
                names.push(name);
            }
        }
        let mut pairs: Vec<String> = names
            .iter()
            .map(|name| {
                let values: Vec<&str> = query
                    .iter()
                    .filter(|(key, _)| key == *name)
                    .map(|(_, value)| value.as_str())
                    .collect();
                format!("{name}={}", values.join(","))
            })
            .collect();
        pairs.sort();
        hex_hmac(secret, &pairs.concat())
    }

    #[test]
    fn the_query_is_signed_as_shopify_documents_it() {
        // shopify.dev, "Authenticate app proxies": the request of a logged-in customer and of an
        // anonymous one, secret `hush`, and the URL Shopify forwards it to.
        let query = vec![
            ("extra".to_string(), "1".to_string()),
            ("extra".to_string(), "2".to_string()),
        ];
        let shopify = |customer_id| Shopify {
            shop: "shop-name.myshopify.com",
            customer_id,
            path_prefix: "/apps/awesome_reviews",
            timestamp: 1317327555,
            secret: "hush",
        };
        assert_eq!(
            target_url(
                "https://proxy-domain.com/proxy",
                "/extra/path/components",
                &signed_query(&query, &shopify(Some(1)))
            ),
            "https://proxy-domain.com/proxy/extra/path/components?extra=1&extra=2&shop=shop-name.myshopify.com&logged_in_customer_id=1&path_prefix=%2Fapps%2Fawesome_reviews&timestamp=1317327555&signature=4c68c8624d737112c91818c11017d24d334b524cb5c2b8ba08daa056f7395ddb"
        );
        let anonymous = signed_query(&query, &shopify(None));
        assert_eq!(
            anonymous.last().unwrap(),
            &(
                "signature".to_string(),
                "e072b6d7e6622d85912a5214b860d3100dc1e73d9bc29f43796ac8c9ff8093cb".to_string()
            )
        );
        assert_eq!(
            anonymous[3],
            ("logged_in_customer_id".to_string(), String::new())
        );
        // The proxied path itself, and a URL that ends with a slash.
        assert_eq!(
            target_url("http://localhost:3000/proxy/", "", &[]),
            "http://localhost:3000/proxy/?"
        );
        assert_eq!(
            target_url("http://localhost:3000/proxy/", "/a b/é", &[]),
            "http://localhost:3000/proxy/a%20b/%C3%A9?"
        );
    }

    #[test]
    fn a_backend_verifies_the_signature_as_shopifys_libraries_do() {
        let (url, requests) = backend(|_| http("200 OK", &[], "ok"));
        let state = server(app("verify", PROXY), &url, false);
        let reply = get(
            &state,
            "/apps/reviews?q=red+shoes&ref=%2Fa%2Fb&tag=x&tag=y",
            "verify",
        );
        assert_eq!(reply.status, 200, "{}", text(&reply));
        let seen = received(&requests);
        let query = seen.query();
        assert!(
            shopify_api_validates(SECRET, &query, now()),
            "{}",
            seen.target
        );
        assert!(!shopify_api_validates("other", &query, now()));
        // Ninety seconds later the library refuses it: lsf signs with the time of the request.
        assert!(!shopify_api_validates(SECRET, &query, now() + 120));
        assert_eq!(
            Some(shopify_app_gem_signature(SECRET, &query)),
            seen.param("signature")
        );
        // A parameter changed on the way breaks it.
        let mut tampered = query.clone();
        tampered[0].1 = "blue shoes".to_string();
        assert!(!shopify_api_validates(SECRET, &tampered, now()));
    }

    #[test]
    fn a_request_is_forwarded_with_shopifys_parameters_its_method_its_body_and_its_headers() {
        let (url, requests) =
            backend(|_| http("200 OK", &[("content-type", "text/plain")], "saved"));
        let state = server(app("forward", PROXY), &url, false);
        let mut incoming = request(
            "POST",
            "/apps/reviews/extra/path?extra=1&extra=2",
            "forward",
        );
        for (name, value) in [
            ("content-type", "application/x-www-form-urlencoded"),
            ("cookie", "_lsf_session=forward; cart=c1-abc"),
            ("user-agent", "Mozilla/5.0"),
            ("accept", "text/html"),
            ("x-requested-with", "XMLHttpRequest"),
        ] {
            incoming.headers.insert(name.to_string(), value.to_string());
        }
        incoming.body = bytes::Bytes::from_static(b"rating=5&comment=great");
        let reply = state.dispatch(&incoming);
        assert_eq!((reply.status, text(&reply).as_str()), (200, "saved"));

        let seen = received(&requests);
        assert_eq!(seen.method, "POST");
        assert_eq!(seen.path(), "/proxy/extra/path");
        let names: Vec<String> = seen.query().into_iter().map(|(name, _)| name).collect();
        assert_eq!(
            names,
            [
                "extra",
                "extra",
                "shop",
                "logged_in_customer_id",
                "path_prefix",
                "timestamp",
                "signature"
            ]
        );
        assert!(
            seen.target.contains("&path_prefix=%2Fapps%2Freviews&"),
            "{}",
            seen.target
        );
        let admin = state.admin.as_ref().unwrap();
        assert_eq!(seen.param("shop"), Some(admin.api.default_shop()));
        assert_eq!(seen.param("logged_in_customer_id").as_deref(), Some(""));
        let timestamp: u64 = seen.param("timestamp").unwrap().parse().unwrap();
        assert!(now().abs_diff(timestamp) <= 5);
        assert_eq!(seen.body, b"rating=5&comment=great");

        // The browser's headers, without its cookies, with Shopify's two.
        let headers = &seen.headers;
        assert_eq!(headers["content-type"], "application/x-www-form-urlencoded");
        assert_eq!(headers["user-agent"], "Mozilla/5.0");
        assert_eq!(headers["accept"], "text/html");
        assert_eq!(headers["x-requested-with"], "XMLHttpRequest");
        assert_eq!(headers["x-forwarded-host"], "shop.test");
        assert_eq!(headers["x-forwarded-for"], "127.0.0.1");
        assert_eq!(headers["host"], url.trim_start_matches("http://"));
        assert!(!headers.contains_key("cookie"), "{headers:?}");
        assert!(!headers.contains_key(SESSION_HEADER), "{headers:?}");
    }

    #[test]
    fn a_liquid_answer_is_rendered_in_the_layout_of_the_theme() {
        let (url, requests) = backend(|_| {
            http(
                "200 OK",
                &[
                    ("content-type", "application/liquid; charset=utf-8"),
                    ("set-cookie", "backend=1"),
                    ("x-review-count", "3"),
                ],
                "<p id=\"proxied\">{{ shop.name }}|{{ customer.email }}|{{ cart.item_count }}|{{ request.path }}</p>",
            )
        });
        let state = server(app("liquid", PROXY), &url, false);
        let reply = get(&state, "/apps/reviews/product/1", "liquid");
        assert_eq!(reply.status, 200, "{}", text(&reply));
        assert_eq!(
            header(&reply, "content-type"),
            Some("text/html; charset=utf-8")
        );
        let shop = state.loaded().store.shop.name.clone();
        let body = text(&reply);
        assert!(body.starts_with("<!doctype html>"), "{body}");
        assert!(
            body.contains(&format!(
                "<main>\n<p id=\"proxied\">{shop}||0|/apps/reviews/product/1</p>\n</main>"
            )),
            "{body}"
        );
        // What Shopify adds to a page of the theme is there, and no condition on a page type
        // holds.
        assert!(body.contains("class=\"template-\""), "{body}");
        assert!(body.contains("data-page-type=\"\""), "{body}");
        // The headers of the backend reach the browser, but those Shopify strips.
        assert_eq!(header(&reply, "x-review-count"), Some("3"));
        assert_eq!(header(&reply, "set-cookie"), None);
        assert_eq!(header(&reply, "x-lsf-template"), Some("app_proxy"));
        assert_eq!(seen_path(&requests), "/proxy/product/1");
    }

    fn seen_path(requests: &mpsc::Receiver<Seen>) -> String {
        received(requests).path().to_string()
    }

    #[test]
    fn a_liquid_answer_with_layout_none_is_rendered_alone() {
        let (url, _requests) =
            backend(|_| liquid("{% layout none %}<p id=\"alone\">{{ shop.name }}</p>"));
        let state = server(app("layout-none", PROXY), &url, false);
        let reply = get(&state, "/apps/reviews", "layout-none");
        assert_eq!(reply.status, 200, "{}", text(&reply));
        let shop = state.loaded().store.shop.name.clone();
        assert_eq!(text(&reply).trim(), format!("<p id=\"alone\">{shop}</p>"));
    }

    #[test]
    fn without_a_theme_a_liquid_answer_is_rendered_without_a_layout() {
        let (url, _requests) = backend(|_| liquid("<p>{{ shop.name }}</p>"));
        let state = server(app("standalone", PROXY), &url, true);
        let reply = get(&state, "/apps/reviews", "standalone");
        assert_eq!(reply.status, 200, "{}", text(&reply));
        let shop = state.loaded().store.shop.name.clone();
        assert_eq!(text(&reply).trim(), format!("<p>{shop}</p>"));
    }

    #[test]
    fn the_customer_of_the_session_is_sent_and_rendered() {
        let (url, requests) = backend(|_| liquid("{% layout none %}{{ customer.email }}"));
        let state = server(app("customer", PROXY), &url, false);
        let customer = state.loaded().store.customers[0].clone();
        state.with_session("signed-in", |entry| {
            entry.session.customer_id = Some(customer.id)
        });
        let reply = get(&state, "/apps/reviews/mine", "signed-in");
        assert_eq!(text(&reply).trim(), customer.email);
        let seen = received(&requests);
        assert_eq!(
            seen.param("logged_in_customer_id"),
            Some(customer.id.to_string())
        );
        assert!(shopify_api_validates(SECRET, &seen.query(), now()));

        // Logged out, the parameter is there and empty, and so is `customer`.
        let reply = get(&state, "/apps/reviews/mine", "anonymous");
        assert_eq!(text(&reply).trim(), "");
        assert_eq!(
            received(&requests)
                .param("logged_in_customer_id")
                .as_deref(),
            Some("")
        );
    }

    #[test]
    fn json_is_passed_through_with_its_status_and_the_headers_shopify_keeps() {
        let (url, _requests) = backend(|_| {
            http(
                "201 Created",
                &[
                    ("content-type", "application/json"),
                    ("cache-control", "no-store"),
                    ("x-request-id", "abc"),
                    ("set-cookie", "session=1"),
                    ("server", "express"),
                    ("x-powered-by", "Express"),
                    ("date", "Sat, 31 Oct 2026 10:00:00 GMT"),
                ],
                r#"{"saved":true}"#,
            )
        });
        let state = server(app("json", PROXY), &url, false);
        let reply = get(&state, "/apps/reviews/wishlist.json", "json");
        assert_eq!(reply.status, 201);
        assert_eq!(text(&reply), r#"{"saved":true}"#);
        assert_eq!(header(&reply, "content-type"), Some("application/json"));
        assert_eq!(header(&reply, "cache-control"), Some("no-store"));
        assert_eq!(header(&reply, "x-request-id"), Some("abc"));
        for stripped in ["set-cookie", "server", "x-powered-by", "date", "connection"] {
            assert_eq!(header(&reply, stripped), None, "{stripped}");
        }
    }

    #[test]
    fn a_redirect_is_passed_through() {
        let (url, _requests) = backend(|_| {
            http(
                "302 Found",
                &[("location", "/account/login?return_url=/apps/reviews")],
                "",
            )
        });
        let state = server(app("redirect", PROXY), &url, false);
        let reply = get(&state, "/apps/reviews/write", "redirect");
        assert_eq!(reply.status, 302);
        assert_eq!(
            header(&reply, "location"),
            Some("/account/login?return_url=/apps/reviews")
        );
    }

    #[test]
    fn a_backend_that_is_down_gets_a_page_that_says_what_to_do() {
        let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", closed.local_addr().unwrap());
        drop(closed);
        let state = server(app("down", PROXY), &url, false);
        let reply = get(&state, "/apps/reviews", "down");
        assert_eq!(reply.status, 502);
        let body = text(&reply);
        assert!(body.contains(&format!("{url}/proxy")), "{body}");
        assert!(body.contains("--app-url"), "{body}");
        assert!(
            header(&reply, "x-lsf-reason").is_some_and(|reason| reason.contains("nothing listens")),
            "{:?}",
            reply.headers
        );
    }

    #[test]
    fn a_backend_that_does_not_answer_in_time_times_out() {
        let (url, _requests) = backend(|_| {
            std::thread::sleep(Duration::from_millis(1500));
            http("200 OK", &[], "late")
        });
        let mut state = server(app("timeout", PROXY), &url, false);
        state.app_proxy = state
            .app_proxy
            .take()
            .map(|proxy| proxy.with_timeout(Duration::from_millis(200)));
        let reply = get(&state, "/apps/reviews", "timeout");
        assert_eq!(reply.status, 504, "{}", text(&reply));
        assert!(text(&reply).contains("did not answer within 0.2 s"));
    }

    #[test]
    fn a_localized_url_is_proxied_with_its_language() {
        let (url, requests) = backend(|_| liquid("{% layout none %}{{ request.locale.iso_code }}"));
        let state = server(app("locale", PROXY), &url, false);
        let reply = get(&state, "/fr/apps/reviews/x", "locale");
        assert_eq!(text(&reply).trim(), "fr");
        assert_eq!(header(&reply, "content-language"), Some("fr"));
        let seen = received(&requests);
        assert_eq!(seen.path(), "/proxy/x");
        assert_eq!(seen.param("path_prefix").as_deref(), Some("/apps/reviews"));
        let primary = state.loaded().store.primary_language().iso_code.clone();
        assert_eq!(
            seen.headers["accept-language"],
            format!("fr;q=1.0, {primary};q=0.5")
        );
    }

    #[test]
    fn only_the_proxied_path_and_below_are_proxied() {
        let (url, requests) = backend(|_| http("200 OK", &[], "proxied"));
        let state = server(app("paths", PROXY), &url, false);
        for path in ["/apps/reviews", "/apps/reviews/", "/apps/reviews/a/b"] {
            assert_eq!(text(&get(&state, path, "paths")), "proxied", "{path}");
            received(&requests);
        }
        for path in [
            "/apps/reviewsx",
            "/apps/other",
            "/a/reviews",
            "/pages/reviews",
        ] {
            assert!(
                route(&state, &request("GET", path, "paths")).is_none(),
                "{path}"
            );
        }
        // lsf's own paths come first.
        assert_ne!(text(&get(&state, "/__lsf", "paths")), "proxied");
    }

    #[test]
    fn the_url_moves_to_the_app_url_as_webhooks_do() {
        let config = |url: &str| AppConfig {
            application_url: Some("https://example.trycloudflare.com".to_string()),
            app_proxy: Some(lsf_checkout::app::AppProxyConfig {
                url: url.to_string(),
                subpath: "reviews".to_string(),
                prefix: "apps".to_string(),
            }),
            ..Default::default()
        };
        let target =
            |url: &str, app_url: Option<&str>| AppProxy::new(&config(url), app_url).unwrap().target;
        let http = |url: &str| Destination::Http {
            url: url.to_string(),
        };
        let local = Some("http://localhost:3000");
        assert_eq!(
            target("https://example.trycloudflare.com/proxy", local),
            http("http://localhost:3000/proxy")
        );
        // A path joins the app URL, as Shopify's CLI prepends it.
        assert_eq!(target("/proxy", local), http("http://localhost:3000/proxy"));
        assert_eq!(
            target("/proxy", None),
            http("https://example.trycloudflare.com/proxy")
        );
        // Elsewhere, the URL is used as given.
        assert_eq!(
            target("https://reviews.example.com/proxy", local),
            http("https://reviews.example.com/proxy")
        );
    }

    #[test]
    fn the_banner_names_the_local_path_and_where_it_goes() {
        let state = server(app("banner", PROXY), "http://localhost:3000", false);
        let proxy = state.app_proxy.as_ref().unwrap();
        assert_eq!(
            proxy.banner("http://127.0.0.1:9292"),
            "proxy:  http://127.0.0.1:9292/apps/reviews → http://localhost:3000/proxy"
        );
    }

    #[test]
    fn an_app_proxy_shopify_would_refuse_is_not_served() {
        let state = server(
            app(
                "refused",
                "url = \"/proxy\"\nsubpath = \"reviews\"\nprefix = \"shop\"",
            ),
            "http://localhost:3000",
            false,
        );
        let proxy = state.app_proxy.as_ref().unwrap();
        assert_eq!(
            proxy.banner("http://127.0.0.1:9292"),
            "proxy:  not served: [app_proxy] prefix `shop` is not one of a, apps, community, tools"
        );
        assert!(route(&state, &request("GET", "/shop/reviews", "refused")).is_none());
        let reserved = server(
            app(
                "reserved",
                "url = \"/proxy\"\nsubpath = \"login\"\nprefix = \"apps\"",
            ),
            "http://localhost:3000",
            false,
        );
        assert!(reserved.app_proxy.as_ref().unwrap().problem.is_some());
    }
}
