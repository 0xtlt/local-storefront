//! Webhook subscriptions: those of `shopify.app.toml`, which hold for every shop the app is
//! installed on, and those the backend creates through the Admin API (`webhookSubscriptionCreate`),
//! which belong to one shop. And where each one delivers: its `uri`, resolved against the URL the
//! backend of the app listens on.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Value, json};

use super::topics::{self, Topics};
use crate::admin::state::{Record, gid, parse_gid, value};
use crate::admin::version::{self, ApiVersion};
use crate::config::AppConfig;

/// The API version of a subscription that names none: [`version::DEFAULT`]. What a payload is
/// built in is the subscription's version as the server serves it
/// ([`version::Served::for_webhook`]).
pub const API_VERSION: &str = version::DEFAULT;

/// The topics a subscription in `api_version` can be to: those of the version, or of the
/// default one for a version lsf does not know (its deliveries say what lsf did instead).
fn topics_of(api_version: &str) -> &'static Topics {
    topics::for_version(ApiVersion::get(api_version).unwrap_or_default())
}

/// Where a subscription comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Source {
    /// A `[[webhooks.subscriptions]]` of `shopify.app.toml`: its position in the file, from 1.
    AppConfig { entry: usize },
    /// Created through the Admin API, for one shop.
    AdminApi { shop: String },
}

/// The format of the payloads, `WebhookSubscriptionFormat`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Format {
    #[default]
    Json,
    /// lsf does not build XML payloads: the deliveries of such a subscription are not sent.
    Xml,
}

impl Format {
    pub fn parse(text: &str) -> Option<Format> {
        match text {
            "JSON" => Some(Format::Json),
            "XML" => Some(Format::Xml),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Format::Json => "JSON",
            Format::Xml => "XML",
        }
    }
}

/// A metafield a subscription asks payloads to carry, `WebhookSubscriptionMetafieldIdentifier`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MetafieldIdentifier {
    pub namespace: String,
    pub key: String,
}

/// One subscription to one topic. A `[[webhooks.subscriptions]]` with several topics is one
/// subscription per topic, as Shopify stores it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    /// `shopify.app.toml#2` for the second entry of the file; the GID
    /// (`gid://shopify/WebhookSubscription/1`) for one created through the Admin API.
    pub id: String,
    pub source: Source,
    /// The topic as written: `orders/create`.
    pub topic: String,
    /// As given: a path relative to the app URL, an absolute URL, `pubsub://…`,
    /// `arn:aws:events:…`.
    pub uri: String,
    /// `name`: sent as `X-Shopify-Name`.
    pub name: Option<String>,
    /// `filter`, in Shopify's search syntax, over the payload.
    pub filter: Option<String>,
    /// `include_fields`: the fields the payload keeps (`variants.price` for a nested one).
    pub include_fields: Vec<String>,
    pub metafield_namespaces: Vec<String>,
    pub metafields: Vec<MetafieldIdentifier>,
    pub format: Format,
    /// The API version it asks for: `[webhooks] api_version`, or the version of the request
    /// that created it.
    pub api_version: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Subscription {
    /// Whether it is one of the compliance topics, which are only sent on demand.
    pub fn is_compliance(&self) -> bool {
        topics::is_compliance(&self.topic)
    }

    /// The record of a subscription created through the Admin API, in the shape of the
    /// `WebhookSubscription` type, for the resolvers of `webhookSubscription(s)` and the payloads
    /// of the mutations.
    pub fn to_record(&self) -> Record {
        let topic = topics_of(&self.api_version)
            .by_written(&self.topic)
            .map_or(self.topic.to_ascii_uppercase(), |topic| topic.name.clone());
        let legacy = parse_gid(&self.id).map(|(_, id)| id.to_string());
        let value = json!({
            "__typename": "WebhookSubscription",
            "id": self.id,
            "legacyResourceId": legacy,
            "topic": topic,
            "uri": self.uri,
            "callbackUrl": self.uri,
            "endpoint": endpoint_of(&self.uri),
            "name": self.name,
            "filter": self.filter,
            "includeFields": self.include_fields,
            "metafieldNamespaces": self.metafield_namespaces,
            "metafields": self.metafields,
            "format": self.format.as_str(),
            "apiVersion": {
                "handle": self.api_version,
                "displayName": self.api_version,
                "supported": !version::is_inaccessible(&self.api_version),
            },
            "createdAt": value::date_time(self.created_at),
            "updatedAt": value::date_time(self.updated_at),
        });
        value::object(value)
    }
}

/// The `endpoint` of a `WebhookSubscription`: the union member its `uri` is.
fn endpoint_of(uri: &str) -> Value {
    if let Some(rest) = uri.strip_prefix("pubsub://") {
        let (project, topic) = rest.split_once(':').unwrap_or((rest, ""));
        return json!({"__typename": "WebhookPubSubEndpoint", "pubSubProject": project, "pubSubTopic": topic});
    }
    if uri.starts_with("arn:aws:events:") {
        return json!({"__typename": "WebhookEventBridgeEndpoint", "arn": uri});
    }
    json!({"__typename": "WebhookHttpEndpoint", "callbackUrl": uri})
}

/// The subscriptions of `shopify.app.toml`: one per topic of each `[[webhooks.subscriptions]]`
/// (its `topics`, then its `compliance_topics`), with the `api_version` of `[webhooks]`
/// ([`API_VERSION`] when there is none). `at` is their `createdAt`: when the server read the
/// file.
pub fn from_app_config(config: &AppConfig, at: DateTime<Utc>) -> Vec<Subscription> {
    let api_version = config
        .webhooks_api_version
        .clone()
        .unwrap_or_else(|| API_VERSION.to_string());
    let topics = topics_of(&api_version);
    let mut subscriptions = Vec::new();
    for (index, entry) in config.webhook_subscriptions.iter().enumerate() {
        for topic in entry.topics.iter().chain(&entry.compliance_topics) {
            subscriptions.push(Subscription {
                id: format!("shopify.app.toml#{}", index + 1),
                source: Source::AppConfig { entry: index + 1 },
                // `ORDERS_CREATE` is read as `orders/create`; an unknown topic is kept as written,
                // so that its deliveries say why they are not sent.
                topic: topics
                    .written_form(topic)
                    .map_or_else(|| topic.clone(), str::to_string),
                uri: entry.uri.clone(),
                name: entry.name.clone(),
                filter: entry
                    .filter
                    .clone()
                    .filter(|filter| !filter.trim().is_empty()),
                include_fields: entry.include_fields.clone(),
                metafield_namespaces: Vec::new(),
                metafields: Vec::new(),
                format: Format::Json,
                api_version: api_version.clone(),
                created_at: at,
                updated_at: at,
            });
        }
    }
    subscriptions
}

/// Where the backend of the app is: what a subscription's `uri` is resolved against.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppUrls {
    /// `--app-url`: where the backend listens, on this machine.
    pub app_url: Option<String>,
    /// `application_url` of `shopify.app.toml`.
    pub application_url: Option<String>,
}

impl AppUrls {
    /// The URL relative `uri`s join: `--app-url`, else `application_url`.
    pub fn base(&self) -> Option<&str> {
        self.app_url
            .as_deref()
            .or(self.application_url.as_deref())
            .filter(|url| !url.is_empty())
    }

    /// Whether lsf may post a webhook to `url`: one on this machine (the loopback), or on the
    /// origin of `--app-url`, where the user pointed lsf. A webhook is signed with the app's
    /// real secret when `.env` has it: sent to `application_url` (a production backend, a stale
    /// tunnel) or to an address a subscription names, it would leave the machine unasked.
    pub fn may_send_to(&self, url: &str) -> bool {
        let Some(target) = Endpoint::of(url) else {
            return false;
        };
        target.is_loopback()
            || self
                .app_url
                .as_deref()
                .and_then(Endpoint::of)
                .is_some_and(|app| app == target)
    }
}

/// Why the server does not send a webhook to `url`: it is neither on this machine nor on
/// `--app-url` ([`AppUrls::may_send_to`]).
pub fn off_machine(url: &str) -> String {
    off_machine_with(
        url,
        &format!(
            "start {} with --app-url http://localhost:<port> (your backend)",
            lsf_core::util::command()
        ),
    )
}

/// [`off_machine`], with what to do about it: `lsf app webhook trigger` starts no server, and
/// takes an `--address` too.
pub fn off_machine_with(url: &str, advice: &str) -> String {
    format!("lsf sends webhooks to this machine or to --app-url only: {url} is neither; {advice}")
}

/// Where a request to a URL connects: its scheme, host and port, read as the HTTP client of the
/// deliveries reads them, so that what is checked is what is reached (`http://localhost@host/`
/// connects to `host`).
#[derive(Debug, PartialEq, Eq)]
struct Endpoint {
    scheme: String,
    host: String,
    port: u16,
}

impl Endpoint {
    fn of(url: &str) -> Option<Endpoint> {
        let uri: ureq::http::Uri = url.trim().parse().ok()?;
        let scheme = uri.scheme_str()?.to_ascii_lowercase();
        let default_port = match scheme.as_str() {
            "http" => 80,
            "https" => 443,
            _ => return None,
        };
        let host = uri.host()?;
        let host = host
            .strip_prefix('[')
            .and_then(|ipv6| ipv6.strip_suffix(']'))
            .unwrap_or(host)
            .to_ascii_lowercase();
        Some(Endpoint {
            port: uri.port_u16().unwrap_or(default_port),
            scheme,
            host,
        })
    }

    fn is_loopback(&self) -> bool {
        is_loopback_host(&self.host)
    }
}

/// Whether a host is this machine: `localhost` and the names under it (`app.localhost`, which
/// RFC 6761 reserves for the loopback, and which the deliveries connect to the loopback whatever
/// the system resolver says: [`super::delivery`]), `127.0.0.0/8` or `::1`. Not `0.0.0.0`, nor
/// `::ffff:127.0.0.1`. `host` is without the brackets of an IPv6 address.
pub fn is_loopback_host(host: &str) -> bool {
    is_localhost_name(host)
        || host
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

/// `localhost`, or a name under it (`app.localhost`): RFC 6761, section 6.3.
pub fn is_localhost_name(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "localhost"
        || host
            .strip_suffix(".localhost")
            .is_some_and(|label| !label.is_empty())
}

/// Where a subscription delivers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Destination {
    /// An HTTP(S) URL lsf posts to.
    Http { url: String },
    /// A `uri` lsf cannot deliver to, and why.
    Undeliverable { reason: String },
}

/// Resolves a `uri`, as Shopify's CLI and Shopify do, then for the machine lsf runs on:
///
/// - a path (`/webhooks`) joins the app URL (`--app-url`, else `application_url`), trailing
///   slashes removed, as Shopify's CLI prepends `application_url`;
/// - an absolute URL under `application_url` (or on its origin) moves to `--app-url` when it is
///   given: `application_url` is usually a tunnel of `shopify app dev` that is not running;
/// - another absolute HTTP(S) URL is kept;
/// - `pubsub://`, `arn:aws:events:` and `kafka://` are not HTTP: lsf does not deliver to them.
pub fn resolve_uri(uri: &str, urls: &AppUrls) -> Destination {
    let uri = uri.trim();
    let undeliverable = |reason: String| Destination::Undeliverable { reason };
    if uri.starts_with("pubsub://") {
        return undeliverable(format!(
            "`{uri}` is a Google Cloud Pub/Sub topic: lsf delivers over HTTP only"
        ));
    }
    if uri.starts_with("arn:aws:events:") {
        return undeliverable(format!(
            "`{uri}` is an Amazon EventBridge source: lsf delivers over HTTP only"
        ));
    }
    if uri.starts_with("kafka://") {
        return undeliverable(format!(
            "`{uri}` is a Kafka topic: lsf delivers over HTTP only"
        ));
    }
    if uri.is_empty() {
        return undeliverable("the subscription has no uri".to_string());
    }
    if uri.starts_with('/') {
        return match urls.base() {
            // Shopify's CLI removes the trailing slashes of the `uri` too: `/` is the root.
            Some(base) => Destination::Http {
                url: format!(
                    "{}{}",
                    base.trim_end_matches('/'),
                    match uri.trim_end_matches('/') {
                        "" => "/",
                        path => path,
                    }
                ),
            },
            None => undeliverable(format!(
                "`{uri}` is relative and there is no app URL to join it to: give `--app-url` \
                 (or `application_url` in shopify.app.toml)"
            )),
        };
    }
    let Some((scheme, _)) = uri.split_once("://") else {
        return undeliverable(format!(
            "`{uri}` is neither a path starting with `/` nor an absolute URL"
        ));
    };
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return undeliverable(format!(
            "`{uri}`: lsf delivers over HTTP only, not `{scheme}`"
        ));
    }
    if let (Some(app_url), Some(application_url)) = (
        urls.app_url.as_deref().filter(|url| !url.is_empty()),
        urls.application_url
            .as_deref()
            .filter(|url| !url.is_empty()),
    ) {
        let application_url = application_url.trim_end_matches('/');
        let app_url = app_url.trim_end_matches('/');
        // Under `application_url`: the part after it goes under `--app-url`.
        if let Some(rest) = uri.strip_prefix(application_url)
            && (rest.is_empty() || rest.starts_with(['/', '?', '#']))
        {
            return Destination::Http {
                url: format!("{app_url}{rest}"),
            };
        }
        // On its origin, outside its path: the path is kept, on the origin of `--app-url`.
        if let (Some(origin), Some(app_origin)) = (origin(application_url), origin(app_url))
            && let Some(rest) = uri.strip_prefix(origin)
            && (rest.is_empty() || rest.starts_with(['/', '?', '#']))
        {
            return Destination::Http {
                url: format!("{app_origin}{rest}"),
            };
        }
    }
    Destination::Http {
        url: uri.to_string(),
    }
}

/// `scheme://host[:port]` of an absolute URL.
fn origin(url: &str) -> Option<&str> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    (!authority.is_empty()).then(|| &url[..scheme.len() + 3 + authority.len()])
}

/// Whether the host of an `http://` URL is this machine: the only plain-HTTP addresses a
/// subscription created through the Admin API may have (Shopify takes none).
pub fn is_local_http(url: &str) -> bool {
    let Some(rest) = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("HTTP://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = match host.strip_prefix('[') {
        Some(ipv6) => ipv6.split(']').next().unwrap_or(""),
        None => host.rsplit_once(':').map_or(host, |(host, _)| host),
    };
    is_loopback_host(host)
}

/// What `webhookSubscriptionCreate` and `webhookSubscriptionUpdate` take,
/// `WebhookSubscriptionInput`. A field left out is left as it is by an update.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SubscriptionInput {
    pub uri: Option<String>,
    pub name: Option<String>,
    pub filter: Option<String>,
    pub include_fields: Option<Vec<String>>,
    pub metafield_namespaces: Option<Vec<String>>,
    pub metafields: Option<Vec<MetafieldIdentifier>>,
    pub format: Option<Format>,
}

/// A `userErrors` entry of the subscription mutations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct UserError {
    pub field: Vec<String>,
    pub message: String,
}

impl UserError {
    fn new(field: &[&str], message: impl Into<String>) -> UserError {
        UserError {
            field: field.iter().map(|part| part.to_string()).collect(),
            message: message.into(),
        }
    }
}

/// The subscriptions created through the Admin API, per shop. Ids are GIDs of
/// `WebhookSubscription`, from a counter shared by every shop. Also the shops the app is not
/// installed on, for which nothing is delivered (see [`super::Webhooks::emit`]).
#[derive(Clone, Debug, Default)]
pub struct Registry {
    shops: HashMap<String, Vec<Subscription>>,
    last_id: u64,
    /// The shops the app was uninstalled from and not installed on again.
    uninstalled: HashSet<String>,
}

impl Registry {
    pub fn new() -> Registry {
        Registry::default()
    }

    /// The subscriptions of a shop, in the order they were created.
    pub fn list(&self, shop: &str) -> &[Subscription] {
        self.shops.get(shop).map_or(&[], Vec::as_slice)
    }

    pub fn get(&self, shop: &str, id: &str) -> Option<&Subscription> {
        self.list(shop)
            .iter()
            .find(|subscription| subscription.id == id)
    }

    /// `webhookSubscriptionCreate`: `topic` as the enum value (`ORDERS_CREATE`) or written, one
    /// of the topics of `api_version`, the version of the request.
    pub fn create(
        &mut self,
        shop: &str,
        topic: &str,
        input: SubscriptionInput,
        api_version: &str,
        now: DateTime<Utc>,
    ) -> Result<Subscription, Vec<UserError>> {
        let Some(topic) = topics_of(api_version)
            .find(topic)
            .map(|topic| topic.written.as_str())
        else {
            return Err(vec![UserError::new(
                &["topic"],
                format!("Topic `{topic}` is not a webhook subscription topic"),
            )]);
        };
        let uri = input.uri.clone().unwrap_or_default();
        let mut errors = check_uri(&uri);
        if self
            .list(shop)
            .iter()
            .any(|existing| existing.topic == topic && existing.uri == uri)
        {
            errors.push(address_taken());
        }
        errors.extend(check_name(input.name.as_deref()));
        if !errors.is_empty() {
            return Err(errors);
        }
        self.last_id += 1;
        let subscription = Subscription {
            id: gid("WebhookSubscription", self.last_id),
            source: Source::AdminApi {
                shop: shop.to_string(),
            },
            topic: topic.to_string(),
            uri,
            name: input.name.filter(|name| !name.is_empty()),
            filter: input.filter.filter(|filter| !filter.trim().is_empty()),
            include_fields: input.include_fields.unwrap_or_default(),
            metafield_namespaces: input.metafield_namespaces.unwrap_or_default(),
            metafields: input.metafields.unwrap_or_default(),
            format: input.format.unwrap_or_default(),
            api_version: api_version.to_string(),
            created_at: now,
            updated_at: now,
        };
        self.shops
            .entry(shop.to_string())
            .or_default()
            .push(subscription.clone());
        Ok(subscription)
    }

    /// `webhookSubscriptionUpdate`: the fields given replace those of the subscription.
    pub fn update(
        &mut self,
        shop: &str,
        id: &str,
        input: SubscriptionInput,
        now: DateTime<Utc>,
    ) -> Result<Subscription, Vec<UserError>> {
        let Some(existing) = self.get(shop, id).cloned() else {
            return Err(vec![not_found()]);
        };
        let mut errors = Vec::new();
        if let Some(uri) = &input.uri {
            errors.extend(check_uri(uri));
            if self.list(shop).iter().any(|other| {
                other.id != existing.id && other.topic == existing.topic && &other.uri == uri
            }) {
                errors.push(address_taken());
            }
        }
        errors.extend(check_name(input.name.as_deref()));
        if !errors.is_empty() {
            return Err(errors);
        }
        let Some(subscription) = self
            .shops
            .get_mut(shop)
            .and_then(|list| list.iter_mut().find(|subscription| subscription.id == id))
        else {
            return Err(vec![not_found()]);
        };
        if let Some(uri) = input.uri {
            subscription.uri = uri;
        }
        if let Some(name) = input.name {
            subscription.name = Some(name).filter(|name| !name.is_empty());
        }
        if let Some(filter) = input.filter {
            subscription.filter = Some(filter).filter(|filter| !filter.trim().is_empty());
        }
        if let Some(fields) = input.include_fields {
            subscription.include_fields = fields;
        }
        if let Some(namespaces) = input.metafield_namespaces {
            subscription.metafield_namespaces = namespaces;
        }
        if let Some(metafields) = input.metafields {
            subscription.metafields = metafields;
        }
        if let Some(format) = input.format {
            subscription.format = format;
        }
        subscription.updated_at = now;
        Ok(subscription.clone())
    }

    /// `webhookSubscriptionDelete`: the subscription that was deleted.
    pub fn delete(&mut self, shop: &str, id: &str) -> Result<Subscription, Vec<UserError>> {
        let list = self.shops.get_mut(shop).ok_or_else(|| vec![not_found()])?;
        let position = list
            .iter()
            .position(|subscription| subscription.id == id)
            .ok_or_else(|| vec![not_found()])?;
        Ok(list.remove(position))
    }

    /// Forgets the subscriptions of a shop: when its state is seeded again, or the app
    /// uninstalled from it. How many there were.
    pub fn clear(&mut self, shop: &str) -> usize {
        self.shops.remove(shop).map_or(0, |list| list.len())
    }

    /// Forgets every subscription, and every uninstall: a reset of the server, which installs
    /// the app on every shop again.
    pub fn clear_all(&mut self) {
        self.shops.clear();
        self.uninstalled.clear();
    }

    /// Whether the app is installed on `shop`: what [`super::Webhooks::emit`] delivers for.
    pub fn is_installed(&self, shop: &str) -> bool {
        !self.uninstalled.contains(shop)
    }

    /// Records that the app is no longer installed on `shop` (`installed: false`), or is again.
    pub fn set_installed(&mut self, shop: &str, installed: bool) {
        if installed {
            self.uninstalled.remove(shop);
        } else {
            self.uninstalled.insert(shop.to_string());
        }
    }

    /// The shops the app is not installed on, in no particular order.
    pub fn uninstalled_shops(&self) -> Vec<String> {
        self.uninstalled.iter().cloned().collect()
    }
}

fn not_found() -> UserError {
    UserError::new(&["id"], "Webhook subscription does not exist")
}

fn address_taken() -> UserError {
    UserError::new(
        &["webhookSubscription", "callbackUrl"],
        "Address for this topic has already been taken",
    )
}

/// What Shopify accepts as the `uri` of a subscription created through the Admin API: HTTPS,
/// Pub/Sub, EventBridge, and for lsf plain HTTP to this machine, where the backend under test
/// listens.
fn check_uri(uri: &str) -> Vec<UserError> {
    let field = ["webhookSubscription", "uri"];
    let uri = uri.trim();
    if uri.is_empty() {
        return vec![UserError::new(&field, "Address can't be blank")];
    }
    let lowered = uri.to_ascii_lowercase();
    if lowered.starts_with("https://")
        || uri.starts_with("pubsub://")
        || uri.starts_with("arn:aws:events:")
        || is_local_http(uri)
    {
        return Vec::new();
    }
    if lowered.starts_with("http://") {
        return vec![UserError::new(
            &field,
            "Address protocol http:// is not supported",
        )];
    }
    vec![UserError::new(&field, "Address is invalid")]
}

/// Shopify's CLI takes a `name` of 50 characters at most.
fn check_name(name: Option<&str>) -> Option<UserError> {
    let name = name?;
    (name.chars().count() > 50).then(|| {
        UserError::new(
            &["webhookSubscription", "name"],
            "Name is too long (maximum is 50 characters)",
        )
    })
}

#[cfg(test)]
mod tests {
    use crate::config::WebhookSubscriptionConfig;

    use super::*;

    fn urls(app_url: Option<&str>, application_url: Option<&str>) -> AppUrls {
        AppUrls {
            app_url: app_url.map(str::to_string),
            application_url: application_url.map(str::to_string),
        }
    }

    fn http(url: &str) -> Destination {
        Destination::Http {
            url: url.to_string(),
        }
    }

    #[test]
    fn a_relative_uri_joins_the_app_url_else_application_url() {
        let both = urls(
            Some("http://localhost:3000/"),
            Some("https://tunnel.trycloudflare.com"),
        );
        assert_eq!(
            resolve_uri("/webhooks/app/uninstalled", &both),
            http("http://localhost:3000/webhooks/app/uninstalled")
        );
        assert_eq!(
            resolve_uri("/webhooks/", &both),
            http("http://localhost:3000/webhooks")
        );
        assert_eq!(resolve_uri("/", &both), http("http://localhost:3000/"));
        let config_only = urls(None, Some("https://tunnel.trycloudflare.com/"));
        assert_eq!(
            resolve_uri("/webhooks", &config_only),
            http("https://tunnel.trycloudflare.com/webhooks")
        );
        let Destination::Undeliverable { reason } = resolve_uri("/webhooks", &AppUrls::default())
        else {
            panic!("a relative uri without an app URL is not delivered");
        };
        assert!(reason.contains("--app-url"), "{reason}");
    }

    #[test]
    fn an_absolute_uri_on_application_url_moves_to_the_app_url() {
        let both = urls(
            Some("http://localhost:3000"),
            Some("https://tunnel.trycloudflare.com"),
        );
        assert_eq!(
            resolve_uri("https://tunnel.trycloudflare.com/webhooks?x=1", &both),
            http("http://localhost:3000/webhooks?x=1")
        );
        // Under the path of `application_url`, the rest goes under `--app-url`.
        let with_path = urls(
            Some("http://localhost:3000"),
            Some("https://example.com/app"),
        );
        assert_eq!(
            resolve_uri("https://example.com/app/webhooks", &with_path),
            http("http://localhost:3000/webhooks")
        );
        // On its origin but outside its path: the path is kept.
        assert_eq!(
            resolve_uri("https://example.com/other", &with_path),
            http("http://localhost:3000/other")
        );
        // `https://example.com.evil/…` is not on the origin of `https://example.com`.
        assert_eq!(
            resolve_uri("https://example.com.evil/x", &with_path),
            http("https://example.com.evil/x")
        );
        // Another host is kept, and so is everything without `--app-url`.
        assert_eq!(
            resolve_uri("https://hooks.example.net/orders", &both),
            http("https://hooks.example.net/orders")
        );
        assert_eq!(
            resolve_uri(
                "https://tunnel.trycloudflare.com/webhooks",
                &urls(None, Some("https://tunnel.trycloudflare.com"))
            ),
            http("https://tunnel.trycloudflare.com/webhooks")
        );
    }

    #[test]
    fn webhooks_go_to_this_machine_or_to_the_app_url_only() {
        let local = urls(None, Some("https://tunnel.trycloudflare.com"));
        for url in [
            "http://localhost:3000/webhooks",
            "https://LOCALHOST/x",
            "http://127.0.0.1:3000/x",
            "http://127.1.2.3/x",
            "http://[::1]:3000/x",
            // RFC 6761 reserves the names under `localhost` for the loopback.
            "http://app.localhost/x",
            "HTTP://Shop.App.LOCALHOST:3000/x",
        ] {
            assert!(local.may_send_to(url), "{url} is this machine");
            assert!(
                url.starts_with("https:") || is_local_http(url),
                "{url} is plain HTTP a subscription may have"
            );
        }
        for url in [
            // `application_url` is not where the user pointed lsf.
            "https://tunnel.trycloudflare.com/webhooks",
            "https://hooks.example.com/x",
            // What the client connects to is the host after the user info.
            "http://localhost@hooks.example.com/x",
            "http://app.localhost@hooks.example.com/x",
            "http://localhost.example.com/x",
            "http://applocalhost/x",
            "http://0.0.0.0:3000/x",
            "http://[::ffff:127.0.0.1]/x",
            "not a url",
        ] {
            assert!(!local.may_send_to(url), "{url} is not this machine");
            assert!(
                !is_local_http(url),
                "{url} is no plain HTTP to this machine"
            );
        }
        // The origin of `--app-url`, wherever it is: scheme, host and port.
        let pointed = urls(
            Some("https://Backend.example.com/app"),
            Some("https://tunnel.trycloudflare.com"),
        );
        assert!(pointed.may_send_to("https://backend.example.com/webhooks"));
        assert!(pointed.may_send_to("https://backend.example.com:443/webhooks"));
        assert!(!pointed.may_send_to("http://backend.example.com/webhooks"));
        assert!(!pointed.may_send_to("https://backend.example.com:8443/webhooks"));
        assert!(!pointed.may_send_to("https://tunnel.trycloudflare.com/webhooks"));
        assert!(pointed.may_send_to("http://localhost:3000/webhooks"));
        assert_eq!(
            off_machine("https://hooks.example.com/x"),
            "lsf sends webhooks to this machine or to --app-url only: https://hooks.example.com/x \
             is neither; start lsf with --app-url http://localhost:<port> (your backend)"
        );
    }

    #[test]
    fn pubsub_eventbridge_and_kafka_are_listed_with_why_they_are_not_sent() {
        let urls = urls(Some("http://localhost:3000"), None);
        for (uri, said) in [
            ("pubsub://my-project:my-topic", "Pub/Sub"),
            (
                "arn:aws:events:us-east-1::event-source/aws.partner/shopify.com/1/source",
                "EventBridge",
            ),
            ("kafka://topic", "Kafka"),
            ("ftp://example.com/x", "`ftp`"),
            ("example.com/x", "absolute URL"),
            ("", "no uri"),
        ] {
            let Destination::Undeliverable { reason } = resolve_uri(uri, &urls) else {
                panic!("{uri} is not delivered");
            };
            assert!(reason.contains(said), "{uri}: {reason}");
        }
    }

    #[test]
    fn the_subscriptions_of_the_app_are_one_per_topic() {
        let config = AppConfig {
            webhooks_api_version: Some("2026-07".to_string()),
            webhook_subscriptions: vec![
                WebhookSubscriptionConfig {
                    topics: vec!["orders/create".to_string(), "ORDERS_UPDATED".to_string()],
                    uri: "/webhooks/orders".to_string(),
                    filter: Some("total_price:>100".to_string()),
                    include_fields: vec!["id".to_string()],
                    name: Some("orders".to_string()),
                    ..Default::default()
                },
                WebhookSubscriptionConfig {
                    compliance_topics: vec!["customers/redact".to_string()],
                    uri: "/webhooks/compliance".to_string(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let at = Utc::now();
        let subscriptions = from_app_config(&config, at);
        let topics: Vec<(&str, &str)> = subscriptions
            .iter()
            .map(|subscription| (subscription.id.as_str(), subscription.topic.as_str()))
            .collect();
        assert_eq!(
            topics,
            [
                ("shopify.app.toml#1", "orders/create"),
                ("shopify.app.toml#1", "orders/updated"),
                ("shopify.app.toml#2", "customers/redact"),
            ]
        );
        assert_eq!(subscriptions[0].api_version, "2026-07");
        assert_eq!(subscriptions[1].filter.as_deref(), Some("total_price:>100"));
        assert_eq!(subscriptions[1].name.as_deref(), Some("orders"));
        assert!(subscriptions[2].is_compliance());
        assert_eq!(
            from_app_config(&AppConfig::default(), at),
            Vec::<Subscription>::new()
        );
    }

    #[test]
    fn the_registry_creates_updates_and_deletes_per_shop() {
        let mut registry = Registry::new();
        let now = Utc::now();
        let input = SubscriptionInput {
            uri: Some("http://localhost:3000/webhooks".to_string()),
            include_fields: Some(vec!["id".to_string()]),
            ..Default::default()
        };
        let created = registry
            .create(
                "a.myshopify.com",
                "PRODUCTS_UPDATE",
                input.clone(),
                "2026-10",
                now,
            )
            .unwrap();
        assert_eq!(created.id, "gid://shopify/WebhookSubscription/1");
        assert_eq!(created.topic, "products/update");
        // The same address for the same topic is taken; another shop has its own.
        let taken = registry
            .create(
                "a.myshopify.com",
                "products/update",
                input.clone(),
                "2026-10",
                now,
            )
            .unwrap_err();
        assert_eq!(
            taken[0].message,
            "Address for this topic has already been taken"
        );
        let other = registry
            .create("b.myshopify.com", "PRODUCTS_UPDATE", input, "2026-10", now)
            .unwrap();
        assert_eq!(other.id, "gid://shopify/WebhookSubscription/2");
        assert_eq!(registry.list("a.myshopify.com").len(), 1);
        assert!(
            registry
                .get("b.myshopify.com", "gid://shopify/WebhookSubscription/1")
                .is_none()
        );

        let updated = registry
            .update(
                "a.myshopify.com",
                &created.id,
                SubscriptionInput {
                    filter: Some("status:active".to_string()),
                    ..Default::default()
                },
                now,
            )
            .unwrap();
        assert_eq!(updated.filter.as_deref(), Some("status:active"));
        assert_eq!(updated.uri, "http://localhost:3000/webhooks");

        let record = updated.to_record();
        assert_eq!(record["topic"], "PRODUCTS_UPDATE");
        assert_eq!(record["legacyResourceId"], "1");
        assert_eq!(record["format"], "JSON");
        assert_eq!(record["includeFields"], json!(["id"]));
        assert_eq!(record["apiVersion"]["handle"], "2026-10");
        assert_eq!(record["endpoint"]["__typename"], "WebhookHttpEndpoint");
        assert_eq!(
            record["endpoint"]["callbackUrl"],
            "http://localhost:3000/webhooks"
        );

        assert_eq!(
            registry.delete("a.myshopify.com", &created.id).unwrap().id,
            created.id
        );
        assert_eq!(
            registry.delete("a.myshopify.com", &created.id).unwrap_err()[0].message,
            "Webhook subscription does not exist"
        );
        registry.clear_all();
        assert!(registry.list("b.myshopify.com").is_empty());
    }

    #[test]
    fn the_registry_checks_the_topic_and_the_address() {
        let mut registry = Registry::new();
        let now = Utc::now();
        let with_uri = |uri: &str| SubscriptionInput {
            uri: Some(uri.to_string()),
            ..Default::default()
        };
        let error =
            |result: Result<Subscription, Vec<UserError>>| result.unwrap_err()[0].message.clone();
        assert!(
            error(registry.create(
                "s",
                "ORDERS_CREATED",
                with_uri("https://x.example"),
                "2026-10",
                now
            ))
            .contains("not a webhook subscription topic")
        );
        assert_eq!(
            error(registry.create(
                "s",
                "ORDERS_CREATE",
                with_uri("http://x.example/hooks"),
                "2026-10",
                now
            )),
            "Address protocol http:// is not supported"
        );
        assert_eq!(
            error(registry.create("s", "ORDERS_CREATE", with_uri(""), "2026-10", now)),
            "Address can't be blank"
        );
        assert_eq!(
            error(registry.create("s", "ORDERS_CREATE", with_uri("/relative"), "2026-10", now)),
            "Address is invalid"
        );
        for uri in [
            "https://x.example/hooks",
            "http://127.0.0.1:3000/hooks",
            "http://app.localhost/hooks",
            "pubsub://project:topic",
        ] {
            assert!(
                registry
                    .create("s", "ORDERS_CREATE", with_uri(uri), "2026-10", now)
                    .is_ok(),
                "{uri}"
            );
        }
        let pubsub = registry.list("s").last().unwrap().to_record();
        assert_eq!(pubsub["endpoint"]["pubSubProject"], "project");
        assert_eq!(pubsub["endpoint"]["pubSubTopic"], "topic");
    }
}
