//! The versions of the Admin API: the ones lsf knows ([`KNOWN`], the one place that lists them),
//! the ones a server serves ([`Served`]), and which one answers a request.
//!
//! **A version is data.** Each one has, in `crates/app/schemas/admin/<version>/`, what
//! `mise run admin:version -- <version>` writes from Shopify's introspection: its schema
//! (`admin.graphql.br`, with `introspection.graphql`), the access scopes of its fields
//! (`access.json`), the search filters of its connections (`search.json`) and its webhook topics
//! (`topics.json`); and, for the tests, Shopify's example payloads of its webhooks
//! (`tests/fixtures/webhooks/examples-<version>.json`). Each module parses its part the first
//! time a request of the version needs it: [`crate::admin::schema::for_version`],
//! [`crate::admin::auth::scopes::for_version`], [`crate::admin::search::for_version`],
//! [`crate::admin::webhooks::topics::for_version`]. The records are one store for every version; where a
//! version shows a field differently, its [`Adapter`]s say how (see [`adapt`]).
//!
//! **Which version answers.** Shopify serves each version it supports at
//! `/admin/api/<version>/graphql.json`, and a version it no longer supports with the oldest stable
//! one it does, saying so in `X-Shopify-API-Version` ("If your app targets an inaccessible
//! version, Shopify falls forward and responds using the oldest accessible stable version", and
//! "API responses include the `X-Shopify-API-Version` header reflecting the version used to
//! fulfill the request": shopify.dev, "API versioning",
//! <https://shopify.dev/docs/api/usage/versioning>; webhooks fall forward the same way). lsf does
//! the same with the versions it serves ([`Served::resolve`]). A version Shopify still supports
//! but lsf does not serve (`2026-07`, `unstable`, a release candidate the server was not asked to
//! serve) answers 404, saying which versions lsf serves: a local tool says what it does not do,
//! rather than answering with a schema the app did not ask for. The REST Admin API
//! (`/admin/api/<version>/<resource>.json`) is not served either.

pub mod adapt;

use std::fmt;

use serde_json::{Value, json};

pub use adapt::Adapter;

/// The version a server serves when it is not told otherwise, the one resolvers run alone use,
/// and the one the records are written in (see [`adapt`]). The one place lsf names it.
pub const DEFAULT: &str = "2026-10";

/// The oldest version Shopify still serves: an older one falls forward. shopify.dev's release
/// schedule ("API versioning", read on 2026-10-31): 2025-10 was accessible until October 16,
/// 2026; 2026-01 is accessible until January 16, 2027. `mise run admin:version` prints the one
/// the pinned `@shopify/dev-mcp` lists.
pub const OLDEST_ACCESSIBLE: &str = "2026-01";

/// The versions a server serves by default: [`DEFAULT`].
pub const SERVED: &[&str] = &[DEFAULT];

/// How Shopify labels a version (shopify.dev, "API versioning": "Shopify supports three version
/// types"; and "Unsupported" in its release schedule).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    /// A release, "guaranteed not to change for its supported lifetime".
    Stable,
    /// The next release, published with the latest stable one; "may include
    /// backwards-incompatible changes".
    ReleaseCandidate,
    /// `unstable`: "updated continuously with in-progress changes".
    Unstable,
    /// Past its supported lifetime: a request for it falls forward.
    Unsupported,
}

impl Status {
    /// As shopify.dev words it: `stable`, `release candidate`.
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Stable => "stable",
            Status::ReleaseCandidate => "release candidate",
            Status::Unstable => "unstable",
            Status::Unsupported => "unsupported",
        }
    }
}

/// The files of a version, embedded.
pub(crate) struct Files {
    /// `admin.graphql.br`: the SDL, Brotli-compressed.
    pub schema: &'static [u8],
    /// `introspection.graphql`: the introspection types (`__Schema`, …) as Shopify defines them.
    pub introspection: &'static str,
    /// `access.json`: the `requiredAccess` texts of its types and fields.
    pub access: &'static str,
    /// `search.json`: the filters the `query` argument of each connection documents.
    pub search: &'static str,
    /// `topics.json`: its `WebhookSubscriptionTopic` values.
    pub topics: &'static str,
    /// Shopify's example payloads of the webhooks lsf builds, which the tests compare with.
    #[cfg(test)]
    pub webhook_examples: &'static str,
}

/// A version lsf knows.
struct Entry {
    name: &'static str,
    status: Status,
    files: Files,
    adapters: &'static [Adapter],
}

/// An entry of [`KNOWN`], its files at their place.
macro_rules! version {
    ($name:literal, $status:ident, $adapters:expr) => {
        Entry {
            name: $name,
            status: Status::$status,
            adapters: $adapters,
            files: Files {
                schema: include_bytes!(concat!("../../schemas/admin/", $name, "/admin.graphql.br")),
                introspection: include_str!(concat!(
                    "../../schemas/admin/",
                    $name,
                    "/introspection.graphql"
                )),
                access: include_str!(concat!("../../schemas/admin/", $name, "/access.json")),
                search: include_str!(concat!("../../schemas/admin/", $name, "/search.json")),
                topics: include_str!(concat!("../../schemas/admin/", $name, "/topics.json")),
                #[cfg(test)]
                webhook_examples: include_str!(concat!(
                    "../../tests/fixtures/webhooks/examples-",
                    $name,
                    ".json"
                )),
            },
        }
    };
}

/// How many versions lsf knows: the length of [`KNOWN`].
pub const COUNT: usize = 2;

/// Every version lsf knows, oldest first: its name, its status, its files, its adapters. Adding a
/// version is a line here, once `mise run admin:version -- <version>` wrote its files (see "Add
/// a version" in `crates/app/docs/admin/design.md`).
static KNOWN: [Entry; COUNT] = [
    version!("2026-10", Stable, &[]),
    // The release candidate: served with `--admin-versions 2026-10,2027-01`.
    version!("2027-01", ReleaseCandidate, adapt::V2027_01),
];

/// A version of the Admin API lsf knows: a handle on its entry of [`KNOWN`], cheap to copy and
/// compare. What a request runs in: a resolver asks it with `ShopResolver::version`, a mutation
/// with `Mutation::version`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ApiVersion(usize);

impl ApiVersion {
    /// Every version lsf knows, oldest first.
    pub fn all() -> impl Iterator<Item = ApiVersion> {
        (0..COUNT).map(ApiVersion)
    }

    /// The version of a name: `2026-10`. `None` for one lsf does not know.
    pub fn get(name: &str) -> Option<ApiVersion> {
        KNOWN
            .iter()
            .position(|entry| entry.name == name.trim())
            .map(ApiVersion)
    }

    /// [`DEFAULT`].
    pub fn default_version() -> ApiVersion {
        ApiVersion::get(DEFAULT).expect("the default version is one of KNOWN")
    }

    fn entry(self) -> &'static Entry {
        &KNOWN[self.0]
    }

    /// Its name: `2026-10`, `unstable`.
    pub fn name(self) -> &'static str {
        self.entry().name
    }

    pub fn status(self) -> Status {
        self.entry().status
    }

    /// Its place in [`KNOWN`]: what the tables parsed per version are indexed by.
    pub fn index(self) -> usize {
        self.0
    }

    pub(crate) fn files(self) -> &'static Files {
        &self.entry().files
    }

    /// How this version shows the records differently from how they are kept.
    pub fn adapters(self) -> &'static [Adapter] {
        self.entry().adapters
    }

    /// Whether this version is `name` or a later one: `version.since("2027-01")`. Versions are
    /// dates, `unstable` after all of them.
    pub fn since(self, name: &str) -> bool {
        order(self.name()) >= order(name)
    }

    /// Its schema: [`crate::admin::schema::for_version`].
    pub fn schema(self) -> &'static crate::admin::schema::Schema {
        crate::admin::schema::for_version(self)
    }
}

impl Default for ApiVersion {
    fn default() -> ApiVersion {
        ApiVersion::default_version()
    }
}

impl fmt::Display for ApiVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

impl fmt::Debug for ApiVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ApiVersion({})", self.name())
    }
}

/// A version name in an order that compares: `2026-10` as itself, `unstable` after every date.
fn order(name: &str) -> (bool, &str) {
    (name == "unstable", name)
}

/// Whether Shopify no longer serves a version: a dated one older than [`OLDEST_ACCESSIBLE`], or
/// one lsf knows as unsupported.
pub fn is_inaccessible(name: &str) -> bool {
    if ApiVersion::get(name).is_some_and(|version| version.status() == Status::Unsupported) {
        return true;
    }
    name != "unstable" && is_version_name(name) && name < OLDEST_ACCESSIBLE
}

/// The versions a server serves: [`DEFAULT`] unless it is told others (`--admin-versions`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Served(Vec<ApiVersion>);

impl Default for Served {
    fn default() -> Served {
        Served(vec![ApiVersion::default_version()])
    }
}

/// Which version answers a request for one.
#[derive(Clone, Debug, PartialEq)]
pub enum Resolution {
    /// The version asked for, which the server serves.
    Served(ApiVersion),
    /// One Shopify no longer serves: the oldest accessible stable version the server serves
    /// answers instead, as Shopify falls forward.
    FellForward { asked: String, served: ApiVersion },
    /// One the server does not serve: the body of the 404, saying which it does.
    NotServed(Value),
}

impl Served {
    /// Serves these versions, in the order of [`KNOWN`]. At least one.
    pub fn new(versions: impl IntoIterator<Item = ApiVersion>) -> Served {
        let mut versions: Vec<ApiVersion> = versions.into_iter().collect();
        versions.sort_by_key(|version| version.index());
        versions.dedup();
        if versions.is_empty() {
            return Served::default();
        }
        Served(versions)
    }

    /// `2026-10,2027-01` (commas or spaces), as `--admin-versions` takes it. Each must be a
    /// version lsf knows.
    pub fn parse(list: &str) -> Result<Served, String> {
        let mut versions = Vec::new();
        for name in list
            .split([',', ' '])
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            match ApiVersion::get(name) {
                Some(version) => versions.push(version),
                None => {
                    return Err(format!(
                        "`{name}` is not an API version lsf knows: it knows {} (adding one: \
                         `mise run admin:version -- {name}`, see docs/admin.md)",
                        known_list()
                    ));
                }
            }
        }
        if versions.is_empty() {
            return Err(format!("no API version given: lsf knows {}", known_list()));
        }
        Ok(Served::new(versions))
    }

    /// Every version served, oldest first.
    pub fn all(&self) -> &[ApiVersion] {
        &self.0
    }

    /// Whether the server serves `version`.
    pub fn serves(&self, version: ApiVersion) -> bool {
        self.0.contains(&version)
    }

    /// The version the server names when it names one (its banner, `GET /__lsf/admin`, the
    /// answer of a new shop): [`DEFAULT`] when it serves it, else the oldest it serves.
    pub fn primary(&self) -> ApiVersion {
        let default = ApiVersion::default_version();
        if self.serves(default) {
            default
        } else {
            self.0[0]
        }
    }

    /// What a request for an inaccessible version gets: the oldest stable version served, as
    /// Shopify answers with "the oldest accessible stable version"; [`Served::primary`] when the
    /// server serves no stable version.
    pub fn fallback(&self) -> ApiVersion {
        self.0
            .iter()
            .copied()
            .find(|version| version.status() == Status::Stable)
            .unwrap_or_else(|| self.primary())
    }

    /// The versions served, for messages: `2026-10, 2027-01`.
    pub fn names(&self) -> String {
        let names: Vec<&str> = self.0.iter().map(|version| version.name()).collect();
        names.join(", ")
    }

    /// Which version answers a request for `asked` (the version of its path).
    pub fn resolve(&self, asked: &str) -> Resolution {
        if let Some(version) = ApiVersion::get(asked)
            && self.serves(version)
            && version.status() != Status::Unsupported
        {
            return Resolution::Served(version);
        }
        if is_inaccessible(asked) {
            return Resolution::FellForward {
                asked: asked.to_string(),
                served: self.fallback(),
            };
        }
        Resolution::NotServed(self.refusal(asked))
    }

    /// The version a webhook of a subscription that asks for `asked` is built in, and what to
    /// say when it is not that one: the same as a request (an inaccessible version falls
    /// forward, as shopify.dev says webhooks do), except that a version the server does not
    /// serve is built in [`Served::primary`], since a delivery cannot answer 404.
    pub fn for_webhook(&self, asked: &str) -> (ApiVersion, Option<String>) {
        match self.resolve(asked) {
            Resolution::Served(version) => (version, None),
            Resolution::FellForward { served, .. } => (
                served,
                Some(format!(
                    "API version {asked} is no longer supported: the payload is {served}'s, as \
                     Shopify falls forward to the oldest supported version"
                )),
            ),
            Resolution::NotServed(_) => {
                let primary = self.primary();
                let hint = match ApiVersion::get(asked) {
                    Some(known) => format!(
                        " (lsf knows {known}: serve it with `--admin-versions {}`)",
                        Served::new(self.0.iter().copied().chain([known]))
                            .names()
                            .replace(' ', "")
                    ),
                    None => String::new(),
                };
                (
                    primary,
                    Some(format!(
                        "the subscription asks for API version {asked}, which is not served here: \
                         the payload is {primary}'s{hint}"
                    )),
                )
            }
        }
    }

    /// The body of the 404 for a version the server does not serve: in the shape of Shopify's
    /// errors (`{"errors": "..."}`), saying which versions it serves.
    pub fn refusal(&self, version: &str) -> Value {
        let served = self.names();
        let message = if version.is_empty() {
            format!(
                "lsf serves the Admin API at /admin/api/<version>/graphql.json, for API version {served} only"
            )
        } else if let Some(known) = ApiVersion::get(version) {
            let with = Served::new(self.0.iter().copied().chain([known]));
            format!(
                "lsf knows API version {version} ({}) but this server serves {served} only: \
                 start it with `--admin-versions {}`",
                known.status().as_str(),
                with.names().replace(' ', "")
            )
        } else if is_version_name(version) {
            format!("lsf serves the Admin API for API version {served} only, not {version}")
        } else {
            format!(
                "{version} is not an API version: lsf serves the Admin API for API version {served}"
            )
        };
        json!({ "errors": message })
    }
}

/// The versions lsf knows, for messages: `2026-10 (stable), 2027-01 (release candidate)`.
fn known_list() -> String {
    let names: Vec<String> = ApiVersion::all()
        .map(|version| format!("{version} ({})", version.status().as_str()))
        .collect();
    names.join(", ")
}

/// What the path of a request under `/admin/api` is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdminPath<'a> {
    /// `/admin/api/<version>/graphql.json`: the GraphQL Admin API of a version, served or not
    /// ([`Served::resolve`] says).
    Graphql(&'a str),
    /// `/admin/api/<x>/graphql.json` where `x` is not a version, or `/admin/api/graphql.json`
    /// without one.
    UnsupportedVersion(&'a str),
    /// `/admin/api/<version>/<resource>.json`: the REST Admin API, which lsf does not serve.
    Rest { version: &'a str, resource: &'a str },
}

/// Reads the path of a request (without its query string). `None` when it is not under
/// `/admin/api/`.
pub fn parse_path(path: &str) -> Option<AdminPath<'_>> {
    let rest = path.strip_prefix("/admin/api/")?;
    let rest = rest.trim_end_matches('/');
    if rest == "graphql.json" || rest == "graphql" {
        return Some(AdminPath::UnsupportedVersion(""));
    }
    let (version, resource) = rest.split_once('/')?;
    if resource == "graphql.json" || resource == "graphql" {
        return Some(if is_version_name(version) {
            AdminPath::Graphql(version)
        } else {
            AdminPath::UnsupportedVersion(version)
        });
    }
    Some(AdminPath::Rest { version, resource })
}

/// Whether a string is an API version as Shopify names them: `2026-10`, or `unstable`.
pub fn is_version_name(version: &str) -> bool {
    if version == "unstable" {
        return true;
    }
    let bytes = version.as_bytes();
    version.len() == 7
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[4] == b'-'
        && bytes[5..].iter().all(u8::is_ascii_digit)
}

/// The body of the 404 for the REST Admin API, which lsf does not serve, naming the GraphQL
/// endpoint of the server's [`Served::primary`] version.
pub fn rest_not_served_by(served: &Served, resource: &str) -> Value {
    json!({
        "errors": format!(
            "lsf does not simulate the REST Admin API ({resource}), except GET /admin/oauth/access_scopes.json: use the GraphQL Admin API at /admin/api/{}/graphql.json",
            served.primary()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_version_of_a_path() {
        assert_eq!(
            parse_path("/admin/api/2026-10/graphql.json"),
            Some(AdminPath::Graphql("2026-10"))
        );
        assert_eq!(
            parse_path("/admin/api/2027-01/graphql.json"),
            Some(AdminPath::Graphql("2027-01"))
        );
        assert_eq!(
            parse_path("/admin/api/unstable/graphql.json"),
            Some(AdminPath::Graphql("unstable"))
        );
        assert_eq!(
            parse_path("/admin/api/latest/graphql.json"),
            Some(AdminPath::UnsupportedVersion("latest"))
        );
        assert_eq!(
            parse_path("/admin/api/graphql.json"),
            Some(AdminPath::UnsupportedVersion(""))
        );
        assert_eq!(
            parse_path("/admin/api/2026-10/products.json"),
            Some(AdminPath::Rest {
                version: "2026-10",
                resource: "products.json"
            })
        );
        assert_eq!(
            parse_path("/admin/api/2026-10/products/1.json"),
            Some(AdminPath::Rest {
                version: "2026-10",
                resource: "products/1.json"
            })
        );
        assert_eq!(parse_path("/admin/oauth/authorize"), None);
    }

    #[test]
    fn the_registry_lists_each_version_once_oldest_first_the_default_among_them() {
        let names: Vec<&str> = ApiVersion::all().map(ApiVersion::name).collect();
        assert!(names.windows(2).all(|pair| order(pair[0]) < order(pair[1])));
        assert!(names.iter().all(|name| is_version_name(name)));
        assert_eq!(ApiVersion::default_version().name(), DEFAULT);
        assert_eq!(ApiVersion::default().status(), Status::Stable);
        assert_eq!(
            ApiVersion::get("2026-10").map(ApiVersion::name),
            Some("2026-10")
        );
        assert_eq!(ApiVersion::get("1999-01"), None);
        for version in ApiVersion::all() {
            assert_eq!(ApiVersion::all().nth(version.index()), Some(version));
        }
    }

    #[test]
    fn versions_compare_as_dates_unstable_last() {
        let version = ApiVersion::default_version();
        assert!(version.since("2026-07"));
        assert!(version.since(DEFAULT));
        assert!(!version.since("2027-01"));
        assert!(!version.since("unstable"));
        assert!(order("unstable") > order("2099-01"));
    }

    #[test]
    fn a_version_shopify_no_longer_serves_falls_forward_to_the_oldest_stable_one_served() {
        let served = Served::default();
        assert!(is_inaccessible("2025-10"));
        assert!(is_inaccessible("2019-04"));
        assert!(!is_inaccessible(OLDEST_ACCESSIBLE));
        assert!(!is_inaccessible("unstable"));
        assert!(!is_inaccessible("latest"));
        assert_eq!(
            served.resolve("2025-07"),
            Resolution::FellForward {
                asked: "2025-07".into(),
                served: ApiVersion::default_version()
            }
        );
        assert_eq!(
            served.resolve(DEFAULT),
            Resolution::Served(ApiVersion::default_version())
        );
        let (version, note) = served.for_webhook("2024-01");
        assert_eq!(version, ApiVersion::default_version());
        assert!(note.unwrap().contains("falls forward"));
        assert_eq!(served.for_webhook(DEFAULT), (ApiVersion::default(), None));
    }

    #[test]
    fn says_which_versions_it_serves() {
        assert_eq!(
            Served::default().refusal("2026-07"),
            json!({"errors": "lsf serves the Admin API for API version 2026-10 only, not 2026-07"})
        );
        assert_eq!(
            Served::default().resolve("unstable"),
            Resolution::NotServed(json!({
                "errors": "lsf serves the Admin API for API version 2026-10 only, not unstable"
            }))
        );
        assert!(is_version_name("2027-01"));
        assert!(is_version_name("unstable"));
        assert!(!is_version_name("v1"));
        assert!(
            rest_not_served_by(&Served::default(), "products.json")["errors"]
                .as_str()
                .unwrap()
                .contains("/admin/api/2026-10/graphql.json")
        );
        let (version, note) = Served::default().for_webhook("2026-07");
        assert_eq!(version, ApiVersion::default_version());
        assert!(note.unwrap().contains("not served here"));
    }

    #[test]
    fn the_served_versions_are_read_from_a_list() {
        assert_eq!(Served::parse("2026-10").unwrap(), Served::default());
        assert_eq!(Served::parse(" 2026-10, 2026-10 ").unwrap().all().len(), 1);
        assert!(
            Served::parse("2026-07")
                .unwrap_err()
                .contains("not an API version lsf knows")
        );
        assert!(Served::parse(" , ").is_err());
        assert_eq!(Served::default().primary().name(), DEFAULT);
        assert_eq!(Served::default().fallback().name(), DEFAULT);
    }
}
