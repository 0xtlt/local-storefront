//! The parts of lsf a server runs (`--only`): the storefront of the theme, the checkout of the
//! app (its Functions, its UI extensions, its scenarios), and the Admin API (OAuth, webhooks,
//! lsf's admin, managed pricing). A part that is off answers `404` on its routes, with an
//! `X-LSF-Reason` that says which `--only` would serve it.

use serde_json::{Value as Json, json};

use super::reply::Reply;
use super::{Incoming, ServerState};

/// One part of what a server runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Part {
    /// The storefront rendered from the theme.
    Theme,
    /// What the app of `--app` brings to the shop: the checkout, its Functions, its UI
    /// extensions, the scenarios, the app's home page.
    Checkout,
    /// The Admin API, OAuth, webhooks, the app proxy, lsf's admin, managed pricing.
    Admin,
}

impl Part {
    pub fn name(self) -> &'static str {
        match self {
            Part::Theme => "theme",
            Part::Checkout => "checkout",
            Part::Admin => "admin",
        }
    }
}

impl std::fmt::Display for Part {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The parts a server runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Parts {
    pub theme: bool,
    pub checkout: bool,
    pub admin: bool,
}

impl Parts {
    /// Every part: `serve --app`.
    pub const ALL: Parts = Parts {
        theme: true,
        checkout: true,
        admin: true,
    };

    /// The parts of a list, each named once or more.
    pub fn of(list: &[Part]) -> Parts {
        Parts {
            theme: list.contains(&Part::Theme),
            checkout: list.contains(&Part::Checkout),
            admin: list.contains(&Part::Admin),
        }
    }

    pub fn has(self, part: Part) -> bool {
        match part {
            Part::Theme => self.theme,
            Part::Checkout => self.checkout,
            Part::Admin => self.admin,
        }
    }

    /// These parts and `part`.
    pub fn with(self, part: Part) -> Parts {
        let mut parts = self;
        match part {
            Part::Theme => parts.theme = true,
            Part::Checkout => parts.checkout = true,
            Part::Admin => parts.admin = true,
        }
        parts
    }

    /// The parts on, always in the same order: theme, checkout, admin.
    pub fn list(self) -> Vec<Part> {
        [Part::Theme, Part::Checkout, Part::Admin]
            .into_iter()
            .filter(|part| self.has(*part))
            .collect()
    }

    /// `theme, checkout, admin`: what the `serving:` line says.
    pub fn describe(self) -> String {
        self.names().join(", ")
    }

    /// `theme,checkout,admin`: the value of `--only` that serves these parts.
    pub fn only(self) -> String {
        self.names().join(",")
    }

    fn names(self) -> Vec<&'static str> {
        self.list().into_iter().map(Part::name).collect()
    }

    /// The parts as `GET /__lsf/status` and `GET /__lsf/admin` list them.
    pub fn to_json(self) -> Json {
        json!(self.names())
    }
}

impl ServerState {
    /// The parts this server runs: the theme when it has one, the checkout and the Admin API
    /// when it was given them.
    pub fn parts(&self) -> Parts {
        Parts {
            theme: self.app.has_theme(),
            checkout: self.options.checkout.is_some(),
            admin: self.admin.is_some(),
        }
    }
}

/// The part that owns the path of `incoming`, when that part is off and nothing else answers
/// the path. The theme owns what nothing else does: the caller answers for it last.
pub fn off_part_of(state: &ServerState, incoming: &Incoming) -> Option<Part> {
    let parts = state.parts();
    let path = incoming.path.as_str();
    if !parts.admin && is_admin_path(path) {
        return Some(Part::Admin);
    }
    if !parts.checkout && is_checkout_control_path(path) {
        return Some(Part::Checkout);
    }
    // Without a theme, nothing stands in for the checkout's pages, nor for the app's home page
    // at `/`, which the checkout part serves (`app dev`).
    if !parts.checkout && !parts.theme {
        let (_, _, local) = state.localize(&state.store_of(incoming), path);
        let first = local.trim_start_matches('/').split('/').next();
        if matches!(first, Some("" | "checkout" | "checkouts")) {
            return Some(Part::Checkout);
        }
    }
    // A profile renders a page of the theme.
    if !parts.theme && path.trim_end_matches('/') == "/__lsf/profile" {
        return Some(Part::Theme);
    }
    None
}

/// The paths of the Admin API: GraphQL and OAuth, lsf's admin, the Partner API, the control
/// endpoints of the Admin API and of webhooks, and Polaris.
fn is_admin_path(path: &str) -> bool {
    let under = |prefix: &str| path == prefix || path.starts_with(&format!("{prefix}/"));
    under("/admin")
        || under("/__lsf/admin")
        || under("/__lsf/webhooks")
        || under("/__lsf/polaris")
        || under(lsf_app::admin::graphiql::PATH)
        || path.starts_with(super::polaris::PREFIX)
        || lsf_app::admin::partner::parse_path(path).is_some()
}

/// The control endpoints of the checkout: its state, its home page, the files of its extensions.
fn is_checkout_control_path(path: &str) -> bool {
    let under = |prefix: &str| path == prefix || path.starts_with(&format!("{prefix}/"));
    under("/__lsf/checkout") || under("/__lsf/checkouts") || path.starts_with("/__lsf/extensions/")
}

/// Why `part` is not served: the parts that are, and the `--only` that would serve it.
pub fn reason(state: &ServerState, part: Part) -> String {
    let parts = state.parts();
    let wanted = parts.with(part);
    // Checkout and admin need the app: say so when the server has none.
    let app = if part != Part::Theme && !parts.checkout && !parts.admin {
        " --app <dir>"
    } else {
        ""
    };
    let serving = if parts == Parts::default() {
        "nothing".to_string()
    } else {
        parts.describe()
    };
    format!(
        "the {part} part is not served (serving: {serving}): `serve{app} --only {}` serves it",
        wanted.only()
    )
}

/// The answer of a route of `part`, which is off: `404`, and why in `X-LSF-Reason`, in the
/// body, and on the terminal.
pub fn not_served(state: &ServerState, part: Part) -> Reply {
    let reason = reason(state, part);
    if !state.options.quiet {
        eprintln!("    404: {reason}");
    }
    Reply::text(404, format!("Not found: {reason}.\n"))
        .header("x-lsf-reason", super::storefront::header_text(&reason))
}
