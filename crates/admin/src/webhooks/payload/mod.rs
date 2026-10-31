//! The bodies of webhooks. Shopify's webhook payloads are the REST representation of the resource,
//! not its GraphQL one: a numeric `id` with the GID in `admin_graphql_api_id`, snake_case keys,
//! prices as strings with two decimals, enums in lowercase, tags as one comma-separated string.
//! Each builder turns a record, in the shape of its GraphQL type (see "The state of a shop" in
//! `crates/admin/docs/design.md`), into that representation, in the key order of the example
//! payload of the topic in Shopify's webhook reference for API 2026-10
//! (<https://shopify.dev/docs/api/webhooks/2026-10>).
//!
//! The builders read the state through a [`Lookup`] only: the records a record refers to by GID
//! (the variants of a product, the line items of an order) and the shop. A reference may also be
//! embedded as an object, and a connection may be an array of GIDs or of objects, or
//! `{"nodes": […]}`: what a builder reads it does not need to know.
//!
//! A topic without a builder has no payload: [`payload`] answers `None`, and the delivery says it
//! is not built rather than sending a body made up for it.

mod catalog;
mod content;
mod customers;
mod orders;
mod shop;

pub use orders::{
    EDIT_ADDITIONS, EDIT_COMMITTED_AT, EDIT_NOTIFY_CUSTOMER, EDIT_REMOVALS, EDIT_STAFF_NOTE,
    FULFILLMENT_ORDER_EVENT,
};

use std::collections::HashMap;

use indexmap::IndexMap;
use serde_json::{Map, Value, json};

use crate::state::{Record, parse_gid};

/// What a payload builder reads from the state, besides the record it is given.
pub trait Lookup {
    /// The record of a GID, `None` when there is none.
    fn record(&self, gid: &str) -> Option<Record>;

    /// The `Shop` record of the shop the webhook is about: the payloads that name the shop
    /// (`shop/update`, the compliance topics, `app/scopes_update`) read its id, its domain, its
    /// currency. `None` leaves those fields `null`.
    fn shop(&self) -> Option<Record> {
        None
    }
}

impl<F> Lookup for F
where
    F: Fn(&str) -> Option<Record>,
{
    fn record(&self, gid: &str) -> Option<Record> {
        self(gid)
    }
}

/// Records in memory, by GID, and the shop: a [`Lookup`] for the tests and for the sample records
/// of `lsf app webhook trigger`.
#[derive(Clone, Debug, Default)]
pub struct Records {
    pub records: HashMap<String, Record>,
    pub shop: Option<Record>,
}

impl Records {
    /// The records of a list, each under its `id`.
    pub fn of(records: impl IntoIterator<Item = Record>, shop: Option<Record>) -> Records {
        Records {
            records: records
                .into_iter()
                .filter_map(|record| Some((record.get("id")?.as_str()?.to_string(), record)))
                .collect(),
            shop,
        }
    }
}

impl Lookup for Records {
    fn record(&self, gid: &str) -> Option<Record> {
        self.records.get(gid).cloned()
    }

    fn shop(&self) -> Option<Record> {
        self.shop.clone()
    }
}

/// The record field a `…/delete` payload reads its date of deletion from (`discounts/delete`):
/// the webhooks set it to the time of the event on the record they build the payload from.
pub const DELETED_AT: &str = "deletedAt";

/// The record field `app/scopes_update` reads the scopes the app had before from: the webhooks
/// set it from the event's snapshot, the installation as it was.
pub const PREVIOUS_ACCESS_SCOPES: &str = "previousAccessScopes";

/// The record field the builders of a topic that says what changed read the record before the
/// change from (`customer.tags_added`: the tags it did not have): the webhooks set it from the
/// event's snapshot ([`crate::state::ShopState::emit_with`]). Without it, the payload reports
/// the record as it is (the tags it has), as a webhook triggered on demand does.
pub const PREVIOUS: &str = "__previous";

/// The topics lsf builds a payload for.
pub const BUILT_TOPICS: &[&str] = &[
    "app/scopes_update",
    "app/uninstalled",
    "app_purchases_one_time/update",
    "app_subscriptions/approaching_capped_amount",
    "app_subscriptions/update",
    "bulk_operations/finish",
    "collection_publications/create",
    "collection_publications/delete",
    "collection_publications/update",
    "collections/create",
    "collections/delete",
    "collections/update",
    "companies/create",
    "companies/delete",
    "companies/update",
    "company_contact_roles/assign",
    "company_contact_roles/revoke",
    "company_contacts/create",
    "company_contacts/delete",
    "company_contacts/update",
    "company_locations/create",
    "company_locations/delete",
    "company_locations/update",
    "customer.tags_added",
    "customer.tags_removed",
    "customers/create",
    "customers/data_request",
    "customers/delete",
    "customers/disable",
    "customers/enable",
    "customers/redact",
    "customers/update",
    "customers_email_marketing_consent/update",
    "customers_marketing_consent/update",
    "discounts/create",
    "discounts/delete",
    "discounts/redeemcode_added",
    "discounts/redeemcode_removed",
    "discounts/update",
    "draft_orders/create",
    "draft_orders/delete",
    "draft_orders/update",
    "fulfillment_events/create",
    "fulfillment_holds/added",
    "fulfillment_holds/released",
    "fulfillment_orders/cancellation_request_accepted",
    "fulfillment_orders/cancellation_request_rejected",
    "fulfillment_orders/cancellation_request_submitted",
    "fulfillment_orders/cancelled",
    "fulfillment_orders/fulfillment_request_accepted",
    "fulfillment_orders/fulfillment_request_rejected",
    "fulfillment_orders/fulfillment_request_submitted",
    "fulfillment_orders/fulfillment_service_failed_to_complete",
    "fulfillment_orders/hold_released",
    "fulfillment_orders/moved",
    "fulfillment_orders/placed_on_hold",
    "fulfillment_orders/rescheduled",
    "fulfillment_orders/scheduled_fulfillment_order_ready",
    "fulfillment_orders/split",
    "fulfillments/create",
    "fulfillments/update",
    "inventory_items/create",
    "inventory_items/delete",
    "inventory_items/update",
    "inventory_levels/connect",
    "inventory_levels/disconnect",
    "inventory_levels/update",
    "locations/activate",
    "locations/create",
    "locations/deactivate",
    "locations/delete",
    "locations/update",
    "metafield_definitions/create",
    "metafield_definitions/delete",
    "metafield_definitions/update",
    "metaobjects/create",
    "metaobjects/delete",
    "metaobjects/update",
    "order_transactions/create",
    "orders/cancelled",
    "orders/create",
    "orders/delete",
    "orders/edited",
    "orders/fulfilled",
    "orders/paid",
    "orders/partially_fulfilled",
    "orders/updated",
    "product_publications/create",
    "product_publications/delete",
    "product_publications/update",
    "products/create",
    "products/delete",
    "products/update",
    "refunds/create",
    "returns/approve",
    "returns/cancel",
    "returns/close",
    "returns/decline",
    "returns/reopen",
    "returns/request",
    "selling_plan_groups/create",
    "selling_plan_groups/delete",
    "selling_plan_groups/update",
    "shop/redact",
    "shop/update",
    "variants/in_stock",
    "variants/out_of_stock",
];

/// Whether lsf builds the payload of a topic.
pub fn is_built(topic: &str) -> bool {
    BUILT_TOPICS.contains(&topic)
}

/// The payload of a webhook of `topic` about `record`, as Shopify sends it. `None` for a topic
/// lsf does not build (see [`BUILT_TOPICS`]).
///
/// For a `…/delete` topic, `record` is the record as it was before it was deleted; the payload is
/// what Shopify sends for it, often its id alone.
pub fn payload(topic: &str, record: &Record, lookup: &dyn Lookup) -> Option<Value> {
    Some(match topic {
        "products/create" | "products/update" => catalog::product(record, lookup),
        "products/delete" | "orders/delete" | "draft_orders/delete" | "locations/delete" => {
            json!({ "id": id(record) })
        }
        "collections/create" | "collections/update" => catalog::collection(record),
        "collections/delete" => catalog::collection_deleted(record),
        "inventory_items/create" | "inventory_items/update" => catalog::inventory_item(record),
        "inventory_items/delete" => catalog::inventory_item_deleted(record),
        "inventory_levels/update" | "inventory_levels/connect" => catalog::inventory_level(record),
        "inventory_levels/disconnect" => catalog::inventory_level_disconnected(record),
        "locations/create" | "locations/update" | "locations/activate" | "locations/deactivate" => {
            catalog::location(record)
        }
        "variants/in_stock" | "variants/out_of_stock" => catalog::variant_stock(record),
        "product_publications/create" | "product_publications/update" => {
            catalog::resource_publication(record, "product")
        }
        "collection_publications/create" | "collection_publications/update" => {
            catalog::resource_publication(record, "collection")
        }
        "product_publications/delete" | "collection_publications/delete" => json!({"id": null}),
        "selling_plan_groups/create" | "selling_plan_groups/update" => {
            catalog::selling_plan_group(record, lookup)
        }
        "selling_plan_groups/delete" => catalog::selling_plan_group_deleted(record),
        "customers/create" | "customers/update" | "customers/enable" | "customers/disable" => {
            customers::customer(record, lookup)
        }
        "customers/delete" => customers::deleted(record),
        "customer.tags_added" => customers::tags_changed(record, true),
        "customer.tags_removed" => customers::tags_changed(record, false),
        "customers_email_marketing_consent/update" => customers::email_consent_update(record),
        "customers_marketing_consent/update" => customers::sms_consent_update(record),
        "companies/create" | "companies/update" | "companies/delete" => customers::company(record),
        "company_locations/create" | "company_locations/update" | "company_locations/delete" => {
            customers::company_location(record, lookup)
        }
        "company_contacts/create" | "company_contacts/update" | "company_contacts/delete" => {
            customers::company_contact(record, lookup)
        }
        "company_contact_roles/assign" | "company_contact_roles/revoke" => {
            customers::role_assignment(record, lookup)
        }
        "orders/create"
        | "orders/updated"
        | "orders/paid"
        | "orders/cancelled"
        | "orders/fulfilled"
        | "orders/partially_fulfilled" => orders::order(record, lookup),
        "draft_orders/create" | "draft_orders/update" => orders::draft_order(record, lookup),
        "fulfillments/create" | "fulfillments/update" => orders::fulfillment(record, None, lookup),
        "refunds/create" => orders::refund(record, lookup),
        "order_transactions/create" => orders::order_transaction(record),
        "orders/edited" => orders::order_edit(record),
        "fulfillment_events/create" => orders::fulfillment_event(record, lookup),
        "fulfillment_holds/added" | "fulfillment_holds/released" => {
            orders::fulfillment_hold(record)
        }
        "fulfillment_orders/cancellation_request_accepted"
        | "fulfillment_orders/cancellation_request_rejected"
        | "fulfillment_orders/cancellation_request_submitted"
        | "fulfillment_orders/cancelled"
        | "fulfillment_orders/fulfillment_request_accepted"
        | "fulfillment_orders/fulfillment_request_rejected"
        | "fulfillment_orders/fulfillment_request_submitted"
        | "fulfillment_orders/fulfillment_service_failed_to_complete"
        | "fulfillment_orders/hold_released"
        | "fulfillment_orders/moved"
        | "fulfillment_orders/placed_on_hold"
        | "fulfillment_orders/rescheduled"
        | "fulfillment_orders/scheduled_fulfillment_order_ready"
        | "fulfillment_orders/split" => orders::fulfillment_order_event(topic, record, lookup),
        "returns/approve" | "returns/cancel" | "returns/close" | "returns/decline"
        | "returns/reopen" | "returns/request" => orders::return_event(topic, record),
        "app/uninstalled" | "shop/update" => shop::shop(record, lookup),
        "app/scopes_update" => shop::scopes_update(record, lookup),
        "app_subscriptions/update" => shop::app_subscription(record, lookup),
        "app_subscriptions/approaching_capped_amount" => {
            shop::approaching_capped_amount(record, lookup)
        }
        "app_purchases_one_time/update" => shop::app_purchase_one_time(record, lookup),
        "discounts/redeemcode_added" | "discounts/redeemcode_removed" => shop::redeem_code(record),
        "bulk_operations/finish" => shop::bulk_operation(record),
        "metafield_definitions/create" | "metafield_definitions/update" => {
            content::metafield_definition(record, lookup)
        }
        "metafield_definitions/delete" => content::metafield_definition_deleted(record, lookup),
        "metaobjects/create" | "metaobjects/update" => shop::metaobject(record),
        "metaobjects/delete" => shop::metaobject_deleted(record),
        "discounts/create" | "discounts/update" => shop::discount(record, lookup),
        "discounts/delete" => shop::discount_deleted(record),
        "customers/data_request" => shop::customer_data_request(record, lookup),
        "customers/redact" => shop::customer_redact(record, lookup),
        "shop/redact" => shop::shop_redact(record, lookup),
        _ => return None,
    })
}

/// The payload with only the fields of `include_fields`: `id` keeps the field, `variants.price`
/// the field `price` of each variant. The fields keep the order of the payload. An empty list
/// keeps everything.
pub fn include_fields(payload: &Value, fields: &[String]) -> Value {
    if fields.is_empty() {
        return payload.clone();
    }
    let mut tree = Tree::default();
    for field in fields {
        let mut node = &mut tree;
        let mut named = false;
        for part in field
            .split('.')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            // Under a field kept whole, there is nothing more to keep.
            if node.whole {
                break;
            }
            node = node.children.entry(part.to_string()).or_default();
            named = true;
        }
        // `variants` after `variants.price` keeps all of `variants`.
        if named {
            node.whole = true;
            node.children.clear();
        }
    }
    project(payload, &tree)
}

/// The fields `include_fields` keeps, as a tree: a node keeps all of its value (`whole`) or the
/// fields of its children.
#[derive(Default)]
struct Tree {
    whole: bool,
    children: IndexMap<String, Tree>,
}

fn project(value: &Value, tree: &Tree) -> Value {
    if tree.whole {
        return value.clone();
    }
    match value {
        Value::Object(object) => Value::Object(
            object
                .iter()
                .filter_map(|(key, value)| {
                    let child = tree.children.get(key)?;
                    Some((key.clone(), project(value, child)))
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(|item| project(item, tree)).collect()),
        // `title.x` on a string: there is nothing under it.
        _ => value.clone(),
    }
}

// --- What the builders share ---

/// A field of a record or of a nested object, `Null` when absent.
fn field<'a>(value: &'a Map<String, Value>, key: &str) -> &'a Value {
    value.get(key).unwrap_or(&Value::Null)
}

/// The first of several fields that is not null: an Admin field and the deprecated one it
/// replaced (`descriptionHtml`, `bodyHtml`).
fn first<'a>(value: &'a Map<String, Value>, keys: &[&str]) -> &'a Value {
    keys.iter()
        .map(|key| field(value, key))
        .find(|value| !value.is_null())
        .unwrap_or(&Value::Null)
}

/// A string field, as is, or `Null`.
fn text(value: &Map<String, Value>, key: &str) -> Value {
    match field(value, key) {
        Value::String(text) => Value::String(text.clone()),
        _ => Value::Null,
    }
}

/// A string field, or `fallback` when it has none.
fn text_or(value: &Map<String, Value>, key: &str, fallback: &str) -> Value {
    match field(value, key) {
        Value::String(text) => Value::String(text.clone()),
        _ => Value::String(fallback.to_string()),
    }
}

fn boolean(value: &Map<String, Value>, key: &str, fallback: bool) -> Value {
    Value::Bool(field(value, key).as_bool().unwrap_or(fallback))
}

/// An integer field (also from a string, as `UnsignedInt64` is written), or `fallback`.
fn integer(value: &Map<String, Value>, key: &str, fallback: Value) -> Value {
    match field(value, key) {
        Value::Number(number) => Value::Number(number.clone()),
        Value::String(text) => text.parse::<i64>().map_or(fallback, Value::from),
        _ => fallback,
    }
}

/// An enum value in lowercase, as REST writes it (`ACTIVE` is `active`), or `Null`.
fn lower(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(text.to_ascii_lowercase()),
        _ => Value::Null,
    }
}

/// The numeric id of a GID, `Null` when it is not one.
fn numeric(gid: &Value) -> Value {
    gid.as_str()
        .and_then(parse_gid)
        .map_or(Value::Null, |(_, id)| Value::from(id))
}

/// The numeric id of a record: from its GID, else its `legacyResourceId`.
fn id(record: &Map<String, Value>) -> Value {
    match numeric(field(record, "id")) {
        Value::Null => integer(record, "legacyResourceId", Value::Null),
        id => id,
    }
}

/// The numeric id a reference points to: a GID, or an embedded object with an `id`.
fn ref_id(reference: &Value) -> Value {
    match reference {
        Value::String(_) => numeric(reference),
        Value::Object(object) => id(object),
        _ => Value::Null,
    }
}

/// The GID a reference points to.
fn ref_gid(reference: &Value) -> Value {
    match reference {
        Value::String(gid) => Value::String(gid.clone()),
        Value::Object(object) => text(object, "id"),
        _ => Value::Null,
    }
}

/// The record a reference points to: looked up when it is a GID, the object itself when it is
/// embedded.
fn resolve(reference: &Value, lookup: &dyn Lookup) -> Option<Record> {
    match reference {
        Value::String(gid) => lookup.record(gid),
        Value::Object(object) => Some(object.clone()),
        _ => None,
    }
}

/// The items of a list or a connection, as they are: an array, or `{"nodes": […]}`, or
/// `{"edges": [{"node": …}]}`.
fn items(value: &Value) -> Vec<Value> {
    match value {
        Value::Array(items) => items.clone(),
        Value::Object(connection) => match (connection.get("nodes"), connection.get("edges")) {
            (Some(Value::Array(nodes)), _) => nodes.clone(),
            (_, Some(Value::Array(edges))) => edges
                .iter()
                .filter_map(|edge| edge.get("node").cloned())
                .collect(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// The records of a list or a connection (see [`items`]), each a GID looked up or an embedded
/// object. The GIDs that resolve to nothing are left out.
fn records(value: &Value, lookup: &dyn Lookup) -> Vec<Record> {
    items(value)
        .iter()
        .filter_map(|item| resolve(item, lookup))
        .collect()
}

/// A list of strings: an array, or `Null` as an empty one.
fn strings(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .filter(|item| item.is_string())
                .cloned()
                .collect(),
        ),
        _ => Value::Array(Vec::new()),
    }
}

/// Tags as REST writes them: one string, `"a, b"`.
fn tags(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::String(
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", "),
        ),
        Value::String(text) => Value::String(text.clone()),
        _ => Value::String(String::new()),
    }
}

/// A price as REST writes it: a string with two decimals at least (`19.0` is `"19.00"`), or
/// `Null`.
fn price(value: &Value) -> Value {
    match decimal_text(value) {
        Some(text) => Value::String(two_decimals(&text)),
        None => Value::Null,
    }
}

/// A price, `"0.00"` when there is none.
fn price_or_zero(value: &Value) -> Value {
    match price(value) {
        Value::Null => Value::String("0.00".to_string()),
        price => price,
    }
}

/// The text of a `Decimal` or `Money` value, written as a string or a number.
fn decimal_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// `19` and `19.0` are `19.00`; `0.125` stays as it is.
fn two_decimals(text: &str) -> String {
    match text.split_once('.') {
        Some((whole, fraction)) if fraction.len() >= 2 => format!("{whole}.{fraction}"),
        Some((whole, fraction)) => format!("{whole}.{fraction:0<2}"),
        None => format!("{text}.00"),
    }
}

/// The amount of a `MoneyV2`, as a price.
fn money_amount(money: &Value) -> Value {
    price(money.get("amount").unwrap_or(&Value::Null))
}

/// The shop amount of a `MoneyBag`, as a price.
fn bag_amount(bag: &Value) -> Value {
    money_amount(bag.get("shopMoney").unwrap_or(&Value::Null))
}

/// A `MoneyBag` as REST writes it: `{"shop_money": {"amount", "currency_code"},
/// "presentment_money": …}`. A `MoneyV2` alone is both. `Null` when there is none.
fn money_set(bag: &Value) -> Value {
    let money = |money: &Value| {
        json!({
            "amount": price_or_zero(money.get("amount").unwrap_or(&Value::Null)),
            "currency_code": money.get("currencyCode").cloned().unwrap_or(Value::Null),
        })
    };
    match (bag.get("shopMoney"), bag.get("presentmentMoney")) {
        (Some(shop), presentment) => json!({
            "shop_money": money(shop),
            "presentment_money": money(presentment.unwrap_or(shop)),
        }),
        (None, _) if bag.get("amount").is_some() => json!({
            "shop_money": money(bag),
            "presentment_money": money(bag),
        }),
        _ => Value::Null,
    }
}

/// A `MoneyBag` of zero in a currency: what REST gives for an amount the record does not have.
fn zero_set(currency: &Value) -> Value {
    let money = json!({"amount": "0.00", "currency_code": currency});
    json!({"shop_money": money.clone(), "presentment_money": money})
}

/// The currency of a `MoneyBag`, for the zeros of the same order.
fn bag_currency(bag: &Value) -> Value {
    bag.pointer("/shopMoney/currencyCode")
        .or_else(|| bag.get("currencyCode"))
        .cloned()
        .unwrap_or(Value::Null)
}

/// A date as REST writes it: the record's, which is RFC 3339 already.
fn time(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(text.clone()),
        _ => Value::Null,
    }
}

/// A date with milliseconds in UTC, as `variant_gids` writes `updated_at`
/// (`2022-01-01T00:00:00.000Z`).
fn millis(value: &Value) -> Value {
    value
        .as_str()
        .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
        .map_or_else(
            || value.clone(),
            |at| {
                json!(
                    at.with_timezone(&chrono::Utc)
                        .format("%Y-%m-%dT%H:%M:%S%.3fZ")
                        .to_string()
                )
            },
        )
}

/// A weight in grams, from a `Weight` (`{"unit": "KILOGRAMS", "value": 0.2}`).
fn grams(weight: &Value) -> Value {
    let Some(value) = weight.get("value").and_then(Value::as_f64) else {
        return Value::from(0);
    };
    let factor = match weight.get("unit").and_then(Value::as_str) {
        Some("KILOGRAMS") => 1000.0,
        Some("POUNDS") => 453.592_37,
        Some("OUNCES") => 28.349_523_125,
        _ => 1.0,
    };
    Value::from((value * factor).round() as i64)
}

/// A `WeightUnit` as REST abbreviates it.
fn weight_unit(unit: &Value) -> Value {
    Value::String(
        match unit.as_str() {
            Some("GRAMS") => "g",
            Some("POUNDS") => "lb",
            Some("OUNCES") => "oz",
            _ => "kg",
        }
        .to_string(),
    )
}

/// `customAttributes` (`[{key, value}]`) as REST's `properties` and `note_attributes`
/// (`[{name, value}]`).
fn attributes(value: &Value) -> Value {
    Value::Array(
        value
            .as_array()
            .into_iter()
            .flatten()
            .map(|attribute| {
                json!({
                    "name": attribute.get("key").cloned().unwrap_or(Value::Null),
                    "value": attribute.get("value").cloned().unwrap_or(Value::Null),
                })
            })
            .collect(),
    )
}

/// The fields of several objects, in order, in one.
fn merge<const N: usize>(parts: [Value; N]) -> Value {
    let mut all = Map::new();
    for part in parts {
        if let Value::Object(part) = part {
            all.extend(part);
        }
    }
    Value::Object(all)
}

/// A record as an object, for the builders that take the fields of a nested one.
fn object(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests;
