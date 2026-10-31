//! The records a webhook is built from outside of an event: `POST /__lsf/webhooks/trigger` and
//! `lsf app webhook trigger` send the payload of a record of the shop (the one named, else a
//! representative one of the topic's type), so that the backend gets the products, orders and
//! customers of the store data rather than Shopify's examples. A topic whose type the shop has
//! no record of falls back to the built-in sample of [`samples::fixed`].

use chrono::Utc;
use serde_json::{Value, json};

use crate::state::{Record, ShopState};
use crate::webhooks::payload::{self, DELETED_AT, Lookup, PREVIOUS_ACCESS_SCOPES, Records};
use crate::webhooks::samples::{self, Sample, Samples};

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
        ("customers", _)
        | ("customers_email_marketing_consent", _)
        | ("customers_marketing_consent", _)
        | ("customer.tags_added" | "customer.tags_removed", _) => &["Customer"],
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
        None => record_types(topic)
            .iter()
            .find_map(|type_name| shop.all(type_name).next().cloned()),
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
            json!(crate::state::value::date_time(Utc::now())),
        );
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
            .filter(|(key, _)| crate::state::gid_type(key).is_some())
            .map(|(_, record)| record.clone()),
        shop.shop().cloned(),
    );
    Ok(Sample { record, lookup })
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
