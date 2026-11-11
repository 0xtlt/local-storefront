//! The Admin API of the server: lsf-app's [`AdminApi`], built from the store data and the
//! app of `--app`, and routed before the storefront and the checkout. Its requests (the GraphQL
//! Admin API, OAuth, `/__lsf/admin`, `/__lsf/webhooks`) are answered here; what the checkout
//! places is recorded in its default shop. What the backend changes in the default shop shows
//! in the storefront and the checkout: see `store_sync`.

use std::path::Path;
use std::sync::Arc;

use lsf_app::admin::api::{AdminApi, AdminOptions, HttpRequest, ThrottleSetting};
use lsf_app::admin::auth::SecretSource;
use lsf_app::admin::domains::checkouts;
use lsf_app::admin::state::ShopState;
use lsf_app::admin::version::Served;
use lsf_app::admin::webhooks::DeliveryOptions;
use lsf_core::Store;

use super::reply::Reply;
use super::{Incoming, ServerState};

/// How the server sets up its Admin API: the options of `serve` and `app dev`.
#[derive(Clone, Debug, Default)]
pub struct AdminSettings {
    /// `--client-secret` (`LSF_CLIENT_SECRET`).
    pub client_secret: Option<String>,
    /// `--app-url` (`LSF_APP_URL`): where the backend of the app listens.
    pub app_url: Option<String>,
    /// `--admin-throttle`.
    pub throttle: ThrottleSetting,
    /// `--admin-versions` (`LSF_ADMIN_VERSIONS`): the versions of the Admin API served, the
    /// default one alone unless told others.
    pub versions: Served,
    /// `--admin-fetch-local` (`LSF_ADMIN_FETCH_LOCAL`): the sources of the theme and redirect
    /// mutations may be on this machine or its network.
    pub fetch_local: bool,
    /// `--admin-source <url>=<file>`: the URLs lsf answers itself with a file, never fetched.
    pub sources: Vec<(String, std::path::PathBuf)>,
    /// The app of `--app`, whose client id, scopes and webhooks the Admin API has. `None` takes
    /// the app of the checkout, else none (a local client id, every scope).
    pub app_dir: Option<std::path::PathBuf>,
    /// `--pricing-plans <file>` (`LSF_PRICING_PLANS`): the plans of the app's managed pricing,
    /// read from the file.
    pub pricing: Option<(
        lsf_app::admin::domains::app::pricing::Pricing,
        std::path::PathBuf,
    )>,
}

/// The Admin API of a server, and where its client secret came from.
pub struct Admin {
    pub api: AdminApi,
    pub secret: String,
    pub secret_source: SecretSource,
    /// The `.env` the secret was read from, when it was.
    secret_file: Option<std::path::PathBuf>,
}

impl Admin {
    /// The Admin API of the store data, for the app in `app_dir` when there is one, with store
    /// sync on: what the backend changes in the default shop is projected onto `store`, the
    /// store of the files.
    /// The theme the server serves is the shops' main theme: its directory is `theme`. Unless
    /// `quiet`, the terminal says when the app is uninstalled from a shop or installed again.
    pub fn new(
        store: &Arc<Store>,
        settings: &AdminSettings,
        app_dir: Option<&Path>,
        theme: &Path,
        quiet: bool,
    ) -> Admin {
        let app = app_dir.and_then(|dir| lsf_app::config::load(dir).ok());
        let (secret, secret_source) =
            lsf_app::admin::auth::client_secret(settings.client_secret.as_deref(), app_dir);
        let secret_file = app_dir.map(|dir| dir.join(".env"));
        let api = AdminApi::new(
            store,
            AdminOptions {
                app,
                client_secret: secret.clone(),
                app_url: settings.app_url.clone(),
                throttle: settings.throttle,
                // A webhook the backend refuses says which secret lsf signed it with.
                delivery: DeliveryOptions {
                    refusal_hint: Some(secret_source.refusal(&secret, secret_file.as_deref())),
                    ..DeliveryOptions::default()
                },
                token_seed: None,
            },
        )
        .with_versions(settings.versions.clone())
        .with_fetch(lsf_app::admin::sources::FetchOptions {
            allow_local: settings.fetch_local,
        })
        .with_sources(lsf_app::admin::sources::Stubs::configured(
            settings.sources.iter().cloned(),
        ))
        .with_theme(theme.to_path_buf())
        // A token a reset, a shop deletion or an uninstall revoked: said once per token, even
        // with `--quiet`, since the backend that sends it fails until it forgets it.
        .with_log(Arc::new(|line: &str| eprintln!("admin:  {line}")));
        let api = match &settings.pricing {
            Some((pricing, path)) => api.with_pricing(pricing.clone(), path.display().to_string()),
            None => api,
        };
        // What an uninstall stops and a reinstall brings back: the app's extensions and Functions.
        let api = if quiet {
            api
        } else {
            api.with_notices(Arc::new(|line: &str| eprintln!("admin:  {line}")))
        };
        // A webhook the backend refuses (`401`, `403`): said once per delivery, `--quiet` or
        // not, since every webhook fails until the two secrets agree.
        api.webhooks()
            .deliveries()
            .print_refusals(Arc::new(|line: &str| eprintln!("webhooks: {line}")));
        api.sync_store(store.clone());
        Admin {
            api,
            secret,
            secret_source,
            secret_file,
        }
    }

    /// lsf's admin links to these pages of the server, in a new tab: the storefront and the
    /// checkout it runs.
    pub fn with_previews(mut self, previews: Vec<lsf_app::admin::api::Preview>) -> Admin {
        self.api = self.api.with_previews(previews);
        self
    }

    /// What the server prints of its Admin API when it starts with an app, before `ready:`: a
    /// tool that reads up to `ready:` has the token.
    pub fn banner(&self, origin: &str) -> Vec<String> {
        let shop = self.api.default_shop();
        let mut lines = vec![format!("admin:  {origin}{}", self.api.graphql_path())];
        let primary = self.api.versions().primary();
        let others: Vec<String> = self
            .api
            .versions()
            .all()
            .iter()
            .filter(|version| **version != primary)
            .map(|version| format!("{version} ({})", version.status().as_str()))
            .collect();
        if !others.is_empty() {
            lines.push(format!("        also API version {}", others.join(", ")));
        }
        lines.push(format!(
            "        X-Shopify-Access-Token: {}   (shop: {shop})",
            self.api.token(&shop)
        ));
        if let Some(app) = self.api.app()
            && let Some(source) = app.client_id_source.describe()
        {
            lines.push(format!(
                "        client id: {}   ({source})",
                self.api.client_id()
            ));
        }
        lines.push(format!("        client secret: {}", self.secret_line()));
        let mut webhooks = self.api.webhooks().summary();
        if let Some(app) = self.api.app()
            && !app.config.event_subscriptions.is_empty()
        {
            let handles: Vec<&str> = app
                .config
                .event_subscriptions
                .iter()
                .map(|event| event.handle.as_str())
                .collect();
            webhooks.push_str(&format!("; Events ([events]): {}", handles.join(", ")));
        }
        lines.push(format!("webhooks: {webhooks}"));
        lines
    }

    /// The path of lsf's admin with the app open (`/admin/apps/<handle>`), when the server runs
    /// an embedded app: the client id names it when the app has no handle. `None` without an
    /// app, or for an app whose `shopify.app.toml` says `embedded = false`.
    pub fn embedded_path(&self) -> Option<String> {
        let app = self.api.app()?;
        if app.config.embedded == Some(false) {
            return None;
        }
        let key = app
            .admin_handle()
            .unwrap_or_else(|| self.api.client_id().to_string());
        Some(format!("/admin/apps/{key}"))
    }

    /// What the server prints of lsf's admin when it starts with an embedded app: where the app
    /// opens in it, and the App Bridge its pages load; or what is missing to open it.
    pub fn embedded_banner(&self, origin: &str) -> Option<String> {
        let path = self.embedded_path()?;
        Some(match self.api.app_url() {
            Some(_) => format!(
                "embedded: {origin}{path}   (the app in lsf's admin; its pages load {origin}/__lsf/admin/app-bridge.js)"
            ),
            None => format!(
                "embedded: {origin}{path} opens the app once lsf knows its URL: give --app-url (where the backend listens)"
            ),
        })
    }

    /// What the server prints of the app's managed pricing when plans are declared: how many,
    /// where from, and the page where the merchant chooses one.
    /// An app with no `handle` in `shopify.app.toml` is told that Shopify's page needs one: lsf
    /// serves the page at its client id, where Shopify's fails.
    pub fn pricing_banner(&self, origin: &str) -> Option<String> {
        let (pricing, source) = self.api.pricing()?;
        let handle = self.api.app().and_then(|app| app.handle.clone());
        let no_handle = self.api.app().is_some() && handle.is_none();
        let key = self
            .api
            .app()
            .and_then(|app| app.admin_handle())
            .unwrap_or_else(|| self.api.client_id().to_string());
        Some(format!(
            "pricing: {} ({source}): {origin}/admin/charges/{key}/pricing_plans   (managed pricing: the Billing API creates no charge{})",
            crate::output::count(pricing.plans.len(), "plan", "plans"),
            if no_handle {
                "; shopify.app.toml has no handle, which Shopify's page needs in place of the client id"
            } else {
                ""
            },
        ))
    }

    /// What the server prints of the Partner API when plans are declared: where an app on Shopify
    /// App Pricing reads its plan, with the token and the app's GID it takes.
    pub fn partner_banner(&self, origin: &str) -> Option<String> {
        self.api.pricing()?;
        let client_id = self.api.client_id();
        let id = lsf_app::admin::partner::partner_id(client_id);
        Some(format!(
            "partner: {}   (Partner API: activeSubscription and events of gid://shopify/App/{}; token {})",
            lsf_app::admin::partner::endpoint(origin, id),
            lsf_app::admin::domains::app::app_id(self.api.app()),
            lsf_app::admin::partner::token(client_id),
        ))
    }

    /// Where the client secret came from: the value only when it is lsf's own, which is no
    /// secret, so that a webhook the backend refuses has a first place to look.
    fn secret_line(&self) -> String {
        self.secret_source
            .describe(&self.secret, self.secret_file.as_deref())
    }
}

/// The answer of the Admin API to a request, `None` when the path is not one of its own.
pub fn route(state: &ServerState, incoming: &Incoming) -> Option<Reply> {
    let admin = state.admin.as_ref()?;
    let path = incoming.path.as_str();
    // The association files of the shop's mobile apps (`mobilePlatformApplicationCreate`),
    // which Shopify serves on the online store's domain; the storefront's 404 without one.
    if path.starts_with("/.well-known/") || path == "/apple-app-site-association" {
        let file = admin.api.association_file(path)?;
        return Some(Reply::new(200, "application/json", file.to_string()));
    }
    let ours = path == "/admin"
        || path.starts_with("/admin/")
        || path == "/__lsf/admin"
        || path.starts_with("/__lsf/admin/")
        || path == "/__lsf/webhooks"
        || path.starts_with("/__lsf/webhooks/")
        // The Partner API: `/{partner_id}/api/{version}/graphql.json`.
        || lsf_app::admin::partner::parse_path(path).is_some();
    if !ours {
        return None;
    }
    let request = HttpRequest {
        method: incoming.method.clone(),
        path: incoming.path.clone(),
        query: incoming.query.clone(),
        headers: incoming.headers.clone(),
        body: incoming.body.to_vec(),
    };
    let mut response = admin.api.handle(&request)?;
    // What the server runs besides the Admin API (`--only`).
    if incoming.method == "GET"
        && path == "/__lsf/admin"
        && response.status == 200
        && let Ok(serde_json::Value::Object(mut status)) =
            serde_json::from_slice::<serde_json::Value>(&response.body)
    {
        status.insert("parts".to_string(), state.parts().to_json());
        response.body = serde_json::Value::Object(status).to_string().into_bytes();
    }
    Some(Reply {
        status: response.status,
        headers: response.headers,
        body: response.body,
    })
}

/// Reports a change of a storefront cart to the default shop of the Admin API: `carts/create`
/// for a cart that had nothing, `carts/update` after (see `lsf_app::admin::domains::checkouts`).
/// Nothing without the Admin API.
pub fn record_cart(state: &ServerState, token: &str, session: &lsf_core::Session) {
    if state.admin.is_none() {
        return;
    }
    let lines: Vec<checkouts::CartLine> = session
        .cart_lines
        .iter()
        .map(|line| checkouts::CartLine {
            variant_id: line.variant_id,
            quantity: line.quantity,
            properties: pairs(&line.properties),
            key: lsf_core::cart::line_key(line),
        })
        .collect();
    let attributes = pairs(&session.cart_attributes);
    let cart = checkouts::Cart {
        token,
        note: &session.cart_note,
        attributes: &attributes,
        lines: &lines,
    };
    change_default_shop(state, |shop| checkouts::record_cart(shop, &cart));
}

/// What the local checkout computed, reported to the default shop of the Admin API:
/// `checkouts/create` the first time, `checkouts/update` when it changed.
pub fn record_checkout(state: &ServerState, checkout: &checkouts::Checkout) {
    change_default_shop(state, |shop| {
        checkouts::record_checkout(shop, checkout);
    });
}

/// The order of a checkout was placed: the checkout is completed (`checkouts/update`).
pub fn complete_checkout(state: &ServerState, cart_token: &str, generation: u64) {
    change_default_shop(state, |shop| {
        checkouts::complete_checkout(shop, cart_token, generation)
    });
}

/// Changes the default shop of the Admin API. Nothing without the Admin API.
fn change_default_shop(state: &ServerState, change: impl FnOnce(&mut ShopState)) {
    if let Some(admin) = &state.admin {
        admin.api.change_shop(&admin.api.default_shop(), change);
    }
}

/// The entries of a map of the session (a cart's attributes, a line's properties) as the Admin
/// API takes them.
pub(super) fn pairs<'a>(
    map: impl IntoIterator<Item = (&'a String, &'a String)>,
) -> Vec<(String, String)> {
    map.into_iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}
