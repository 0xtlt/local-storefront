//! The local storefront server.

mod account;
mod cart;
mod cdn;
mod control;
pub use control::routes;
mod forms;
mod live_reload;
pub mod params;
pub mod reply;
mod storefront;
#[cfg(test)]
mod tests;
pub mod throttle;

use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime};

use axum::body::Body;
use axum::extract::State;
use axum::extract::ws::{WebSocketUpgrade, rejection::WebSocketUpgradeRejection};
use axum::http::{HeaderName, HeaderValue, Request as HttpRequest, Response, StatusCode};
use lsf_core::diagnostics::Diagnostics;
use lsf_core::theme::THEME_DIRECTORIES;
use lsf_core::{Request, Session, Store};
use serde_json::Value as Json;

use self::reply::Reply;
use crate::app::App;

/// The cookie that identifies a visitor's session.
pub const SESSION_COOKIE: &str = "_lsf_session";
/// The cookie Shopify keeps the cart token in. Scripts read it to know whether a cart exists.
pub const CART_COOKIE: &str = "cart";

/// The cart token of a session: stable for the session, different between sessions.
pub fn cart_token_of(session_id: &str) -> String {
    format!("c1-{}", lsf_core::util::short_hash(session_id))
}
/// A header that selects a session explicitly, for API clients without a cookie jar.
pub const SESSION_HEADER: &str = "x-lsf-session";

/// The bytes of a transformed image and their content type.
pub type CachedImage = Arc<(Vec<u8>, &'static str)>;

pub struct ServeOptions {
    /// Inject a script that reloads the page when the theme or the data change.
    pub live_reload: bool,
    /// Reload the store data when its files change.
    pub watch: bool,
    /// Do not log requests.
    pub quiet: bool,
    /// How long each kind of request is held before it is answered.
    pub throttle: throttle::Throttle,
    /// Who new sessions are logged in as, instead of what the store data says: an email,
    /// `default` or `none`.
    pub customer: Option<String>,
}

/// The store as last loaded from disk.
pub struct Loaded {
    pub store: Arc<Store>,
    pub diagnostics: Diagnostics,
    fingerprint: u64,
}

pub struct SessionEntry {
    pub session: Session,
    /// Store data specific to this session, set through the control API.
    pub store: Option<Arc<Store>>,
    /// A throttle of its own, which replaces the server's for this session.
    pub throttle: Option<throttle::Throttle>,
}

pub struct ServerState {
    pub app: App,
    pub options: ServeOptions,
    loaded: RwLock<Arc<Loaded>>,
    last_check: Mutex<Instant>,
    sessions: Mutex<HashMap<String, SessionEntry>>,
    pub(crate) image_cache: Mutex<HashMap<String, CachedImage>>,
    session_counter: AtomicU64,
    started: Instant,
    /// The token of the files when live reload last looked at them.
    last_token: AtomicU64,
    changes: live_reload::Changes,
}

/// One HTTP request, decoded.
pub struct Incoming {
    pub method: String,
    /// The path as requested, percent-decoded.
    pub path: String,
    pub query: Vec<(String, String)>,
    pub headers: HashMap<String, String>,
    /// The body, parsed into nested parameters.
    pub params: Json,
    pub host: String,
}

impl Incoming {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    pub fn query_param(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The body and query parameters merged, the body taking precedence.
    pub fn all_params(&self) -> Json {
        let mut merged = params::nest(self.query.clone());
        if let (Json::Object(target), Json::Object(body)) = (&mut merged, &self.params) {
            for (key, value) in body {
                target.insert(key.clone(), value.clone());
            }
        }
        merged
    }

    /// The path of the page the request came from, for redirects back to it.
    pub fn referer_path(&self) -> Option<String> {
        let referer = self.header("referer")?;
        let after_scheme = referer.split_once("://").map_or(referer, |(_, rest)| rest);
        let path = after_scheme.find('/').map(|index| &after_scheme[index..])?;
        Some(path.to_string())
    }
}

/// A fingerprint of a directory tree: it changes when a file is added, removed or modified.
fn fingerprint(root: &Path) -> u64 {
    fn walk(directory: &Path, hasher: &mut std::collections::hash_map::DefaultHasher) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        let mut entries: Vec<_> = entries.filter_map(|entry| entry.ok()).collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if metadata.is_dir() {
                hasher.write(name.as_encoded_bytes());
                walk(&entry.path(), hasher);
            } else {
                hasher.write(name.as_encoded_bytes());
                hasher.write_u64(metadata.len());
                let modified = metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map_or(0, |since| since.as_nanos() as u64);
                hasher.write_u64(modified);
            }
        }
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    walk(root, &mut hasher);
    hasher.finish()
}

impl ServerState {
    pub fn new(app: App, options: ServeOptions) -> (ServerState, Diagnostics) {
        let (store, diagnostics) = app.load_store();
        let fingerprint = app.data_dir().map_or(0, |directory| fingerprint(directory));
        let state = ServerState {
            loaded: RwLock::new(Arc::new(Loaded {
                store: Arc::new(store),
                diagnostics: diagnostics.clone(),
                fingerprint,
            })),
            app,
            options,
            last_check: Mutex::new(Instant::now()),
            sessions: Mutex::new(HashMap::new()),
            image_cache: Mutex::new(HashMap::new()),
            session_counter: AtomicU64::new(0),
            started: Instant::now(),
            last_token: AtomicU64::new(0),
            changes: live_reload::Changes::new(),
        };
        (state, diagnostics)
    }

    pub fn loaded(&self) -> Arc<Loaded> {
        self.loaded.read().expect("store lock poisoned").clone()
    }

    /// Reloads the store from disk. Sessions keep their cart and customer.
    pub fn reload(&self) -> Arc<Loaded> {
        let (store, diagnostics) = self.app.load_store();
        let loaded = Arc::new(Loaded {
            store: Arc::new(store),
            diagnostics,
            fingerprint: self
                .app
                .data_dir()
                .map_or(0, |directory| fingerprint(directory)),
        });
        *self.loaded.write().expect("store lock poisoned") = loaded.clone();
        self.image_cache
            .lock()
            .expect("image cache poisoned")
            .clear();
        loaded
    }

    /// Reloads the store when its files changed since the last check. The files are looked at
    /// every 300 ms at most, unless the caller knows that they changed (`now`).
    fn refresh_if_changed(&self, now: bool) {
        if !self.options.watch {
            return;
        }
        let Some(directory) = self.app.data_dir() else {
            return;
        };
        {
            let mut last_check = self.last_check.lock().expect("check lock poisoned");
            if !now && last_check.elapsed() < Duration::from_millis(300) {
                return;
            }
            *last_check = Instant::now();
        }
        if fingerprint(directory) != self.loaded().fingerprint {
            let loaded = self.reload();
            if !self.options.quiet {
                eprintln!("store data reloaded");
                crate::output::print_diagnostics(&loaded.diagnostics);
            }
        }
    }

    /// A token that changes whenever the theme or the data change, for live reload. Only the
    /// directories a theme is made of are looked at: what else lives next to them (sources,
    /// `node_modules`) is not served, and can be large.
    fn change_token(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let root = self.app.theme.files().root();
        for directory in THEME_DIRECTORIES {
            hasher.write_u64(fingerprint(&root.join(directory)));
        }
        if let Some(directory) = self.app.data_dir() {
            hasher.write_u64(fingerprint(directory));
        }
        hasher.finish()
    }

    /// The token of the files as they are now, for live reload. When it is not the one seen
    /// last, what is cached about the files is dropped: the pages reload at once, and must be
    /// rendered from the files as they are, not as they were a moment ago.
    pub fn observe_changes(&self) -> u64 {
        let token = self.change_token();
        if self.last_token.swap(token, Ordering::Relaxed) != token {
            self.app.theme.files().expire();
            self.refresh_if_changed(true);
        }
        token
    }

    fn new_session_id(&self) -> String {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(self.session_counter.fetch_add(1, Ordering::Relaxed));
        hasher.write_u128(self.started.elapsed().as_nanos());
        let first = hasher.finish();
        hasher.write_u64(first);
        format!("{first:016x}{:016x}", hasher.finish())
    }

    /// The session id a request belongs to, and whether it was just created.
    pub fn session_id(&self, incoming: &Incoming) -> (String, bool) {
        if let Some(id) = incoming.header(SESSION_HEADER).filter(|id| !id.is_empty()) {
            return (id.to_string(), false);
        }
        let from_cookie = incoming.header("cookie").and_then(|cookies| {
            cookies
                .split(';')
                .filter_map(|cookie| cookie.trim().split_once('='))
                .find(|(name, _)| *name == SESSION_COOKIE)
                .map(|(_, value)| value.to_string())
        });
        match from_cookie {
            Some(id) => (id, false),
            None => (self.new_session_id(), true),
        }
    }

    /// The session a request names, if it names one. Unlike [`ServerState::session_id`], this
    /// never starts a session.
    fn named_session(&self, incoming: &Incoming) -> Option<String> {
        if let Some(id) = incoming.header(SESSION_HEADER).filter(|id| !id.is_empty()) {
            return Some(id.to_string());
        }
        incoming.header("cookie").and_then(|cookies| {
            cookies
                .split(';')
                .filter_map(|cookie| cookie.trim().split_once('='))
                .find(|(name, _)| *name == SESSION_COOKIE)
                .map(|(_, value)| value.to_string())
        })
    }

    /// How long to hold a request before answering it: the throttle of its session when it
    /// has one, the server's otherwise.
    pub fn delay_for(&self, incoming: &Incoming) -> Duration {
        let (_, _, path) = self.localize(&self.loaded().store, &incoming.path);
        let wants_sections = incoming.query_param("section_id").is_some()
            || incoming.query_param("sections").is_some();
        let Some(kind) = throttle::classify(&incoming.method, &path, wants_sections) else {
            return Duration::ZERO;
        };
        let of_session = self.named_session(incoming).and_then(|id| {
            let sessions = self.sessions.lock().expect("sessions poisoned");
            sessions.get(&id).and_then(|entry| entry.throttle.clone())
        });
        match of_session {
            Some(throttle) => throttle.delay(kind),
            None => self.options.throttle.delay(kind),
        }
    }

    /// The state a new session starts with: what the store data says, with the customer of
    /// `--customer` when there is one.
    pub fn initial_session(&self, store: &Store) -> Session {
        let mut session = Session::initial(store);
        if let Some(who) = &self.options.customer
            && let Ok(customer) = store.customer_named(who)
        {
            let id = customer.map(|customer| customer.id);
            if session.customer_id != id {
                session.customer_id = id;
                session.company_location = None;
            }
        }
        session
    }

    /// Reads the session, creating it with the store's defaults when it is new.
    pub fn with_session<T>(&self, id: &str, action: impl FnOnce(&mut SessionEntry) -> T) -> T {
        let mut sessions = self.sessions.lock().expect("sessions poisoned");
        let entry = sessions
            .entry(id.to_string())
            .or_insert_with(|| SessionEntry {
                session: Session {
                    cart_token: cart_token_of(id),
                    ..self.initial_session(&self.loaded().store)
                },
                store: None,
                throttle: None,
            });
        action(entry)
    }

    pub fn reset_session(&self, id: &str) {
        self.sessions.lock().expect("sessions poisoned").remove(id);
    }

    pub fn session_count(&self) -> usize {
        self.sessions.lock().expect("sessions poisoned").len()
    }

    /// A snapshot of a session and the store it sees.
    pub fn snapshot(&self, id: &str) -> (Session, Arc<Store>) {
        let base = self.loaded().store.clone();
        self.with_session(id, |entry| {
            (entry.session.clone(), entry.store.clone().unwrap_or(base))
        })
    }

    /// Splits the locale prefix off a path: `/fr/cart` → (`fr`, `/fr`, `/cart`).
    pub fn localize(&self, store: &Store, path: &str) -> (String, String, String) {
        let primary = store.primary_language().iso_code.clone();
        let trimmed = path.trim_start_matches('/');
        let (first, rest) = trimmed.split_once('/').unwrap_or((trimmed, ""));
        let language = store
            .languages
            .iter()
            .find(|language| !language.primary && language.iso_code.eq_ignore_ascii_case(first));
        match language {
            Some(language) => (
                language.iso_code.clone(),
                language.root_url.clone(),
                format!("/{rest}"),
            ),
            None => (primary, String::new(), path.to_string()),
        }
    }

    /// The core request for a storefront path.
    pub fn storefront_request(
        &self,
        store: &Store,
        incoming: &Incoming,
        path: &str,
        query: Vec<(String, String)>,
    ) -> Request {
        let (locale, root, path) = self.localize(store, path);
        Request {
            host: incoming.host.clone(),
            scheme: incoming
                .header("x-forwarded-proto")
                .unwrap_or("http")
                .to_string(),
            path,
            query,
            locale,
            root,
        }
    }

    fn dispatch(&self, incoming: &Incoming) -> Reply {
        if incoming.path.starts_with("/__lsf") {
            return control::handle(self, incoming);
        }
        if incoming.path.starts_with("/cdn/") {
            return cdn::handle(self, incoming);
        }
        self.refresh_if_changed(false);
        storefront::handle(self, incoming)
    }
}

fn percent_decode(input: &str) -> String {
    percent_encoding::percent_decode_str(input)
        .decode_utf8_lossy()
        .into_owned()
}

async fn handle(
    State(state): State<Arc<ServerState>>,
    request: HttpRequest<Body>,
) -> Response<Body> {
    let started = Instant::now();
    let (parts, body) = request.into_parts();
    let headers: HashMap<String, String> = parts
        .headers
        .iter()
        .filter_map(|(name, value)| {
            Some((name.as_str().to_string(), value.to_str().ok()?.to_string()))
        })
        .collect();
    let content_type = headers.get("content-type").cloned().unwrap_or_default();
    let body = axum::body::to_bytes(body, 32 * 1024 * 1024)
        .await
        .unwrap_or_default();
    let incoming = Incoming {
        method: parts.method.as_str().to_string(),
        path: percent_decode(parts.uri.path()),
        query: params::query_pairs(parts.uri.query().unwrap_or_default()),
        host: headers
            .get("host")
            .cloned()
            .unwrap_or_else(|| "localhost".to_string()),
        params: params::parse_body(&content_type, body).await,
        headers,
    };

    let log_line = format!("{} {}", incoming.method, parts.uri);
    let quiet = state.options.quiet;
    // Held here rather than on a render thread: waiting costs nothing.
    let delay = state.delay_for(&incoming);
    if !delay.is_zero() {
        tokio::time::sleep(delay).await;
    }
    // Rendering is CPU-bound: keep it off the async workers.
    let worker_state = state.clone();
    let reply = tokio::task::spawn_blocking(move || worker_state.dispatch(&incoming))
        .await
        .unwrap_or_else(|error| Reply::text(500, format!("internal error: {error}")));

    if !quiet
        && !parts.uri.path().starts_with("/cdn/")
        && !parts.uri.path().starts_with("/__lsf/livereload")
    {
        let errors = reply
            .headers
            .iter()
            .find(|(name, _)| name == "x-lsf-liquid-errors")
            .map(|(_, count)| format!(" ({count} Liquid errors)"))
            .unwrap_or_default();
        eprintln!(
            "{} {log_line} {:.1}ms{errors}",
            reply.status,
            started.elapsed().as_secs_f64() * 1000.0
        );
    }

    let reply = if delay.is_zero() {
        reply
    } else {
        reply.header("x-lsf-throttle", format!("{}ms", delay.as_millis()))
    };
    let mut response =
        Response::builder().status(StatusCode::from_u16(reply.status).unwrap_or(StatusCode::OK));
    for (name, value) in &reply.headers {
        if let (Ok(name), Ok(value)) = (
            HeaderName::try_from(name.as_str()),
            HeaderValue::try_from(value.as_str()),
        ) {
            response = response.header(name, value);
        }
    }
    response
        .body(Body::from(reply.body))
        .unwrap_or_else(|_| Response::new(Body::empty()))
}

/// `/__lsf/livereload`: a WebSocket for the pages that open one, and otherwise the token of
/// the files, like the rest of the control API.
async fn live_reload(
    State(state): State<Arc<ServerState>>,
    upgrade: Result<WebSocketUpgrade, WebSocketUpgradeRejection>,
    request: HttpRequest<Body>,
) -> Response<Body> {
    match upgrade {
        Ok(upgrade) => upgrade.on_upgrade(move |socket| live_reload::serve(socket, state)),
        Err(_) => handle(State(state), request).await,
    }
}

/// Serves the storefront until the process is interrupted.
pub async fn run(
    state: Arc<ServerState>,
    listener: tokio::net::TcpListener,
) -> std::io::Result<()> {
    let router = axum::Router::new()
        .route("/__lsf/livereload", axum::routing::get(live_reload))
        .fallback(handle)
        .with_state(state);
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
}
