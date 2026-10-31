//! The webhook topics of each version: every value of its `WebhookSubscriptionTopic` enum with
//! the topic as Shopify writes it in `shopify.app.toml` and in `X-Shopify-Topic`, and the three
//! compliance topics, which have no enum value (an app subscribes to them in `shopify.app.toml`
//! only).
//!
//! The written form is not derived from the enum value: `ORDERS_CREATE` is `orders/create`, but
//! `CUSTOMERS_EMAIL_MARKETING_CONSENT_UPDATE` is `customers_email_marketing_consent/update` and
//! `CUSTOMER_TAGS_ADDED` is `customer.tags_added`. The table of a version comes from the
//! description of each enum value in its introspection (`tools/admin/webhook_topics.mjs
//! --version <v>` writes `schemas/<v>/topics.json`). A topic a version does not have is refused
//! there: `webhookSubscriptionCreate` by validation (the enum of its schema), a subscription of
//! `shopify.app.toml` by its deliveries, which say so.

use std::sync::OnceLock;

use serde::Deserialize;

use crate::version::{self, ApiVersion};

/// A topic of the `WebhookSubscriptionTopic` enum.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Topic {
    /// The enum value: `ORDERS_CREATE`.
    pub name: String,
    /// The topic as Shopify writes it: `orders/create`.
    pub written: String,
    /// The access scopes a subscription to it requires: any one of them. Empty when the
    /// description names none.
    pub scopes: Vec<String>,
    /// What it reports, from the description of the enum value.
    pub summary: String,
    pub deprecated: bool,
}

/// The mandatory compliance topics, with what each asks of the app. They have no value in the
/// enum: `webhookSubscriptionCreate` cannot subscribe to them.
pub const COMPLIANCE_TOPICS: [(&str, &str); 3] = [
    (
        "customers/data_request",
        "Requests the data the app stores about a customer.",
    ),
    (
        "customers/redact",
        "Requests that the app deletes the data it stores about a customer.",
    ),
    (
        "shop/redact",
        "Sent 48 hours after a shop uninstalled the app: requests that the app deletes the data it stores about the shop.",
    ),
];

/// The topics of a version: `schemas/<version>/topics.json`.
#[derive(Debug, Deserialize)]
pub struct Topics {
    topics: Vec<Topic>,
}

/// The topics of a version, parsed the first time they are needed.
pub fn for_version(version: ApiVersion) -> &'static Topics {
    static PARSED: [OnceLock<Topics>; version::COUNT] = [const { OnceLock::new() }; version::COUNT];
    PARSED[version.index()].get_or_init(|| {
        serde_json::from_str(version.files().topics).unwrap_or_else(|error| {
            panic!("schemas/{version}/topics.json is the table tools/admin/webhook_topics.mjs writes: {error}")
        })
    })
}

impl Topics {
    /// Every topic of the enum, in its order.
    pub fn all(&'static self) -> &'static [Topic] {
        &self.topics
    }

    /// The topic of an enum value: `ORDERS_CREATE`.
    pub fn by_name(&'static self, name: &str) -> Option<&'static Topic> {
        self.topics.iter().find(|topic| topic.name == name)
    }

    /// The topic of its written form: `orders/create`.
    pub fn by_written(&'static self, written: &str) -> Option<&'static Topic> {
        self.topics.iter().find(|topic| topic.written == written)
    }

    /// The topic given either way: `ORDERS_CREATE` or `orders/create`.
    pub fn find(&'static self, text: &str) -> Option<&'static Topic> {
        self.by_name(text).or_else(|| self.by_written(text))
    }

    /// The written form of a topic given either way, as `shopify app webhook trigger --topic`
    /// takes it: `orders/create` or `ORDERS_CREATE`. The compliance topics too. `None` for a
    /// topic the version does not have.
    pub fn written_form(&'static self, text: &str) -> Option<&'static str> {
        let text = text.trim();
        if let Some((topic, _)) = COMPLIANCE_TOPICS.iter().find(|(topic, _)| *topic == text) {
            return Some(topic);
        }
        self.find(text).map(|topic| topic.written.as_str())
    }

    /// Whether `written` is a topic of the version (or a compliance topic): what a delivery of a
    /// subscription in this version can be about.
    pub fn has(&'static self, written: &str) -> bool {
        is_compliance(written) || self.by_written(written).is_some()
    }

    /// Every written topic, the compliance topics last: what a "did you mean" hint picks from.
    pub fn every_written(&'static self) -> impl Iterator<Item = &'static str> {
        self.topics
            .iter()
            .map(|topic| topic.written.as_str())
            .chain(COMPLIANCE_TOPICS.iter().map(|(topic, _)| *topic))
    }

    /// The topic closest to one that does not exist, for a "did you mean" hint.
    pub fn suggestion(&'static self, text: &str) -> Option<&'static str> {
        let lowered = text.trim().to_ascii_lowercase();
        lsf_core::util::closest_match(&lowered, self.every_written()).or_else(|| {
            // `ORDERS_CREATED` given the way the enum writes it.
            let names = self.topics.iter().map(|topic| topic.name.as_str());
            let name = lsf_core::util::closest_match(&text.trim().to_ascii_uppercase(), names)?;
            self.by_name(name).map(|topic| topic.written.as_str())
        })
    }
}

/// The topics of the default version: what code that does not know the version it is about
/// reads.
fn default_topics() -> &'static Topics {
    for_version(ApiVersion::default_version())
}

/// Every topic of the enum of the default version, in its order.
pub fn all() -> &'static [Topic] {
    default_topics().all()
}

/// [`Topics::by_name`] in the default version.
pub fn by_name(name: &str) -> Option<&'static Topic> {
    default_topics().by_name(name)
}

/// [`Topics::by_written`] in the default version.
pub fn by_written(written: &str) -> Option<&'static Topic> {
    default_topics().by_written(written)
}

/// Whether `written` is one of the compliance topics.
pub fn is_compliance(written: &str) -> bool {
    COMPLIANCE_TOPICS.iter().any(|(topic, _)| *topic == written)
}

/// [`Topics::written_form`] in the default version.
pub fn written_form(text: &str) -> Option<&'static str> {
    default_topics().written_form(text)
}

/// [`Topics::every_written`] in the default version.
pub fn every_written() -> impl Iterator<Item = &'static str> {
    default_topics().every_written()
}

/// [`Topics::suggestion`] in the default version.
pub fn suggestion(text: &str) -> Option<&'static str> {
    default_topics().suggestion(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_has_every_value_of_the_enum_once() {
        assert_eq!(all().len(), 227);
        let mut names: Vec<&str> = all().iter().map(|topic| topic.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 227);
        let mut written: Vec<&str> = all().iter().map(|topic| topic.written.as_str()).collect();
        written.sort_unstable();
        written.dedup();
        assert_eq!(written.len(), 227);
    }

    #[test]
    fn the_written_form_is_shopifys_not_a_mechanical_one() {
        for (name, written) in [
            ("ORDERS_CREATE", "orders/create"),
            ("ORDERS_UPDATED", "orders/updated"),
            (
                "CUSTOMERS_EMAIL_MARKETING_CONSENT_UPDATE",
                "customers_email_marketing_consent/update",
            ),
            ("APP_SCOPES_UPDATE", "app/scopes_update"),
            ("APP_UNINSTALLED", "app/uninstalled"),
            ("APP_SUBSCRIPTIONS_UPDATE", "app_subscriptions/update"),
            ("BULK_OPERATIONS_FINISH", "bulk_operations/finish"),
            ("INVENTORY_LEVELS_DISCONNECT", "inventory_levels/disconnect"),
            (
                "FULFILLMENT_ORDERS_CANCELLATION_REQUEST_ACCEPTED",
                "fulfillment_orders/cancellation_request_accepted",
            ),
            ("CUSTOMER_TAGS_ADDED", "customer.tags_added"),
            ("CUSTOMER_JOINED_SEGMENT", "customer.joined_segment"),
            ("SHOP_UPDATE", "shop/update"),
            ("DOMAINS_DESTROY", "domains/destroy"),
        ] {
            let topic = by_name(name).unwrap();
            assert_eq!(topic.written, written, "{name}");
            assert_eq!(by_written(written).unwrap().name, name);
        }
        // Every written form is a resource and an action, split by `/` (or `.` for the four
        // topics of customer events).
        for topic in all() {
            let separators = topic.written.matches(['/', '.']).count();
            assert_eq!(separators, 1, "{}", topic.written);
            assert_eq!(
                topic.written.replace(['/', '.'], "_").to_ascii_uppercase(),
                topic.name
            );
        }
    }

    #[test]
    fn a_topic_is_read_either_way_and_the_compliance_topics_too() {
        assert_eq!(written_form("orders/create"), Some("orders/create"));
        assert_eq!(written_form("ORDERS_CREATE"), Some("orders/create"));
        assert_eq!(written_form(" shop/redact "), Some("shop/redact"));
        assert_eq!(written_form("orders/created"), None);
        assert!(is_compliance("customers/redact"));
        assert!(!is_compliance("customers/create"));
        assert!(by_written("customers/redact").is_none());
        assert_eq!(suggestion("orders/created"), Some("orders/create"));
        assert_eq!(suggestion("ORDERS_CREATED"), Some("orders/create"));
        assert_eq!(suggestion("nothing/like/it/at/all"), None);
    }

    #[test]
    fn the_topics_of_each_version_are_the_values_of_its_enum() {
        for version in ApiVersion::all() {
            let schema = crate::schema::for_version(version);
            let values: Vec<&str> = schema
                .enum_values("WebhookSubscriptionTopic")
                .unwrap()
                .map(|value| value.name.as_str())
                .collect();
            let names: Vec<&str> = for_version(version)
                .all()
                .iter()
                .map(|topic| topic.name.as_str())
                .collect();
            assert_eq!(names, values, "schemas/{version}/topics.json");
            assert!(for_version(version).has("orders/create"));
            assert!(for_version(version).has("shop/redact"));
            assert!(!for_version(version).has("pages/create"));
        }
    }

    #[test]
    fn the_scopes_come_from_the_description() {
        assert_eq!(
            by_written("products/create").unwrap().scopes,
            ["read_products"]
        );
        assert_eq!(
            by_written("orders/create").unwrap().scopes,
            ["read_orders", "read_marketplace_orders"]
        );
        assert!(by_written("app/uninstalled").unwrap().scopes.is_empty());
        assert_eq!(
            by_written("products/create").unwrap().summary,
            "Occurs whenever a product is created."
        );
    }
}
