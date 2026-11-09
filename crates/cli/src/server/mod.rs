//! The local storefront server.

mod account;
pub mod admin;
pub mod app_proxy;
mod cart;
mod cdn;
mod checkout;
mod checkout_account;
mod compress;
mod control;
mod customer_account;
mod data_files;
pub use control::{apply_session_input, routes};
mod forms;
mod home;
mod home_check;
pub use home_check::{Unmet, run_with_expectations as check_scenarios};
mod live_reload;
mod minify;
pub mod params;
pub mod polaris;
pub mod reply;
mod speedscope;
mod store_sync;
mod storefront;
#[cfg(test)]
mod tests;
pub mod throttle;
mod timing;

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
use lsf_core::store::load::{LoadOptions, SourceData};
use lsf_core::theme::THEME_DIRECTORIES;
use lsf_core::{Session, Store};
use lsf_theme::Request;
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
    /// Compress responses for the clients that accept it.
    pub compress: bool,
    /// Minify the stylesheets and the scripts of the theme.
    pub minify: bool,
    /// How long each kind of request is held before it is answered.
    pub throttle: throttle::Throttle,
    /// Name every section and every theme block in the `Server-Timing` header of what is
    /// rendered.
    pub timings: bool,
    /// Who new sessions are logged in as, instead of what the store data says: an email,
    /// `default` or `none`.
    pub customer: Option<String>,
    /// The Shopify app whose checkout the server shows. Without it, `/checkout` is a page that
    /// lists the cart.
    pub checkout: Option<crate::checkout::CheckoutOptions>,
    /// Serve the Admin API of the store (and the webhooks of the app of `checkout`). `None`
    /// for the commands that only run the checkout.
    pub admin: Option<admin::AdminSettings>,
    /// What `/__lsf/cdn/` answers when the server starts (`--polaris`): Shopify's CDN, or lsf's
    /// copy of Polaris.
    pub polaris: polaris::Mode,
}

/// The store as last loaded from disk, with what the Admin API changed on top.
pub struct Loaded {
    /// What the storefront and the checkout see: the files, and what the Admin API changed in
    /// its default shop (`store_sync`).
    pub store: Arc<Store>,
    /// The problems of the files.
    pub diagnostics: Diagnostics,
    /// The store of the files alone: what the shops of the Admin API were seeded from, which
    /// store sync projects their changes onto. `store` itself while the Admin API changed
    /// nothing the store data shows.
    pub base: Arc<Store>,
    /// The data the files gave when `base` was built, and the options it was built with: store
    /// sync and the sessions with data of their own build on them, never on the files again.
    /// `--static` reads the files once; without `--watch`, a file changed on disk is not seen
    /// until the store is reloaded, by the storefront or by what is built on top of it.
    pub source: Arc<SourceData>,
    pub load_options: LoadOptions,
    /// What the Admin API changed, as `store` has it.
    pub synced: store_sync::Applied,
}

pub struct SessionEntry {
    pub session: Session,
    /// Store data specific to this session, set through the control API.
    pub store: Option<Arc<Store>>,
    /// The data `store` was built from, to build it again when what the Admin API changed does.
    pub data: Option<store_sync::SessionData>,
    /// A throttle of its own, which replaces the server's for this session.
    pub throttle: Option<throttle::Throttle>,
    /// What the buyer entered at the checkout of the app.
    pub checkout: checkout::CheckoutSession,
}

pub struct ServerState {
    pub app: App,
    pub options: ServeOptions,
    loaded: RwLock<Arc<Loaded>>,
    /// Held while the store is built: from the files (`reload`), or with what the Admin API
    /// changed (`store_sync`). One build at a time, each swapped into `loaded` whole.
    rebuilding: Mutex<()>,
    /// The count of the Admin API's changes ([`lsf_app::admin::api::AdminApi::store_changes`]) the
    /// store was last brought up to date with.
    admin_changes: AtomicU64,
    last_check: Mutex<Instant>,
    /// The files of the data directory as the store was last read from them: what tells which
    /// changed (`--watch`), and so whether the shops of the Admin API are seeded again.
    data_files: Mutex<data_files::DataFiles>,
    sessions: Mutex<HashMap<String, SessionEntry>>,
    /// The variants of the images that were encoded, by what they were made from: encoding
    /// one costs too much to do it again while the image has not changed.
    pub(crate) image_cache: Mutex<HashMap<String, CachedImage>>,
    /// The stylesheets and the scripts that were minified, by their path and their content.
    /// `None` for the ones that are served as written.
    pub(crate) minified: Mutex<HashMap<String, Option<Arc<minify::Minified>>>>,
    session_counter: AtomicU64,
    started: Instant,
    /// The token of the files when live reload last looked at them.
    last_token: AtomicU64,
    changes: live_reload::Changes,
    /// How long the templates, the sections and the blocks took so far, for `/__lsf/timings`.
    pub(crate) timings: timing::Totals,
    /// The Functions that fail when the scenarios run, as the home page last found them.
    pub(crate) home_check: home_check::Cache,
    /// The builds of `--watch`: the ones that failed, and when the pages should reload.
    pub(crate) builds: Arc<crate::watch::Builds>,
    /// The Admin API of the store, when the server serves it.
    pub admin: Option<admin::Admin>,
    /// The app proxy of the app, when it has an `[app_proxy]` and the server serves the Admin
    /// API (whose client secret signs the requests, and whose shop they are from).
    pub app_proxy: Option<app_proxy::AppProxy>,
    /// The Polaris mode, which the control API changes (`/__lsf/polaris`).
    pub polaris: polaris::Polaris,
}

/// One HTTP request, decoded.
pub struct Incoming {
    pub method: String,
    /// The path as requested, percent-decoded.
    pub path: String,
    pub query: Vec<(String, String)>,
    /// The query as sent, without the `?`, not decoded.
    pub raw_query: String,
    pub headers: HashMap<String, String>,
    /// The body, parsed into nested parameters.
    pub params: Json,
    /// The body as sent: what the Admin API reads (a GraphQL request, an OAuth form).
    pub body: bytes::Bytes,
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

/// The files of the data directory of `app` as they are now: none for the demo store.
fn data_files_of(app: &App) -> data_files::DataFiles {
    app.data_dir()
        .map(|directory| data_files::DataFiles::read(&directory))
        .unwrap_or_default()
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

/// Locks `mutex`, even after a panic while it was held: the locks this takes guard no data a
/// panic can leave half done.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Loaded {
    /// The store of the files of `app` alone, read now.
    fn of_files(app: &App) -> Loaded {
        let source = Arc::new(app.read_store());
        let load_options = app.load_options();
        let (store, diagnostics) = source.build(&load_options, &[]);
        let base = Arc::new(store);
        Loaded {
            store: base.clone(),
            diagnostics,
            base,
            source,
            load_options,
            synced: Default::default(),
        }
    }
}

impl ServerState {
    pub fn new(app: App, options: ServeOptions) -> (ServerState, Diagnostics) {
        // Before the files are read: one changed meanwhile is read again.
        let data_files = data_files_of(&app);
        let files = Loaded::of_files(&app);
        let diagnostics = files.diagnostics.clone();
        let admin = options.admin.as_ref().map(|settings| {
            let app_dir = options
                .checkout
                .as_ref()
                .map(|checkout| checkout.app_dir.as_path());
            admin::Admin::new(
                &files.base,
                settings,
                app_dir,
                app.theme.files().root(),
                options.quiet,
            )
        });
        let app_proxy = options.admin.as_ref().and_then(|settings| {
            let app_dir = options.checkout.as_ref()?.app_dir.as_path();
            app_proxy::AppProxy::of_app(app_dir, settings.app_url.as_deref())
        });
        let admin_changes = admin.as_ref().map_or(0, |admin| admin.api.store_changes());
        let polaris = polaris::Polaris::new(options.polaris);
        let state = ServerState {
            loaded: RwLock::new(Arc::new(files)),
            rebuilding: Mutex::new(()),
            admin_changes: AtomicU64::new(admin_changes),
            data_files: Mutex::new(data_files),
            app,
            options,
            last_check: Mutex::new(Instant::now()),
            sessions: Mutex::new(HashMap::new()),
            image_cache: Mutex::new(HashMap::new()),
            minified: Mutex::new(HashMap::new()),
            session_counter: AtomicU64::new(0),
            started: Instant::now(),
            last_token: AtomicU64::new(0),
            changes: live_reload::Changes::new(),
            timings: timing::Totals::default(),
            home_check: home_check::Cache::default(),
            builds: Default::default(),
            admin,
            app_proxy,
            polaris,
        };
        (state, diagnostics)
    }

    /// Reads the app of the checkout. With `--watch`, the Functions it builds run the copy of
    /// their last module that built, never what a build is writing; with `--no-functions`, it
    /// has none.
    pub(crate) fn load_app(&self, dir: &Path) -> Result<lsf_app::config::App, String> {
        let mut app = lsf_app::config::load(dir)?;
        self.builds.apply(&mut app);
        if self
            .options
            .checkout
            .as_ref()
            .is_some_and(|options| options.no_functions)
        {
            app.without_functions();
        }
        Ok(app)
    }

    /// Whether the Admin API has the app uninstalled from its default shop, the shop of the
    /// checkout (`POST /__lsf/admin/uninstall`, `appUninstall`): its Functions then do not run.
    /// `false` without the Admin API.
    pub fn app_uninstalled(&self) -> bool {
        self.admin.as_ref().is_some_and(|admin| {
            admin
                .api
                .installed_scopes(&admin.api.default_shop())
                .is_none()
        })
    }

    pub fn loaded(&self) -> Arc<Loaded> {
        self.loaded.read().expect("store lock poisoned").clone()
    }

    /// Reloads the store from disk, whatever changed (`POST /__lsf/reload`). Sessions keep their
    /// cart and customer; the shops of the Admin API are seeded again, and what it changed in
    /// them is dropped (the terminal says so): the store is the files', and the theme the
    /// directory's, without the Admin API's files or script tags. `--watch` reads the files
    /// again only when they changed, and seeds the shops again only when the store's own data
    /// did ([`ServerState::refresh_if_changed`]).
    pub fn reload(&self) -> Arc<Loaded> {
        let (loaded, dropped) = self.reload_and_reseed();
        if !dropped.is_empty() && !self.options.quiet {
            eprintln!(
                "admin:  the store data was read again: the shops were seeded again, and what the Admin API changed in {} was dropped",
                dropped.join(", ")
            );
        }
        loaded
    }

    /// Reads every file again and seeds the shops of the Admin API again from them: the store
    /// is the files', the theme the directory's. Answers the store and the shops whose changes
    /// were dropped.
    fn reload_and_reseed(&self) -> (Arc<Loaded>, Vec<String>) {
        let _rebuilding = lock(&self.rebuilding);
        *lock(&self.data_files) = data_files_of(&self.app);
        let files = Loaded::of_files(&self.app);
        let mut dropped = Vec::new();
        let loaded = match &self.admin {
            Some(admin) => {
                dropped = admin.api.reseed(&files.base).dropped;
                admin.api.sync_store(files.base.clone());
                // The reseed dropped what the Admin API wrote to the theme and the script tags:
                // the storefront serves the directory again, as the Admin API now reports it.
                self.sync_theme(&admin.api);
                self.with_synced(files, admin)
            }
            None => files,
        };
        (self.swap_loaded(loaded), dropped)
    }

    /// Reads every file again when only checkout scenarios changed: the store data the shops
    /// of the Admin API were seeded from is the same, so they keep what the backend changed
    /// (its subscription, its customizations, its other writes), and the store is the files
    /// with those changes on top, as before, with the new scenarios.
    fn reload_scenarios(&self) -> Arc<Loaded> {
        let _rebuilding = lock(&self.rebuilding);
        *lock(&self.data_files) = data_files_of(&self.app);
        let files = Loaded::of_files(&self.app);
        let loaded = match &self.admin {
            Some(admin) => {
                admin.api.sync_store(files.base.clone());
                self.with_synced(files, admin)
            }
            None => files,
        };
        self.swap_loaded(loaded)
    }

    /// The store of `files` with what the Admin API changed on top. Nothing changed since the
    /// shops were seeded, unless a request of the backend came in meanwhile: then the store
    /// has it already. Called while `rebuilding` is held.
    fn with_synced(&self, files: Loaded, admin: &admin::Admin) -> Loaded {
        match admin.api.synced() {
            Some(synced) => {
                let loaded = self.with_admin_changes(&files, &synced);
                self.admin_changes.store(synced.changes, Ordering::Release);
                loaded
            }
            None => files,
        }
    }

    /// Makes `loaded` the store the server serves.
    fn swap_loaded(&self, loaded: Loaded) -> Arc<Loaded> {
        let loaded = Arc::new(loaded);
        *self.loaded.write().expect("store lock poisoned") = loaded.clone();
        loaded
    }

    /// Reloads the store when its files changed since they were last read (`--watch`). The
    /// files are looked at every 300 ms at most, unless the caller knows that they changed
    /// (`now`). A change of checkout scenarios alone reads the files again without touching
    /// the shops of the Admin API; a change of the store's own data seeds them again; a file
    /// the store does not read (`schema/`, `README.md`) changes nothing. The terminal says
    /// which files changed and what was done.
    fn refresh_if_changed(&self, now: bool) {
        if !self.options.watch {
            return;
        }
        {
            let mut last_check = self.last_check.lock().expect("check lock poisoned");
            if !now && last_check.elapsed() < Duration::from_millis(300) {
                return;
            }
            *last_check = Instant::now();
        }
        // The plans of `--pricing-plans`, wherever their file is.
        if let Some(admin) = &self.admin {
            match admin.api.reload_pricing_file_if_changed() {
                Ok(Some(file)) if !self.options.quiet => {
                    eprintln!("pricing plans reloaded ({file})")
                }
                Ok(_) => {}
                Err(error) => eprintln!("pricing plans not reloaded: {error}"),
            }
        }
        let Some(directory) = self.app.data_dir() else {
            return;
        };
        let files = data_files::DataFiles::read(&directory);
        let change = {
            let mut seen = lock(&self.data_files);
            if *seen == files {
                return;
            }
            let change = seen.change_to(&files);
            *seen = files;
            change
        };
        match change {
            data_files::DataChange::Unread => {}
            data_files::DataChange::Scenarios(changed) => {
                let loaded = self.reload_scenarios();
                if !self.options.quiet {
                    let kept = if self.admin.is_some() {
                        ": the store and what the Admin API changed are kept (no re-seed)"
                    } else {
                        ""
                    };
                    eprintln!(
                        "checkout scenarios reloaded ({}){kept}",
                        data_files::describe(&changed)
                    );
                    crate::output::print_diagnostics(&loaded.diagnostics);
                }
            }
            data_files::DataChange::Store(changed) => {
                let (loaded, dropped) = self.reload_and_reseed();
                if !self.options.quiet {
                    let seeded = match (&self.admin, dropped.is_empty()) {
                        (None, _) => String::new(),
                        (Some(_), true) => {
                            ": the shops of the Admin API were seeded again".to_string()
                        }
                        (Some(_), false) => format!(
                            ": the shops of the Admin API were seeded again, and what the Admin API changed in {} was dropped",
                            dropped.join(", ")
                        ),
                    };
                    eprintln!(
                        "store data reloaded ({}){seeded}",
                        data_files::describe(&changed)
                    );
                    crate::output::print_diagnostics(&loaded.diagnostics);
                }
            }
        }
    }

    /// A token that changes whenever the theme or the data change, for live reload. Only the
    /// directories a theme is made of are looked at: what else lives next to them (sources,
    /// `node_modules`) is not served, and can be large. With `--watch`, also when a build is over
    /// or a file of the app changed.
    fn change_token(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        hasher.write_u64(self.builds.generation());
        // What the Admin API changed, once the store has it, and in the theme.
        hasher.write_u64(self.loaded().synced.generation);
        hasher.write_u64(self.app.theme.files().overlay_generation());
        for src in self.app.theme.script_tags().iter() {
            hasher.write(src.as_bytes());
        }
        if let Some(identity) = self.app.theme.identity() {
            hasher.write_u64(identity.id);
            hasher.write(identity.name.as_deref().unwrap_or_default().as_bytes());
        }
        let root = self.app.theme.files().root();
        for directory in THEME_DIRECTORIES {
            hasher.write_u64(fingerprint(&root.join(directory)));
        }
        if let Some(directory) = self.app.data_dir() {
            hasher.write_u64(fingerprint(&directory));
        }
        hasher.finish()
    }

    /// The token of the files as they are now, for live reload. When it is not the one seen
    /// last, what is cached about the files is dropped: the pages reload at once, and must be
    /// rendered from the files as they are, not as they were a moment ago.
    pub fn observe_changes(&self) -> u64 {
        // What the Admin API changed is built first: the token then says so, and the pages that
        // reload find it built.
        self.sync_admin_changes(None);
        let token = self.change_token();
        if self.last_token.swap(token, Ordering::Relaxed) != token {
            self.app.theme.files().expire();
            self.refresh_if_changed(true);
        }
        token
    }

    pub(crate) fn new_session_id(&self) -> String {
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

    /// The store a request sees: the one of its session when the session has its own, the loaded
    /// one otherwise. Unlike [`ServerState::snapshot`], this never starts a session, so that
    /// routing can ask it of any request.
    pub fn store_of(&self, incoming: &Incoming) -> Arc<Store> {
        let of_session = self.named_session(incoming).and_then(|id| {
            let sessions = self.sessions.lock().expect("sessions poisoned");
            sessions.get(&id).and_then(|entry| entry.store.clone())
        });
        of_session.unwrap_or_else(|| self.loaded().store.clone())
    }

    /// How long to hold a request before answering it: the throttle of its session when it
    /// has one, the server's otherwise.
    pub fn delay_for(&self, incoming: &Incoming) -> Duration {
        let (_, _, path) = self.localize(&self.store_of(incoming), &incoming.path);
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
                data: None,
                throttle: None,
                checkout: Default::default(),
            });
        action(entry)
    }

    pub fn reset_session(&self, id: &str) {
        self.sessions.lock().expect("sessions poisoned").remove(id);
    }

    /// How many orders the sessions placed, all together.
    #[cfg(test)]
    pub fn placed_orders(&self) -> u64 {
        self.sessions
            .lock()
            .expect("sessions poisoned")
            .values()
            .map(|entry| entry.checkout.placed)
            .sum()
    }

    /// Forgets every session, with its cart, its checkout and the order it placed. Returns how
    /// many there were.
    pub fn reset_sessions(&self) -> usize {
        let mut sessions = self.sessions.lock().expect("sessions poisoned");
        let count = sessions.len();
        sessions.clear();
        count
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
        // Shopify's CDN for the app's pages: nothing of the store.
        if incoming.path.starts_with(polaris::PREFIX) {
            return polaris::cdn(self, incoming);
        }
        // The Admin API, OAuth and their control endpoints come first: a backend's request is
        // never a page.
        if let Some(reply) = admin::route(self, incoming) {
            return reply;
        }
        // What follows sees what the Admin API changed.
        self.sync_admin_changes(Some(incoming));
        if incoming.path.starts_with("/__lsf") {
            return control::handle(self, incoming);
        }
        if incoming.path.starts_with("/cdn/") {
            // As Shopify's CDN: a page of another origin may fetch its files with headers that
            // need a preflight.
            if incoming.method == "OPTIONS" {
                return Reply::preflight(incoming.header("access-control-request-headers"));
            }
            return cdn::handle(self, incoming);
        }
        self.refresh_if_changed(false);
        // The app proxy before the theme's pages: its path is the backend's.
        if let Some(reply) = app_proxy::route(self, incoming) {
            return reply;
        }
        if let Some(reply) = checkout::route(self, incoming) {
            return reply;
        }
        storefront::handle(self, incoming)
    }
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
    let limit = body_limit(parts.uri.path());
    let body = match read_body(body, limit, headers.get("content-length")).await {
        Ok(body) => body,
        Err(refused) => return refused.into_response(limit),
    };
    let path = lsf_core::urls::percent_decode(parts.uri.path());
    let incoming = Incoming {
        method: parts.method.as_str().to_string(),
        query: params::query_pairs(parts.uri.query().unwrap_or_default()),
        raw_query: parts.uri.query().unwrap_or_default().to_string(),
        host: headers
            .get("host")
            .cloned()
            .unwrap_or_else(|| "localhost".to_string()),
        params: params::parse_body(&path, &content_type, body.clone()).await,
        path,
        body,
        headers,
    };

    let log_line = format!("{} {}", incoming.method, parts.uri);
    let quiet = state.options.quiet;
    let encoding = compress::negotiate(incoming.header("accept-encoding"));
    // Held here rather than on a render thread: waiting costs nothing.
    let delay = state.delay_for(&incoming);
    let mut waited = Duration::ZERO;
    if !delay.is_zero() {
        let waiting = Instant::now();
        tokio::time::sleep(delay).await;
        waited = waiting.elapsed();
    }
    // Rendering is CPU-bound, and so is compressing: keep them off the async workers.
    let worker_state = state.clone();
    let (reply, compressing) = tokio::task::spawn_blocking(move || {
        let reply = worker_state.dispatch(&incoming);
        if !worker_state.options.compress {
            return (reply, None);
        }
        let compressing = Instant::now();
        let encoded = reply.has_header("content-encoding");
        let reply = compress::apply(reply, encoding);
        let compressed = !encoded && reply.has_header("content-encoding");
        (reply, compressed.then(|| compressing.elapsed()))
    })
    .await
    .unwrap_or_else(|error| (Reply::text(500, format!("internal error: {error}")), None));

    if !quiet
        && !parts.uri.path().starts_with("/cdn/")
        && !parts.uri.path().starts_with(polaris::PREFIX)
        && !parts.uri.path().starts_with("/__lsf/livereload")
    {
        let errors = reply
            .headers
            .iter()
            .find(|(name, _)| name == "x-lsf-liquid-errors")
            .and_then(|(_, count)| count.parse().ok())
            .map(|n| {
                format!(
                    " ({})",
                    crate::output::count(n, "Liquid error", "Liquid errors")
                )
            })
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
    // The time a throttle made the request wait is not time the server took.
    let processing = started.elapsed().saturating_sub(waited);
    let reply = timing::complete(reply, processing, compressing, delay);
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

/// The largest body most requests may have: a form, a GraphQL request, the data of a session.
pub const BODY_LIMIT: usize = 32 << 20;

/// The largest body of an upload to a staged upload target: Shopify's largest staged upload, a
/// video ("Max 1 GB"; an image 20 MB, a 3D model 500 MB, a theme ZIP 50 MB), and a MiB for the
/// form around it.
pub const UPLOAD_BODY_LIMIT: usize = (1 << 30) + (1 << 20);

/// The largest body of `PUT /__lsf/admin/sources`: room for a source over 50 MB, for a test of
/// `ZIP_TOO_LARGE`.
pub const SOURCE_BODY_LIMIT: usize = lsf_app::admin::sources::MAX_SOURCE_BYTES + (1 << 20);

fn body_limit(path: &str) -> usize {
    if path.starts_with(lsf_app::admin::domains::app::uploads::UPLOADS_PATH) {
        UPLOAD_BODY_LIMIT
    } else if path == "/__lsf/admin/sources" {
        SOURCE_BODY_LIMIT
    } else {
        BODY_LIMIT
    }
}

#[derive(Debug, PartialEq)]
enum BodyRefused {
    /// More than the limit: `413`, rather than an empty body, which an upload took for an empty
    /// file.
    TooLarge,
    /// The connection failed while it was sent.
    Unreadable(String),
}

impl BodyRefused {
    fn into_response(self, limit: usize) -> Response<Body> {
        let (status, message) = match self {
            BodyRefused::TooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                format!(
                    "the body is larger than the {} MiB this path takes",
                    limit >> 20
                ),
            ),
            BodyRefused::Unreadable(why) => (
                StatusCode::BAD_REQUEST,
                format!("the body could not be read: {why}"),
            ),
        };
        Response::builder()
            .status(status)
            .header("content-type", "text/plain; charset=utf-8")
            .body(Body::from(message))
            .unwrap_or_else(|_| Response::new(Body::empty()))
    }
}

/// The body of a request, of `limit` bytes at most: one that says it is larger is refused before
/// it is read, one that turns out larger as soon as it passes the limit.
async fn read_body(
    body: Body,
    limit: usize,
    content_length: Option<&String>,
) -> Result<bytes::Bytes, BodyRefused> {
    use futures_util::StreamExt;
    if content_length
        .and_then(|length| length.trim().parse::<u64>().ok())
        .is_some_and(|length| length > limit as u64)
    {
        return Err(BodyRefused::TooLarge);
    }
    let mut stream = body.into_data_stream();
    let mut bytes = bytes::BytesMut::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| BodyRefused::Unreadable(error.to_string()))?;
        if bytes.len() + chunk.len() > limit {
            return Err(BodyRefused::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes.freeze())
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

/// How long the requests being answered when the server is asked to stop may still take: then
/// they are abandoned, and the server stops. A render ends at its own budget
/// (`lsf_liquid::RenderLimits`, 10 s) at the latest.
pub const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(3);

/// Serves the storefront until the process is interrupted.
pub async fn run(
    state: Arc<ServerState>,
    listener: tokio::net::TcpListener,
) -> std::io::Result<()> {
    run_until(state, listener, interrupted(), SHUTDOWN_DEADLINE).await
}

/// Serves the storefront until `stop` completes, then for the requests being answered, and for
/// `deadline` at most: what is still running then (a render that does not end) is abandoned.
/// A server that waited for it never stopped.
pub async fn run_until(
    state: Arc<ServerState>,
    listener: tokio::net::TcpListener,
    stop: impl std::future::Future<Output = ()> + Send + 'static,
    deadline: Duration,
) -> std::io::Result<()> {
    use std::future::IntoFuture;
    let quiet = state.options.quiet;
    let router = axum::Router::new()
        .route("/__lsf/livereload", axum::routing::get(live_reload))
        .fallback(handle)
        .with_state(state);
    let (stopping, mut stopped) = tokio::sync::watch::channel(false);
    let signal = async move {
        stop.await;
        let _ = stopping.send(true);
    };
    let serving = axum::serve(listener, router)
        .with_graceful_shutdown(signal)
        .into_future();
    let abandoned = async move {
        if stopped.wait_for(|stopped| *stopped).await.is_err() {
            std::future::pending::<()>().await;
        }
        tokio::time::sleep(deadline).await;
    };
    tokio::select! {
        result = serving => result,
        () = abandoned => {
            if !quiet {
                eprintln!(
                    "stopped: the requests still running {} s after the server was asked to stop were abandoned",
                    deadline.as_secs_f64()
                );
            }
            Ok(())
        }
    }
}

/// Waits for the process to be asked to stop: Ctrl-C, and on Unix `SIGTERM` (a process manager,
/// `kill`) and `SIGHUP` (the terminal closed). The server then stops as it does on Ctrl-C: with
/// `--watch`, the builds that run are stopped and the published modules removed. On Windows,
/// only Ctrl-C is waited for.
async fn interrupted() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut term), Ok(mut hangup)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::hangup()),
        ) else {
            let _ = tokio::signal::ctrl_c().await;
            return;
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
            _ = hangup.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
