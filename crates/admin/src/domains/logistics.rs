//! Logistics: inventory transfers and their shipments, the fulfillment order mutations the
//! orders family leaves (split, merge, reroute, progress reports, pickup, deadlines), fulfillment
//! constraint rules, shipping labels, carrier services, delivery profiles, delivery promises,
//! the delivery settings and the shop's shipping packages.
//!
//! One module per area, each with its mutations, its custom fields and the shopify.dev sources
//! of what it does:
//!
//! | Module | What |
//! |---|---|
//! | `transfers` | `inventoryTransfer…`: drafts, ready to ship (stock committed at the origin), edits, cancellation |
//! | `shipments` | `inventoryShipment…`: draft, in transit (stock out of the origin, `incoming` at the destination), received |
//! | `stock` | The stock moves of both, through the orders family's `model::move_inventory` |
//! | `fulfillment_orders` | `fulfillmentOrderSplit`, `Merge`, `ReportProgress`, `LineItemsPreparedForPickup`, `fulfillmentOrdersReroute`, `SetFulfillmentDeadline` |
//! | `constraint_rules` | `fulfillmentConstraintRule…` over the app's Functions |
//! | `labels` | `shippingLabelPurchase`: a label "purchased" at once, in the shop's outbox |
//! | `carriers` | `carrierService…` and `availableCarrierServices` |
//! | `profiles` | `deliveryProfile…`, the general profile the seed makes from `checkout.shipping_rates`, the delivery profile of variants |
//! | `delivery` | delivery promise providers and participants, `deliveryPromiseSettings`, `deliverySettingUpdate`, `deliveryShippingOriginAssign`, the shipping packages |
//!
//! What lsf "sends" or "buys" for the outside world (a pickup notification, a shipping label)
//! is recorded in the shop's outbox ([`crate::outbox`], `GET /__lsf/admin/outbox`): nothing
//! leaves the machine.

pub mod carriers;
pub mod constraint_rules;
pub mod delivery;
pub mod fulfillment_orders;
pub mod labels;
pub mod profiles;
pub mod shipments;
pub mod stock;
pub mod transfers;

#[cfg(test)]
mod tests;

use serde_json::{Map, Value, json};

use crate::domains::catalog::{self, gids};
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError};
use crate::state::seed::Seed;
pub use crate::state::value::str_field as text;
use crate::state::{Record, ShopState};

/// The custom resolvers and the mutations of the family: see "Writing a resolver or a mutation"
/// in `crates/admin/docs/design.md`.
pub fn register(registry: &mut Registry) {
    transfers::register(registry);
    shipments::register(registry);
    fulfillment_orders::register(registry);
    constraint_rules::register(registry);
    labels::register(registry);
    carriers::register(registry);
    profiles::register(registry);
    delivery::register(registry);
}

/// The records of the family the store data gives: the general delivery profile and the shop's
/// shipping packages.
pub fn seed(seed: &mut Seed) {
    profiles::seed(seed);
    delivery::seed(seed);
}

/// A user error with the code `code`, whose message is the description the schema gives the
/// code in the code enum of the payload's error type (`InventoryTransferCreateUserErrorCode`
/// for `inventoryTransferCreate`): shopify.dev documents these errors by their codes only, and
/// Shopify words the messages it does publish as these descriptions (see
/// `catalog::records::coded`). A description of several sentences gives its first. The schema
/// is the request's, so the message is that of the version asked.
pub fn coded(ctx: &FieldContext<'_>, field: &[&str], code: &str) -> UserError {
    let schema = ctx.schema();
    let message = schema
        .field(ctx.ty().named(), "userErrors")
        .map(|errors| errors.ty.named().to_string())
        .and_then(|error| schema.field(&error, "code").map(|code| code.ty.named()))
        .and_then(|codes| schema.get(codes))
        .and_then(|codes| codes.enum_values.get(code))
        .and_then(|value| value.description.as_deref())
        .map(first_sentence)
        .unwrap_or_else(|| crate::state::value::humanize(&code.to_ascii_lowercase()));
    UserError::new(field, message).code(code)
}

/// The first sentence of a description: up to the first `. ` (or the end).
fn first_sentence(text: &str) -> String {
    let text = text.split("\n\n").next().unwrap_or(text).trim();
    let text = text.replace('\n', " ");
    match text.find(". ") {
        Some(end) => text[..=end].to_string(),
        None => text,
    }
}

/// Runs a mutation that takes an `@idempotent(key:)` directive: optional as of 2026-01,
/// required as of 2026-04 (the "Caution" of each such mutation's reference). With a key, or on a
/// version that requires one, the catalog's idempotency rules apply
/// ([`catalog::with_idempotency`]: the same key and arguments answer the first answer again, a
/// key reused with other arguments is `IDEMPOTENCY_KEY_PARAMETER_MISMATCH`); without one on an
/// older version, the mutation simply runs.
pub fn idempotent(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    empty: Value,
    body: fn(&mut Mutation<'_>, &FieldContext<'_>) -> Result<Value, FieldError>,
) -> Result<Value, FieldError> {
    let keyed = ctx
        .directive("idempotent")
        .and_then(|directive| directive.get("key").cloned())
        .is_some();
    if keyed || requires_idempotency(mutation.api_version()) {
        catalog::with_idempotency(mutation, ctx, empty, "userErrors", body)
    } else {
        body(mutation, ctx)
    }
}

/// Whether a version requires the `@idempotent` key of the mutations that take one: 2026-04 and
/// later (`unstable` included, which sorts after the dated versions).
fn requires_idempotency(version: &str) -> bool {
    version >= "2026-04"
}

/// The `LocationSnapshot` of a location: its name and address as they are now, a link to it,
/// and when they were recorded. `Null` without a location.
pub fn location_snapshot(state: &ShopState, location: Option<&str>) -> Value {
    let Some((location, record)) =
        location.and_then(|location| Some((location, state.get(location)?)))
    else {
        return Value::Null;
    };
    json!({
        "name": record.get("name").cloned().unwrap_or_else(|| json!("")),
        "address": record.get("address").cloned().unwrap_or_else(|| json!({})),
        "location": location,
        "snapshottedAt": state.timestamp(),
    })
}

/// The live location a snapshot links to.
pub fn snapshot_location(snapshot: Option<&Value>) -> Option<String> {
    snapshot?.get("location")?.as_str().map(str::to_string)
}

/// Whether a text is an `http(s)` URL with a host: a carrier's callback, a tracking link.
pub fn is_url(text: &str) -> bool {
    ["http://", "https://"].iter().any(|scheme| {
        text.strip_prefix(scheme)
            .is_some_and(|rest| !rest.is_empty() && !rest.starts_with('/'))
    })
}

/// An integer field of a record, 0 when absent.
pub fn int(record: &Record, field: &str) -> i64 {
    record.get(field).and_then(Value::as_i64).unwrap_or(0)
}

/// The object an argument holds (an empty one when it holds none).
pub fn input(ctx: &FieldContext<'_>, name: &str) -> Map<String, Value> {
    ctx.arg(name)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// The objects of a list (an input field or an argument), `[]` when it is none.
pub fn objects(value: Option<&Value>) -> Vec<Map<String, Value>> {
    match value {
        Some(Value::Array(items)) => items.iter().filter_map(Value::as_object).cloned().collect(),
        Some(Value::Object(item)) => vec![item.clone()],
        _ => Vec::new(),
    }
}

/// Appends a key to an array field of a record (made when missing).
pub fn push(state: &mut ShopState, record: &str, field: &str, key: &str) {
    if let Some(record) = state.get_mut(record) {
        let list = record.entry(field).or_insert_with(|| json!([]));
        if !list.is_array() {
            *list = json!([]);
        }
        if let Value::Array(items) = list
            && !items.iter().any(|item| item.as_str() == Some(key))
        {
            items.push(json!(key));
        }
    }
}

/// Removes a key from an array field of a record.
pub fn pull(state: &mut ShopState, record: &str, field: &str, key: &str) {
    if let Some(Value::Array(items)) = state
        .get_mut(record)
        .and_then(|record| record.get_mut(field))
    {
        items.retain(|item| item.as_str() != Some(key));
    }
}

/// The keys of an array field of a record.
pub fn keys_of(state: &ShopState, record: &str, field: &str) -> Vec<String> {
    state
        .get(record)
        .map(|record| gids(record.get(field)))
        .unwrap_or_default()
}

/// The event context a payload of the family reads (see `webhooks::payload::logistics`): the
/// time of the change and what it was about, kept as the event's snapshot so that two changes in
/// one request each keep theirs.
pub fn emit_change(state: &mut ShopState, topic: &str, gid: &str, context: Value) {
    let mut snapshot = Map::new();
    snapshot.insert("happenedAt".into(), json!(state.timestamp()));
    snapshot.insert(crate::webhooks::payload::LOGISTICS_EVENT.into(), context);
    state.emit_with(topic, gid, snapshot);
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn idempotency_is_required_from_2026_04() {
        assert!(!requires_idempotency("2026-01"));
        assert!(requires_idempotency("2026-04"));
        assert!(requires_idempotency("2026-10"));
        assert!(requires_idempotency("unstable"));
    }

    #[test]
    fn a_description_gives_its_first_sentence() {
        assert_eq!(
            first_sentence("The item was not found."),
            "The item was not found."
        );
        assert_eq!(
            first_sentence("Cannot remove it. The error name refers to\nsomething else."),
            "Cannot remove it."
        );
    }
}
