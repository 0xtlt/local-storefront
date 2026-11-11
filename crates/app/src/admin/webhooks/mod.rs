//! The webhooks of the app: its subscriptions, the payloads of the topics, their signature, and
//! their delivery to the backend of the app, from worker threads, with a log a test can wait on.
//!
//! An event of the state (`products/update` of `gid://shopify/Product/1` in `my-shop`) goes in
//! through [`Webhooks::emit`]; out comes a delivery for each subscription of the shop to its topic:
//! its payload built from the record, cut to the subscription's `include_fields`, checked against
//! its `filter`, signed, and posted to its `uri` resolved against the app URL. What lsf cannot do
//! (a topic whose payload it does not build, a Pub/Sub address) is logged as not sent, with why.
//!
//! See "Events and webhooks" in `crates/app/docs/admin/design.md`.

mod control;
pub mod delivery;
pub mod events;
pub mod payload;
pub mod samples;
pub mod sign;
pub mod subscriptions;
pub mod topics;

use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::config::AppConfig;

pub use control::{BuildTrigger, ShopList, TriggerError, TriggerRequest};
pub use delivery::{
    Deliveries, Delivery, DeliveryFilter, DeliveryOptions, DeliveryState, NewDelivery, Request,
    Waited,
};
pub use payload::{Lookup, Records, payload};
pub use subscriptions::{
    API_VERSION, AppUrls, Destination, Registry, Subscription, SubscriptionInput, resolve_uri,
};
pub use topics::Topic;

use delivery::SubscriptionRef;
use sign::HeaderValues;

use crate::admin::state::{Event, Record};
use crate::admin::version::{ApiVersion, Served, adapt};

/// Evaluates the `filter` of a subscription: Shopify's search syntax over the fields of the
/// payload, nested ones by a dot (`variants.price:>=100`), an array matching when one of its
/// items does. Unlike a search, `:` is equality and case counts.
///
/// `lsf_app::admin::search` plugs in here. Without an evaluator, a subscription's filter is taken to
/// match, and its deliveries say that it was not evaluated.
pub trait PayloadFilter: Send + Sync {
    /// Whether the payload matches, `now` the time of the event by the shop's clock (what a
    /// relative date of the filter counts from). An error is a filter that cannot be evaluated
    /// (a field the payload does not have, a syntax error): Shopify then delivers nothing.
    fn matches(&self, filter: &str, payload: &Value, now: DateTime<Utc>) -> Result<bool, String>;
}

impl<F> PayloadFilter for F
where
    F: Fn(&str, &Value, DateTime<Utc>) -> Result<bool, String> + Send + Sync,
{
    fn matches(&self, filter: &str, payload: &Value, now: DateTime<Utc>) -> Result<bool, String> {
        self(filter, payload, now)
    }
}

/// What the webhooks of a server are set up with.
#[derive(Clone, Debug)]
pub struct Settings {
    /// Where the backend of the app is.
    pub urls: AppUrls,
    /// The secret deliveries are signed with (`lsf_app::admin::auth::client_secret`).
    pub client_secret: String,
    pub delivery: DeliveryOptions,
}

/// The webhooks of one app, for every shop it is installed on.
pub struct Webhooks {
    urls: AppUrls,
    client_secret: String,
    /// The subscriptions of `shopify.app.toml`: every shop has them.
    app_subscriptions: Vec<Subscription>,
    /// Those created through the Admin API, per shop.
    registry: Mutex<Registry>,
    filter: Option<Arc<dyn PayloadFilter>>,
    deliveries: Deliveries,
    /// The versions payloads are built in: those the server serves.
    versions: Served,
    /// The one version every payload is built in, whatever its subscription asks for:
    /// `lsf app webhook trigger --api-version`.
    payload_version: Option<ApiVersion>,
}

/// A webhook sent on demand: `lsf app webhook trigger`, `POST /__lsf/webhooks/trigger`.
#[derive(Clone, Debug)]
pub struct Trigger<'a> {
    /// As written: `orders/create`.
    pub topic: &'a str,
    pub shop: &'a str,
    /// The body, before `include_fields`.
    pub payload: &'a Value,
    /// Send it there, rather than to the subscriptions of the topic, as
    /// `shopify app webhook trigger --address` does: the whole payload, no filter.
    pub address: Option<&'a str>,
    /// `X-Shopify-Triggered-At`.
    pub at: DateTime<Utc>,
}

impl Webhooks {
    /// The webhooks of an app: the subscriptions of its `shopify.app.toml`, none created yet
    /// through the Admin API. Starts the workers that deliver them.
    pub fn new(app_subscriptions: Vec<Subscription>, settings: Settings) -> Webhooks {
        Webhooks {
            urls: settings.urls,
            client_secret: settings.client_secret,
            app_subscriptions,
            registry: Mutex::new(Registry::new()),
            filter: None,
            deliveries: Deliveries::new(settings.delivery),
            versions: Served::default(),
            payload_version: None,
        }
    }

    /// The webhooks of the app of `config`, delivered to `app_url` (`--app-url`) or its
    /// `application_url`.
    pub fn for_app(
        config: &AppConfig,
        app_url: Option<String>,
        client_secret: String,
        delivery: DeliveryOptions,
    ) -> Webhooks {
        Webhooks::new(
            subscriptions::from_app_config(config, Utc::now()),
            Settings {
                urls: AppUrls {
                    app_url,
                    application_url: config.application_url.clone(),
                },
                client_secret,
                delivery,
            },
        )
    }

    /// Evaluates the filters of the subscriptions with `filter`.
    pub fn with_filter(mut self, filter: Arc<dyn PayloadFilter>) -> Webhooks {
        self.filter = Some(filter);
        self
    }

    /// Builds the payloads in the versions a server serves: a subscription's own when it is one
    /// of them, else as [`Served::for_webhook`] says. [`Served::default`] otherwise.
    pub fn with_versions(mut self, versions: Served) -> Webhooks {
        self.versions = versions;
        self
    }

    /// Builds every payload in `version`, as `shopify app webhook trigger --api-version` sends
    /// the sample of that version: a subscription that asks for another gets it all the same,
    /// and its delivery's notes say so.
    pub fn with_payload_version(mut self, version: ApiVersion) -> Webhooks {
        self.versions = Served::new([version]);
        self.payload_version = Some(version);
        self
    }

    /// The subscriptions of `shopify.app.toml`.
    pub fn app_subscriptions(&self) -> &[Subscription] {
        &self.app_subscriptions
    }

    /// Every subscription of a shop: those of `shopify.app.toml`, then those created through the
    /// Admin API.
    pub fn subscriptions(&self, shop: &str) -> Vec<Subscription> {
        let mut all = self.app_subscriptions.clone();
        all.extend(self.registry().list(shop).iter().cloned());
        all
    }

    /// The subscriptions created through the Admin API, for `webhookSubscriptionCreate` and the
    /// others.
    pub fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Whether the app, installed on the shop, has a subscription to `topic` there (one of
    /// `shopify.app.toml`, or the shop's own): a topic only reported when asked for
    /// (`customers/purchasing_summary`) is not emitted otherwise.
    pub fn subscribes(&self, shop: &str, topic: &str) -> bool {
        let registry = self.registry();
        registry.is_installed(shop)
            && self
                .app_subscriptions
                .iter()
                .chain(registry.list(shop))
                .any(|subscription| subscription.topic == topic)
    }

    /// Where the subscriptions of the shop to `topic` would deliver that lsf does not send to:
    /// neither this machine nor `--app-url` ([`AppUrls::may_send_to`]). What
    /// `lsf app webhook trigger` refuses before it sends anything.
    pub fn refused_urls(&self, shop: &str, topic: &str) -> Vec<String> {
        self.subscriptions(shop)
            .iter()
            .filter(|subscription| subscription.topic == topic)
            .filter_map(
                |subscription| match resolve_uri(&subscription.uri, &self.urls) {
                    Destination::Http { url } if !self.urls.may_send_to(&url) => Some(url),
                    _ => None,
                },
            )
            .collect()
    }

    /// The log of deliveries.
    pub fn deliveries(&self) -> &Deliveries {
        &self.deliveries
    }

    /// What the server prints of the webhooks when it starts:
    /// `3 subscriptions of shopify.app.toml → http://localhost:3000`.
    pub fn summary(&self) -> String {
        let count = self.app_subscriptions.len();
        let target = match self.urls.base() {
            None => "no app URL (give --app-url)".to_string(),
            Some(base) if count > 0 && !self.urls.may_send_to(base) => format!(
                "{base}, not this machine: not sent (give --app-url http://localhost:<port>)"
            ),
            Some(base) => base.to_string(),
        };
        format!(
            "{count} subscription{} of shopify.app.toml → {target}",
            if count == 1 { "" } else { "s" }
        )
    }

    /// Something changed: a delivery for each subscription of the shop to the topic of the event
    /// (the compliance topics are only sent on demand). Their ids in the log.
    ///
    /// Nothing is delivered for a shop the app is not installed on ([`Registry::is_installed`])
    /// but `app/uninstalled`, which says so: the subscriptions of `shopify.app.toml` apply "across
    /// every shop that installs your app" (shopify.dev, "Manage webhook subscriptions",
    /// <https://shopify.dev/docs/apps/build/webhooks/subscribe>), and those made through the
    /// Admin API are deleted on uninstall.
    ///
    /// The payload is built now, from the record the event names as `lookup` has it: the record
    /// as the change left it. For a `…/delete` topic (and `inventory_levels/disconnect`), the
    /// event's snapshot is used, the record being gone. For `app/scopes_update`, the snapshot is
    /// the installation before the change, whose scopes are `previous`.
    pub fn emit(&self, event: &Event, lookup: &dyn Lookup) -> Vec<u64> {
        let subscriptions: Vec<Subscription> = {
            let registry = self.registry();
            if !registry.is_installed(&event.shop) && event.topic != "app/uninstalled" {
                return Vec::new();
            }
            self.app_subscriptions
                .iter()
                .chain(registry.list(&event.shop))
                .filter(|subscription| {
                    subscription.topic == event.topic && !subscription.is_compliance()
                })
                .cloned()
                .collect()
        };
        if subscriptions.is_empty() {
            return Vec::new();
        }
        let lookup = WithShop {
            inner: lookup,
            domain: &event.shop,
        };
        let payload = event_payload(event, &lookup);
        let occurrence = Occurrence {
            topic: &event.topic,
            shop: &event.shop,
            event_id: &event.id,
            at: event.at,
            resource: Some(&event.gid),
        };
        subscriptions
            .iter()
            .map(|subscription| self.deliver(subscription, &occurrence, payload.as_ref()))
            .collect()
    }

    /// A webhook on demand: to the address of the trigger, else to every subscription of the
    /// shop to its topic (the compliance topics included). Their ids in the log; none when there
    /// is no address and no subscription to the topic.
    pub fn trigger(&self, trigger: &Trigger) -> Vec<u64> {
        let event_id = sign::new_id();
        let occurrence = Occurrence {
            topic: trigger.topic,
            shop: trigger.shop,
            event_id: &event_id,
            at: trigger.at,
            resource: trigger
                .payload
                .get("admin_graphql_api_id")
                .and_then(Value::as_str),
        };
        if let Some(address) = trigger.address {
            let subscription = Subscription {
                id: String::new(),
                source: subscriptions::Source::AppConfig { entry: 0 },
                topic: trigger.topic.to_string(),
                uri: address.to_string(),
                name: None,
                filter: None,
                include_fields: Vec::new(),
                metafield_namespaces: Vec::new(),
                metafields: Vec::new(),
                format: subscriptions::Format::Json,
                // Built in the server's primary version: `lsf app webhook trigger --api-version`
                // serves the version asked for.
                api_version: self.versions.primary().name().to_string(),
                created_at: trigger.at,
                updated_at: trigger.at,
            };
            return vec![self.deliver_to(&subscription, None, &occurrence, Ok(trigger.payload))];
        }
        self.subscriptions(trigger.shop)
            .iter()
            .filter(|subscription| subscription.topic == trigger.topic)
            .map(|subscription| self.deliver(subscription, &occurrence, Ok(trigger.payload)))
            .collect()
    }

    fn deliver(
        &self,
        subscription: &Subscription,
        occurrence: &Occurrence,
        payload: Result<&Value, &String>,
    ) -> u64 {
        self.deliver_to(
            subscription,
            Some(SubscriptionRef::of(subscription)),
            occurrence,
            payload,
        )
    }

    /// One delivery of `payload` for `subscription`, logged, and queued unless something keeps
    /// it from being sent. `reference` is `None` for an address given explicitly (`--address`,
    /// the `address` of a trigger), which is sent where it says; a subscription's goes only to
    /// this machine or to `--app-url` ([`AppUrls::may_send_to`]).
    fn deliver_to(
        &self,
        subscription: &Subscription,
        reference: Option<SubscriptionRef>,
        occurrence: &Occurrence,
        payload: Result<&Value, &String>,
    ) -> u64 {
        let Occurrence {
            topic,
            shop,
            event_id,
            at,
            resource,
        } = *occurrence;
        let mut notes = Vec::new();
        let (version, note) = match self.payload_version {
            Some(version) => (
                version,
                (subscription.api_version != version.name()).then(|| {
                    format!(
                        "the subscription asks for API version {}: the payload is {version}'s, the version this webhook was triggered in",
                        subscription.api_version
                    )
                }),
            ),
            None => self.versions.for_webhook(&subscription.api_version),
        };
        notes.extend(note);
        let destination = resolve_uri(&subscription.uri, &self.urls);
        let url = match &destination {
            Destination::Http { url } => Some(url.clone()),
            Destination::Undeliverable { .. } => None,
        };
        let off_machine = url
            .as_deref()
            .filter(|url| reference.is_some() && !self.urls.may_send_to(url))
            .map(subscriptions::off_machine);
        let webhook_id = sign::new_id();
        let delivery = |request: Option<Request>, notes: Vec<String>| NewDelivery {
            webhook_id: webhook_id.clone(),
            event_id: event_id.to_string(),
            topic: topic.to_string(),
            shop: shop.to_string(),
            resource: resource.map(str::to_string),
            subscription: reference.clone(),
            url: url.clone(),
            api_version: version.name().to_string(),
            triggered_at: at,
            request,
            notes,
        };
        let topics = topics::for_version(version);
        if !subscription.is_compliance() && !topics.has(topic) {
            return self.deliveries.not_sent(
                delivery(None, notes),
                format!("`{topic}` is not a webhook topic of API version {version}"),
            );
        }
        let payload = match payload {
            Ok(payload) => payload,
            Err(reason) => {
                return self
                    .deliveries
                    .not_sent(delivery(None, notes), reason.clone());
            }
        };
        // The builders write the payload of the records' version; this version's adapters give
        // its own.
        let adapted;
        let payload = if adapt::adapts_payload(version.adapters(), topic) {
            adapted = adapt::payload(version.adapters(), topic, payload);
            &adapted
        } else {
            payload
        };
        if subscription.format == subscriptions::Format::Xml {
            return self.deliveries.not_sent(
                delivery(None, notes),
                "the subscription asks for XML payloads: lsf builds JSON ones only".to_string(),
            );
        }
        let body = payload::include_fields(payload, &subscription.include_fields);
        let mut filtered_out = None;
        if let Some(filter) = &subscription.filter {
            match &self.filter {
                None => notes.push(format!(
                    "the filter `{filter}` was not evaluated: lsf delivered as if it matched"
                )),
                Some(evaluator) => match evaluator.matches(filter, &body, at) {
                    Ok(true) => {}
                    Ok(false) => {
                        filtered_out =
                            Some(format!("the filter `{filter}` does not match the payload"))
                    }
                    Err(error) => {
                        filtered_out = Some(format!(
                            "the filter `{filter}` cannot be evaluated ({error}): Shopify delivers nothing then"
                        ))
                    }
                },
            }
        }
        let text = serde_json::to_string(&body).unwrap_or_default();
        let headers = sign::headers(
            &HeaderValues {
                topic,
                shop,
                api_version: version.name(),
                webhook_id: &webhook_id,
                event_id,
                triggered_at: at,
                name: subscription.name.as_deref(),
            },
            &self.client_secret,
            text.as_bytes(),
        );
        let request = Request {
            headers: headers.into_iter().collect(),
            body: text,
        };
        let new = delivery(Some(request), notes);
        if let Some(reason) = filtered_out {
            return self.deliveries.not_sent(new, reason);
        }
        match (destination, off_machine) {
            (Destination::Http { .. }, None) => self.deliveries.send(new),
            (Destination::Http { .. }, Some(reason))
            | (Destination::Undeliverable { reason }, _) => self.deliveries.not_sent(new, reason),
        }
    }
}

/// What a delivery is of: an event, or a trigger.
#[derive(Clone, Copy)]
struct Occurrence<'a> {
    topic: &'a str,
    shop: &'a str,
    /// `X-Shopify-Event-Id`, shared by the deliveries of one occurrence.
    event_id: &'a str,
    /// `X-Shopify-Triggered-At`.
    at: DateTime<Utc>,
    /// The GID of the record it is about.
    resource: Option<&'a str>,
}

/// The payload of an event, or why there is none.
fn event_payload(event: &Event, lookup: &dyn Lookup) -> Result<Value, String> {
    if !payload::is_built(&event.topic) {
        return Err(format!(
            "lsf does not build the payload of `{}`",
            event.topic
        ));
    }
    let gone = event.topic.ends_with("/delete") || event.topic == "inventory_levels/disconnect";
    let record = if gone {
        event.snapshot.clone().or_else(|| lookup.record(&event.gid))
    } else {
        lookup.record(&event.gid).or_else(|| event.snapshot.clone())
    };
    let Some(mut record) = record else {
        return Err(format!(
            "`{}` is not a record of the shop: there is nothing to build the payload from",
            event.gid
        ));
    };
    if gone && !record.contains_key(payload::DELETED_AT) {
        record.insert(
            payload::DELETED_AT.to_string(),
            json!(event.at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
        );
    }
    // A change whose event kept the record before it: the builders of the topics that say what
    // changed (`customer.tags_added`) read it.
    if !gone
        && let Some(before) = &event.snapshot
        && !record.contains_key(payload::PREVIOUS)
    {
        record.insert(payload::PREVIOUS.to_string(), Value::Object(before.clone()));
    }
    if event.topic == "app/scopes_update" && !record.contains_key(payload::PREVIOUS_ACCESS_SCOPES) {
        let previous = event
            .snapshot
            .as_ref()
            .and_then(|before| before.get("accessScopes").cloned())
            .unwrap_or_else(|| json!([]));
        record.insert(payload::PREVIOUS_ACCESS_SCOPES.to_string(), previous);
    }
    payload::payload(&event.topic, &record, lookup)
        .ok_or_else(|| format!("lsf does not build the payload of `{}`", event.topic))
}

/// A lookup whose shop has at least its domain: what the payloads that name the shop need when
/// the state gives no `Shop` record.
struct WithShop<'a> {
    inner: &'a dyn Lookup,
    domain: &'a str,
}

impl Lookup for WithShop<'_> {
    fn record(&self, gid: &str) -> Option<Record> {
        self.inner.record(gid)
    }

    fn shop(&self) -> Option<Record> {
        let mut shop = self.inner.shop().unwrap_or_default();
        shop.entry("myshopifyDomain".to_string())
            .or_insert_with(|| json!(self.domain));
        Some(shop)
    }
}

#[cfg(test)]
mod tests;
