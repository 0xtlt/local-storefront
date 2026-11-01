//! The payloads of the logistics family (`domains::logistics`): inventory transfers and
//! shipments, the fulfillment order topics of splits, merges, progress and pickup, delivery
//! profiles (`profiles/*`) and the delivery promise settings, in the shape and key order of the
//! examples of Shopify's webhook reference for 2026-10.
//!
//! Unlike most topics, the transfer and shipment ones are not the REST representation of a
//! resource: they name records by GID (`"id": "gid://shopify/InventoryShipment/…"`), keep enums
//! in upper case, and say what the change was (`items_added`, `items_received` with the
//! quantities before and after). What the change was is the event's context, which the family
//! emits as the event's snapshot ([`crate::domains::logistics::emit_change`]: the time of the
//! change, `happenedAt`, and [`LOGISTICS_EVENT`]), read here from [`PREVIOUS`]. A webhook
//! triggered on demand has no context: its lists are empty and `happened_at` is the record's
//! date.

use serde_json::{Map, Value, json};

use super::{
    DELETED_AT, Lookup, PREVIOUS, field, id, items, lower, numeric, object, ref_gid, resolve, text,
    time,
};

/// The key of the event context in an event's snapshot: what a change was about (the items it
/// added, the quantities it changed).
pub const LOGISTICS_EVENT: &str = "lsfLogistics";

/// The context of the event a record's payload is built for.
fn context(record: &Map<String, Value>) -> Map<String, Value> {
    object(field(&object(field(record, PREVIOUS)), LOGISTICS_EVENT))
}

/// When the change happened: the event's time, else (a webhook triggered on demand) the
/// deletion's, else the record's own date.
fn happened_at(record: &Map<String, Value>, fallback: &str) -> Value {
    let previous = object(field(record, PREVIOUS));
    [
        field(&previous, "happenedAt"),
        field(record, DELETED_AT),
        field(record, fallback),
    ]
    .into_iter()
    .find(|value| !value.is_null())
    .map_or(Value::Null, time)
}

/// The items of the context under `items`, as the context gives them.
fn context_items(record: &Map<String, Value>) -> Value {
    Value::Array(items(field(&context(record), "items")))
}

/// `{"id": <location GID>}` of a transfer's snapshot of a location, `Null` without one.
fn location(snapshot: &Value) -> Value {
    reference(snapshot.get("location").unwrap_or(&Value::Null))
}

/// `inventory_transfers/…`: an `InventoryTransfer`.
pub(super) fn transfer(topic: &str, record: &Map<String, Value>) -> Value {
    let mut payload = Map::new();
    payload.insert("id".into(), text(record, "id"));
    payload.insert("happened_at".into(), happened_at(record, "dateCreated"));
    payload.insert("origin".into(), location(field(record, "origin")));
    payload.insert("destination".into(), location(field(record, "destination")));
    match topic {
        "inventory_transfers/add_items" => {
            payload.insert("items_added".into(), context_items(record));
        }
        "inventory_transfers/remove_items" => {
            payload.insert("items_removed".into(), context_items(record));
        }
        "inventory_transfers/update_item_quantities" => {
            payload.insert("status".into(), text(record, "status"));
            payload.insert("items_updated".into(), context_items(record));
        }
        "inventory_transfers/ready_to_ship" => {
            payload.insert("status".into(), text(record, "status"));
            let lines: Vec<Value> = items(field(&context(record), "items"))
                .iter()
                .map(|item| json!({"id": item.get("line_item_id"), "quantity": item.get("quantity")}))
                .collect();
            payload.insert("line_items".into(), Value::Array(lines));
        }
        "inventory_transfers/cancel" | "inventory_transfers/complete" => {
            payload.insert("status".into(), text(record, "status"));
        }
        // `updated`.
        _ => {}
    }
    Value::Object(payload)
}

/// The tracking of a shipment as the shipment topics write it, in the order of `topic`'s
/// example; `Null` without one.
fn tracking(record: &Map<String, Value>, update: bool) -> Value {
    let Value::Object(tracking) = field(record, "tracking") else {
        return Value::Null;
    };
    let arrives = time(field(tracking, "arrivesAt"));
    let (number, url, company) = (
        text(tracking, "trackingNumber"),
        text(tracking, "trackingUrl"),
        text(tracking, "company"),
    );
    if update {
        json!({"tracking_number": number, "tracking_url": url, "company": company, "arrives_at": arrives})
    } else {
        json!({"arrives_at": arrives, "tracking_number": number, "tracking_url": url, "company": company})
    }
}

/// `inventory_shipments/…`: an `InventoryShipment`, its transfer kept on it by lsf.
pub(super) fn shipment(topic: &str, record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let transfer = ref_gid(field(record, "lsfTransfer"));
    let at = happened_at(record, "dateCreated");
    if topic == "inventory_shipments/update_tracking" {
        return json!({
            "shipment_id": text(record, "id"),
            "inventory_transfer_id": transfer,
            "updated_tracking": tracking(record, true),
            "happened_at": at,
        });
    }
    let mut payload = Map::new();
    payload.insert("id".into(), text(record, "id"));
    payload.insert("inventory_transfer_id".into(), transfer);
    match topic {
        "inventory_shipments/create" => {
            payload.insert("status".into(), text(record, "status"));
            payload.insert("happened_at".into(), at);
            payload.insert("tracking".into(), tracking(record, false));
            let lines: Vec<Value> = items(field(record, "lineItems"))
                .iter()
                .filter_map(|line| resolve(line, lookup))
                .map(|line| json!({"id": text(&line, "id"), "quantity": field(&line, "quantity")}))
                .collect();
            payload.insert("line_items".into(), Value::Array(lines));
            return Value::Object(payload);
        }
        "inventory_shipments/mark_in_transit" => {
            payload.insert("status".into(), text(record, "status"));
        }
        "inventory_shipments/receive_items" => {
            payload.insert("status".into(), text(record, "status"));
            payload.insert("items_received".into(), context_items(record));
        }
        "inventory_shipments/add_items" => {
            payload.insert("items_added".into(), context_items(record));
        }
        "inventory_shipments/remove_items" => {
            payload.insert("items_removed".into(), context_items(record));
        }
        "inventory_shipments/update_item_quantities" => {
            payload.insert("items_updated".into(), context_items(record));
        }
        // `delete`.
        _ => {}
    }
    payload.insert("happened_at".into(), at);
    Value::Object(payload)
}

/// A fulfillment order as the topics write it: its GID and its status in lower case.
fn fulfillment_order_head(record: &Map<String, Value>) -> Map<String, Value> {
    let mut head = Map::new();
    head.insert("id".into(), text(record, "id"));
    head.insert("status".into(), lower(field(record, "status")));
    head
}

/// `{"id": <GID>}` of a reference, `Null` without one.
fn reference(value: &Value) -> Value {
    match ref_gid(value) {
        Value::Null => Value::Null,
        gid => json!({"id": gid}),
    }
}

/// The `fulfillment_orders/…` topics of the family: a `FulfillmentOrder`, with the context of
/// the change.
pub(super) fn fulfillment_order(topic: &str, record: &Map<String, Value>) -> Value {
    let context = context(record);
    let head = fulfillment_order_head(record);
    match topic {
        "fulfillment_orders/line_items_prepared_for_pickup"
        | "fulfillment_orders/line_items_prepared_for_local_delivery" => {
            let mut order = head;
            order.insert(
                "preparable".into(),
                Value::Bool(field(&context, "preparable").as_bool().unwrap_or(true)),
            );
            // REST writes `PICK_UP` as one word, as the example does (`pickup`).
            let method = match field(record, "deliveryMethod").get("methodType") {
                Some(Value::String(kind)) if kind == "PICK_UP" => json!("pickup"),
                Some(kind) => lower(kind),
                None => Value::Null,
            };
            order.insert("delivery_method".into(), json!({"method_type": method}));
            json!({"fulfillment_order": order})
        }
        "fulfillment_orders/progress_reported" => json!({
            "fulfillment_order": head,
            "progress_report": {
                "reason_notes": text(&context, "reasonNotes"),
                "progress_reported_by_app": reference(field(&context, "app")),
                "progress_reported_by_user": reference(field(&context, "user")),
            },
            "initial_status": lower(field(&context, "initialStatus")),
        }),
        "fulfillment_orders/manually_reported_progress_stopped" => json!({
            "fulfillment_order": head,
            "progress_stopped_by_app": reference(field(&context, "app")),
            "progress_stopped_by_user": reference(field(&context, "user")),
        }),
        "fulfillment_orders/merged" => {
            let intents: Vec<Value> = items(field(&context, "intents"))
                .iter()
                .map(|intent| {
                    let lines: Vec<Value> = items(intent.get("lineItems").unwrap_or(&Value::Null))
                        .iter()
                        .map(|line| {
                            json!({
                                "id": numeric(line.get("id").unwrap_or(&Value::Null)),
                                "quantity": line.get("quantity"),
                            })
                        })
                        .collect();
                    json!({
                        "fulfillment_order_id": numeric(intent.get("fulfillmentOrder").unwrap_or(&Value::Null)),
                        "fulfillment_order_line_items": lines,
                    })
                })
                .collect();
            json!({
                "merge_intents": intents,
                "fulfillment_order_merges": {"fulfillment_order": head},
            })
        }
        // `order_routing_complete`.
        _ => json!({"fulfillment_order": head}),
    }
}

/// `profiles/…`: a `DeliveryProfile`.
pub(super) fn delivery_profile(record: &Map<String, Value>) -> Value {
    json!({
        "id": id(record),
        "admin_graphql_api_id": text(record, "id"),
        "name": text(record, "name"),
        "default": field(record, "default").as_bool().unwrap_or(false),
        // REST's `profile_type` has no field in GraphQL; Shopify's example has `null`.
        "profile_type": Value::Null,
        "version": field(record, "version").as_i64().unwrap_or(1),
    })
}

/// `delivery_promise_settings/update`: the shop's `DeliveryPromiseSetting`, with the shop's
/// GID.
pub(super) fn delivery_promise_settings(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let shop = lookup.shop().map_or(Value::Null, |shop| text(&shop, "id"));
    json!({
        "shop_id": shop,
        "processing_time": text(record, "processingTime"),
        "delivery_dates_enabled": field(record, "deliveryDatesEnabled").as_bool().unwrap_or(false),
    })
}
