//! The Admin API of the server: `lsf-admin`'s [`AdminApi`], built from the store data and the
//! app of `--app`, and routed before the storefront and the checkout. Its requests (the GraphQL
//! Admin API, OAuth, `/__lsf/admin`, `/__lsf/webhooks`) are answered here; what the checkout
//! places is recorded in its default shop. What the backend changes in the default shop shows
//! in the storefront and the checkout: see `store_sync`.

use std::path::Path;
use std::sync::Arc;

use lsf_admin::api::{AdminApi, AdminOptions, HttpRequest, ThrottleSetting};
use lsf_admin::auth::SecretSource;
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
}

/// The Admin API of a server, and where its client secret came from.
pub struct Admin {
    pub api: AdminApi,
    pub secret: String,
    pub secret_source: SecretSource,
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
        let api = AdminApi::new(
            store,
            AdminOptions {
                app,
                client_secret: secret.clone(),
                app_url: settings.app_url.clone(),
                throttle: settings.throttle,
                delivery: DeliveryOptions::default(),
                token_seed: None,
            },
        )
        .with_versions(settings.versions.clone())
        .with_theme(theme.to_path_buf());
        api.sync_store(store.clone());
        Admin {
            api,
            secret,
            secret_source,
        }
    }

    /// What the server prints of its Admin API when it starts, under `ready:`.
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
        if self.secret_source == SecretSource::Default {
            lines.push(format!(
                "        client secret: {}   (lsf's own: give --client-secret, or SHOPIFY_API_SECRET in the app's .env)",
                self.secret
            ));
        }
        lines.push(format!("webhooks: {}", self.api.webhooks().summary()));
        lines
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
