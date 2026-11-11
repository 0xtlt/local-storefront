//! Shopify's Events: the `[[events.subscription]]` of `shopify.app.toml`
//! (<https://shopify.dev/docs/apps/build/events-webhooks>), which the app templates ship for
//! `webhooks.products.update.tsx`.
//!
//! An Event names a resource (`topic = "Product"`), the `actions` it is sent for (`create`,
//! `update`, `delete`), and for `update` the `triggers`, the fields whose change sends it
//! (`product.variants.price`, `product.variants.*`, `product.*`). Its body is `topic`, `action`,
//! `handle`, `fields_changed` (`added`, `updated`, `removed`), `query_variables` (the ids of what
//! changed: `productId`, and `variantsId` for a variant), and, with a `query`, `data` (and
//! `errors`): what the query answers with those variables. A `query_filter` (Shopify's search
//! syntax over `data`) that does not match sends nothing. The headers are `Shopify-Topic`,
//! `Shopify-Action`, `Shopify-Handle`, `Shopify-Hmac-Sha256`, `Shopify-Shop-Domain`,
//! `Shopify-Api-Version`, `Shopify-Webhook-Id` and `Shopify-Triggered-At`, without `X-`, as
//! `@shopify/shopify-api` 15 reads them (`ShopifyEventsHeader`).
//!
//! lsf delivers the Events of `Product`: of its fields (`title`, `handle`, `status`, `tags`,
//! `vendor`, `productType`, `descriptionHtml`, `templateSuffix`) and of its variants' (`title`,
//! `price`, `compareAtPrice`, `sku`, `barcode`, `inventoryPolicy`, `taxable`, `position`). A
//! variant added or removed is in `added` or `removed` (`product.variants`). Each changed
//! variant is a delivery of its own, with its ids; the product's own fields are one more.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};

use super::Webhooks;
use super::delivery::{NewDelivery, Request, SubscriptionRef};
use super::sign::{self, triggered_at};
use super::subscriptions::{Destination, resolve_uri};
use crate::admin::auth::signing::webhook_hmac;
use crate::admin::state::ShopState;
use crate::config::EventSubscriptionConfig;

/// The product fields an Event can be triggered by.
const PRODUCT_FIELDS: &[&str] = &[
    "title",
    "handle",
    "status",
    "tags",
    "vendor",
    "productType",
    "descriptionHtml",
    "templateSuffix",
];

/// The variant fields an Event can be triggered by.
const VARIANT_FIELDS: &[&str] = &[
    "title",
    "price",
    "compareAtPrice",
    "sku",
    "barcode",
    "inventoryPolicy",
    "taxable",
    "position",
];

/// The topic and the action of an Event, for a webhook topic lsf emits: `products/update` is
/// `Product` `update`.
pub fn topic_action(webhook_topic: &str) -> Option<(&'static str, &'static str)> {
    match webhook_topic {
        "products/create" => Some(("Product", "create")),
        "products/update" => Some(("Product", "update")),
        "products/delete" => Some(("Product", "delete")),
        _ => None,
    }
}

/// What an Event compares of a product: its fields and its variants', by id.
pub fn snapshot(state: &ShopState, product: &str) -> Option<Value> {
    let record = state.get(product)?;
    let fields: Map<String, Value> = PRODUCT_FIELDS
        .iter()
        .map(|field| {
            (
                field.to_string(),
                record.get(*field).cloned().unwrap_or(Value::Null),
            )
        })
        .collect();
    let variants: Map<String, Value> = record
        .get("variants")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|id| {
            let variant = state.get(id)?;
            let fields: Map<String, Value> = VARIANT_FIELDS
                .iter()
                .map(|field| {
                    (
                        field.to_string(),
                        variant.get(*field).cloned().unwrap_or(Value::Null),
                    )
                })
                .collect();
            Some((id.to_string(), Value::Object(fields)))
        })
        .collect();
    Some(json!({"fields": fields, "variants": variants}))
}

/// The snapshots of every product of a shop: what the first change of each is compared with.
pub fn snapshots(state: &ShopState) -> BTreeMap<String, Value> {
    let products: Vec<String> = state.keys("Product").map(str::to_string).collect();
    products
        .into_iter()
        .filter_map(|id| Some((id.clone(), snapshot(state, &id)?)))
        .collect()
}

/// A change of a product, as an Event says it: one per entity changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Change {
    pub variables: Map<String, Value>,
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub removed: Vec<String>,
    /// The paths without their ids (`product.variants.price`), what the triggers match.
    pub fields: Vec<String>,
}

/// What changed of `product` from `before` to `after`, an entity per [`Change`].
pub fn changes(product: &str, before: Option<&Value>, after: Option<&Value>) -> Vec<Change> {
    let base = format!("product[id: '{product}']");
    let product_vars = || {
        let mut variables = Map::new();
        variables.insert("productId".into(), json!(product));
        variables
    };
    let (before, after) = match (before, after) {
        (None, None) => return Vec::new(),
        (None, Some(_)) => {
            return vec![Change {
                variables: product_vars(),
                added: vec![base],
                fields: vec!["product".into()],
                ..Change::default()
            }];
        }
        (Some(_), None) => {
            return vec![Change {
                variables: product_vars(),
                removed: vec![base],
                fields: vec!["product".into()],
                ..Change::default()
            }];
        }
        (Some(before), Some(after)) => (before, after),
    };
    let mut out = Vec::new();
    let mut own = Change {
        variables: product_vars(),
        ..Change::default()
    };
    for field in PRODUCT_FIELDS {
        if before["fields"][*field] != after["fields"][*field] {
            own.updated.push(format!("{base}.{field}"));
            own.fields.push(format!("product.{field}"));
        }
    }
    if !own.fields.is_empty() {
        out.push(own);
    }
    let empty = Map::new();
    let old = before["variants"].as_object().unwrap_or(&empty);
    let new = after["variants"].as_object().unwrap_or(&empty);
    let mut ids: Vec<&String> = old.keys().chain(new.keys()).collect();
    ids.sort();
    ids.dedup();
    for id in ids {
        let path = format!("{base}.variants[id: '{id}']");
        let mut change = Change {
            variables: product_vars(),
            ..Change::default()
        };
        change.variables.insert("variantsId".into(), json!(id));
        match (old.get(id), new.get(id)) {
            (None, Some(_)) => {
                change.added.push(path);
                change.fields.push("product.variants".into());
            }
            (Some(_), None) => {
                change.removed.push(path);
                change.fields.push("product.variants".into());
            }
            (Some(was), Some(is)) => {
                for field in VARIANT_FIELDS {
                    if was[*field] != is[*field] {
                        change.updated.push(format!("{path}.{field}"));
                        change.fields.push(format!("product.variants.{field}"));
                    }
                }
            }
            (None, None) => {}
        }
        if !change.fields.is_empty() {
            out.push(change);
        }
    }
    out
}

/// Whether a trigger (`product.variants.price`, `product.variants.*`, `product.*`) covers a
/// changed field (`product.variants.price`).
pub fn trigger_matches(trigger: &str, field: &str) -> bool {
    let trigger = trigger.trim();
    match trigger.strip_suffix(".*") {
        Some(parent) => field == parent || field.starts_with(&format!("{parent}.")),
        None => trigger == field,
    }
}

/// Whether the subscription is sent for this change: its action, and for `update` a trigger.
pub fn wanted(subscription: &EventSubscriptionConfig, action: &str, change: &Change) -> bool {
    if !subscription
        .actions
        .iter()
        .any(|wanted| wanted.eq_ignore_ascii_case(action))
    {
        return false;
    }
    action != "update"
        || subscription.triggers.iter().any(|trigger| {
            change
                .fields
                .iter()
                .any(|field| trigger_matches(trigger, field))
        })
}

/// The body of an Event.
pub fn body(
    subscription: &EventSubscriptionConfig,
    action: &str,
    change: &Change,
    data: Option<(Value, Vec<String>)>,
) -> Value {
    let mut body = Map::new();
    body.insert("topic".into(), json!(subscription.topic));
    body.insert("action".into(), json!(action));
    body.insert("handle".into(), json!(subscription.handle));
    if let Some((data, errors)) = data {
        body.insert("data".into(), data);
        if !errors.is_empty() {
            let errors: Vec<Value> = errors
                .into_iter()
                .map(|message| json!({"message": message}))
                .collect();
            body.insert("errors".into(), Value::Array(errors));
        }
    }
    body.insert(
        "fields_changed".into(),
        json!({"added": change.added, "updated": change.updated, "removed": change.removed}),
    );
    body.insert(
        "query_variables".into(),
        Value::Object(change.variables.clone()),
    );
    Value::Object(body)
}

/// One Event to send: what [`Webhooks::send_event`] takes.
pub struct Outgoing<'a> {
    pub subscription: &'a EventSubscriptionConfig,
    pub action: &'a str,
    pub shop: &'a str,
    pub api_version: &'a str,
    pub resource: &'a str,
    pub at: DateTime<Utc>,
    pub body: Value,
    /// Why it is not sent: its `query_filter` did not match.
    pub filtered_out: Option<String>,
}

impl Webhooks {
    /// Whether the app is installed on the shop: Events, like webhooks, go to installed apps.
    pub fn installed(&self, shop: &str) -> bool {
        self.registry().is_installed(shop)
    }

    /// Signs and queues an Event, logged under its topic (`Product`). Its id in the log.
    pub fn send_event(&self, event: Outgoing<'_>) -> u64 {
        let text = serde_json::to_string(&event.body).unwrap_or_default();
        let webhook_id = sign::new_id();
        let headers: Vec<(String, String)> = [
            ("Content-Type", "application/json".to_string()),
            ("Shopify-Topic", event.subscription.topic.clone()),
            ("Shopify-Action", event.action.to_string()),
            ("Shopify-Handle", event.subscription.handle.clone()),
            (
                "Shopify-Hmac-Sha256",
                webhook_hmac(&self.client_secret, text.as_bytes()),
            ),
            ("Shopify-Shop-Domain", event.shop.to_string()),
            ("Shopify-Api-Version", event.api_version.to_string()),
            ("Shopify-Webhook-Id", webhook_id.clone()),
            ("Shopify-Triggered-At", triggered_at(event.at)),
            ("User-Agent", sign::USER_AGENT.to_string()),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect();
        let destination = resolve_uri(&event.subscription.uri, &self.urls);
        let url = match &destination {
            Destination::Http { url } => Some(url.clone()),
            Destination::Undeliverable { .. } => None,
        };
        let new = NewDelivery {
            webhook_id: webhook_id.clone(),
            event_id: webhook_id,
            topic: event.subscription.topic.clone(),
            shop: event.shop.to_string(),
            resource: Some(event.resource.to_string()),
            subscription: Some(SubscriptionRef {
                id: format!("events:{}", event.subscription.handle),
                name: Some(event.subscription.handle.clone()),
                uri: event.subscription.uri.clone(),
            }),
            url: url.clone(),
            api_version: event.api_version.to_string(),
            triggered_at: event.at,
            request: Some(Request {
                headers: headers.into_iter().collect(),
                body: text,
            }),
            notes: vec![format!(
                "an Event of [[events.subscription]] {} of shopify.app.toml ({} {})",
                event.subscription.handle, event.subscription.topic, event.action
            )],
        };
        if let Some(reason) = event.filtered_out {
            return self.deliveries.not_sent(new, reason);
        }
        match (destination, url) {
            (Destination::Http { .. }, Some(url)) if self.urls.may_send_to(&url) => {
                self.deliveries.send(new)
            }
            (Destination::Http { .. }, Some(url)) => self
                .deliveries
                .not_sent(new, super::subscriptions::off_machine(&url)),
            (Destination::Undeliverable { reason }, _) => self.deliveries.not_sent(new, reason),
            (Destination::Http { .. }, None) => self
                .deliveries
                .not_sent(new, "the Event has no URL".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn product(price: &str, title: &str) -> Value {
        json!({
            "fields": {"title": title, "status": "ACTIVE"},
            "variants": {"gid://shopify/ProductVariant/2": {"price": price, "title": "Default"}},
        })
    }

    #[test]
    fn a_price_change_is_a_variant_change_with_its_ids() {
        let changes = changes(
            "gid://shopify/Product/1",
            Some(&product("10.00", "Tote")),
            Some(&product("12.00", "Tote")),
        );
        assert_eq!(changes.len(), 1);
        assert_eq!(
            changes[0].updated,
            vec![
                "product[id: 'gid://shopify/Product/1'].variants[id: 'gid://shopify/ProductVariant/2'].price"
            ]
        );
        assert_eq!(changes[0].fields, vec!["product.variants.price"]);
        assert_eq!(
            Value::Object(changes[0].variables.clone()),
            json!({"productId": "gid://shopify/Product/1", "variantsId": "gid://shopify/ProductVariant/2"})
        );
        let subscription = EventSubscriptionConfig {
            handle: "price".into(),
            topic: "Product".into(),
            actions: vec!["update".into()],
            triggers: vec!["product.variants.price".into()],
            ..EventSubscriptionConfig::default()
        };
        assert!(wanted(&subscription, "update", &changes[0]));
        assert!(!wanted(&subscription, "create", &changes[0]));
        let title = super::changes(
            "gid://shopify/Product/1",
            Some(&product("10.00", "Tote")),
            Some(&product("10.00", "Bag")),
        );
        assert_eq!(title[0].fields, vec!["product.title"]);
        assert!(!wanted(&subscription, "update", &title[0]));
    }

    #[test]
    fn triggers_cover_their_children() {
        assert!(trigger_matches("product.*", "product.variants.price"));
        assert!(trigger_matches(
            "product.variants.*",
            "product.variants.price"
        ));
        assert!(trigger_matches("product.variants.*", "product.variants"));
        assert!(!trigger_matches("product.variants.*", "product.title"));
        assert!(!trigger_matches(
            "product.variants.price",
            "product.variants.compareAtPrice"
        ));
    }
}
