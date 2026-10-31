//! `flowTriggerReceive`: "Triggers any workflows that begin with the trigger specified in the
//! request body."
//!
//! lsf runs no Flow workflow. It checks the request as Flow does, against the app's Flow
//! trigger extensions (`type = "flow_trigger"`, "Flow trigger reference", read on 2026-10-31):
//!
//! - `handle` must be one of the app's triggers;
//! - "When sending a trigger payload, all fields defined in a trigger are required": a custom
//!   field by its `key`, a reference field by its payload key (`customer_id`, …), which must be
//!   the `legacyResourceId` of a record of the shop;
//! - "The size of the payload, including keys, must be under 50000 bytes", else
//!   `Properties size exceeds the limit of 50000 bytes.` (the guide's words).
//!
//! A trigger that passes is written to the shop's outbox (`GET /__lsf/admin/outbox`, channel
//! `flow`): its handle as `to`, its payload as `data`, as Flow would have received it. The deprecated `body` (a JSON string
//! with `trigger_id`/`trigger_title` and `properties`) is read too.

use serde_json::{Map, Value, json};

use crate::graphql::{FieldContext, FieldError};
use crate::outbox::{Channel, Entry};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{ShopState, gid};

/// The setting of the shop that holds the app's Flow triggers, as the seed read them.
pub const FLOW_TRIGGERS: &str = "flowTriggers";

/// The most a trigger's payload may weigh, keys included.
pub const MAX_PAYLOAD_BYTES: usize = 50_000;

pub fn register(registry: &mut Registry) {
    registry.mutation("flowTriggerReceive", flow_trigger_receive);
}

/// The record type a reference field's id names.
fn reference_type(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "customer_reference" => "Customer",
        "order_reference" => "Order",
        "product_reference" => "Product",
        "company_reference" => "Company",
        "company_contact_reference" => "CompanyContact",
        _ => return None,
    })
}

fn triggers(state: &ShopState) -> Vec<Value> {
    state
        .setting(FLOW_TRIGGERS)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The checks of a trigger's payload against its fields.
fn check_payload(
    state: &ShopState,
    trigger: &Value,
    payload: &Map<String, Value>,
    errors: &mut Vec<UserError>,
) {
    for field in trigger
        .get("fields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let kind = field
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(key) = field.get("payloadKey").and_then(Value::as_str) else {
            continue;
        };
        let Some(given) = payload.get(key) else {
            errors.push(UserError::new(
                &["payload", key],
                format!("{key} is required: every field of the trigger is"),
            ));
            continue;
        };
        if let Some(type_name) = reference_type(kind) {
            let id = match given {
                Value::Number(number) => number.as_u64(),
                Value::String(text) => text.trim().parse().ok(),
                _ => None,
            };
            if id.is_none_or(|id| !state.contains(&gid(type_name, id))) {
                errors.push(UserError::new(
                    &["payload", key],
                    format!("{key} is not the legacyResourceId of a {type_name} of the shop"),
                ));
            }
        }
    }
}

/// `flowTriggerReceive(handle:, payload:)` (or the deprecated `body:`).
fn flow_trigger_receive(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let mut errors = Vec::new();
    let (handle, given) = match (ctx.arg_str("handle"), ctx.arg_str("body")) {
        (Some(handle), _) => (handle.trim().to_string(), ctx.arg("payload").cloned()),
        (None, Some(body)) => match serde_json::from_str::<Value>(body) {
            Ok(body) => (
                body.get("trigger_id")
                    .or_else(|| body.get("trigger_title"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                body.get("properties").cloned(),
            ),
            Err(_) => {
                errors.push(UserError::new(&["body"], "Body is not valid JSON"));
                (String::new(), None)
            }
        },
        (None, None) => {
            errors.push(UserError::new(&["handle"], "Handle can't be blank"));
            (String::new(), None)
        }
    };
    if !errors.is_empty() {
        return Ok(payload(json!({}), &errors));
    }
    let payload_object = match given {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(object)) => object,
        Some(Value::String(text)) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(object)) => object,
            _ => {
                let error = UserError::new(&["payload"], "Payload must be a JSON object");
                return Ok(payload(json!({}), &[error]));
            }
        },
        Some(_) => {
            let error = UserError::new(&["payload"], "Payload must be a JSON object");
            return Ok(payload(json!({}), &[error]));
        }
    };
    let state = m.state();
    let triggers = triggers(state);
    let Some(trigger) = triggers
        .iter()
        .find(|trigger| trigger.get("handle").and_then(Value::as_str) == Some(handle.as_str()))
    else {
        let known: Vec<&str> = triggers
            .iter()
            .filter_map(|trigger| trigger.get("handle")?.as_str())
            .collect();
        let error = UserError::new(
            &["handle"],
            if known.is_empty() {
                format!("No Flow trigger `{handle}`: the app has no flow_trigger extension")
            } else {
                format!(
                    "No Flow trigger `{handle}`: the app's are {}",
                    known.join(", ")
                )
            },
        );
        return Ok(payload(json!({}), &[error]));
    };
    let size = Value::Object(payload_object.clone()).to_string().len();
    if size >= MAX_PAYLOAD_BYTES {
        let error = UserError::new(
            &["payload"],
            format!("Properties size exceeds the limit of {MAX_PAYLOAD_BYTES} bytes."),
        );
        return Ok(payload(json!({}), &[error]));
    }
    check_payload(state, trigger, &payload_object, &mut errors);
    if !errors.is_empty() {
        return Ok(payload(json!({}), &errors));
    }
    let state = m.state_mut();
    crate::outbox::record(
        state,
        Entry::new(Channel::Flow, "flow_trigger")
            .to(handle.as_str())
            .data(Value::Object(payload_object)),
    );
    state.emit(
        super::super::app::CHANGED,
        &format!("flow_trigger/{handle}"),
    );
    m.note(format!(
        "flowTriggerReceive: lsf runs no Flow workflow; the trigger `{handle}` is in GET /__lsf/admin/outbox"
    ));
    Ok(payload(json!({}), &[]))
}
