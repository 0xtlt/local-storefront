//! The Admin API of the server: `lsf-admin`'s [`AdminApi`], built from the store data and the
//! app of `--app`, and routed before the storefront and the checkout. Its requests (the GraphQL
//! Admin API, OAuth, `/__lsf/admin`, `/__lsf/webhooks`) are answered here; what the checkout
//! places is recorded in its default shop.

use std::path::Path;

use lsf_admin::api::{AdminApi, AdminOptions, HttpRequest, ThrottleSetting};
use lsf_admin::auth::SecretSource;
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
}

/// The Admin API of a server, and where its client secret came from.
pub struct Admin {
    pub api: AdminApi,
    pub secret: String,
    pub secret_source: SecretSource,
}

impl Admin {
    /// The Admin API of the store data, for the app in `app_dir` when there is one.
    pub fn new(store: &Store, settings: &AdminSettings, app_dir: Option<&Path>) -> Admin {
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
        );
        Admin {
            api,
            secret,
            secret_source,
        }
    }

    /// What the server prints of its Admin API when it starts, under `ready:`.
    pub fn banner(&self, origin: &str) -> Vec<String> {
        let shop = self.api.default_shop();
        let mut lines = vec![
            format!(
                "admin:  {origin}/admin/api/{}/graphql.json",
                lsf_admin::version::ADMIN_2026_10
            ),
            format!(
                "        X-Shopify-Access-Token: {}   (shop: {shop})",
                self.api.token(&shop)
            ),
        ];
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
