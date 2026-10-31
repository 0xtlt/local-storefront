//! [`AdminApi`]: the shops, the tokens, the webhooks and the throttle of a server, and the one
//! function that answers their HTTP requests.

use std::sync::{Arc, Mutex, MutexGuard, RwLock};

use chrono::Utc;
use indexmap::IndexMap;
use lsf_checkout::app::App;
use lsf_core::Store;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::samples::{self, SampleError, ShopLookup};
use super::sync::{StoreSync, Synced};
use super::{HttpRequest, HttpResponse};
use crate::auth::oauth::{self, AppAuth};
use crate::auth::scopes::{self, ALL_SCOPES};
use crate::auth::tokens::{Grant, TokenError, Tokens};
use crate::domains::platform::{self, access::AuthEffect};
use crate::domains::{app as app_domain, orders};
use crate::graphql::{self, Bucket, Request};
use crate::resolve::{Registry, ShopResolver};
use crate::state::{Event, ShopState, Shops};
use crate::throttle::{self, Plan, Throttle};
use crate::version::{self, AdminPath, ApiVersion, Resolution, Served};
use crate::webhooks::{AppUrls, DeliveryOptions, Settings, TriggerError, TriggerRequest, Webhooks};

/// The answer to a request without a valid access token: Shopify's, word for word.
pub const INVALID_TOKEN: &str =
    "[API] Invalid API key or access token (unrecognized login or wrong password)";

/// The size of the cost bucket of every shop and app.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ThrottleSetting {
    /// The plan of the store data: Plus for a shop with companies (B2B is a Plus feature),
    /// Standard otherwise.
    #[default]
    Store,
    /// No throttle: a full bucket, nothing refused.
    Off,
    /// A plan's bucket.
    Plan(Plan),
}

impl ThrottleSetting {
    /// `off`, `store`, or a plan: `standard`, `advanced`, `plus`, `enterprise`, `legacy`.
    pub fn parse(text: &str) -> Result<ThrottleSetting, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "off" | "none" => Ok(ThrottleSetting::Off),
            "store" | "auto" | "" => Ok(ThrottleSetting::Store),
            name => Plan::from_name(name).map(ThrottleSetting::Plan).ok_or_else(|| {
                format!(
                    "`{text}` is not a throttle plan: off, store, standard, advanced, plus, enterprise or legacy"
                )
            }),
        }
    }
}

/// How the Admin API of a server is set up.
#[derive(Clone, Debug, Default)]
pub struct AdminOptions {
    /// The app of `--app`: its client id, scopes, webhooks and redirect URLs. Without it, lsf's
    /// local client id, every scope, and no subscription.
    pub app: Option<App>,
    /// What OAuth, session tokens and webhooks are signed with: [`crate::auth::client_secret`].
    pub client_secret: String,
    /// `--app-url`: where the backend listens.
    pub app_url: Option<String>,
    pub throttle: ThrottleSetting,
    pub delivery: DeliveryOptions,
    /// Makes the tokens OAuth issues the same every run (tests). The default tokens are always.
    pub token_seed: Option<u64>,
}

/// A shop of the server, behind its own lock.
struct LiveShop {
    state: ShopState,
    /// Whether the Admin API changed it since it was seeded: a change emits an event.
    changed: bool,
    /// Whether it is the default shop, the one store sync projects. It stays the default one
    /// when a reseed moves it to another domain.
    default: bool,
}

/// What a reseed dropped, for the server to say.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reseeded {
    /// The shops the Admin API had changed, whose changes are gone.
    pub dropped: Vec<String>,
}

/// The Admin API of a server: its shops (each behind a lock), the tokens, the webhooks of the
/// app, the throttle, and [`AdminApi::handle`], which answers the HTTP requests of all of it.
///
/// Locks are taken in one order: the seed, the tokens, the map of the shops, a shop, the
/// registry of the webhooks. None is taken while one that comes after it is held.
pub struct AdminApi {
    registry: &'static Registry,
    /// The seed every shop is made from.
    seed: Mutex<Shops>,
    /// The shops, by domain, the default one first.
    shops: RwLock<IndexMap<String, Arc<Mutex<LiveShop>>>>,
    tokens: Mutex<Tokens>,
    webhooks: Webhooks,
    throttle: Throttle,
    auth: AppAuth,
    app: Option<App>,
    /// The scopes the app is installed with: `[access_scopes] scopes`, every scope without an
    /// app.
    scopes: Vec<String>,
    /// Store sync of the default shop, once the server turns it on ([`AdminApi::sync_store`]).
    sync: StoreSync,
    /// The versions of the Admin API it serves ([`AdminApi::with_versions`]).
    versions: Served,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl AdminApi {
    /// The Admin API of the store data and the app: the default shop seeded, the app installed
    /// on it with its scopes and [`crate::auth::default_token`], the webhooks' worker started.
    pub fn new(store: &Store, options: AdminOptions) -> AdminApi {
        let AdminOptions {
            app,
            client_secret,
            app_url,
            throttle: setting,
            delivery,
            token_seed,
        } = options;
        let client_id = app_domain::client_id(app.as_ref());
        let scopes: Vec<String> = match &app {
            Some(app) => app.config.scopes.clone(),
            None => ALL_SCOPES.iter().map(|scope| scope.to_string()).collect(),
        };
        let auth = match &app {
            Some(app) => {
                AppAuth::from_app(&app.config, &client_id, &client_secret, app_url.as_deref())
            }
            None => AppAuth {
                client_id: client_id.clone(),
                client_secret: client_secret.clone(),
                scopes: scopes.clone(),
                redirect_urls: None,
                application_url: None,
                app_url: app_url.clone(),
            },
        };
        let webhooks = match &app {
            Some(app) => Webhooks::for_app(&app.config, app_url, client_secret, delivery),
            None => Webhooks::new(
                Vec::new(),
                Settings {
                    urls: AppUrls {
                        app_url,
                        application_url: None,
                    },
                    client_secret,
                    delivery,
                },
            ),
        };
        let webhooks = webhooks.with_filter(Arc::new(|filter: &str, payload: &Value| {
            crate::search::filter::webhook_filter_matches(
                filter,
                payload,
                &crate::search::Options::utc(Utc::now()),
            )
            .map_err(|error| error.to_string())
        }));
        let seed = Shops::new(store, app.as_ref());
        let default = seed
            .default_shop()
            .cloned()
            .unwrap_or_else(|| ShopState::seeded(store, app.as_ref()));
        let plan = match setting {
            ThrottleSetting::Plan(plan) => plan,
            _ if is_plus(&default) => Plan::PLUS,
            _ => Plan::STANDARD,
        };
        let throttle = Throttle::new(plan);
        throttle.set_enabled(setting != ThrottleSetting::Off);
        let mut tokens = token_seed.map_or_else(Tokens::new, Tokens::with_seed);
        tokens.install(default.domain(), &client_id, &scopes, Utc::now());
        let mut shops = IndexMap::new();
        shops.insert(
            default.domain().to_string(),
            Arc::new(Mutex::new(LiveShop {
                state: default,
                changed: false,
                default: true,
            })),
        );
        AdminApi {
            registry: Registry::standard(),
            seed: Mutex::new(seed),
            shops: RwLock::new(shops),
            tokens: Mutex::new(tokens),
            webhooks,
            throttle,
            auth,
            app,
            scopes,
            sync: StoreSync::default(),
            versions: Served::default(),
        }
    }

    /// Serves these versions of the Admin API (`--admin-versions`), for requests and for the
    /// payloads of webhooks: [`version::DEFAULT`] alone otherwise.
    pub fn with_versions(mut self, versions: Served) -> AdminApi {
        self.webhooks = self.webhooks.with_versions(versions.clone());
        self.versions = versions;
        self
    }

    /// The versions of the Admin API it serves.
    pub fn versions(&self) -> &Served {
        &self.versions
    }

    /// The path of the GraphQL Admin API of its primary version:
    /// `/admin/api/2026-10/graphql.json`.
    pub fn graphql_path(&self) -> String {
        format!("/admin/api/{}/graphql.json", self.versions.primary())
    }

    /// The domain of the default shop: `shop.permanent_domain` of the store data.
    pub fn default_shop(&self) -> String {
        self.shops
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .keys()
            .next()
            .cloned()
            .unwrap_or_default()
    }

    /// The client id of the app.
    pub fn client_id(&self) -> &str {
        &self.auth.client_id
    }

    /// The token the app is installed with on `shop`: [`crate::auth::default_token`].
    pub fn token(&self, shop: &str) -> String {
        crate::auth::default_token(shop, &self.auth.client_id)
    }

    /// The scopes the app is installed with when the server starts.
    pub fn scopes(&self) -> &[String] {
        &self.scopes
    }

    /// The webhooks of the app.
    pub fn webhooks(&self) -> &Webhooks {
        &self.webhooks
    }

    /// The throttle of the shops.
    pub fn throttle(&self) -> &Throttle {
        &self.throttle
    }

    fn shop(&self, domain: &str) -> Option<Arc<Mutex<LiveShop>>> {
        self.shops
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&domain.to_ascii_lowercase())
            .cloned()
    }

    fn all_shops(&self) -> Vec<(String, Arc<Mutex<LiveShop>>)> {
        self.shops
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(|(domain, shop)| (domain.clone(), shop.clone()))
            .collect()
    }

    /// Reads the default shop.
    pub fn with_default_shop<T>(&self, read: impl FnOnce(&ShopState) -> T) -> Option<T> {
        let shop = self.shop(&self.default_shop())?;
        let live = lock(&shop);
        Some(read(&live.state))
    }

    /// Changes a shop, then delivers the webhooks of what it emitted, the payloads built from the
    /// shop as the change left it. A change of the default shop is counted for store sync.
    pub fn change_shop<T>(
        &self,
        domain: &str,
        change: impl FnOnce(&mut ShopState) -> T,
    ) -> Option<T> {
        let shop = self.shop(domain)?;
        let mut live = lock(&shop);
        let revision = live.state.revision();
        let result = change(&mut live.state);
        self.count_change(&live, revision);
        let events = live.state.take_events();
        if !events.is_empty() {
            live.changed = true;
            self.emit(&live.state, &events);
        }
        Some(result)
    }

    /// Counts a change of the default shop for store sync, when a record of it was written since
    /// `revision`: called while the shop is locked.
    fn count_change(&self, live: &LiveShop, revision: u64) {
        if live.default && live.state.revision() != revision {
            self.sync.changed();
        }
    }

    /// Turns store sync on: from now on, what the Admin API changes in the default shop is
    /// projected onto `base`, the store the shops were seeded from (the files, without any
    /// overlay), when [`AdminApi::synced`] asks for it. A server calls it once the API is made,
    /// and again after each [`AdminApi::reseed`], with the store it reseeded from.
    pub fn sync_store(&self, base: Arc<Store>) {
        let Some(shop) = self.shop(&self.default_shop()) else {
            return;
        };
        let _live = lock(&shop);
        self.sync.start(base);
    }

    /// How many times the default shop changed (an Admin API request or a recorded order that
    /// wrote a record, a reset, a reseed): cheap, so that a server can ask it on every request,
    /// and call [`AdminApi::synced`] only when it moved.
    pub fn store_changes(&self) -> u64 {
        self.sync.changes()
    }

    /// What the Admin API changed in the default shop, as an overlay of the store data, made
    /// again (while the shop is locked) when the shop changed since it was last made. `None`
    /// while store sync is off. The other shops never sync: they isolate tests.
    pub fn synced(&self) -> Option<Synced> {
        if !self.sync.is_on() {
            return None;
        }
        let shop = self.shop(&self.default_shop())?;
        let live = lock(&shop);
        self.sync.update(&live.state)
    }

    /// Notes what the server built with the overlay of `generation`, for `GET /__lsf/admin`: how
    /// long it took, and what the store data said of the overlay.
    pub fn sync_built(
        &self,
        generation: u64,
        took: std::time::Duration,
        diagnostics: &lsf_core::diagnostics::Diagnostics,
    ) {
        self.sync.built(generation, took, diagnostics);
    }

    /// Records an order placed in the local checkout in the default shop, and delivers its
    /// `orders/create` (and `orders/paid`) webhooks. Returns the GID and the number the shop
    /// gave the order (its next one when the checkout gives none).
    pub fn record_order(&self, placed: &orders::PlacedOrder) -> Option<orders::RecordedOrder> {
        self.change_shop(&self.default_shop(), |state| {
            orders::record_placed_order(state, placed)
        })
    }

    /// Seeds every shop again from new store data (the data changed on disk). The webhook
    /// subscriptions the backend made stay: they are the app's setup of the shop, not data.
    pub fn reseed(&self, store: &Store) -> Reseeded {
        let mut seed = lock(&self.seed);
        let old_default = seed.default_domain().to_string();
        seed.reseed(store, self.app.as_ref());
        let new_default = seed.default_domain().to_string();
        let mut dropped = Vec::new();
        let mut shops = self
            .shops
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if new_default != old_default
            && let Some(shop) = shops.shift_remove(&old_default)
        {
            if let Some(fresh) = seed.default_shop().cloned() {
                let mut live = lock(&shop);
                if live.changed {
                    dropped.push(old_default.clone());
                }
                live.state = fresh;
                live.changed = false;
            }
            shops.insert(new_default.clone(), shop);
            let last = shops.len() - 1;
            shops.move_index(last, 0);
        }
        for (domain, shop) in shops.iter() {
            if *domain == new_default && new_default != old_default {
                continue;
            }
            let mut live = lock(shop);
            if live.changed {
                dropped.push(domain.clone());
            }
            seed.reset_shop(&mut live.state);
            live.changed = false;
        }
        drop(shops);
        drop(seed);
        if new_default != old_default {
            lock(&self.tokens).install(
                &new_default,
                &self.auth.client_id,
                &self.scopes,
                Utc::now(),
            );
        }
        Reseeded { dropped }
    }

    /// Puts every shop back to the seed, forgets the subscriptions the backend made and the
    /// tokens OAuth issued, and installs the app again on every shop: `DELETE /__lsf/admin`.
    pub fn reset(&self) -> Vec<String> {
        let seed = lock(&self.seed);
        let mut tokens = lock(&self.tokens);
        let mut domains = Vec::new();
        for (domain, shop) in self.all_shops() {
            let mut live = lock(&shop);
            seed.reset_shop(&mut live.state);
            live.changed = false;
            if live.default {
                self.sync.changed();
            }
            tokens.revoke_shop(&domain);
            tokens.install(&domain, &self.auth.client_id, &self.scopes, Utc::now());
            domains.push(domain);
        }
        self.webhooks.registry().clear_all();
        self.throttle.reset();
        domains
    }

    /// Creates a shop seeded from the same data, with the app installed: its domain and token.
    pub fn create_shop(&self, name: &str) -> Result<(String, String), (u16, String)> {
        let shop = lock(&self.seed)
            .new_shop(name)
            .map_err(|error| (422, error))?;
        let domain = shop.domain().to_string();
        {
            let mut shops = self
                .shops
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if shops.contains_key(&domain) {
                return Err((409, format!("the shop {domain} already exists")));
            }
            shops.insert(
                domain.clone(),
                Arc::new(Mutex::new(LiveShop {
                    state: shop,
                    changed: false,
                    default: false,
                })),
            );
        }
        // The map of the shops is released before the tokens are locked (see `AdminApi`).
        let token =
            lock(&self.tokens).install(&domain, &self.auth.client_id, &self.scopes, Utc::now());
        Ok((domain, token))
    }

    /// Tells the webhooks the shops the app was installed on again, as `tokens` has them: the
    /// subscriptions of `shopify.app.toml` deliver for them again. Called with the tokens locked,
    /// which come before the registry of the webhooks.
    fn note_installs(&self, tokens: &Tokens) {
        let mut registry = self.webhooks.registry();
        for shop in registry.uninstalled_shops() {
            if tokens
                .installed_scopes(&shop, &self.auth.client_id)
                .is_some()
            {
                registry.set_installed(&shop, true);
            }
        }
    }

    /// Delivers the webhooks of events of a shop.
    fn emit(&self, state: &ShopState, events: &[Event]) {
        let lookup = ShopLookup(state);
        for event in events {
            self.webhooks.emit(event, &lookup);
        }
    }

    /// The answer to a request of the Admin API, of OAuth, or of the control endpoints of the
    /// Admin API and of the webhooks; `None` for any other path.
    ///
    /// | Path | What answers |
    /// |---|---|
    /// | `/admin/api/<version>/graphql.json` | the GraphQL Admin API of a version it serves; one Shopify no longer serves falls forward; another is a `404` |
    /// | `/admin/api/<version>/<other>.json` | `404`: the REST Admin API is not served |
    /// | `/admin/oauth/*`, `/__lsf/admin/session-token`, `/__lsf/admin/launch` | [`oauth::handle`] |
    /// | `/__lsf/admin`, `/__lsf/admin/shops`, `/__lsf/admin/install`, `/__lsf/admin/uninstall` | the control endpoints |
    /// | `/__lsf/webhooks`, `/__lsf/webhooks/trigger` | [`Webhooks::handle`] |
    pub fn handle(&self, request: &HttpRequest) -> Option<HttpResponse> {
        let path = request.path.as_str();
        if let Some(admin) = version::parse_path(path) {
            let primary = self.versions.primary();
            return Some(match admin {
                AdminPath::Graphql(asked) => match self.versions.resolve(asked) {
                    Resolution::Served(version) => self.graphql(request, version),
                    // As Shopify: the oldest supported version answers, and says so in
                    // `X-Shopify-API-Version`.
                    Resolution::FellForward { asked, served } => {
                        let response = self.graphql(request, served);
                        if response
                            .headers
                            .iter()
                            .any(|(name, _)| name == "x-lsf-reason")
                        {
                            response
                        } else {
                            response.header(
                                "x-lsf-reason",
                                format!(
                                    "API version {asked} is no longer supported: {served} answered, \
                                     as Shopify falls forward to the oldest supported version"
                                ),
                            )
                        }
                    }
                    Resolution::NotServed(body) => {
                        Self::api_headers(HttpResponse::json(404, &body), primary)
                    }
                },
                AdminPath::UnsupportedVersion(asked) => Self::api_headers(
                    HttpResponse::json(404, &self.versions.refusal(asked)),
                    primary,
                ),
                AdminPath::Rest { resource, .. } => Self::api_headers(
                    HttpResponse::json(404, &version::rest_not_served_by(&self.versions, resource)),
                    primary,
                ),
            });
        }
        if path.starts_with("/admin/oauth/")
            || path.trim_end_matches('/') == "/__lsf/admin/session-token"
            || path.trim_end_matches('/') == "/__lsf/admin/launch"
        {
            let names = ShopNames(self);
            let cx = oauth::Context {
                app: &self.auth,
                shops: &names,
                now: Utc::now(),
            };
            let mut tokens = lock(&self.tokens);
            let response = oauth::handle(request, &cx, &mut tokens);
            // An authorization or a token exchange installs the app where it was not.
            self.note_installs(&tokens);
            return response;
        }
        let trimmed = path.trim_end_matches('/');
        if trimmed == "/__lsf/admin" || trimmed.starts_with("/__lsf/admin/") {
            return Some(self.control(request, trimmed));
        }
        if trimmed == "/__lsf/webhooks" || trimmed.starts_with("/__lsf/webhooks/") {
            return self
                .webhooks
                .handle(request, &|trigger| self.build_trigger(trigger));
        }
        None
    }

    /// The headers of every answer of the Admin API: `X-Shopify-API-Version`, the version that
    /// answered.
    fn api_headers(response: HttpResponse, version: ApiVersion) -> HttpResponse {
        response
            .header("x-shopify-api-version", version.name())
            .header("x-request-id", crate::webhooks::sign::new_id())
    }

    /// `POST /admin/api/<version>/graphql.json`, in `version`: its schema, its access scopes, its
    /// search filters, its adapters.
    fn graphql(&self, request: &HttpRequest, version: ApiVersion) -> HttpResponse {
        let method = request.method.to_ascii_uppercase();
        if method != "POST" && method != "GET" {
            return Self::api_headers(
                HttpResponse::json(
                    405,
                    &json!({"errors": format!("{method} is not allowed: POST the query (or GET it with ?query=)")}),
                ),
                version,
            );
        }
        let grant = match self.grant_of(request) {
            Ok(grant) => grant,
            Err(reason) => {
                return Self::api_headers(
                    HttpResponse::json(401, &json!({ "errors": INVALID_TOKEN })),
                    version,
                )
                .header("x-lsf-reason", reason);
            }
        };
        let query = match graphql_request(request) {
            Ok(query) => query,
            Err(reason) => {
                return Self::api_headers(
                    HttpResponse::json(
                        400,
                        &json!({"errors": {"query": "Required parameter missing or invalid"}}),
                    ),
                    version,
                )
                .header("x-lsf-reason", reason);
            }
        };
        let Some(shop) = self.shop(&grant.shop) else {
            return Self::api_headers(
                HttpResponse::json(401, &json!({ "errors": INVALID_TOKEN })),
                version,
            )
            .header(
                "x-lsf-reason",
                format!("the shop {} of this token is gone", grant.shop),
            );
        };
        let key = throttle::key(&grant.shop, &grant.client_id);
        let mut live = lock(&shop);
        let origin = origin_of(request);
        let revision = live.state.revision();
        let mut resolver = ShopResolver::new(&mut live.state, self.registry)
            .with_version(version)
            .with_grant(&grant)
            .with_webhooks(&self.webhooks)
            .with_origin(&origin);
        let bucket = Bucket {
            throttle: &self.throttle,
            key: &key,
        };
        let mut response = graphql::run(version.schema(), &mut resolver, &query, Some(bucket));
        response.extensions.extend(resolver.extensions());
        self.count_change(&live, revision);
        let events = live.state.take_events();
        let effects = platform::access::effects(&events);
        if !events.is_empty() {
            live.changed = true;
            // Built now from the shop as the request left it, queued for the worker: nothing
            // waits on the backend while the shop is locked.
            self.emit(&live.state, &events);
        }
        drop(live);
        // What the request changed of the tokens, now that the shop is unlocked (the tokens
        // come first in the order of the locks), before the backend hears the answer.
        self.apply_auth_effects(&grant.shop, &effects);
        Self::api_headers(HttpResponse::json(200, &response.to_json()), version)
    }

    /// What the request's `X-Shopify-Access-Token` grants, or why it grants nothing.
    fn grant_of(&self, request: &HttpRequest) -> Result<Grant, String> {
        let Some(token) = request
            .header("x-shopify-access-token")
            .map(str::trim)
            .filter(|token| !token.is_empty())
        else {
            return Err("no X-Shopify-Access-Token header: send the token the server printed when it started".to_string());
        };
        match lock(&self.tokens).lookup(token, Utc::now()) {
            Ok(grant) => Ok(grant.clone()),
            Err(TokenError::Expired) => Err("the access token expired".to_string()),
            Err(TokenError::Unknown) => Err(
                "lsf did not issue this access token, or it was revoked (the app was uninstalled, or the server reset)"
                    .to_string(),
            ),
        }
    }

    /// The shop and the payload of `POST /__lsf/webhooks/trigger`: the payload from the shop's
    /// records, unless the trigger gives its own. A given payload may be for a shop lsf does not
    /// have (sent to an `address`).
    fn build_trigger(&self, trigger: &TriggerRequest) -> Result<(String, Value), TriggerError> {
        let domain = match &trigger.shop {
            Some(shop) => oauth::normalize_shop(shop),
            None => self.default_shop(),
        };
        if let Some(payload) = &trigger.payload {
            return Ok((domain, payload.clone()));
        }
        let shop = self
            .shop(&domain)
            .ok_or_else(|| TriggerError::NotFound(format!("there is no shop {domain}")))?;
        let live = lock(&shop);
        samples::payload_of(&live.state, &trigger.topic, trigger.id.as_deref())
            .map(|payload| (domain, payload))
            .map_err(|error| match error {
                SampleError::NotFound(message) => TriggerError::NotFound(message),
                SampleError::NotBuilt(message) => TriggerError::NotBuilt(message),
            })
    }

    /// The control endpoints under `/__lsf/admin`.
    fn control(&self, request: &HttpRequest, path: &str) -> HttpResponse {
        if let Some(response) = super::app::handle(self, request, path) {
            return response;
        }
        let method = request.method.to_ascii_uppercase();
        match (method.as_str(), path) {
            ("GET", "/__lsf/admin") => HttpResponse::json(200, &self.status()),
            ("DELETE", "/__lsf/admin") => {
                let shops = self.reset();
                HttpResponse::json(200, &json!({"ok": true, "shops": shops}))
            }
            ("POST", "/__lsf/admin/shops") => {
                let body: ShopBody = match body_of(request) {
                    Ok(body) => body,
                    Err(response) => return response,
                };
                let Some(name) = body.shop else {
                    return HttpResponse::json(
                        422,
                        &json!({"error": "the body is {\"shop\": \"test-1\"}"}),
                    );
                };
                match self.create_shop(&name) {
                    Ok((domain, token)) => HttpResponse::json(
                        201,
                        &json!({
                            "shop": domain,
                            "token": token,
                            "graphql": self.graphql_path(),
                        }),
                    ),
                    Err((status, error)) => HttpResponse::json(status, &json!({ "error": error })),
                }
            }
            ("GET", "/__lsf/admin/outbox") => {
                let domain = request
                    .query
                    .iter()
                    .find(|(name, _)| name == "shop")
                    .map_or_else(
                        || self.default_shop(),
                        |(_, shop)| oauth::normalize_shop(shop),
                    );
                match self.outbox(&domain) {
                    Some(entries) => {
                        HttpResponse::json(200, &json!({"shop": domain, "outbox": entries}))
                    }
                    None => HttpResponse::json(
                        404,
                        &json!({"error": format!("there is no shop {domain}")}),
                    ),
                }
            }
            ("POST", "/__lsf/admin/uninstall") => self.uninstall(request),
            ("POST", "/__lsf/admin/install") => self.install(request),
            (
                _,
                "/__lsf/admin"
                | "/__lsf/admin/shops"
                | "/__lsf/admin/uninstall"
                | "/__lsf/admin/install",
            ) => HttpResponse::json(
                405,
                &json!({"error": format!("{method} is not allowed here")}),
            ),
            _ => HttpResponse::json(
                404,
                &json!({"error": format!("no control endpoint at {path}")}),
            ),
        }
    }

    /// `GET /__lsf/admin`.
    pub fn status(&self) -> Value {
        let now = Utc::now();
        let all_shops = self.all_shops();
        // What the tokens say of each shop, copied so the tokens are not locked while a shop
        // is: a shop busy with a long mutation would hold up every request's authentication.
        let mut tokens_of: std::collections::HashMap<String, (Option<Vec<String>>, Vec<Value>)> = {
            let tokens = lock(&self.tokens);
            all_shops
                .iter()
                .map(|(domain, _)| {
                    let installed = tokens
                        .installed_scopes(domain, &self.auth.client_id)
                        .map(<[String]>::to_vec);
                    let grants = tokens
                        .grants()
                        .filter(|(_, grant)| grant.shop == *domain && !grant.expired(now))
                        .map(|(token, grant)| {
                            json!({
                                "token": token,
                                "mode": grant.mode,
                                "source": grant.source,
                                "scopes": grant.scopes.join(","),
                                "expiresAt": grant.expires_at.map(|at| at.to_rfc3339()),
                                "user": grant.user.as_ref().map(|user| user.id),
                            })
                        })
                        .collect();
                    (domain.clone(), (installed, grants))
                })
                .collect()
        };
        let shops: Vec<Value> = all_shops
            .into_iter()
            .map(|(domain, shop)| {
                let (installed, grants) = tokens_of.remove(&domain).unwrap_or_default();
                let live = lock(&shop);
                let mut counts: Vec<(&str, usize)> = live
                    .state
                    .types()
                    .map(|type_name| (type_name, live.state.count(type_name)))
                    .collect();
                counts.sort();
                let subscriptions: Vec<Value> = self
                    .webhooks
                    .registry()
                    .list(&domain)
                    .iter()
                    .map(|subscription| json!(subscription))
                    .collect();
                json!({
                    "shop": domain,
                    "installed": installed.is_some(),
                    "scopes": installed,
                    "token": self.token(&domain),
                    "changed": live.changed,
                    "records": live.state.len(),
                    "counts": counts.into_iter().collect::<IndexMap<_, _>>(),
                    "tokens": grants,
                    "subscriptions": subscriptions,
                })
            })
            .collect();
        let plan = self.throttle.plan();
        json!({
            "apiVersion": self.versions.primary().name(),
            "graphql": self.graphql_path(),
            "apiVersions": ApiVersion::all()
                .map(|version| json!({
                    "version": version.name(),
                    "status": version.status().as_str(),
                    "served": self.versions.serves(version),
                    "graphql": format!("/admin/api/{version}/graphql.json"),
                }))
                .collect::<Vec<_>>(),
            "defaultShop": self.default_shop(),
            "app": {
                "clientId": self.auth.client_id,
                "appUrl": self.auth.app_url(),
                "scopes": self.scopes,
            },
            "throttle": {
                "enabled": self.throttle.is_enabled(),
                "maximumAvailable": plan.maximum_available,
                "restoreRate": plan.restore_rate,
            },
            "webhooks": {
                "summary": self.webhooks.summary(),
                "appSubscriptions": self.webhooks.app_subscriptions(),
                "deliveries": self.webhooks.deliveries().list(&Default::default()).len(),
            },
            "mutations": self.registry.mutations(),
            "storeSync": self.sync.status(),
            "shops": shops,
        })
    }

    /// `POST /__lsf/admin/uninstall`: the app's tokens on the shop are revoked, `app/uninstalled`
    /// is delivered, then the subscriptions the backend made on the shop are deleted and nothing
    /// more is delivered for it until the app is installed again (`POST /__lsf/admin/install`,
    /// OAuth, a token exchange), which brings back those of `shopify.app.toml`.
    ///
    /// As Shopify does: "When an app uninstalls, Shopify automatically performs cleanup tasks,
    /// such as deleting WebhookSubscription objects and admin links associated with the app"
    /// (shopify.dev, `appUninstall`,
    /// <https://shopify.dev/docs/api/admin-graphql/2026-10/mutations/appUninstall>), and the
    /// subscriptions of `shopify.app.toml` are "applied uniformly across every shop that
    /// installs your app" (<https://shopify.dev/docs/apps/build/webhooks/subscribe>).
    /// `shop/redact` comes "48 hours after a store owner uninstalls your app"
    /// (<https://shopify.dev/docs/apps/build/compliance/privacy-law-compliance>): lsf does not
    /// wait, it is sent on demand (`POST /__lsf/webhooks/trigger`).
    fn uninstall(&self, request: &HttpRequest) -> HttpResponse {
        let body: InstallBody = match body_of(request) {
            Ok(body) => body,
            Err(response) => return response,
        };
        let domain = body
            .shop
            .as_deref()
            .map_or_else(|| self.default_shop(), oauth::normalize_shop);
        if self.shop(&domain).is_none() {
            return HttpResponse::json(
                404,
                &json!({"error": format!("there is no shop {domain}")}),
            );
        }
        let installed = lock(&self.tokens)
            .installed_scopes(&domain, &self.auth.client_id)
            .is_some();
        let (revoked, deleted) = self.finish_uninstall(&domain, || {
            if installed {
                self.change_shop(&domain, |state| {
                    // Shopify cancels the app's subscription when it is uninstalled.
                    app_domain::billing::cancel_on_uninstall(state);
                    if let Some(shop) = state.shop_gid().map(str::to_string) {
                        state.emit("app/uninstalled", &shop);
                    }
                });
            }
        });
        HttpResponse::json(
            200,
            &json!({"ok": true, "shop": domain, "revoked": revoked, "deletedSubscriptions": deleted}),
        )
    }

    /// The uninstall of the app from a shop, whether the control endpoint or `appUninstall` asked
    /// for it: its tokens revoked, then `delivered` (what delivers `app/uninstalled` through the
    /// subscriptions the shop had, if it was not delivered already), then nothing more is
    /// delivered for the shop and the subscriptions it made are deleted. Answers how many tokens
    /// were revoked and subscriptions deleted.
    fn finish_uninstall(&self, domain: &str, delivered: impl FnOnce()) -> (usize, usize) {
        let revoked = lock(&self.tokens).uninstall(domain, &self.auth.client_id);
        // From now on only `app/uninstalled` is delivered for the shop, through the
        // subscriptions it had; an app uninstalled already is not uninstalled again.
        self.webhooks.registry().set_installed(domain, false);
        delivered();
        let deleted = self.webhooks.registry().clear(domain);
        (revoked, deleted)
    }

    /// Applies to the tokens what the mutations of a request changed of them
    /// (`domains::platform::access`): an uninstall, the installation's new scopes, a delegate
    /// token made or destroyed.
    fn apply_auth_effects(&self, shop: &str, effects: &[AuthEffect]) {
        for effect in effects {
            match effect {
                AuthEffect::Uninstall => {
                    // `appUninstall` delivered `app/uninstalled` with the request's events.
                    self.finish_uninstall(shop, || {});
                }
                AuthEffect::Scopes(scopes) => {
                    lock(&self.tokens).set_scopes(shop, &self.auth.client_id, scopes);
                }
                AuthEffect::Delegate {
                    token,
                    scopes,
                    expires_at,
                } => lock(&self.tokens).delegate(
                    token,
                    shop,
                    &self.auth.client_id,
                    scopes,
                    *expires_at,
                    Utc::now(),
                ),
                AuthEffect::Destroy { token } => {
                    lock(&self.tokens).revoke(token);
                }
            }
        }
    }

    /// What the app platform's mutations would have sent outside (a Flow trigger, …): the
    /// outbox of a shop, oldest first (`GET /__lsf/admin/outbox?shop=`). `None` for a shop
    /// lsf does not have.
    pub fn outbox(&self, shop: &str) -> Option<Vec<Value>> {
        let shop = self.shop(shop)?;
        let live = lock(&shop);
        Some(platform::outbox_entries(&live.state))
    }

    /// The association file the storefront serves at `path` (`/.well-known/assetlinks.json`,
    /// `/.well-known/apple-app-site-association`) from the mobile platform applications of the
    /// default shop: `None` when there is none to serve.
    pub fn association_file(&self, path: &str) -> Option<Value> {
        self.with_default_shop(|state| platform::mobile::association_file(state, path))
            .flatten()
    }

    /// The `src` of the script tags of the default shop that the storefront loads, in order
    /// (`domains::storefront::storefront_scripts`).
    pub fn storefront_scripts(&self) -> Vec<String> {
        self.with_default_shop(crate::domains::storefront::storefront_scripts)
            .unwrap_or_default()
    }

    /// `POST /__lsf/admin/install`: installs the app again (its token works again), with other
    /// scopes when the body gives some; `app/scopes_update` is delivered when they changed.
    fn install(&self, request: &HttpRequest) -> HttpResponse {
        let body: InstallBody = match body_of(request) {
            Ok(body) => body,
            Err(response) => return response,
        };
        let domain = body
            .shop
            .as_deref()
            .map_or_else(|| self.default_shop(), oauth::normalize_shop);
        if self.shop(&domain).is_none() {
            return HttpResponse::json(
                404,
                &json!({"error": format!("there is no shop {domain}")}),
            );
        }
        let scopes = match body.scopes {
            None => self.scopes.clone(),
            Some(Value::String(list)) => scopes::parse_scopes(&list),
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
            Some(_) => {
                return HttpResponse::json(
                    422,
                    &json!({"error": "`scopes` is a list of scopes, or a comma-separated string"}),
                );
            }
        };
        if let Some(bad) = scopes.iter().find(|scope| !scopes::is_scope_name(scope)) {
            return HttpResponse::json(
                422,
                &json!({"error": format!("`{bad}` is not an access scope")}),
            );
        }
        let (token, before) = {
            let mut tokens = lock(&self.tokens);
            let before = tokens
                .installed_scopes(&domain, &self.auth.client_id)
                .map(<[String]>::to_vec);
            let token = tokens.install(&domain, &self.auth.client_id, &scopes, Utc::now());
            self.note_installs(&tokens);
            (token, before)
        };
        let changed = before
            .as_ref()
            .is_some_and(|before| scopes::expand(before) != scopes::expand(&scopes));
        let handles: Vec<Value> = scopes
            .iter()
            .map(|scope| json!({"handle": scope}))
            .collect();
        if let Some(shop) = self.shop(&domain) {
            let mut live = lock(&shop);
            let installation = live
                .state
                .keys("AppInstallation")
                .next()
                .map(str::to_string);
            if let Some(installation) = installation {
                let previous = live.state.get(&installation).cloned();
                if let Some(record) = live.state.get_mut(&installation) {
                    record.insert("accessScopes".into(), Value::Array(handles));
                }
                if changed {
                    live.state.emit("app/scopes_update", &installation);
                    let mut events = live.state.take_events();
                    // The payload reads the scopes before the change from the event's snapshot.
                    if let Some(event) = events.last_mut() {
                        event.snapshot = previous;
                    }
                    live.changed = true;
                    self.emit(&live.state, &events);
                }
            }
        }
        HttpResponse::json(
            200,
            &json!({"ok": true, "shop": domain, "token": token, "scopes": scopes, "scopesChanged": changed}),
        )
    }
}

/// Where a request reached lsf, `http://<Host>`: the origin of the URLs lsf answers that point
/// back at itself (a charge's confirmation page, an upload target, a bulk operation's file).
/// `X-Forwarded-Proto` says the scheme behind a proxy; `X-Forwarded-Host` does not move the
/// host, since clients send the shop's domain in it (`docs/admin.md`).
pub(super) fn origin_of(request: &HttpRequest) -> String {
    let host = request
        .header("host")
        .map(str::trim)
        .filter(|host| !host.is_empty());
    match host {
        Some(host) => {
            let scheme = request
                .header("x-forwarded-proto")
                .filter(|scheme| matches!(*scheme, "http" | "https"))
                .unwrap_or("http");
            format!("{scheme}://{host}")
        }
        None => crate::resolve::DEFAULT_ORIGIN.to_string(),
    }
}

/// Whether the shop is on Shopify Plus: its `plan.shopifyPlus`.
fn is_plus(shop: &ShopState) -> bool {
    shop.shop()
        .and_then(|record| record.get("plan"))
        .and_then(|plan| plan.get("shopifyPlus"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The shops, as OAuth sees them.
struct ShopNames<'a>(&'a AdminApi);

impl oauth::Shops for ShopNames<'_> {
    fn exists(&self, shop: &str) -> bool {
        self.0.shop(shop).is_some()
    }

    fn default_shop(&self) -> String {
        self.0.default_shop()
    }

    fn locale(&self, shop: &str) -> String {
        self.0
            .shop(shop)
            .and_then(|shop| {
                let live = lock(&shop);
                live.state
                    .all("ShopLocale")
                    .find(|locale| locale.get("primary").and_then(Value::as_bool) == Some(true))
                    .and_then(|locale| locale.get("locale").and_then(Value::as_str))
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "en".to_string())
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ShopBody {
    shop: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallBody {
    shop: Option<String>,
    scopes: Option<Value>,
}

/// The JSON body of a control request; an empty body is the default.
fn body_of<T: Default + for<'de> Deserialize<'de>>(
    request: &HttpRequest,
) -> Result<T, HttpResponse> {
    if request.body.iter().all(u8::is_ascii_whitespace) {
        return Ok(T::default());
    }
    serde_json::from_slice(&request.body).map_err(|error| {
        HttpResponse::json(
            422,
            &json!({"error": format!("the body is not what this endpoint takes: {error}")}),
        )
    })
}

/// The GraphQL request of an HTTP request: a JSON body `{"query", "variables",
/// "operationName"}`, a body of `application/graphql` that is the query, or `?query=` (with
/// `?variables=` and `?operationName=`) of a `GET`.
pub(super) fn graphql_request(request: &HttpRequest) -> Result<Request, String> {
    let content_type = request
        .header("content-type")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let (query, variables, operation_name) = if request.method.eq_ignore_ascii_case("GET")
        || (request.body.is_empty() && request.query_param("query").is_some())
    {
        let variables = match request.query_param("variables") {
            Some(text) if !text.trim().is_empty() => serde_json::from_str(text)
                .map_err(|error| format!("?variables= is not JSON: {error}"))?,
            _ => Value::Null,
        };
        (
            request.query_param("query").map(str::to_string),
            variables,
            request.query_param("operationName").map(str::to_string),
        )
    } else if content_type.starts_with("application/graphql") {
        (
            Some(String::from_utf8_lossy(&request.body).into_owned()),
            Value::Null,
            None,
        )
    } else {
        let body: Value = serde_json::from_slice(&request.body)
            .map_err(|error| format!("the body is not JSON: {error}"))?;
        let field = |name: &str| body.get(name).and_then(Value::as_str).map(str::to_string);
        (
            field("query"),
            body.get("variables").cloned().unwrap_or(Value::Null),
            field("operationName"),
        )
    };
    let query = query
        .filter(|query| !query.trim().is_empty())
        .ok_or_else(|| "no query: send {\"query\": \"...\"}".to_string())?;
    let variables = match variables {
        Value::Object(map) => map,
        Value::String(text) if !text.trim().is_empty() => match serde_json::from_str(&text) {
            Ok(Value::Object(map)) => map,
            _ => return Err("`variables` is not a JSON object".to_string()),
        },
        Value::Null | Value::String(_) => Map::new(),
        _ => return Err("`variables` is not a JSON object".to_string()),
    };
    Ok(Request {
        query,
        variables,
        operation_name: operation_name.filter(|name| !name.is_empty()),
    })
}
