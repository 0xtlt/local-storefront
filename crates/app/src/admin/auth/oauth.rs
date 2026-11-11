//! The OAuth endpoints of a shop, and lsf's endpoints that stand in for the Shopify admin: one
//! request in, one answer out ([`HttpRequest`] → [`HttpResponse`]), as the rest of [`crate::admin::api`].
//!
//! | Endpoint | What it does |
//! |---|---|
//! | `GET /admin/oauth/authorize` | Approves at once and redirects to `redirect_uri` with `code`, `hmac`, `host`, `shop`, `state`, `timestamp`. |
//! | `POST /admin/oauth/access_token` | The authorization code grant, token exchange (of a session token, or of a non-expiring offline token to migrate it), the refresh token grant and the client credentials grant. |
//! | `POST /__lsf/admin/session-token` | Mints a session token (`{"shop"?, "user"?}`), for a test to call the backend's authenticated routes. |
//! | `GET /__lsf/admin/launch` | Redirects to the app URL with the query the admin opens an embedded app with. |
//!
//! **Which shop.** On Shopify the shop is the host (`https://my-shop.myshopify.com/admin/oauth/…`);
//! lsf serves every shop on one address (`http://127.0.0.1:9292`). The shop of a request is
//! `?shop=` (a domain, or a name: `test-1` is `test-1.myshopify.com`), else the `Host` (or
//! `X-Forwarded-Host`) header when it is a `*.myshopify.com` domain (a backend whose requests
//! keep the shop's host), else the default shop. A code, a session token and a refresh token
//! carry their shop: when the request names another one, it is refused as Shopify would refuse
//! a code sent to another shop.
//!
//! **What the backend sees.** Requests and answers are those of Shopify's libraries, checked
//! against `@shopify/shopify-api` 15.0.1 (`lib/auth/oauth/*.ts`, `lib/session/decode-session-token.ts`,
//! `lib/utils/hmac-validator.ts`), `@shopify/shopify-app-react-router` 4.0.0 (`authenticate.admin`,
//! its token exchange strategy) and the `shopify_api` gem 16.3.1 (`Auth::Oauth`,
//! `Auth::TokenExchange`, `Auth::RefreshToken`, `Auth::ClientCredentials`): JSON or form bodies,
//! `expiring` as `"1"`, `1` or `"0"`, and the JSON fields they read. Error bodies are
//! `{"error", "error_description"}`; those shopify.dev documents are word for word (a refresh
//! token that can no longer be used: `401` `invalid_request` "This request requires an active
//! refresh_token"; a code used twice: `400` `invalid_request` "The authorization code was not found
//! or was already used"; an unknown client id: `application_cannot_be_found`; an invalid session
//! token: `400` `invalid_subject_token`, which the libraries branch on). The others follow
//! the same form; `X-Lsf-Reason` says in plain words what lsf refused.

use base64::Engine;
use base64::engine::general_purpose::STANDARD_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};

use super::scopes::{is_scope_name, parse_scopes};
use super::session_token::{self, Claims};
use super::signing::{constant_time_eq, query_hmac};
use super::tokens::{
    AccessMode, AssociatedUser, CODE_LIFETIME, Code, GrantSource, MigrationError, RefreshError,
    Tokens,
};
use crate::admin::api::{HttpRequest, HttpResponse};

/// `grant_type` of token exchange.
pub const TOKEN_EXCHANGE: &str = "urn:ietf:params:oauth:grant-type:token-exchange";
/// `subject_token_type` of a session token.
pub const ID_TOKEN: &str = "urn:ietf:params:oauth:token-type:id_token";
/// `requested_token_type` (and `subject_token_type`, to migrate one) of an offline token.
pub const OFFLINE_ACCESS_TOKEN: &str = "urn:shopify:params:oauth:token-type:offline-access-token";
/// `requested_token_type` of an online token.
pub const ONLINE_ACCESS_TOKEN: &str = "urn:shopify:params:oauth:token-type:online-access-token";

/// The shops of the server, as the OAuth endpoints need them. The integration implements it
/// over the state's shops.
pub trait Shops {
    /// Whether `shop` (`my-shop.myshopify.com`) exists.
    fn exists(&self, shop: &str) -> bool;

    /// The shop a request goes to when it names none: the store data's.
    fn default_shop(&self) -> String;

    /// The staff member `id` of `shop`, for an online token and a session token; `None` when
    /// the shop has no such user. By default every id is a user, 1 being the account owner
    /// ([`AssociatedUser::local`]).
    fn user(&self, shop: &str, id: u64) -> Option<AssociatedUser> {
        Some(AssociatedUser::local(shop, id))
    }

    /// The locale the admin of `shop` opens an app with (`locale=`).
    fn locale(&self, _shop: &str) -> String {
        "en".to_string()
    }
}

/// The user a session token and an online token are for when a request names none: the account
/// owner.
pub const DEFAULT_USER: u64 = 1;

/// What the OAuth endpoints know of the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppAuth {
    /// `client_id` of `shopify.app.toml`, or lsf's own without an app.
    pub client_id: String,
    /// What OAuth, session tokens and webhooks are signed with ([`super::client_secret`]).
    pub client_secret: String,
    /// `[access_scopes] scopes`: what an authorization without `scope`, a token exchange and a
    /// client credentials grant give when the app is not installed yet.
    pub scopes: Vec<String>,
    /// `[auth] redirect_urls`; `None` without an app, when any `redirect_uri` is accepted.
    pub redirect_urls: Option<Vec<String>>,
    /// `application_url`: where the admin opens the app.
    pub application_url: Option<String>,
    /// `--app-url`: where the backend listens locally, which replaces the origin of
    /// `application_url` (see [`AppAuth::redirect_allowed`] and [`AppAuth::app_url`]).
    pub app_url: Option<String>,
}

impl AppAuth {
    /// The app of `shopify.app.toml`.
    pub fn from_app(
        config: &crate::config::AppConfig,
        client_id: &str,
        client_secret: &str,
        app_url: Option<&str>,
    ) -> AppAuth {
        AppAuth {
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
            scopes: config.scopes.clone(),
            redirect_urls: Some(config.redirect_urls.clone()),
            application_url: config.application_url.clone(),
            app_url: app_url.map(str::to_string),
        }
    }

    /// Whether `redirect_uri` is allowed: equal to one of `[auth] redirect_urls`, as Shopify
    /// checks it ("Must exactly match a redirect URI you've configured"). An allowed URL on the
    /// origin of `application_url` is allowed on the origin of `--app-url` too, the way webhooks
    /// move there: `https://example.trycloudflare.com/auth/callback` becomes
    /// `http://localhost:3000/auth/callback`. Without an app, any `http(s)` URL is allowed.
    pub fn redirect_allowed(&self, redirect_uri: &str) -> bool {
        let Some(urls) = &self.redirect_urls else {
            return redirect_uri.starts_with("http://") || redirect_uri.starts_with("https://");
        };
        urls.iter().any(|url| {
            url == redirect_uri
                || self
                    .rebased(url)
                    .is_some_and(|rebased| rebased == redirect_uri)
        })
    }

    /// The redirect URLs, moved to `--app-url` where [`AppAuth::redirect_allowed`] moves them.
    pub fn allowed_redirects(&self) -> Vec<String> {
        let Some(urls) = &self.redirect_urls else {
            return Vec::new();
        };
        let mut allowed = urls.clone();
        allowed.extend(urls.iter().filter_map(|url| self.rebased(url)));
        allowed
    }

    /// `url` on the origin of `--app-url`, when it is on the origin of `application_url`.
    fn rebased(&self, url: &str) -> Option<String> {
        let app_url = self.app_url.as_deref()?;
        let application = origin(self.application_url.as_deref()?)?;
        let rest = url.strip_prefix(application)?;
        (rest.is_empty() || rest.starts_with(['/', '?', '#']))
            .then(|| format!("{}{rest}", origin(app_url).unwrap_or(app_url)))
    }

    /// Where the admin opens the app: `--app-url` when given, `application_url` otherwise (its
    /// path kept on the new origin).
    pub fn app_url(&self) -> Option<String> {
        match (self.app_url.as_deref(), self.application_url.as_deref()) {
            (Some(app_url), Some(application)) => {
                let path = origin(application).map_or("", |origin| &application[origin.len()..]);
                Some(format!(
                    "{}{path}",
                    origin(app_url).unwrap_or(app_url).trim_end_matches('/')
                ))
            }
            (Some(app_url), None) => Some(app_url.to_string()),
            (None, application) => application.map(str::to_string),
        }
    }
}

/// `scheme://host[:port]` of a URL.
fn origin(url: &str) -> Option<&str> {
    let (scheme, rest) = url.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    Some(&url[..scheme.len() + 3 + end])
}

/// What an endpoint works with: the app, the shops, and the time.
pub struct Context<'a> {
    pub app: &'a AppAuth,
    pub shops: &'a dyn Shops,
    /// The system clock: the backend checks timestamps and expiries against its own.
    pub now: DateTime<Utc>,
}

/// Answers the requests of this module (see its documentation), `None` for any other.
pub fn handle(request: &HttpRequest, cx: &Context, tokens: &mut Tokens) -> Option<HttpResponse> {
    let method = request.method.to_ascii_uppercase();
    let path = request.path.trim_end_matches('/');
    Some(match (method.as_str(), path) {
        ("GET", "/admin/oauth/authorize") => authorize(request, cx, tokens),
        ("POST", "/admin/oauth/access_token") => access_token(request, cx, tokens),
        (_, "/admin/oauth/authorize" | "/admin/oauth/access_token") => {
            HttpResponse::json(405, &json!({"errors": "Method Not Allowed"}))
        }
        ("POST", "/__lsf/admin/session-token") => session_token_endpoint(request, cx, tokens),
        ("GET", "/__lsf/admin/launch") => launch(request, cx, tokens),
        _ => return None,
    })
}

/// `GET /admin/oauth/authorize?client_id=…&scope=…&redirect_uri=…&state=…&grant_options[]=per-user`.
/// Shopify asks the merchant to approve; lsf approves at once and redirects to `redirect_uri`
/// with `code`, `hmac`, `host`, `shop`, `state` and `timestamp` (the five the `shopify_api` gem
/// signs, and `hmac`). Without `scope`, the app's scopes are granted.
pub fn authorize(request: &HttpRequest, cx: &Context, tokens: &mut Tokens) -> HttpResponse {
    let shop = shop_of(request, cx.shops);
    if !cx.shops.exists(&shop) {
        return page(
            404,
            "There is no shop here",
            &format!("lsf has no shop {shop}. Create it with POST /__lsf/admin/shops."),
        );
    }
    let client_id = request.query_param("client_id").unwrap_or_default();
    if client_id != cx.app.client_id {
        return oauth_page(
            400,
            "application_cannot_be_found",
            &format!("Could not find Shopify API application with api_key {client_id}"),
            &format!("The app's client id is {}.", cx.app.client_id),
        );
    }
    let redirect_uri = request.query_param("redirect_uri").unwrap_or_default();
    if !cx.app.redirect_allowed(redirect_uri) {
        let allowed = cx.app.allowed_redirects();
        return oauth_page(
            400,
            "invalid_request",
            "The redirect_uri is not whitelisted",
            &if allowed.is_empty() {
                "shopify.app.toml has no [auth] redirect_urls.".to_string()
            } else {
                format!("Allowed: {}.", allowed.join(", "))
            },
        );
    }
    let scopes = match request.query_param("scope") {
        Some(list) if !list.trim().is_empty() => parse_scopes(list),
        _ => cx.app.scopes.clone(),
    };
    if let Some(invalid) = scopes.iter().find(|scope| !is_scope_name(scope)) {
        return oauth_page(
            400,
            "invalid_scope",
            &format!("The access scope is invalid: {invalid}"),
            "",
        );
    }
    let online = request
        .query
        .iter()
        .any(|(name, value)| name.starts_with("grant_options") && value == "per-user");
    let user = online.then(|| cx.shops.user(&shop, DEFAULT_USER)).flatten();
    if online && user.is_none() {
        return page(
            400,
            "There is no user to approve this",
            &format!("{shop} has no user {DEFAULT_USER} to give an online token to."),
        );
    }
    let code = tokens.create_code(Code {
        shop: shop.clone(),
        client_id: client_id.to_string(),
        scopes,
        mode: if online {
            AccessMode::Online
        } else {
            AccessMode::Offline
        },
        user,
        redirect_uri: redirect_uri.to_string(),
        expires_at: cx.now + Duration::seconds(CODE_LIFETIME),
    });
    let mut params = vec![
        ("code".to_string(), code),
        ("host".to_string(), host_param(&shop)),
        ("shop".to_string(), shop.clone()),
    ];
    if let Some(state) = request.query_param("state") {
        params.push(("state".to_string(), state.to_string()));
    }
    params.push(("timestamp".to_string(), cx.now.timestamp().to_string()));
    // A backend verifies the hmac over the whole query of its callback, that of `redirect_uri`
    // included.
    let own_query = redirect_uri
        .split_once('?')
        .map_or("", |(_, query)| query.split('#').next().unwrap_or(query));
    let signed = form_urlencoded::parse(own_query.as_bytes())
        .into_owned()
        .chain(params.iter().cloned());
    let hmac = query_hmac(&cx.app.client_secret, signed);
    params.insert(1, ("hmac".to_string(), hmac));
    HttpResponse::redirect(&with_query(redirect_uri, &params))
}

/// `POST /admin/oauth/access_token`, its parameters in a JSON or form body (or the query, as
/// Shopify's older examples send them).
pub fn access_token(request: &HttpRequest, cx: &Context, tokens: &mut Tokens) -> HttpResponse {
    let params = match parameters(request) {
        Ok(params) => params,
        Err(reason) => {
            return oauth_error(
                400,
                "invalid_request",
                "The request body could not be read",
                &reason,
            );
        }
    };
    let param = |name: &str| {
        params
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .filter(|value| !value.is_empty())
    };
    let client_id = param("client_id").unwrap_or_default();
    if client_id != cx.app.client_id {
        return oauth_error(
            400,
            "application_cannot_be_found",
            &format!("Could not find Shopify API application with api_key {client_id}"),
            &format!("the app's client id is {}", cx.app.client_id),
        );
    }
    let grant_type = param("grant_type").unwrap_or("authorization_code");
    if grant_type == "client_credentials" && param("client_assertion").is_some() {
        return oauth_error(
            400,
            "invalid_client",
            "Client authentication failed",
            "lsf does not verify client assertions: authenticate with client_secret",
        );
    }
    let secret = param("client_secret").unwrap_or_default();
    if !constant_time_eq(secret.as_bytes(), cx.app.client_secret.as_bytes()) {
        return oauth_error(
            401,
            "invalid_client",
            "Client authentication failed",
            "client_secret is not the secret lsf signs with (--client-secret, SHOPIFY_API_SECRET)",
        );
    }
    let explicit = explicit_shop(request);
    let expiring = param("expiring").is_some_and(truthy);
    match grant_type {
        "authorization_code" => {
            // A code sent to another shop is not found there, and stays good on its own.
            let Some(code) = param("code")
                .filter(|code| {
                    explicit
                        .as_deref()
                        .is_none_or(|shop| tokens.code_shop(code) == Some(shop))
                })
                .and_then(|code| tokens.redeem_code(code, client_id, cx.now))
            else {
                return oauth_error(
                    400,
                    "invalid_request",
                    "The authorization code was not found or was already used",
                    "a code is good once, for ten minutes, on the shop it was given for",
                );
            };
            // Approving installs the app, with the scopes approved; an online grant leaves an
            // installation as it is.
            if code.mode == AccessMode::Offline
                || tokens.installed_scopes(&code.shop, client_id).is_none()
            {
                tokens.install(&code.shop, client_id, &code.scopes, cx.now);
            }
            let issued = match (code.mode, code.user) {
                (AccessMode::Online, Some(user)) => tokens.issue_online(
                    &code.shop,
                    client_id,
                    &code.scopes,
                    user,
                    GrantSource::AuthorizationCode,
                    cx.now,
                ),
                _ => tokens.issue_offline(
                    &code.shop,
                    client_id,
                    &code.scopes,
                    expiring,
                    GrantSource::AuthorizationCode,
                    cx.now,
                ),
            };
            HttpResponse::json(200, &issued.response(cx.now))
        }
        TOKEN_EXCHANGE => token_exchange(&param, explicit.as_deref(), expiring, cx, tokens),
        "refresh_token" => {
            let refresh_token = param("refresh_token").unwrap_or_default();
            let result = match explicit.as_deref() {
                Some(shop)
                    if tokens
                        .refresh_token_shop(refresh_token)
                        .is_some_and(|own| own != shop) =>
                {
                    Err(RefreshError::Unknown)
                }
                _ => tokens.refresh(refresh_token, client_id, cx.now),
            };
            match result {
                Ok(issued) => HttpResponse::json(200, &issued.response(cx.now)),
                Err(error) => oauth_error(
                    401,
                    "invalid_request",
                    "This request requires an active refresh_token",
                    &match error {
                        RefreshError::Retired => {
                            "the refresh token was replaced by a newer one the app used, or by a new grant".to_string()
                        }
                        RefreshError::Expired => {
                            "the refresh token expired (90 days, or 30 days after its first use)".to_string()
                        }
                        RefreshError::OtherClient => "the refresh token is another app's".to_string(),
                        RefreshError::Unknown => "lsf issued no such refresh token for this shop".to_string(),
                        RefreshError::Revoked { shop, by } => format!(
                            "this refresh token of {shop} was revoked {}: run the token exchange or OAuth again for a new one",
                            by.what(&shop)
                        ),
                    },
                ),
            }
        }
        "client_credentials" => {
            let shop = explicit.unwrap_or_else(|| cx.shops.default_shop());
            if !cx.shops.exists(&shop) {
                return oauth_error(
                    400,
                    "shop_not_permitted",
                    "Client credentials cannot be performed on this shop.",
                    &format!("lsf has no shop {shop}"),
                );
            }
            let scopes = installed_or_app_scopes(&shop, cx, tokens);
            let issued = tokens.issue_client_credentials(&shop, client_id, &scopes, cx.now);
            HttpResponse::json(200, &issued.response(cx.now))
        }
        other => oauth_error(
            400,
            "unsupported_grant_type",
            "The authorization grant type is not supported by the authorization server.",
            &format!("grant_type {other:?}"),
        ),
    }
}

/// Token exchange: a session token for an offline or online token, or a non-expiring offline
/// token for an expiring one.
fn token_exchange<'p>(
    param: &dyn Fn(&str) -> Option<&'p str>,
    explicit: Option<&str>,
    expiring: bool,
    cx: &Context,
    tokens: &mut Tokens,
) -> HttpResponse {
    let subject = param("subject_token").unwrap_or_default();
    let requested = param("requested_token_type").unwrap_or(OFFLINE_ACCESS_TOKEN);
    if requested != OFFLINE_ACCESS_TOKEN && requested != ONLINE_ACCESS_TOKEN {
        return oauth_error(
            400,
            "invalid_requested_token_type",
            "The requested token type is not supported",
            &format!("requested_token_type {requested:?}"),
        );
    }
    match param("subject_token_type") {
        Some(ID_TOKEN) => {}
        Some(OFFLINE_ACCESS_TOKEN) => {
            // Migration to an expiring offline token.
            if requested != OFFLINE_ACCESS_TOKEN || !expiring {
                return oauth_error(
                    400,
                    "invalid_requested_token_type",
                    "The requested token type is not supported",
                    "migrating needs requested_token_type offline-access-token and expiring=1",
                );
            }
            let shop = explicit
                .map(str::to_string)
                .unwrap_or_else(|| cx.shops.default_shop());
            return match tokens.migrate(subject, &cx.app.client_id, &shop, cx.now) {
                Ok(issued) => HttpResponse::json(200, &issued.response(cx.now)),
                Err(error) => oauth_error(
                    400,
                    "invalid_subject_token",
                    "Invalid subject token",
                    match error {
                        MigrationError::Unknown => {
                            "not an active non-expiring offline token of this app on this shop"
                        }
                        MigrationError::NotMigratable => {
                            "only a non-expiring offline token can be migrated"
                        }
                        MigrationError::Delegate => {
                            "a delegate token cannot be exchanged: it makes no token"
                        }
                    },
                ),
            };
        }
        other => {
            return oauth_error(
                400,
                "invalid_request",
                "The subject token type is not supported",
                &format!("subject_token_type {:?}", other.unwrap_or_default()),
            );
        }
    }
    let claims = match session_token::verify(
        subject,
        &cx.app.client_secret,
        &cx.app.client_id,
        cx.now.timestamp(),
    ) {
        Ok(claims) => claims,
        Err(invalid) => {
            return oauth_error(
                400,
                "invalid_subject_token",
                "Subject token is invalid",
                &invalid.to_string(),
            );
        }
    };
    let shop = claims.shop().to_string();
    if !cx.shops.exists(&shop) || explicit.is_some_and(|explicit| explicit != shop) {
        return oauth_error(
            400,
            "invalid_subject_token",
            "Subject token is invalid",
            &format!("the session token is for {shop}, not a shop of this request"),
        );
    }
    let scopes = installed_or_app_scopes(&shop, cx, tokens);
    if tokens.installed_scopes(&shop, &cx.app.client_id).is_none() {
        // A session token means the app is installed: Shopify installs it before App Bridge runs.
        tokens.install(&shop, &cx.app.client_id, &scopes, cx.now);
    }
    let issued = if requested == ONLINE_ACCESS_TOKEN {
        let Some(user) = claims.user_id().and_then(|id| cx.shops.user(&shop, id)) else {
            return oauth_error(
                400,
                "invalid_subject_token",
                "Subject token is invalid",
                &format!("{shop} has no user {:?}", claims.sub),
            );
        };
        tokens.issue_online(
            &shop,
            &cx.app.client_id,
            &scopes,
            user,
            GrantSource::TokenExchange,
            cx.now,
        )
    } else {
        tokens.issue_offline(
            &shop,
            &cx.app.client_id,
            &scopes,
            expiring,
            GrantSource::TokenExchange,
            cx.now,
        )
    };
    HttpResponse::json(200, &issued.response(cx.now))
}

/// The scopes the app is installed with on `shop`, else those of `shopify.app.toml`.
fn installed_or_app_scopes(shop: &str, cx: &Context, tokens: &Tokens) -> Vec<String> {
    tokens
        .installed_scopes(shop, &cx.app.client_id)
        .map_or_else(|| cx.app.scopes.clone(), <[String]>::to_vec)
}

/// A session token of the app for `user` of `shop`, issued now, and its claims.
pub fn mint_session_token(
    cx: &Context,
    tokens: &mut Tokens,
    shop: &str,
    user: u64,
) -> (String, Claims) {
    let jti = Claims::uuid(&tokens.entropy("jti"));
    let claims = Claims::new(shop, &cx.app.client_id, user, cx.now.timestamp(), jti);
    (session_token::mint(&claims, &cx.app.client_secret), claims)
}

/// `POST /__lsf/admin/session-token` with `{"shop"?, "user"?}` (JSON, a form, or the query):
/// a session token of the default shop and its account owner unless the body names others.
/// Answers `{"token", "authorization": "Bearer …", "shop", "claims"}`.
pub fn session_token_endpoint(
    request: &HttpRequest,
    cx: &Context,
    tokens: &mut Tokens,
) -> HttpResponse {
    let params = match parameters(request) {
        Ok(params) => params,
        Err(reason) => return HttpResponse::error(400, reason),
    };
    let param = |name: &str| {
        params
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .filter(|value| !value.is_empty())
    };
    let shop = param("shop")
        .map(normalize_shop)
        .unwrap_or_else(|| shop_of(request, cx.shops));
    if !cx.shops.exists(&shop) {
        return HttpResponse::error(404, format!("lsf has no shop {shop}"));
    }
    let user = match param("user").map(str::parse::<u64>) {
        None => DEFAULT_USER,
        Some(Ok(user)) if cx.shops.user(&shop, user).is_some() => user,
        Some(_) => {
            return HttpResponse::error(
                404,
                format!("{shop} has no user {}", param("user").unwrap_or_default()),
            );
        }
    };
    let (token, claims) = mint_session_token(cx, tokens, &shop, user);
    HttpResponse::json(
        200,
        &json!({
            "token": token,
            "authorization": format!("Bearer {token}"),
            "shop": shop,
            "claims": claims,
        }),
    )
}

/// The URL the admin of `shop` opens the embedded app at for `user`, at `path` of the app (its
/// own query kept): the app URL with `embedded=1`, `hmac`, `host`, `id_token`, `locale`,
/// `session`, `shop` and `timestamp`, the `hmac` signing every other parameter, those of `path`
/// included. `session` is the session id of the session token (`sid`). `None` without an app URL.
pub fn launch_url(
    cx: &Context,
    tokens: &mut Tokens,
    shop: &str,
    path: &str,
    user: u64,
    locale: &str,
) -> Option<String> {
    let base = cx.app.app_url()?;
    let (path, own_query) = path.split_once('?').unwrap_or((path, ""));
    let (id_token, claims) = mint_session_token(cx, tokens, shop, user);
    let mut params: Vec<(String, String)> = form_urlencoded::parse(own_query.as_bytes())
        .into_owned()
        .collect();
    params.extend([
        ("embedded".to_string(), "1".to_string()),
        ("host".to_string(), host_param(shop)),
        ("id_token".to_string(), id_token),
        ("locale".to_string(), locale.to_string()),
        ("session".to_string(), claims.sid),
        ("shop".to_string(), shop.to_string()),
        ("timestamp".to_string(), cx.now.timestamp().to_string()),
    ]);
    let hmac = query_hmac(&cx.app.client_secret, params.iter().cloned());
    params.push(("hmac".to_string(), hmac));
    params.sort_by(|a, b| a.0.cmp(&b.0));
    let url = match path.trim_start_matches('/') {
        "" => base,
        rest => format!("{}/{rest}", base.trim_end_matches('/')),
    };
    Some(with_query(&url, &params))
}

/// `GET /__lsf/admin/launch?shop=…&path=…&user=…&locale=…`: a redirect to [`launch_url`], the
/// default shop, `/`, the account owner and the shop's locale unless named.
pub fn launch(request: &HttpRequest, cx: &Context, tokens: &mut Tokens) -> HttpResponse {
    let shop = shop_of(request, cx.shops);
    if !cx.shops.exists(&shop) {
        return HttpResponse::error(404, format!("lsf has no shop {shop}"));
    }
    let path = request.query_param("path").unwrap_or("/");
    if !path.starts_with('/') || path.starts_with("//") {
        return HttpResponse::error(
            400,
            "path is a path of the app, starting with one /: /app/settings",
        );
    }
    let user = match request.query_param("user").map(str::parse::<u64>) {
        None => DEFAULT_USER,
        Some(Ok(user)) if cx.shops.user(&shop, user).is_some() => user,
        Some(_) => {
            return HttpResponse::error(404, format!("{shop} has no such user"));
        }
    };
    let locale = request
        .query_param("locale")
        .map_or_else(|| cx.shops.locale(&shop), str::to_string);
    match launch_url(cx, tokens, &shop, path, user, &locale) {
        Some(url) => HttpResponse::redirect(&url),
        None => HttpResponse::error(
            400,
            "the app has no URL: give --app-url, or application_url in shopify.app.toml",
        ),
    }
}

/// The `host` parameter of the admin: `admin.shopify.com/store/{name}` in base64, as
/// `@shopify/shopify-api` decodes it (`sanitizeHost` accepts the standard alphabet, padded or
/// not; Shopify sends it without padding).
pub fn host_param(shop: &str) -> String {
    let name = shop.strip_suffix(".myshopify.com").unwrap_or(shop);
    STANDARD_NO_PAD.encode(format!("admin.shopify.com/store/{name}"))
}

/// A shop as a request names it: `test-1`, `test-1.myshopify.com`,
/// `https://test-1.myshopify.com/` all are `test-1.myshopify.com`.
pub fn normalize_shop(shop: &str) -> String {
    let shop = shop.trim().to_ascii_lowercase();
    let shop = shop
        .strip_prefix("https://")
        .or_else(|| shop.strip_prefix("http://"))
        .unwrap_or(&shop)
        .trim_end_matches('/');
    if shop.contains('.') {
        shop.to_string()
    } else {
        format!("{shop}.myshopify.com")
    }
}

/// The shop a request names: `?shop=`, else a `*.myshopify.com` `Host` or `X-Forwarded-Host`.
pub fn explicit_shop(request: &HttpRequest) -> Option<String> {
    if let Some(shop) = request.query_param("shop").filter(|shop| !shop.is_empty()) {
        return Some(normalize_shop(shop));
    }
    ["x-forwarded-host", "host"].into_iter().find_map(|header| {
        let host = request.header(header)?.split(',').next()?.trim();
        let host = host.rsplit_once(':').map_or(host, |(name, port)| {
            if port.bytes().all(|byte| byte.is_ascii_digit()) {
                name
            } else {
                host
            }
        });
        host.to_ascii_lowercase()
            .ends_with(".myshopify.com")
            .then(|| host.to_ascii_lowercase())
    })
}

/// The shop of a request: [`explicit_shop`], else the default shop.
pub fn shop_of(request: &HttpRequest, shops: &dyn Shops) -> String {
    explicit_shop(request).unwrap_or_else(|| shops.default_shop())
}

/// The parameters of a request: its query, then its body (JSON or a form) when it has one. JSON
/// numbers and booleans are written as text (`"expiring": 1` is `1`).
fn parameters(request: &HttpRequest) -> Result<Vec<(String, String)>, String> {
    let mut params = request.query.clone();
    let body = String::from_utf8_lossy(&request.body);
    if body.trim().is_empty() {
        return Ok(params);
    }
    let json = request
        .header("content-type")
        .is_some_and(|kind| kind.contains("json"))
        || body.trim_start().starts_with('{');
    if json {
        let value: Value =
            serde_json::from_str(&body).map_err(|error| format!("invalid JSON: {error}"))?;
        let Value::Object(fields) = value else {
            return Err("the JSON body is not an object".to_string());
        };
        for (name, value) in fields {
            let text = match value {
                Value::String(text) => text,
                Value::Null => continue,
                Value::Bool(flag) => flag.to_string(),
                other => other.to_string(),
            };
            params.push((name, text));
        }
    } else {
        params.extend(form_urlencoded::parse(body.as_bytes()).into_owned());
    }
    Ok(params)
}

/// Whether a flag parameter is set: `1` or `true`.
fn truthy(value: &str) -> bool {
    matches!(value.trim(), "1" | "true")
}

/// `url` with `params` added to its query.
fn with_query(url: &str, params: &[(String, String)]) -> String {
    let (url, fragment) = url
        .split_once('#')
        .map_or((url, None), |(url, fragment)| (url, Some(fragment)));
    let mut serializer = form_urlencoded::Serializer::new(String::new());
    for (name, value) in params {
        serializer.append_pair(name, value);
    }
    let query = serializer.finish();
    let separator = if !url.contains('?') {
        "?"
    } else if url.ends_with(['?', '&']) {
        ""
    } else {
        "&"
    };
    let mut out = format!("{url}{separator}{query}");
    if let Some(fragment) = fragment {
        out.push('#');
        out.push_str(fragment);
    }
    out
}

/// An error of the token endpoint: `{"error", "error_description"}`, and lsf's reason in
/// `X-Lsf-Reason`.
fn oauth_error(status: u16, error: &str, description: &str, reason: &str) -> HttpResponse {
    let response = HttpResponse::json(
        status,
        &json!({"error": error, "error_description": description}),
    );
    if reason.is_empty() {
        response
    } else {
        response.header("x-lsf-reason", reason)
    }
}

/// An error page of `/admin/oauth/authorize`: Shopify's "Oauth error {error}: {description}",
/// and lsf's hint under it.
fn oauth_page(status: u16, error: &str, description: &str, hint: &str) -> HttpResponse {
    page(status, &format!("Oauth error {error}: {description}"), hint)
        .header("x-lsf-reason", format!("{error}: {description}"))
}

fn page(status: u16, title: &str, hint: &str) -> HttpResponse {
    let escape = |text: &str| {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let body = format!(
        "<!DOCTYPE html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><title>{title}</title></head>\
         <body><h1>{title}</h1>{hint}</body></html>\n",
        title = escape(title),
        hint = if hint.is_empty() {
            String::new()
        } else {
            format!("<p>lsf: {}</p>", escape(hint))
        },
    );
    HttpResponse::html(status, body)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::admin::auth::signing::verify_query_hmac;

    const SHOP: &str = "my-shop.myshopify.com";
    const CLIENT_ID: &str = "0123456789abcdef0123456789abcdef";
    const SECRET: &str = "hush";

    struct TestShops;

    impl Shops for TestShops {
        fn exists(&self, shop: &str) -> bool {
            shop == SHOP || shop == "test-1.myshopify.com"
        }

        fn default_shop(&self) -> String {
            SHOP.to_string()
        }

        fn user(&self, shop: &str, id: u64) -> Option<AssociatedUser> {
            (id < 100).then(|| AssociatedUser::local(shop, id))
        }
    }

    fn app() -> AppAuth {
        AppAuth {
            client_id: CLIENT_ID.to_string(),
            client_secret: SECRET.to_string(),
            scopes: vec!["write_products".to_string(), "read_orders".to_string()],
            redirect_urls: Some(vec![
                "https://example.trycloudflare.com/auth/callback".to_string(),
            ]),
            application_url: Some("https://example.trycloudflare.com".to_string()),
            app_url: Some("http://localhost:3000".to_string()),
        }
    }

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).unwrap()
    }

    fn get(path: &str, query: &[(&str, &str)]) -> HttpRequest {
        HttpRequest {
            method: "GET".to_string(),
            path: path.to_string(),
            query: query
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect(),
            headers: HashMap::new(),
            body: Vec::new(),
        }
    }

    /// A JSON POST, as `@shopify/shopify-api` and the `shopify_api` gem send it.
    fn post_json(path: &str, body: Value) -> HttpRequest {
        HttpRequest {
            method: "POST".to_string(),
            path: path.to_string(),
            query: Vec::new(),
            headers: HashMap::from([("content-type".to_string(), "application/json".to_string())]),
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    /// A form POST, as shopify.dev's examples send it.
    fn post_form(path: &str, fields: &[(&str, &str)]) -> HttpRequest {
        let mut form = form_urlencoded::Serializer::new(String::new());
        for (name, value) in fields {
            form.append_pair(name, value);
        }
        HttpRequest {
            method: "POST".to_string(),
            path: path.to_string(),
            query: Vec::new(),
            headers: HashMap::from([(
                "content-type".to_string(),
                "application/x-www-form-urlencoded".to_string(),
            )]),
            body: form.finish().into_bytes(),
        }
    }

    fn json_of(response: &HttpResponse) -> Value {
        serde_json::from_slice(&response.body).unwrap()
    }

    fn header<'a>(response: &'a HttpResponse, name: &str) -> Option<&'a str> {
        response
            .headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// The redirect of a response: its URL without the query, and its query.
    fn location(response: &HttpResponse) -> (String, Vec<(String, String)>) {
        assert_eq!(
            response.status,
            302,
            "{}",
            String::from_utf8_lossy(&response.body)
        );
        let location = header(response, "location").unwrap();
        let (url, query) = location.split_once('?').unwrap();
        (
            url.to_string(),
            form_urlencoded::parse(query.as_bytes())
                .into_owned()
                .collect(),
        )
    }

    fn value<'a>(query: &'a [(String, String)], name: &str) -> &'a str {
        query
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
            .unwrap_or_else(|| panic!("no {name} in {query:?}"))
    }

    fn authorize_query(
        extra: &[(&'static str, &'static str)],
    ) -> Vec<(&'static str, &'static str)> {
        let mut query = vec![
            ("client_id", CLIENT_ID),
            ("scope", "write_products,read_customers"),
            ("redirect_uri", "http://localhost:3000/auth/callback"),
            ("state", "123456789012345"),
        ];
        query.extend_from_slice(extra);
        query
    }

    #[test]
    fn authorization_code_grant_from_authorize_to_token() {
        let app = app();
        let cx = Context {
            app: &app,
            shops: &TestShops,
            now: now(),
        };
        let mut tokens = Tokens::with_seed(1);

        let response = authorize(
            &get("/admin/oauth/authorize", &authorize_query(&[])),
            &cx,
            &mut tokens,
        );
        let (url, query) = location(&response);
        assert_eq!(url, "http://localhost:3000/auth/callback");
        let names: Vec<&str> = query.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            ["code", "hmac", "host", "shop", "state", "timestamp"]
        );
        assert_eq!(value(&query, "shop"), SHOP);
        assert_eq!(value(&query, "state"), "123456789012345");
        assert_eq!(value(&query, "timestamp"), "1800000000");
        // `@shopify/shopify-api` checks the hmac over the decoded query.
        assert!(verify_query_hmac(SECRET, query.iter().cloned()));
        // The `shopify_api` gem signs `URI.encode_www_form(code:, host:, shop:, state:, timestamp:)`.
        let ruby: String = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(query.iter().filter(|(name, _)| name != "hmac"))
            .finish();
        assert_eq!(
            crate::admin::auth::signing::hex(&crate::admin::auth::signing::hmac_sha256(
                SECRET.as_bytes(),
                ruby.as_bytes()
            )),
            value(&query, "hmac")
        );
        assert_eq!(
            STANDARD_NO_PAD.decode(value(&query, "host")).unwrap(),
            b"admin.shopify.com/store/my-shop"
        );

        // The exchange, as `@shopify/shopify-api` sends it.
        let code = value(&query, "code").to_string();
        let exchange = json!({
            "client_id": CLIENT_ID,
            "client_secret": SECRET,
            "code": code,
            "expiring": "0",
        });
        let response = access_token(
            &post_json("/admin/oauth/access_token", exchange.clone()),
            &cx,
            &mut tokens,
        );
        assert_eq!(response.status, 200);
        let body = json_of(&response);
        let token = body["access_token"].as_str().unwrap();
        assert!(token.starts_with("shpat_"));
        assert_eq!(
            body,
            json!({"access_token": token, "scope": "write_products,read_customers"})
        );
        let grant = tokens.lookup(token, now()).unwrap();
        assert_eq!(grant.shop, SHOP);
        assert_eq!(grant.source, GrantSource::AuthorizationCode);
        assert!(grant.covers("read_customers"));
        assert_eq!(
            tokens.installed_scopes(SHOP, CLIENT_ID).unwrap(),
            ["write_products", "read_customers"]
        );

        // A code is good once.
        let response = access_token(
            &post_json("/admin/oauth/access_token", exchange),
            &cx,
            &mut tokens,
        );
        assert_eq!(response.status, 400);
        assert_eq!(
            json_of(&response),
            json!({
                "error": "invalid_request",
                "error_description": "The authorization code was not found or was already used",
            })
        );
    }

    #[test]
    fn authorization_code_grant_gives_online_and_expiring_tokens() {
        let app = app();
        let cx = Context {
            app: &app,
            shops: &TestShops,
            now: now(),
        };
        let mut tokens = Tokens::with_seed(1);

        let online = authorize_query(&[("grant_options[]", "per-user")]);
        let (_, query) = location(&authorize(
            &get("/admin/oauth/authorize", &online),
            &cx,
            &mut tokens,
        ));
        // The `shopify_api` gem sends `expiring` as a number: it is ignored for an online token.
        let response = access_token(
            &post_json(
                "/admin/oauth/access_token",
                json!({"client_id": CLIENT_ID, "client_secret": SECRET, "code": value(&query, "code"), "expiring": 1}),
            ),
            &cx,
            &mut tokens,
        );
        let body = json_of(&response);
        assert_eq!(body["expires_in"], 86_399);
        assert_eq!(
            body["associated_user_scope"],
            "write_products,read_customers"
        );
        assert_eq!(
            body["associated_user"],
            json!({
                "id": 1,
                "first_name": "Shop",
                "last_name": "Owner",
                "email": "owner@my-shop.myshopify.com",
                "email_verified": true,
                "account_owner": true,
                "locale": "en",
                "collaborator": false,
            })
        );
        assert!(body.get("refresh_token").is_none());

        // An expiring offline token, by a form as shopify.dev's example sends it.
        let (_, query) = location(&authorize(
            &get("/admin/oauth/authorize", &authorize_query(&[])),
            &cx,
            &mut tokens,
        ));
        let response = access_token(
            &post_form(
                "/admin/oauth/access_token",
                &[
                    ("client_id", CLIENT_ID),
                    ("client_secret", SECRET),
                    ("code", value(&query, "code")),
                    ("expiring", "1"),
                ],
            ),
            &cx,
            &mut tokens,
        );
        let body = json_of(&response);
        assert_eq!(body["expires_in"], 3_600);
        assert_eq!(body["refresh_token_expires_in"], 7_776_000);
        assert!(
            body["refresh_token"]
                .as_str()
                .unwrap()
                .starts_with("shprt_")
        );
    }

    #[test]
    fn refreshes_an_expiring_token() {
        let app = app();
        let mut tokens = Tokens::with_seed(1);
        let cx = Context {
            app: &app,
            shops: &TestShops,
            now: now(),
        };
        let issued = tokens.issue_offline(
            SHOP,
            CLIENT_ID,
            &app.scopes,
            true,
            GrantSource::TokenExchange,
            now(),
        );
        let refresh_token = issued.refresh_token.unwrap();

        // An hour later the access token is expired; the refresh token gives a new pair.
        let later = Context {
            now: now() + Duration::seconds(3_600),
            ..cx
        };
        assert!(tokens.lookup(&issued.access_token, later.now).is_err());
        let refresh = |token: &str| {
            post_form(
                "/admin/oauth/access_token",
                &[
                    ("client_id", CLIENT_ID),
                    ("client_secret", SECRET),
                    ("grant_type", "refresh_token"),
                    ("refresh_token", token),
                ],
            )
        };
        let response = access_token(&refresh(&refresh_token), &later, &mut tokens);
        assert_eq!(response.status, 200);
        let body = json_of(&response);
        assert_eq!(body["expires_in"], 3_600);
        assert_eq!(body["refresh_token_expires_in"], 7_776_000);
        assert_eq!(body["scope"], "write_products,read_orders");
        let new_access = body["access_token"].as_str().unwrap();
        assert!(tokens.lookup(new_access, later.now).is_ok());

        // Once the new refresh token is used, the old one is refused as Shopify refuses it.
        let new_refresh = body["refresh_token"].as_str().unwrap().to_string();
        assert_eq!(
            access_token(&refresh(&new_refresh), &later, &mut tokens).status,
            200
        );
        let response = access_token(&refresh(&refresh_token), &later, &mut tokens);
        assert_eq!(response.status, 401);
        assert_eq!(
            json_of(&response),
            json!({
                "error": "invalid_request",
                "error_description": "This request requires an active refresh_token",
            })
        );
        assert!(
            header(&response, "x-lsf-reason")
                .unwrap()
                .contains("replaced")
        );
        // Sent to another shop, a refresh token is unknown there, and stays good on its own.
        let newest = tokens
            .issue_offline(
                SHOP,
                CLIENT_ID,
                &app.scopes,
                true,
                GrantSource::TokenExchange,
                now(),
            )
            .refresh_token
            .unwrap();
        let mut elsewhere = refresh(&newest);
        elsewhere
            .query
            .push(("shop".to_string(), "test-1".to_string()));
        assert_eq!(access_token(&elsewhere, &later, &mut tokens).status, 401);
        assert_eq!(
            access_token(&refresh(&newest), &later, &mut tokens).status,
            200
        );
        assert_eq!(
            access_token(&refresh("shprt_unknown"), &later, &mut tokens).status,
            401
        );
    }

    #[test]
    fn token_exchange_of_a_session_token() {
        let app = app();
        let cx = Context {
            app: &app,
            shops: &TestShops,
            now: now(),
        };
        let mut tokens = Tokens::with_seed(1);
        let (session_token, _) = mint_session_token(&cx, &mut tokens, "test-1.myshopify.com", 7);
        let exchange = |requested: &str, expiring: Value| {
            post_json(
                "/admin/oauth/access_token",
                json!({
                    "client_id": CLIENT_ID,
                    "client_secret": SECRET,
                    "grant_type": TOKEN_EXCHANGE,
                    "subject_token": session_token,
                    "subject_token_type": ID_TOKEN,
                    "requested_token_type": requested,
                    "expiring": expiring,
                }),
            )
        };

        // Offline, not expiring: the shop is the session token's `dest`.
        let response = access_token(
            &exchange(OFFLINE_ACCESS_TOKEN, json!("0")),
            &cx,
            &mut tokens,
        );
        assert_eq!(response.status, 200);
        let body = json_of(&response);
        let token = body["access_token"].as_str().unwrap();
        assert_eq!(
            body,
            json!({"access_token": token, "scope": "write_products,read_orders"})
        );
        assert_eq!(
            tokens.lookup(token, now()).unwrap().shop,
            "test-1.myshopify.com"
        );
        assert!(
            tokens
                .installed_scopes("test-1.myshopify.com", CLIENT_ID)
                .is_some()
        );

        // Offline, expiring.
        let body = json_of(&access_token(
            &exchange(OFFLINE_ACCESS_TOKEN, json!("1")),
            &cx,
            &mut tokens,
        ));
        assert_eq!(body["expires_in"], 3_600);
        assert!(body["refresh_token"].is_string());

        // Online: the user of `sub`.
        let body = json_of(&access_token(
            &exchange(ONLINE_ACCESS_TOKEN, json!("0")),
            &cx,
            &mut tokens,
        ));
        assert_eq!(body["associated_user"]["id"], 7);
        assert_eq!(body["associated_user"]["account_owner"], false);
        assert_eq!(body["expires_in"], 86_399);

        let response = access_token(&exchange("urn:other", json!("0")), &cx, &mut tokens);
        assert_eq!(response.status, 400);
        assert_eq!(json_of(&response)["error"], "invalid_requested_token_type");
    }

    #[test]
    fn token_exchange_refuses_invalid_session_tokens() {
        let app = app();
        let cx = Context {
            app: &app,
            shops: &TestShops,
            now: now(),
        };
        let mut tokens = Tokens::with_seed(1);
        let exchange = |token: &str| {
            post_json(
                "/admin/oauth/access_token",
                json!({
                    "client_id": CLIENT_ID,
                    "client_secret": SECRET,
                    "grant_type": TOKEN_EXCHANGE,
                    "subject_token": token,
                    "subject_token_type": ID_TOKEN,
                    "requested_token_type": OFFLINE_ACCESS_TOKEN,
                }),
            )
        };
        let refused = |response: HttpResponse, why: &str| {
            assert_eq!(response.status, 400, "{why}");
            assert_eq!(
                json_of(&response)["error"],
                "invalid_subject_token",
                "{why}"
            );
        };

        // Expired: a minute, and the leeway, later.
        let (token, _) = mint_session_token(&cx, &mut tokens, SHOP, 1);
        let later = Context {
            now: now() + Duration::seconds(71),
            ..cx
        };
        refused(
            access_token(&exchange(&token), &later, &mut tokens),
            "expired",
        );
        // Another app's (wrong `aud`).
        let other = AppAuth {
            client_id: "other".to_string(),
            ..app.clone()
        };
        let other_cx = Context {
            app: &other,
            shops: &TestShops,
            now: now(),
        };
        let (foreign, _) = mint_session_token(&other_cx, &mut tokens, SHOP, 1);
        refused(
            access_token(&exchange(&foreign), &cx, &mut tokens),
            "wrong aud",
        );
        // Signed with another secret.
        let forged = AppAuth {
            client_secret: "not-hush".to_string(),
            ..app.clone()
        };
        let forged_cx = Context {
            app: &forged,
            shops: &TestShops,
            now: now(),
        };
        let (forged, _) = mint_session_token(&forged_cx, &mut tokens, SHOP, 1);
        refused(
            access_token(&exchange(&forged), &cx, &mut tokens),
            "wrong secret",
        );
        // For a shop lsf does not have.
        let (unknown, _) = mint_session_token(&cx, &mut tokens, "nope.myshopify.com", 1);
        refused(
            access_token(&exchange(&unknown), &cx, &mut tokens),
            "unknown shop",
        );
        // Sent to another shop than its own.
        let mut to_other = exchange(&token);
        to_other
            .query
            .push(("shop".to_string(), "test-1".to_string()));
        refused(access_token(&to_other, &cx, &mut tokens), "another shop");
        refused(
            access_token(&exchange("not.a.jwt"), &cx, &mut tokens),
            "garbage",
        );
    }

    #[test]
    fn migrates_a_non_expiring_offline_token() {
        let app = app();
        let cx = Context {
            app: &app,
            shops: &TestShops,
            now: now(),
        };
        let mut tokens = Tokens::with_seed(1);
        let old = tokens.install(SHOP, CLIENT_ID, &app.scopes, now());
        let migrate = |token: &str, expiring: &str| {
            post_form(
                "/admin/oauth/access_token",
                &[
                    ("grant_type", TOKEN_EXCHANGE),
                    ("client_id", CLIENT_ID),
                    ("client_secret", SECRET),
                    ("subject_token", token),
                    ("subject_token_type", OFFLINE_ACCESS_TOKEN),
                    ("requested_token_type", OFFLINE_ACCESS_TOKEN),
                    ("expiring", expiring),
                ],
            )
        };
        let response = access_token(&migrate(&old, "0"), &cx, &mut tokens);
        assert_eq!(json_of(&response)["error"], "invalid_requested_token_type");
        let body = json_of(&access_token(&migrate(&old, "1"), &cx, &mut tokens));
        assert_eq!(body["expires_in"], 3_600);
        assert!(body["refresh_token"].is_string());
        assert!(tokens.lookup(&old, now()).is_err());
        let response = access_token(&migrate("shpat_unknown", "1"), &cx, &mut tokens);
        assert_eq!(response.status, 400);
        assert_eq!(json_of(&response)["error"], "invalid_subject_token");
    }

    #[test]
    fn client_credentials_grant() {
        let app = app();
        let cx = Context {
            app: &app,
            shops: &TestShops,
            now: now(),
        };
        let mut tokens = Tokens::with_seed(1);
        let request = |shop: Option<&str>| {
            let mut request = post_json(
                "/admin/oauth/access_token",
                json!({"client_id": CLIENT_ID, "client_secret": SECRET, "grant_type": "client_credentials"}),
            );
            if let Some(shop) = shop {
                request.headers.insert("host".to_string(), shop.to_string());
            }
            request
        };
        // The shop of the `Host` header, as a backend that keeps it sends.
        let response = access_token(&request(Some("test-1.myshopify.com:443")), &cx, &mut tokens);
        let body = json_of(&response);
        assert_eq!(body["expires_in"], 86_399);
        assert_eq!(body["scope"], "write_products,read_orders");
        assert_eq!(
            tokens
                .lookup(body["access_token"].as_str().unwrap(), now())
                .unwrap()
                .shop,
            "test-1.myshopify.com"
        );
        let response = access_token(&request(Some("nope.myshopify.com")), &cx, &mut tokens);
        assert_eq!(json_of(&response)["error"], "shop_not_permitted");

        let mut assertion = request(None);
        assertion.body = serde_json::to_vec(&json!({
            "client_id": CLIENT_ID,
            "grant_type": "client_credentials",
            "client_assertion": "eyJ…",
            "client_assertion_type": "urn:ietf:params:oauth:client-assertion-type:jwt-bearer",
        }))
        .unwrap();
        assert_eq!(
            json_of(&access_token(&assertion, &cx, &mut tokens))["error"],
            "invalid_client"
        );
    }

    #[test]
    fn refuses_bad_clients_and_redirects() {
        let app = app();
        let cx = Context {
            app: &app,
            shops: &TestShops,
            now: now(),
        };
        let mut tokens = Tokens::with_seed(1);

        // The authorization page.
        let mut bad_client = authorize_query(&[]);
        bad_client[0].1 = "nope";
        let response = authorize(
            &get("/admin/oauth/authorize", &bad_client),
            &cx,
            &mut tokens,
        );
        assert_eq!(response.status, 400);
        assert!(String::from_utf8_lossy(&response.body).contains(
            "Oauth error application_cannot_be_found: Could not find Shopify API application with api_key nope"
        ));
        let mut bad_redirect = authorize_query(&[]);
        bad_redirect[2].1 = "https://evil.example/auth/callback";
        let response = authorize(
            &get("/admin/oauth/authorize", &bad_redirect),
            &cx,
            &mut tokens,
        );
        assert_eq!(response.status, 400);
        let page = String::from_utf8_lossy(&response.body).to_string();
        assert!(page.contains("Oauth error invalid_request: The redirect_uri is not whitelisted"));
        assert!(
            page.contains("http://localhost:3000/auth/callback"),
            "{page}"
        );
        // The URL of shopify.app.toml itself is allowed too.
        let mut toml_redirect = authorize_query(&[]);
        toml_redirect[2].1 = "https://example.trycloudflare.com/auth/callback";
        let (url, _) = location(&authorize(
            &get("/admin/oauth/authorize", &toml_redirect),
            &cx,
            &mut tokens,
        ));
        assert_eq!(url, "https://example.trycloudflare.com/auth/callback");
        let response = authorize(
            &get(
                "/admin/oauth/authorize",
                &authorize_query(&[("shop", "nope.myshopify.com")]),
            ),
            &cx,
            &mut tokens,
        );
        assert_eq!(response.status, 404);
        let mut bad_scope = authorize_query(&[]);
        bad_scope[1].1 = "read_products,manage_everything";
        let response = authorize(&get("/admin/oauth/authorize", &bad_scope), &cx, &mut tokens);
        assert!(String::from_utf8_lossy(&response.body).contains("Oauth error invalid_scope"));

        // The token endpoint.
        let (_, query) = location(&authorize(
            &get("/admin/oauth/authorize", &authorize_query(&[])),
            &cx,
            &mut tokens,
        ));
        let code = value(&query, "code").to_string();
        let response = access_token(
            &post_json(
                "/admin/oauth/access_token",
                json!({"client_id": "nope", "client_secret": SECRET, "code": code}),
            ),
            &cx,
            &mut tokens,
        );
        assert_eq!(response.status, 400);
        assert_eq!(
            json_of(&response),
            json!({
                "error": "application_cannot_be_found",
                "error_description": "Could not find Shopify API application with api_key nope",
            })
        );
        let response = access_token(
            &post_json(
                "/admin/oauth/access_token",
                json!({"client_id": CLIENT_ID, "client_secret": "wrong", "code": code}),
            ),
            &cx,
            &mut tokens,
        );
        assert_eq!(response.status, 401);
        assert_eq!(json_of(&response)["error"], "invalid_client");
        // The code was not spent by those: it still works, on its own shop only.
        let mut elsewhere = post_json(
            "/admin/oauth/access_token",
            json!({"client_id": CLIENT_ID, "client_secret": SECRET, "code": code}),
        );
        elsewhere
            .query
            .push(("shop".to_string(), "test-1".to_string()));
        assert_eq!(access_token(&elsewhere, &cx, &mut tokens).status, 400);
        let mine = json!({"client_id": CLIENT_ID, "client_secret": SECRET, "code": code});
        let response = access_token(
            &post_json("/admin/oauth/access_token", mine),
            &cx,
            &mut tokens,
        );
        assert_eq!(response.status, 200);
        let response = access_token(
            &post_json(
                "/admin/oauth/access_token",
                json!({"client_id": CLIENT_ID, "client_secret": SECRET, "grant_type": "password"}),
            ),
            &cx,
            &mut tokens,
        );
        assert_eq!(json_of(&response)["error"], "unsupported_grant_type");
        let mut garbage = post_json("/admin/oauth/access_token", json!({}));
        garbage.body = b"{not json".to_vec();
        assert_eq!(access_token(&garbage, &cx, &mut tokens).status, 400);
    }

    #[test]
    fn signs_the_query_a_redirect_uri_already_has() {
        let app = AppAuth {
            redirect_urls: None,
            ..app()
        };
        let cx = Context {
            app: &app,
            shops: &TestShops,
            now: now(),
        };
        let mut tokens = Tokens::with_seed(1);
        let mut query = authorize_query(&[]);
        query[2].1 = "http://localhost:3000/auth/callback?app=1";
        let (url, query) = location(&authorize(
            &get("/admin/oauth/authorize", &query),
            &cx,
            &mut tokens,
        ));
        assert_eq!(url, "http://localhost:3000/auth/callback");
        assert_eq!(value(&query, "app"), "1");
        assert!(verify_query_hmac(SECRET, query.iter().cloned()));
        // The online grant of a shop where the app is not installed yet installs it.
        let online = authorize_query(&[("grant_options[]", "per-user"), ("shop", "test-1")]);
        let (_, query) = location(&authorize(
            &get("/admin/oauth/authorize", &online),
            &cx,
            &mut tokens,
        ));
        let response = access_token(
            &post_json(
                "/admin/oauth/access_token",
                json!({"client_id": CLIENT_ID, "client_secret": SECRET, "code": value(&query, "code")}),
            ),
            &cx,
            &mut tokens,
        );
        assert_eq!(response.status, 200);
        assert!(
            tokens
                .installed_scopes("test-1.myshopify.com", CLIENT_ID)
                .is_some()
        );
    }

    #[test]
    fn expired_codes_are_refused() {
        let app = app();
        let cx = Context {
            app: &app,
            shops: &TestShops,
            now: now(),
        };
        let mut tokens = Tokens::with_seed(1);
        let (_, query) = location(&authorize(
            &get("/admin/oauth/authorize", &authorize_query(&[])),
            &cx,
            &mut tokens,
        ));
        let later = Context {
            now: now() + Duration::seconds(CODE_LIFETIME),
            ..cx
        };
        let response = access_token(
            &post_json(
                "/admin/oauth/access_token",
                json!({"client_id": CLIENT_ID, "client_secret": SECRET, "code": value(&query, "code")}),
            ),
            &later,
            &mut tokens,
        );
        assert_eq!(response.status, 400);
    }

    #[test]
    fn mints_session_tokens_for_tests() {
        let app = app();
        let cx = Context {
            app: &app,
            shops: &TestShops,
            now: now(),
        };
        let mut tokens = Tokens::with_seed(1);
        let response = session_token_endpoint(
            &post_json(
                "/__lsf/admin/session-token",
                json!({"shop": "test-1", "user": 5}),
            ),
            &cx,
            &mut tokens,
        );
        assert_eq!(response.status, 200);
        let body = json_of(&response);
        let token = body["token"].as_str().unwrap();
        assert_eq!(body["authorization"], format!("Bearer {token}"));
        assert_eq!(body["shop"], "test-1.myshopify.com");
        let claims = session_token::verify(token, SECRET, CLIENT_ID, now().timestamp()).unwrap();
        assert_eq!(claims.sub, "5");
        assert_eq!(claims.dest, "https://test-1.myshopify.com");
        assert_eq!(body["claims"], serde_json::to_value(&claims).unwrap());

        // Without a body: the default shop and its owner.
        let mut empty = post_json("/__lsf/admin/session-token", json!({}));
        empty.body.clear();
        let body = json_of(&session_token_endpoint(&empty, &cx, &mut tokens));
        assert_eq!(body["claims"]["sub"], "1");
        assert_eq!(body["shop"], SHOP);
        let response = session_token_endpoint(
            &post_json("/__lsf/admin/session-token", json!({"shop": "nope"})),
            &cx,
            &mut tokens,
        );
        assert_eq!(response.status, 404);
    }

    #[test]
    fn launches_the_embedded_app_as_the_admin_does() {
        let app = app();
        let cx = Context {
            app: &app,
            shops: &TestShops,
            now: now(),
        };
        let mut tokens = Tokens::with_seed(1);
        let response = launch(
            &get(
                "/__lsf/admin/launch",
                &[("shop", "test-1"), ("path", "/app/settings?tab=billing")],
            ),
            &cx,
            &mut tokens,
        );
        let (url, query) = location(&response);
        assert_eq!(url, "http://localhost:3000/app/settings");
        let names: Vec<&str> = query.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            [
                "embedded",
                "hmac",
                "host",
                "id_token",
                "locale",
                "session",
                "shop",
                "tab",
                "timestamp"
            ]
        );
        assert_eq!(value(&query, "embedded"), "1");
        assert_eq!(value(&query, "locale"), "en");
        assert_eq!(value(&query, "shop"), "test-1.myshopify.com");
        assert!(verify_query_hmac(SECRET, query.iter().cloned()));
        let claims = session_token::verify(
            value(&query, "id_token"),
            SECRET,
            CLIENT_ID,
            now().timestamp(),
        )
        .unwrap();
        assert_eq!(claims.shop(), "test-1.myshopify.com");
        assert_eq!(value(&query, "session"), claims.sid);
        assert_eq!(
            STANDARD_NO_PAD.decode(value(&query, "host")).unwrap(),
            b"admin.shopify.com/store/test-1"
        );

        // `application_url` when there is no `--app-url`.
        let no_local = AppAuth {
            app_url: None,
            ..app.clone()
        };
        let cx_toml = Context {
            app: &no_local,
            shops: &TestShops,
            now: now(),
        };
        let (url, _) = location(&launch(
            &get("/__lsf/admin/launch", &[]),
            &cx_toml,
            &mut tokens,
        ));
        assert_eq!(url, "https://example.trycloudflare.com");
        let no_url = AppAuth {
            app_url: None,
            application_url: None,
            ..app.clone()
        };
        let cx_none = Context {
            app: &no_url,
            shops: &TestShops,
            now: now(),
        };
        assert_eq!(
            launch(&get("/__lsf/admin/launch", &[]), &cx_none, &mut tokens).status,
            400
        );
        let response = launch(
            &get("/__lsf/admin/launch", &[("path", "//evil.example")]),
            &cx,
            &mut tokens,
        );
        assert_eq!(response.status, 400);
    }

    #[test]
    fn routes_its_endpoints_only() {
        let app = app();
        let cx = Context {
            app: &app,
            shops: &TestShops,
            now: now(),
        };
        let mut tokens = Tokens::with_seed(1);
        assert!(
            handle(
                &get("/admin/api/2026-10/graphql.json", &[]),
                &cx,
                &mut tokens
            )
            .is_none()
        );
        assert_eq!(
            handle(&get("/admin/oauth/access_token", &[]), &cx, &mut tokens)
                .unwrap()
                .status,
            405
        );
        assert_eq!(
            handle(
                &get("/admin/oauth/authorize", &authorize_query(&[])),
                &cx,
                &mut tokens
            )
            .unwrap()
            .status,
            302
        );
        assert_eq!(
            handle(&get("/__lsf/admin/launch", &[]), &cx, &mut tokens)
                .unwrap()
                .status,
            302
        );
    }

    #[test]
    fn matches_redirects_and_app_urls() {
        let app = app();
        assert!(app.redirect_allowed("https://example.trycloudflare.com/auth/callback"));
        assert!(app.redirect_allowed("http://localhost:3000/auth/callback"));
        assert!(!app.redirect_allowed("http://localhost:3000/auth/callback/"));
        assert!(!app.redirect_allowed("https://example.trycloudflare.com/other"));
        let without_app = AppAuth {
            redirect_urls: None,
            ..app.clone()
        };
        assert!(without_app.redirect_allowed("http://anything.example/cb"));
        assert!(!without_app.redirect_allowed("javascript:alert(1)"));
        let with_path = AppAuth {
            application_url: Some("https://example.trycloudflare.com/app".to_string()),
            ..app.clone()
        };
        assert_eq!(
            with_path.app_url().as_deref(),
            Some("http://localhost:3000/app")
        );
        assert_eq!(normalize_shop("Test-1"), "test-1.myshopify.com");
        assert_eq!(
            normalize_shop("https://a.myshopify.com/"),
            "a.myshopify.com"
        );
    }
}
