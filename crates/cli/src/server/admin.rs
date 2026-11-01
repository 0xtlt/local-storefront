//! The Admin API of the server: `lsf-admin`'s [`AdminApi`], built from the store data and the
//! app of `--app`, and routed before the storefront and the checkout. Its requests (the GraphQL
//! Admin API, OAuth, `/__lsf/admin`, `/__lsf/webhooks`) are answered here; what the checkout
//! places is recorded in its default shop. What the backend changes in the default shop shows
//! in the storefront and the checkout: see `store_sync`.

use std::path::Path;
use std::sync::Arc;

use lsf_admin::api::{AdminApi, AdminOptions, HttpRequest, ThrottleSetting};
use lsf_admin::auth::SecretSource;
use lsf_admin::domains::checkouts;
use lsf_admin::state::ShopState;
use lsf_admin::version::Served;
use lsf_admin::webhooks::DeliveryOptions;
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
    /// The theme the server serves is the shops' main theme: its directory is `theme`.
    pub fn new(
        store: &Arc<Store>,
        settings: &AdminSettings,
        app_dir: Option<&Path>,
        theme: &Path,
    ) -> Admin {
        let app = app_dir.and_then(|dir| lsf_checkout::app::load(dir).ok());
        let (secret, secret_source) =
            lsf_admin::auth::client_secret(settings.client_secret.as_deref(), app_dir);
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
        .with_fetch(lsf_admin::sources::FetchOptions {
            allow_local: settings.fetch_local,
        })
        .with_sources(lsf_admin::sources::Stubs::configured(
            settings.sources.iter().cloned(),
        ))
        .with_theme(theme.to_path_buf())
        // A token a reset, a shop deletion or an uninstall revoked: said once per token, even
        // with `--quiet`, since the backend that sends it fails until it forgets it.
        .with_log(Arc::new(|line: &str| eprintln!("admin:  {line}")));
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
        lines.push(format!("        client secret: {}", self.secret_line()));
        lines.push(format!("webhooks: {}", self.api.webhooks().summary()));
        lines
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
    let ours = path.starts_with("/admin/")
        || path == "/__lsf/admin"
        || path.starts_with("/__lsf/admin/")
        || path == "/__lsf/webhooks"
        || path.starts_with("/__lsf/webhooks/");
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
    let response = admin.api.handle(&request)?;
    Some(Reply {
        status: response.status,
        headers: response.headers,
        body: response.body,
    })
}

/// Reports a change of a storefront cart to the default shop of the Admin API: `carts/create`
/// for a cart that had nothing, `carts/update` after (see `lsf_admin::domains::checkouts`).
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
            key: lsf_core::drops::cart::line_key(line),
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
