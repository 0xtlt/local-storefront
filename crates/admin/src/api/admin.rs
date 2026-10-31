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
use super::{HttpRequest, HttpResponse};
use crate::auth::oauth::{self, AppAuth};
use crate::auth::scopes::{self, ALL_SCOPES};
use crate::auth::tokens::{Grant, TokenError, Tokens};
use crate::domains::{app as app_domain, orders};
use crate::graphql::{self, Bucket, Request};
use crate::resolve::{Registry, ShopResolver};
use crate::state::{Event, ShopState, Shops};
use crate::throttle::{self, Plan, Throttle};
use crate::version::{self, ADMIN_2026_10, AdminPath};
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
        }
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
    /// shop as the change left it.
    pub fn change_shop<T>(
        &self,
        domain: &str,
        change: impl FnOnce(&mut ShopState) -> T,
    ) -> Option<T> {
        let shop = self.shop(domain)?;
        let mut live = lock(&shop);
        let result = change(&mut live.state);
        let events = live.state.take_events();
        if !events.is_empty() {
            live.changed = true;
            self.emit(&live.state, &events);
        }
        Some(result)
    }

    /// Records an order placed in the local checkout in the default shop, and delivers its
    /// `orders/create` (and `orders/paid`) webhooks. Returns the GID of the order.
    pub fn record_order(&self, placed: &orders::PlacedOrder) -> Option<String> {
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
                })),
            );
        }
        // The map of the shops is released before the tokens are locked (see `AdminApi`).
        let token =
            lock(&self.tokens).install(&domain, &self.auth.client_id, &self.scopes, Utc::now());
        Ok((domain, token))
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
    /// | `/admin/api/<version>/graphql.json` | the GraphQL Admin API, 2026-10 |
    /// | `/admin/api/<version>/<other>.json` | `404`: the REST Admin API is not served |
    /// | `/admin/oauth/*`, `/__lsf/admin/session-token`, `/__lsf/admin/launch` | [`oauth::handle`] |
    /// | `/__lsf/admin`, `/__lsf/admin/shops`, `/__lsf/admin/install`, `/__lsf/admin/uninstall` | the control endpoints |
    /// | `/__lsf/webhooks`, `/__lsf/webhooks/trigger` | [`Webhooks::handle`] |
    pub fn handle(&self, request: &HttpRequest) -> Option<HttpResponse> {
        let path = request.path.as_str();
        if let Some(admin) = version::parse_path(path) {
            return Some(match admin {
                AdminPath::Graphql(_) => self.graphql(request),
                AdminPath::UnsupportedVersion(version) => self.api_headers(HttpResponse::json(
                    404,
                    &version::unsupported_version(version),
                )),
                AdminPath::Rest { resource, .. } => {
                    self.api_headers(HttpResponse::json(404, &version::rest_not_served(resource)))
                }
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
            return oauth::handle(request, &cx, &mut lock(&self.tokens));
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

    /// The headers of every answer of the Admin API.
    fn api_headers(&self, response: HttpResponse) -> HttpResponse {
        response
            .header("x-shopify-api-version", ADMIN_2026_10)
            .header("x-request-id", crate::webhooks::sign::new_id())
    }

    /// `POST /admin/api/2026-10/graphql.json`.
    fn graphql(&self, request: &HttpRequest) -> HttpResponse {
        let method = request.method.to_ascii_uppercase();
        if method != "POST" && method != "GET" {
            return self.api_headers(HttpResponse::json(
                405,
                &json!({"errors": format!("{method} is not allowed: POST the query (or GET it with ?query=)")}),
            ));
        }
        let grant = match self.grant_of(request) {
            Ok(grant) => grant,
            Err(reason) => {
                return self
                    .api_headers(HttpResponse::json(401, &json!({ "errors": INVALID_TOKEN })))
                    .header("x-lsf-reason", reason);
            }
        };
        let query = match graphql_request(request) {
            Ok(query) => query,
            Err(reason) => {
                return self
                    .api_headers(HttpResponse::json(
                        400,
                        &json!({"errors": {"query": "Required parameter missing or invalid"}}),
                    ))
                    .header("x-lsf-reason", reason);
            }
        };
        let Some(shop) = self.shop(&grant.shop) else {
            return self
                .api_headers(HttpResponse::json(401, &json!({ "errors": INVALID_TOKEN })))
                .header(
                    "x-lsf-reason",
                    format!("the shop {} of this token is gone", grant.shop),
                );
        };
        let key = throttle::key(&grant.shop, &grant.client_id);
        let mut live = lock(&shop);
        let origin = origin_of(request);
        let mut resolver = ShopResolver::new(&mut live.state, self.registry)
            .with_grant(&grant)
            .with_webhooks(&self.webhooks)
            .with_origin(&origin);
        let bucket = Bucket {
            throttle: &self.throttle,
            key: &key,
        };
        let mut response =
            graphql::run(crate::schema::admin(), &mut resolver, &query, Some(bucket));
        response.extensions.extend(resolver.extensions());
        let events = live.state.take_events();
        if !events.is_empty() {
            live.changed = true;
            // Built now from the shop as the request left it, queued for the worker: nothing
            // waits on the backend while the shop is locked.
            self.emit(&live.state, &events);
        }
        drop(live);
        self.api_headers(HttpResponse::json(200, &response.to_json()))
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

    /// The payload of `POST /__lsf/webhooks/trigger`, from the shop's records.
    fn build_trigger(&self, trigger: &TriggerRequest) -> Result<(String, Value), TriggerError> {
        let domain = match &trigger.shop {
            Some(shop) => oauth::normalize_shop(shop),
            None => self.default_shop(),
        };
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
                            "graphql": format!("/admin/api/{ADMIN_2026_10}/graphql.json"),
                        }),
                    ),
                    Err((status, error)) => HttpResponse::json(status, &json!({ "error": error })),
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
        let tokens = lock(&self.tokens);
        let now = Utc::now();
        let shops: Vec<Value> = self
            .all_shops()
            .into_iter()
            .map(|(domain, shop)| {
                let live = lock(&shop);
                let mut counts: Vec<(&str, usize)> = live
                    .state
                    .types()
                    .map(|type_name| (type_name, live.state.count(type_name)))
                    .collect();
                counts.sort();
                let grants: Vec<Value> = tokens
                    .grants()
                    .filter(|(_, grant)| grant.shop == domain && !grant.expired(now))
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
                let subscriptions: Vec<Value> = self
                    .webhooks
                    .registry()
                    .list(&domain)
                    .iter()
                    .map(|subscription| json!(subscription))
                    .collect();
                json!({
                    "shop": domain,
                    "installed": tokens.installed_scopes(&domain, &self.auth.client_id).is_some(),
                    "scopes": tokens.installed_scopes(&domain, &self.auth.client_id),
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
            "apiVersion": ADMIN_2026_10,
            "graphql": format!("/admin/api/{ADMIN_2026_10}/graphql.json"),
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
            "shops": shops,
        })
    }

    /// `POST /__lsf/admin/uninstall`: the app's tokens on the shop are revoked, and
    /// `app/uninstalled` is delivered.
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
        let revoked = lock(&self.tokens).uninstall(&domain, &self.auth.client_id);
        self.change_shop(&domain, |state| {
            // Shopify cancels the app's subscription when it is uninstalled.
            app_domain::billing::cancel_on_uninstall(state);
            if let Some(shop) = state.shop_gid().map(str::to_string) {
                state.emit("app/uninstalled", &shop);
            }
        });
        HttpResponse::json(
            200,
            &json!({"ok": true, "shop": domain, "revoked": revoked}),
        )
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
