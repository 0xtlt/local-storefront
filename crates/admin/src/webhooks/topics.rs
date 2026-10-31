//! The webhook topics: every value of the `WebhookSubscriptionTopic` enum of API 2026-10 with the
//! topic as Shopify writes it in `shopify.app.toml` and in `X-Shopify-Topic`, and the three
//! compliance topics, which have no enum value (an app subscribes to them in `shopify.app.toml`
//! only).
//!
//! The written form is not derived from the enum value: `ORDERS_CREATE` is `orders/create`, but
//! `CUSTOMERS_EMAIL_MARKETING_CONSENT_UPDATE` is `customers_email_marketing_consent/update` and
//! `CUSTOMER_TAGS_ADDED` is `customer.tags_added`. The table comes from the description of each
//! enum value in the introspection of the Admin API (`tools/admin/webhook_topics.mjs`).

/// A topic of the `WebhookSubscriptionTopic` enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Topic {
    /// The enum value: `ORDERS_CREATE`.
    pub name: &'static str,
    /// The topic as Shopify writes it: `orders/create`.
    pub written: &'static str,
    /// The access scopes a subscription to it requires: any one of them. Empty when the
    /// description names none.
    pub scopes: &'static [&'static str],
    /// What it reports, from the description of the enum value.
    pub summary: &'static str,
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

/// Every topic of the enum, in its order.
pub fn all() -> &'static [Topic] {
    super::topics_2026_10::TOPICS
}

/// The topic of an enum value: `ORDERS_CREATE`.
pub fn by_name(name: &str) -> Option<&'static Topic> {
    all().iter().find(|topic| topic.name == name)
}

/// The topic of its written form: `orders/create`.
pub fn by_written(written: &str) -> Option<&'static Topic> {
    all().iter().find(|topic| topic.written == written)
}

/// Whether `written` is one of the compliance topics.
pub fn is_compliance(written: &str) -> bool {
    COMPLIANCE_TOPICS.iter().any(|(topic, _)| *topic == written)
}

/// The written form of a topic given either way, as `shopify app webhook trigger --topic` takes
/// it: `orders/create` or `ORDERS_CREATE`. The compliance topics too. `None` for a topic that does
/// not exist in 2026-10.
pub fn written_form(text: &str) -> Option<&'static str> {
    let text = text.trim();
    if let Some((topic, _)) = COMPLIANCE_TOPICS.iter().find(|(topic, _)| *topic == text) {
        return Some(topic);
    }
    by_written(text)
        .or_else(|| by_name(text))
        .map(|topic| topic.written)
}

/// Every written topic, the compliance topics last: what a "did you mean" hint picks from.
pub fn every_written() -> impl Iterator<Item = &'static str> {
    all()
        .iter()
        .map(|topic| topic.written)
        .chain(COMPLIANCE_TOPICS.iter().map(|(topic, _)| *topic))
}

/// The topic closest to one that does not exist, for a "did you mean" hint.
pub fn suggestion(text: &str) -> Option<&'static str> {
    let lowered = text.trim().to_ascii_lowercase();
    lsf_core::util::closest_match(&lowered, every_written()).or_else(|| {
        // `ORDERS_CREATED` given the way the enum writes it.
        let names = all().iter().map(|topic| topic.name);
        let name = lsf_core::util::closest_match(&text.trim().to_ascii_uppercase(), names)?;
        by_name(name).map(|topic| topic.written)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_has_every_value_of_the_enum_once() {
        assert_eq!(all().len(), 227);
        let mut names: Vec<&str> = all().iter().map(|topic| topic.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 227);
        let mut written: Vec<&str> = all().iter().map(|topic| topic.written).collect();
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
