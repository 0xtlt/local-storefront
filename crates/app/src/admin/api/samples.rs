//! The records a webhook is built from outside of an event: `POST /__lsf/webhooks/trigger` and
//! `lsf app webhook trigger` send the payload of a record of the shop (the one named, else a
//! representative one of the topic's type), so that the backend gets the products, orders and
//! customers of the store data rather than Shopify's examples. A topic whose type the shop has
//! no record of falls back to the built-in sample of [`samples::fixed`].

use chrono::Utc;
use serde_json::{Value, json};

use crate::admin::state::{Record, ShopState};
use crate::admin::webhooks::payload::{self, DELETED_AT, Lookup, PREVIOUS_ACCESS_SCOPES, Records};
use crate::admin::webhooks::samples::{self, Sample, Samples};

/// The records of a shop, for the payload builders.
pub struct ShopLookup<'a>(pub &'a ShopState);

impl Lookup for ShopLookup<'_> {
    fn record(&self, gid: &str) -> Option<Record> {
        self.0.get(gid).cloned()
    }

    fn shop(&self) -> Option<Record> {
        self.0.shop().cloned()
    }
}

/// The types of record a topic's payload is built from, in the order a representative one is
/// looked for.
pub fn record_types(topic: &str) -> &'static [&'static str] {
    let (resource, _) = topic.split_once('/').unwrap_or((topic, ""));
    match (resource, topic) {
        (_, "app/scopes_update") => &["AppInstallation"],
        ("app" | "shop", _) => &["Shop"],
        ("products", _) => &["Product"],
        ("collections", _) => &["Collection"],
        (_, "customers/merge" | "customers/purchasing_summary") => &[],
        ("customers", _)
        | ("customers_email_marketing_consent", _)
        | ("customers_marketing_consent", _)
        | ("customers_whats_app_marketing_consent", _)
        | ("customer.tags_added" | "customer.tags_removed", _) => &["Customer"],
        ("customer_account_settings", _) => &["Shop"],
        ("customer_groups", _) => &["SavedSearch"],
        ("segments", _) => &["Segment"],
        ("checkouts", _) => &["AbandonedCheckout"],
        ("companies", _) => &["Company"],
        ("company_locations", _) => &["CompanyLocation"],
        ("company_contacts", _) => &["CompanyContact"],
        ("company_contact_roles", _) => &["CompanyContactRoleAssignment"],
        (_, "orders/edited") => &["CalculatedOrder"],
        ("orders", _) => &["Order"],
        ("order_transactions", _) => &["OrderTransaction"],
        ("payment_schedules", _) => &["PaymentSchedule"],
        ("payment_terms", _) => &["PaymentTerms"],
        ("tender_transactions", _) => &["TenderTransaction"],
        ("fulfillment_orders", _) => &["FulfillmentOrder"],
        ("fulfillment_holds", _) => &["FulfillmentHold"],
        ("fulfillment_events", _) => &["FulfillmentEvent"],
        ("requested_order_edit", _) => &["RequestedOrderEdit"],
        ("returns", _) => &["Return"],
        ("reverse_deliveries", _) => &["ReverseDelivery"],
        ("reverse_fulfillment_orders", _) => &["ReverseFulfillmentOrder"],
        ("fulfillments", _) => &["Fulfillment"],
        ("refunds", _) => &["Refund"],
        ("draft_orders", _) => &["DraftOrder"],
        ("inventory_items", _) => &["InventoryItem"],
        ("inventory_levels", _) => &["InventoryLevel"],
        ("inventory_shipments", _) => &["InventoryShipment"],
        ("inventory_transfers", _) => &["InventoryTransfer"],
        ("locations", _) => &["Location"],
        ("variants", _) => &["ProductVariant"],
        ("selling_plan_groups", _) => &["SellingPlanGroup"],
        ("profiles", _) => &["DeliveryProfile"],
        ("delivery_promise_settings", _) => &["DeliveryPromiseSetting"],
        ("app_subscriptions", _) => &["AppSubscription"],
        ("app_purchases_one_time", _) => &["AppPurchaseOneTime"],
        (_, "discounts/redeemcode_added" | "discounts/redeemcode_removed") => &["DiscountCodeNode"],
        ("bulk_operations", _) => &["BulkOperation"],
        ("metafield_definitions", _) => &["MetafieldDefinition"],
        ("metaobjects", _) => &["Metaobject"],
        ("discounts", _) => &["DiscountAutomaticNode", "DiscountCodeNode"],
        ("markets", _) => &["Market"],
        ("locales", _) => &["ShopLocale"],
        ("domains", _) => &["Domain"],
        ("channels", _) => &["Channel"],
        ("collection_listings", _) => &["Collection"],
        ("product_listings" | "scheduled_product_listings", _) => &["Product"],
        ("customer_payment_methods", _) => &["CustomerPaymentMethod"],
        ("subscription_contracts", _) => &["SubscriptionContract"],
        ("subscription_billing_attempts", _) => &["SubscriptionBillingAttempt"],
        ("disputes", _) => &["ShopifyPaymentsDispute"],
        ("tax_services", _) => &["TaxAppConfiguration"],
        ("product_feeds", _) => &["ProductFeed"],
        ("finance_app_staff_member", _) => &["StaffMember"],
        ("finance_kyc_information", _) => &["FinanceKycInformation"],
        ("rollouts", _) => &["Rollout"],
        ("themes", _) => &["OnlineStoreTheme"],
        _ => &[],
    }
}

/// Why a sample cannot be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SampleError {
    /// The record named is not one of the shop.
    NotFound(String),
    /// lsf does not build the payload of the topic.
    NotBuilt(String),
}

/// The record of `topic` to build a payload from: `id`, or the first record of the topic's
/// type, prepared as the webhooks prepare the record of an event (`deletedAt` for a
/// `…/delete`, the scopes before for `app/scopes_update`).
pub fn sample(shop: &ShopState, topic: &str, id: Option<&str>) -> Result<Sample, SampleError> {
    if !payload::is_built(topic) {
        return Err(SampleError::NotBuilt(format!(
            "lsf does not build the payload of `{topic}`: it builds {}",
            payload::BUILT_TOPICS.join(", ")
        )));
    }
    let record = match id {
        Some(id) => Some(shop.get(id).cloned().ok_or_else(|| {
            SampleError::NotFound(format!("{id} is not a record of {}", shop.domain()))
        })?),
        None => representative(shop, topic),
    };
    let Some(mut record) = record else {
        return samples::fixed(topic).ok_or_else(|| {
            SampleError::NotFound(format!(
                "{} has no record to build `{topic}` from",
                shop.domain()
            ))
        });
    };
    if (topic.ends_with("/delete") || topic == "inventory_levels/disconnect")
        && !record.contains_key(DELETED_AT)
    {
        record.insert(
            DELETED_AT.into(),
            json!(crate::admin::state::value::date_time(Utc::now())),
        );
    }
    // A sync of a feed is of one of the products of its channel.
    if let Some(sync) =
        crate::admin::domains::catalog::feeds::trigger_snapshot(shop, &record, topic)
    {
        record = sync;
    }
    if topic == "app/scopes_update" && !record.contains_key(PREVIOUS_ACCESS_SCOPES) {
        let scopes = record
            .get("accessScopes")
            .cloned()
            .unwrap_or_else(|| json!([]));
        record.insert(PREVIOUS_ACCESS_SCOPES.into(), scopes);
    }
    let lookup = Records::of(
        shop.iter()
            .filter(|(key, _)| crate::admin::state::gid_type(key).is_some())
            .map(|(_, record)| record.clone()),
        shop.shop().cloned(),
    );
    Ok(Sample { record, lookup })
}

/// The record a topic's payload is built from when none is named: the first of the topic's
/// type. `orders/cancelled` "Occurs whenever an order is cancelled" (`WebhookSubscriptionTopic`),
/// so its record is the first cancelled order, else the first order, cancelled now by the
/// customer (the reason of Shopify's example payload).
fn representative(shop: &ShopState, topic: &str) -> Option<Record> {
    let first = record_types(topic)
        .iter()
        .find_map(|type_name| shop.all(type_name).next().cloned());
    if topic != "orders/cancelled" {
        return first;
    }
    let cancelled = shop
        .all("Order")
        .find(|order| order.get("cancelledAt").is_some_and(|at| !at.is_null()));
    if let Some(order) = cancelled {
        return Some(order.clone());
    }
    first.map(|mut order| {
        order.insert(
            "cancelledAt".into(),
            json!(crate::admin::state::value::date_time(Utc::now())),
        );
        order.insert("cancelReason".into(), json!("CUSTOMER"));
        order
    })
}

/// The samples of a shop seeded from the store data: what `lsf app webhook trigger` builds its
/// payloads from.
pub struct ShopSamples(pub ShopState);

impl Samples for ShopSamples {
    fn shop(&self) -> String {
        self.0.domain().to_string()
    }

    fn sample(&self, topic: &str) -> Option<Sample> {
        sample(&self.0, topic, None).ok()
    }
}

/// The payload of `topic` for a record of the shop.
pub fn payload_of(shop: &ShopState, topic: &str, id: Option<&str>) -> Result<Value, SampleError> {
    let sample = sample(shop, topic, id)?;
    sample.payload(topic).ok_or_else(|| {
        SampleError::NotBuilt(format!("lsf does not build the payload of `{topic}`"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::domains::test_support::{demo, rich};

    /// `orders/cancelled` without an id sends a cancelled order: the data's, else the first
    /// order cancelled now (issue #194, item 3).
    #[test]
    fn orders_cancelled_sends_a_cancelled_order() {
        let shop = rich();
        let cancelled: Vec<&Record> = shop
            .all("Order")
            .filter(|order| order.get("cancelledAt").is_some_and(|at| !at.is_null()))
            .collect();
        assert!(
            !cancelled.is_empty(),
            "the rich store has a cancelled order"
        );
        let first = shop.all("Order").next().unwrap();
        assert!(first.get("cancelledAt").is_none_or(Value::is_null));
        let payload = payload_of(&shop, "orders/cancelled", None).unwrap();
        assert_eq!(payload["name"], cancelled[0]["name"]);
        assert!(payload["cancelled_at"].is_string(), "{payload}");
        // Another topic still takes the first order.
        let created = payload_of(&shop, "orders/create", None).unwrap();
        assert_eq!(created["name"], first["name"]);

        let shop = demo();
        assert!(
            shop.all("Order")
                .all(|order| order.get("cancelledAt").is_none_or(Value::is_null))
        );
        let payload = payload_of(&shop, "orders/cancelled", None).unwrap();
        assert!(payload["cancelled_at"].is_string(), "{payload}");
        assert_eq!(payload["cancel_reason"], "customer");
    }
}
