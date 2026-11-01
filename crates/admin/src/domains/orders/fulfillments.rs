//! Fulfillments and fulfillment orders: `fulfillmentCreate`, `fulfillmentTrackingInfoUpdate`,
//! `fulfillmentCancel`, `fulfillmentEventCreate`, and `fulfillmentOrderHold`,
//! `fulfillmentOrderReleaseHold`, `fulfillmentOrderMove`, `fulfillmentOrderCancel`,
//! `fulfillmentOrderOpen`, `fulfillmentOrderReschedule`.
//!
//! As shopify.dev's reference of 2026-10 describes them (`mutations/<name>`, the fulfillment
//! service tutorial), with the topics of `WebhookSubscriptionTopic`:
//!
//! - `fulfillmentCreate` ships the fulfillment order line items given (all that remain of a
//!   fulfillment order given without them), from fulfillment orders of one order assigned to one
//!   location. A fulfillment order whose items all shipped is `CLOSED`, one partly shipped
//!   `IN_PROGRESS`. The shipped units leave the stock the order committed. Events:
//!   `fulfillments/create`, `orders/fulfilled` or `orders/partially_fulfilled`, `orders/updated`.
//!   A UPS tracking number without a URL gets UPS's URL, the one of Shopify's example payload;
//!   lsf builds no other carrier's.
//! - `fulfillmentTrackingInfoUpdate` replaces the tracking: `fulfillments/update`.
//! - `fulfillmentCancel` cancels a fulfillment: its units are to fulfill again, on their
//!   fulfillment order when it is still open, else on a new one at the same location ("Creates
//!   new FOs for the cancelled items"): `fulfillments/update`, `orders/updated`. lsf does not put
//!   the stock back.
//! - `fulfillmentEventCreate` records a tracking event, which sets the fulfillment's display
//!   status (and `inTransitAt`, `deliveredAt`, `estimatedDeliveryAt`):
//!   `fulfillment_events/create`, `fulfillments/update`.
//! - `fulfillmentOrderHold` puts a hold on a fulfillment order (up to 10 per app, a `handle`
//!   unique among them); a hold of some line items splits the others off into a
//!   `remainingFulfillmentOrder`. `fulfillment_holds/added`, and `fulfillment_orders/
//!   placed_on_hold` when the fulfillment order goes on hold. `fulfillmentOrderReleaseHold`
//!   releases the holds given (all of the app's by default): `fulfillment_holds/released` for
//!   each, `fulfillment_orders/hold_released` when none is left. The messages are the
//!   descriptions of `FulfillmentOrderHoldUserErrorCode`.
//! - `fulfillmentOrderMove` moves a fulfillment order to another location: in place when
//!   nothing of it shipped and all of it moves, else to a new fulfillment order (the original
//!   keeps the rest, or closes): `fulfillment_orders/moved`. Committed stock moves along.
//! - `fulfillmentOrderCancel` (a fulfillment request submitted to a fulfillment service) closes
//!   the fulfillment order and opens a replacement: `fulfillment_orders/cancelled`.
//! - `fulfillmentOrderOpen` opens a scheduled fulfillment order
//!   (`fulfillment_orders/scheduled_fulfillment_order_ready`); `fulfillmentOrderReschedule`
//!   changes when it is due (`fulfillment_orders/rescheduled`).
//!
//! Messages: "Invalid fulfillment order line item quantity requested." and "Fulfillment order
//! <id> has an unfulfillable status= <status>." are quoted from community.shopify.com (they
//! answered `fulfillmentCreateV2`); "Fulfillment order is not in cancelable request state and
//! can’t be canceled." and "Expected fulfillment order status to be scheduled but it was
//! <status>." too. The others are lsf's wording.

use serde_json::{Value, json};

use super::model::{self, array, field_cents, int, text};
use crate::domains::catalog::gids;
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{ShopState, value};
use crate::webhooks::payload::FULFILLMENT_ORDER_EVENT;

pub fn register(registry: &mut Registry) {
    registry
        .mutation("fulfillmentCreate", fulfillment_create)
        .mutation("fulfillmentTrackingInfoUpdate", tracking_info_update)
        .mutation("fulfillmentCancel", fulfillment_cancel)
        .mutation("fulfillmentEventCreate", event_create)
        .mutation("fulfillmentOrderHold", hold)
        .mutation("fulfillmentOrderReleaseHold", release_hold)
        .mutation("fulfillmentOrderMove", move_fulfillment_order)
        .mutation("fulfillmentOrderCancel", cancel_fulfillment_order)
        .mutation("fulfillmentOrderOpen", open_fulfillment_order)
        .mutation("fulfillmentOrderReschedule", reschedule);
}

/// The most holds an app may have on a fulfillment order.
const HOLD_LIMIT: usize = 10;

/// The tracking of a `FulfillmentTrackingInput`: `(company, number, url)` items.
fn tracking_of(input: &Value) -> Vec<Value> {
    let company = input.get("company").and_then(Value::as_str);
    let mut numbers: Vec<String> = input
        .get("numbers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    if let Some(number) = input.get("number").and_then(Value::as_str)
        && !numbers.iter().any(|known| known == number)
    {
        numbers.insert(0, number.to_string());
    }
    let mut urls: Vec<String> = input
        .get("urls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    if let Some(url) = input.get("url").and_then(Value::as_str)
        && !urls.iter().any(|known| known == url)
    {
        urls.insert(0, url.to_string());
    }
    let count = numbers.len().max(urls.len());
    (0..count)
        .map(|index| {
            let number = numbers.get(index).cloned();
            let url = urls
                .get(index)
                .cloned()
                .or_else(|| carrier_url(company, number.as_deref()));
            json!({"company": company, "number": number, "url": url})
        })
        .collect()
}

/// The tracking URL Shopify builds for a carrier it knows: UPS's, as Shopify's example payload
/// of `fulfillments/create` writes it.
fn carrier_url(company: Option<&str>, number: Option<&str>) -> Option<String> {
    let number = number?;
    match company?.to_ascii_lowercase().as_str() {
        "ups" => Some(format!(
            "https://www.ups.com/WebTracking?loc=en_US&requester=ST&trackNums={number}"
        )),
        _ => None,
    }
}

/// A fulfillment order of the shop, with its record.
fn fulfillment_order(state: &ShopState, id: &str) -> Option<serde_json::Map<String, Value>> {
    state.get_of(id, "FulfillmentOrder").cloned()
}

/// `fulfillmentCreate`.
fn fulfillment_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let input = model::input_of(ctx.arg("fulfillment"));
    let groups = array(&input, "lineItemsByFulfillmentOrder");
    let failed = |errors: Vec<UserError>| Ok(payload(json!({"fulfillment": null}), &errors));
    let state = mutation.state();
    if groups.is_empty() {
        // lsf's wording.
        return failed(vec![UserError::new(
            &["fulfillment", "lineItemsByFulfillmentOrder"],
            "Line items by fulfillment order can't be blank",
        )]);
    }
    // What ships: (fulfillment order, its line item, quantity).
    let mut shipping: Vec<(String, String, i64)> = Vec::new();
    let mut order: Option<String> = None;
    let mut location: Option<Option<String>> = None;
    for group in &groups {
        let id = group
            .get("fulfillmentOrderId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(record) = fulfillment_order(state, id) else {
            return failed(vec![UserError::new(
                &["fulfillment"],
                format!("Fulfillment order {} does not exist.", model::number(id)),
            )]);
        };
        let status = text(&record, "status").unwrap_or("OPEN");
        if !matches!(status, "OPEN" | "IN_PROGRESS") {
            return failed(vec![UserError::new(
                &["fulfillment"],
                format!(
                    "Fulfillment order {} has an unfulfillable status= {}.",
                    model::number(id),
                    status.to_ascii_lowercase()
                ),
            )]);
        }
        let its_order = text(&record, "order").map(str::to_string);
        let its_location = model::assigned_location(state, id);
        if order
            .as_ref()
            .is_some_and(|order| Some(order) != its_order.as_ref())
            || location
                .as_ref()
                .is_some_and(|location| *location != its_location)
        {
            // The rule is shopify.dev's ("associated with the same Order and are assigned to
            // the same Location"); the message is lsf's wording.
            return failed(vec![UserError::new(
                &["fulfillment"],
                "All fulfillment orders must belong to the same order and be assigned to the same location.",
            )]);
        }
        order = its_order;
        location = Some(its_location);
        let lines = gids(record.get("lineItems"));
        match group
            .get("fulfillmentOrderLineItems")
            .and_then(Value::as_array)
        {
            Some(items) if !items.is_empty() => {
                for item in items {
                    let line = item.get("id").and_then(Value::as_str).unwrap_or_default();
                    let quantity = item.get("quantity").and_then(Value::as_i64).unwrap_or(0);
                    let remaining = state
                        .get(line)
                        .map_or(0, |line| int(line, "remainingQuantity"));
                    let already: i64 = shipping
                        .iter()
                        .filter(|(_, known, _)| known == line)
                        .map(|(_, _, quantity)| quantity)
                        .sum();
                    if !lines.iter().any(|known| known == line) {
                        // lsf's wording.
                        return failed(vec![UserError::new(
                            &["fulfillment"],
                            format!(
                                "Fulfillment order line item {} does not belong to fulfillment order {}.",
                                model::number(line),
                                model::number(id)
                            ),
                        )]);
                    }
                    if quantity <= 0 || quantity + already > remaining {
                        return failed(vec![UserError::new(
                            &["fulfillment"],
                            "Invalid fulfillment order line item quantity requested.",
                        )]);
                    }
                    shipping.push((id.to_string(), line.to_string(), quantity));
                }
            }
            _ => {
                for line in lines {
                    let remaining = state
                        .get(&line)
                        .map_or(0, |line| int(line, "remainingQuantity"));
                    if remaining > 0 {
                        shipping.push((id.to_string(), line, remaining));
                    }
                }
            }
        }
    }
    let Some(order) = order.filter(|order| state.type_of(order) == Some("Order")) else {
        return failed(vec![UserError::new(
            &["fulfillment"],
            "Order does not exist",
        )]);
    };
    if shipping.is_empty() {
        return failed(vec![UserError::new(
            &["fulfillment"],
            "Invalid fulfillment order line item quantity requested.",
        )]);
    }
    if input.get("notifyCustomer").and_then(Value::as_bool) == Some(true) {
        mutation.note("fulfillmentCreate: lsf sends no email: `notifyCustomer` was ignored");
    }
    let location = location.flatten();
    let tracking = input
        .get("trackingInfo")
        .map(tracking_of)
        .unwrap_or_default();
    let origin = input.get("originAddress").cloned().unwrap_or(Value::Null);

    let state = mutation.state_mut();
    let now = state.timestamp();
    let currency = model::currency(state);
    let id = state.new_gid("Fulfillment");
    let count = state
        .get(&order)
        .map_or(0, |record| gids(record.get("fulfillments")).len());
    let order_name = state
        .get(&order)
        .and_then(|record| text(record, "name"))
        .unwrap_or_default()
        .to_string();
    let mut fulfillment_lines = Vec::new();
    let mut fulfillment_orders: Vec<String> = Vec::new();
    let mut total = 0;
    let mut requires_shipping = false;
    for (fulfillment_order, fo_line, quantity) in &shipping {
        let Some(line) = state
            .get(fo_line)
            .and_then(|record| text(record, "lineItem"))
            .map(str::to_string)
        else {
            continue;
        };
        if let Some(record) = state.get_mut(fo_line) {
            let remaining = int(record, "remainingQuantity");
            record.insert("remainingQuantity".into(), json!(remaining - quantity));
        }
        let item = state.get(&line).cloned().unwrap_or_default();
        let ordered = int(&item, "quantity").max(1);
        requires_shipping |= item.get("requiresShipping").and_then(Value::as_bool) == Some(true);
        let line_id = state.new_gid("FulfillmentLineItem");
        state.insert(value::object(json!({
            "__typename": "FulfillmentLineItem",
            "id": line_id,
            "lineItem": line,
            "quantity": quantity,
            "originalTotalSet": value::money_bag(field_cents(&item, "originalTotalSet") * quantity / ordered, &currency),
            "discountedTotalSet": value::money_bag(field_cents(&item, "discountedTotalSet") * quantity / ordered, &currency),
            "createdAt": now,
        })));
        fulfillment_lines.push(line_id);
        total += quantity;
        model::ship_line(state, &line, location.as_deref(), *quantity);
        if !fulfillment_orders.contains(fulfillment_order) {
            fulfillment_orders.push(fulfillment_order.clone());
        }
    }
    state.insert(value::object(json!({
        "__typename": "Fulfillment",
        "id": id,
        "name": format!("{order_name}-F{}", count + 1),
        "order": order,
        "status": "SUCCESS",
        "displayStatus": "FULFILLED",
        "location": location,
        "originAddress": origin,
        "fulfillmentLineItems": fulfillment_lines,
        "fulfillmentOrders": fulfillment_orders,
        "totalQuantity": total,
        "requiresShipping": requires_shipping,
        "trackingInfo": tracking,
        "service": null,
        "events": [],
        "deliveredAt": null,
        "inTransitAt": null,
        "estimatedDeliveryAt": null,
        "createdAt": now,
        "updatedAt": now,
    })));
    for fulfillment_order in &fulfillment_orders {
        model::push_key(state, fulfillment_order, "fulfillments", &id);
        model::refresh_fulfillment_order(state, fulfillment_order);
        state.touch(fulfillment_order);
    }
    model::push_key(state, &order, "fulfillments", &id);
    model::refresh_order(state, &order);
    state.emit("fulfillments/create", &id);
    emit_fulfillment_status(state, &order);
    model::order_updated(state, &order);
    Ok(payload(json!({"fulfillment": id}), &[]))
}

/// `orders/fulfilled` or `orders/partially_fulfilled`, by the order's fulfillment status.
fn emit_fulfillment_status(state: &mut ShopState, order: &str) {
    match state
        .get(order)
        .and_then(|record| text(record, "displayFulfillmentStatus"))
    {
        Some("FULFILLED") => {
            state.emit("orders/fulfilled", order);
        }
        Some("PARTIALLY_FULFILLED") => {
            state.emit("orders/partially_fulfilled", order);
        }
        _ => {}
    }
}

/// A fulfillment of the shop, or lsf's error about it.
fn find_fulfillment(state: &ShopState, id: &str, field: &str) -> Result<String, UserError> {
    if state.type_of(id) == Some("Fulfillment") {
        Ok(id.to_string())
    } else {
        Err(UserError::new(&[field], "Fulfillment does not exist"))
    }
}

/// `fulfillmentTrackingInfoUpdate`.
fn tracking_info_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("fulfillmentId").unwrap_or_default();
    let fulfillment = match find_fulfillment(mutation.state(), id, "fulfillmentId") {
        Ok(fulfillment) => fulfillment,
        Err(error) => return Ok(payload(json!({"fulfillment": null}), &[error])),
    };
    if mutation
        .state()
        .get(&fulfillment)
        .and_then(|record| text(record, "status"))
        == Some("CANCELLED")
    {
        let error = UserError::new(
            &["fulfillmentId"],
            "The tracking of a cancelled fulfillment cannot be updated",
        );
        return Ok(payload(json!({"fulfillment": null}), &[error]));
    }
    let tracking = ctx
        .arg("trackingInfoInput")
        .map(tracking_of)
        .unwrap_or_default();
    if ctx.arg_bool("notifyCustomer") == Some(true) {
        mutation.note(
            "fulfillmentTrackingInfoUpdate: lsf sends no email: `notifyCustomer` was ignored",
        );
    }
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&fulfillment) {
        record.insert("trackingInfo".into(), json!(tracking));
    }
    state.touch(&fulfillment);
    state.emit("fulfillments/update", &fulfillment);
    Ok(payload(json!({"fulfillment": fulfillment}), &[]))
}

/// `fulfillmentCancel`.
fn fulfillment_cancel(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default();
    let fulfillment = match find_fulfillment(mutation.state(), id, "id") {
        Ok(fulfillment) => fulfillment,
        Err(error) => return Ok(payload(json!({"fulfillment": null}), &[error])),
    };
    let record = mutation
        .state()
        .get(&fulfillment)
        .cloned()
        .unwrap_or_default();
    if text(&record, "status") != Some("SUCCESS") {
        // lsf's wording.
        let error = UserError::new(&["id"], "Fulfillment cannot be cancelled");
        return Ok(payload(json!({"fulfillment": null}), &[error]));
    }
    let Some(order) = model::order_of(mutation.state(), &fulfillment) else {
        return Ok(payload(
            json!({"fulfillment": null}),
            &[UserError::new(&["id"], "Order does not exist")],
        ));
    };
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&fulfillment) {
        record.insert("status".into(), json!("CANCELLED"));
        record.insert("displayStatus".into(), json!("CANCELED"));
    }
    state.touch(&fulfillment);
    let fulfillment_orders = gids(record.get("fulfillmentOrders"));
    for line in gids(record.get("fulfillmentLineItems")) {
        let Some(item) = state.get(&line).cloned() else {
            continue;
        };
        let (Some(line_item), quantity) = (text(&item, "lineItem"), int(&item, "quantity")) else {
            continue;
        };
        // The fulfillment order line item it shipped from, on a fulfillment order still open
        // when there is one.
        let candidates: Vec<(String, String)> =
            model::fulfillment_order_lines(state, &order, line_item)
                .into_iter()
                .filter(|(fo, _)| fulfillment_orders.contains(fo))
                .collect();
        let open = candidates.iter().find(|(fo, _)| {
            matches!(
                state.get(fo).and_then(|record| text(record, "status")),
                Some("OPEN" | "IN_PROGRESS" | "ON_HOLD")
            )
        });
        match open.or(candidates.first()) {
            Some((fo, fo_line))
                if matches!(
                    state.get(fo).and_then(|record| text(record, "status")),
                    Some("OPEN" | "IN_PROGRESS" | "ON_HOLD")
                ) =>
            {
                let (fo, fo_line) = (fo.clone(), fo_line.clone());
                if let Some(record) = state.get_mut(&fo_line) {
                    let remaining = int(record, "remainingQuantity");
                    record.insert("remainingQuantity".into(), json!(remaining + quantity));
                }
                model::refresh_fulfillment_order(state, &fo);
                state.touch(&fo);
            }
            Some((fo, fo_line)) => {
                let (fo, fo_line) = (fo.clone(), fo_line.clone());
                let reopened = split_off(state, &fo, &[(fo_line, quantity)], None, false);
                model::push_key(state, &order, "fulfillmentOrders", &reopened);
            }
            None => {}
        }
    }
    model::refresh_order(state, &order);
    state.emit("fulfillments/update", &fulfillment);
    model::order_updated(state, &order);
    Ok(payload(json!({"fulfillment": fulfillment}), &[]))
}

/// Makes a fulfillment order for some units of another's line items (`(line item of `from`,
/// quantity)`), at `location` or `from`'s: what a split, a partial move, a hold of some items or
/// a cancelled fulfillment of a closed fulfillment order makes. `take` takes the units off
/// `from`'s line items. Returns the new fulfillment order.
pub fn split_off(
    state: &mut ShopState,
    from: &str,
    units: &[(String, i64)],
    location: Option<&str>,
    take: bool,
) -> String {
    let Some(original) = state.get(from).cloned() else {
        return String::new();
    };
    let now = state.timestamp();
    let id = state.new_gid("FulfillmentOrder");
    let mut lines = Vec::new();
    for (line, quantity) in units {
        let Some(mut item) = state.get(line).cloned() else {
            continue;
        };
        if take && let Some(record) = state.get_mut(line) {
            let remaining = int(record, "remainingQuantity");
            let total = int(record, "totalQuantity");
            record.insert(
                "remainingQuantity".into(),
                json!((remaining - quantity).max(0)),
            );
            record.insert("totalQuantity".into(), json!((total - quantity).max(0)));
        }
        let line_id = state.new_gid("FulfillmentOrderLineItem");
        item.insert("id".into(), json!(line_id));
        item.insert("totalQuantity".into(), json!(quantity));
        item.insert("remainingQuantity".into(), json!(quantity));
        state.insert(item);
        lines.push(line_id);
    }
    let mut record = original.clone();
    record.insert("id".into(), json!(id));
    record.insert("status".into(), json!("OPEN"));
    record.insert("requestStatus".into(), json!("UNSUBMITTED"));
    record.insert("lineItems".into(), json!(lines));
    record.insert("fulfillments".into(), json!([]));
    record.insert("fulfillmentHolds".into(), json!([]));
    record.remove(FULFILLMENT_ORDER_EVENT);
    record.insert("createdAt".into(), json!(now));
    record.insert("updatedAt".into(), json!(now));
    if let Some(location) = location {
        record.insert(
            "assignedLocation".into(),
            model::assigned_location_value(state, location),
        );
    }
    state.insert(record);
    if take {
        model::refresh_fulfillment_order(state, from);
        state.touch(from);
    }
    id
}

/// `fulfillmentEventCreate`.
fn event_create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = model::input_of(ctx.arg("fulfillmentEvent"));
    let id = input
        .get("fulfillmentId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let fulfillment = match find_fulfillment(mutation.state(), id, "fulfillmentId") {
        Ok(fulfillment) => fulfillment,
        Err(error) => {
            let error = UserError {
                field: Some(vec!["fulfillmentEvent".into(), "fulfillmentId".into()]),
                ..error
            };
            return Ok(payload(json!({"fulfillmentEvent": null}), &[error]));
        }
    };
    let status = input
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("CONFIRMED")
        .to_string();
    let state = mutation.state_mut();
    let now = state.timestamp();
    let happened = input
        .get("happenedAt")
        .and_then(Value::as_str)
        .map_or_else(|| now.clone(), str::to_string);
    let event = state.new_gid("FulfillmentEvent");
    let field = |name: &str| input.get(name).cloned().unwrap_or(Value::Null);
    state.insert(value::object(json!({
        "__typename": "FulfillmentEvent",
        "id": event,
        "status": status,
        "message": field("message"),
        "happenedAt": happened,
        "createdAt": now,
        "estimatedDeliveryAt": field("estimatedDeliveryAt"),
        "address1": field("address1"),
        "city": field("city"),
        "province": field("province"),
        "country": field("country"),
        "zip": field("zip"),
        "latitude": field("latitude"),
        "longitude": field("longitude"),
        "fulfillment": fulfillment,
    })));
    model::push_key(state, &fulfillment, "events", &event);
    if let Some(record) = state.get_mut(&fulfillment) {
        record.insert("displayStatus".into(), json!(status));
        match status.as_str() {
            "IN_TRANSIT" if record.get("inTransitAt").is_none_or(Value::is_null) => {
                record.insert("inTransitAt".into(), json!(happened));
            }
            "DELIVERED" => {
                record.insert("deliveredAt".into(), json!(happened));
            }
            _ => {}
        }
        if let Some(estimated) = input.get("estimatedDeliveryAt").filter(|at| !at.is_null()) {
            record.insert("estimatedDeliveryAt".into(), estimated.clone());
        }
    }
    state.touch(&fulfillment);
    state.emit("fulfillment_events/create", &event);
    state.emit("fulfillments/update", &fulfillment);
    Ok(payload(json!({"fulfillmentEvent": event}), &[]))
}

/// A `FulfillmentOrder…UserError` with its code.
fn coded(field: &[&str], message: &str, code: &str) -> UserError {
    UserError::new(field, message).code(code)
}

/// What the admin shows for a hold reason *(chosen: the reason in words)*.
fn display_reason(reason: &str, notes: Option<&str>) -> String {
    match reason {
        "OTHER" => notes
            .filter(|notes| !notes.is_empty())
            .unwrap_or("Other")
            .to_string(),
        other => {
            let words = other.to_ascii_lowercase().replace('_', " ");
            let mut chars = words.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + chars.as_str()
            })
        }
    }
}

/// `(fulfillment order line item, quantity)` of a `[FulfillmentOrderLineItemInput]`, checked
/// against the fulfillment order with the codes of `FulfillmentOrderHoldUserErrorCode`.
fn line_items_of(
    state: &ShopState,
    record: &serde_json::Map<String, Value>,
    items: &[Value],
    path: &[&str],
    errors: &mut Vec<UserError>,
) -> Vec<(String, i64)> {
    let lines = gids(record.get("lineItems"));
    let mut picked: Vec<(String, i64)> = Vec::new();
    for item in items {
        let line = item.get("id").and_then(Value::as_str).unwrap_or_default();
        let quantity = item.get("quantity").and_then(Value::as_i64).unwrap_or(0);
        if picked.iter().any(|(known, _)| known == line) {
            errors.push(coded(
                path,
                "The fulfillment order line items are not unique.",
                "DUPLICATED_FULFILLMENT_ORDER_LINE_ITEMS",
            ));
            continue;
        }
        if quantity <= 0 {
            errors.push(coded(
                path,
                "The fulfillment order line item quantity must be greater than 0.",
                "GREATER_THAN_ZERO",
            ));
            continue;
        }
        let remaining = state
            .get(line)
            .map_or(0, |line| int(line, "remainingQuantity"));
        if !lines.iter().any(|known| known == line) || quantity > remaining {
            errors.push(coded(
                path,
                "The fulfillment order line item quantity is invalid.",
                "INVALID_LINE_ITEM_QUANTITY",
            ));
            continue;
        }
        picked.push((line.to_string(), quantity));
    }
    picked
}

/// The units of a fulfillment order that are not in `picked`: what a partial hold or move
/// leaves, everything that remains when nothing is picked.
pub(super) fn the_rest(
    state: &ShopState,
    record: &serde_json::Map<String, Value>,
    picked: &[(String, i64)],
) -> Vec<(String, i64)> {
    gids(record.get("lineItems"))
        .into_iter()
        .filter_map(|line| {
            let remaining = state
                .get(&line)
                .map_or(0, |item| int(item, "remainingQuantity"));
            let taken: i64 = picked
                .iter()
                .filter(|(known, _)| *known == line)
                .map(|(_, quantity)| quantity)
                .sum();
            let left = remaining - taken;
            (left > 0).then_some((line, left))
        })
        .collect()
}

/// The app's GID, which holds what it puts on hold.
fn app_gid(state: &ShopState) -> Option<String> {
    state.keys("App").next().map(str::to_string)
}

/// `fulfillmentOrderHold`.
fn hold(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let input = model::input_of(ctx.arg("fulfillmentHold"));
    let failed = |errors: &[UserError]| {
        Ok(payload(
            json!({"fulfillmentHold": null, "fulfillmentOrder": null, "remainingFulfillmentOrder": null}),
            errors,
        ))
    };
    let state = mutation.state();
    let Some(record) = fulfillment_order(state, &id) else {
        return failed(&[coded(
            &["id"],
            "The fulfillment order could not be found.",
            "FULFILLMENT_ORDER_NOT_FOUND",
        )]);
    };
    if !matches!(
        text(&record, "status"),
        Some("OPEN" | "IN_PROGRESS" | "ON_HOLD" | "SCHEDULED")
    ) {
        // lsf's wording.
        return failed(&[UserError::new(
            &["id"],
            "The fulfillment order is not in a state that can be held.",
        )]);
    }
    let app = app_gid(state);
    let holds: Vec<serde_json::Map<String, Value>> = gids(record.get("fulfillmentHolds"))
        .iter()
        .filter_map(|hold| state.get(hold).cloned())
        .filter(|hold| text(hold, "heldByApp") == app.as_deref())
        .collect();
    let handle = input
        .get("handle")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut errors = Vec::new();
    if holds.len() >= HOLD_LIMIT {
        errors.push(coded(
            &["id"],
            "The maximum number of fulfillment holds for this fulfillment order has been reached for this app. An app can only have up to 10 holds on a single fulfillment order at any one time.",
            "FULFILLMENT_ORDER_HOLD_LIMIT_REACHED",
        ));
    }
    if holds
        .iter()
        .any(|hold| text(hold, "handle").unwrap_or_default() == handle)
    {
        errors.push(coded(
            &["fulfillmentHold", "handle"],
            "The handle provided for the fulfillment hold is already in use by this app for another hold on this fulfillment order.",
            "DUPLICATE_FULFILLMENT_HOLD_HANDLE",
        ));
    }
    let items = array(&input, "fulfillmentOrderLineItems");
    let picked = line_items_of(
        state,
        &record,
        &items,
        &["fulfillmentHold", "fulfillmentOrderLineItems"],
        &mut errors,
    );
    let rest = if items.is_empty() {
        Vec::new()
    } else {
        the_rest(state, &record, &picked)
    };
    if !errors.is_empty() {
        return failed(&errors);
    }
    if input.get("notifyMerchant").and_then(Value::as_bool) == Some(true) {
        mutation.note("fulfillmentOrderHold: lsf sends no email: `notifyMerchant` was ignored");
    }
    let reason = input
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("OTHER")
        .to_string();
    let notes = input
        .get("reasonNotes")
        .and_then(Value::as_str)
        .map(str::to_string);
    let state = mutation.state_mut();
    let remaining = (!rest.is_empty()).then(|| {
        let order = text(&record, "order").unwrap_or_default().to_string();
        let new = split_off(state, &id, &rest, None, true);
        model::push_key(state, &order, "fulfillmentOrders", &new);
        new
    });
    let hold = state.new_gid("FulfillmentHold");
    state.insert(value::object(json!({
        "__typename": "FulfillmentHold",
        "id": hold,
        "reason": reason,
        "reasonNotes": notes,
        "displayReason": display_reason(&reason, notes.as_deref()),
        "handle": if handle.is_empty() { Value::Null } else { json!(handle) },
        "heldByApp": app,
        "heldByRequestingApp": true,
        "fulfillmentOrder": id,
    })));
    model::push_key(state, &id, "fulfillmentHolds", &hold);
    let was_on_hold = text(&record, "status") == Some("ON_HOLD");
    let held: Vec<Value> = if picked.is_empty() {
        the_rest(state, &record, &[])
    } else {
        picked.clone()
    }
    .iter()
    .map(|(line, quantity)| json!({"id": line, "quantity": quantity}))
    .collect();
    if let Some(record) = state.get_mut(&id) {
        record.insert("status".into(), json!("ON_HOLD"));
        record.insert(
            FULFILLMENT_ORDER_EVENT.into(),
            json!({"hold": hold, "remaining": remaining, "lineItems": held}),
        );
    }
    if let Some(remaining) = &remaining {
        let split: Vec<Value> = super::line_items(state, remaining)
            .iter()
            .map(|line| {
                let quantity = state.get(line).map_or(0, |item| int(item, "totalQuantity"));
                json!({"id": line, "quantity": quantity})
            })
            .collect();
        if let Some(record) = state.get_mut(remaining) {
            record.insert(
                FULFILLMENT_ORDER_EVENT.into(),
                json!({"original": id, "lineItems": split, "replacement": null}),
            );
        }
    }
    state.touch(&id);
    if let Some(order) = model::order_of(state, &id) {
        model::refresh_order(state, &order);
    }
    state.emit("fulfillment_holds/added", &hold);
    if !was_on_hold {
        state.emit("fulfillment_orders/placed_on_hold", &id);
    }
    if let Some(remaining) = &remaining {
        state.emit("fulfillment_orders/split", remaining);
    }
    Ok(payload(
        json!({"fulfillmentHold": hold, "fulfillmentOrder": id, "remainingFulfillmentOrder": remaining}),
        &[],
    ))
}

/// `fulfillmentOrderReleaseHold`.
fn release_hold(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let failed = |errors: &[UserError]| Ok(payload(json!({"fulfillmentOrder": null}), errors));
    let state = mutation.state();
    let Some(record) = fulfillment_order(state, &id) else {
        return failed(&[coded(
            &["id"],
            "The fulfillment order could not be found.",
            "FULFILLMENT_ORDER_NOT_FOUND",
        )]);
    };
    let app = app_gid(state);
    let holds = gids(record.get("fulfillmentHolds"));
    let asked: Option<Vec<String>> = ctx.arg("holdIds").map(|ids| gids(Some(ids)));
    let external = ctx.arg_str("externalId");
    if let Some(asked) = &asked
        && let Some(unknown) = asked.iter().find(|hold| !holds.contains(hold))
    {
        // lsf's wording.
        return failed(&[UserError::new(
            &["holdIds"],
            format!(
                "Fulfillment hold {} does not exist on this fulfillment order.",
                model::number(unknown)
            ),
        )]);
    }
    let releasing: Vec<String> = holds
        .iter()
        .filter(|hold| match &asked {
            Some(asked) => asked.contains(hold),
            None => state
                .get(hold)
                .is_some_and(|record| text(record, "heldByApp") == app.as_deref()),
        })
        .cloned()
        .collect();
    if text(&record, "status") != Some("ON_HOLD") || releasing.is_empty() {
        // lsf's wording.
        return failed(&[UserError::new(
            &["id"],
            "The fulfillment order is not on hold.",
        )]);
    }
    if external.is_some() {
        mutation.note(
            "fulfillmentOrderReleaseHold: lsf does not simulate `externalId`: it was ignored",
        );
    }
    let state = mutation.state_mut();
    let left: Vec<String> = holds
        .iter()
        .filter(|hold| !releasing.contains(hold))
        .cloned()
        .collect();
    if let Some(record) = state.get_mut(&id) {
        record.insert("fulfillmentHolds".into(), json!(left));
        if left.is_empty() {
            record.insert("status".into(), json!("OPEN"));
        }
    }
    model::refresh_fulfillment_order(state, &id);
    state.touch(&id);
    if let Some(order) = model::order_of(state, &id) {
        model::refresh_order(state, &order);
    }
    for hold in &releasing {
        state.emit("fulfillment_holds/released", hold);
    }
    if left.is_empty() {
        state.emit("fulfillment_orders/hold_released", &id);
    }
    Ok(payload(json!({"fulfillmentOrder": id}), &[]))
}

/// `fulfillmentOrderMove`.
fn move_fulfillment_order(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let location = ctx.arg_str("newLocationId").unwrap_or_default().to_string();
    let failed = |errors: &[UserError]| {
        Ok(payload(
            json!({"movedFulfillmentOrder": null, "originalFulfillmentOrder": null, "remainingFulfillmentOrder": null}),
            errors,
        ))
    };
    let state = mutation.state();
    let Some(record) = fulfillment_order(state, &id) else {
        return failed(&[UserError::new(&["id"], "Fulfillment order does not exist.")]);
    };
    if state.type_of(&location) != Some("Location") {
        return failed(&[UserError::new(
            &["newLocationId"],
            "Location does not exist.",
        )]);
    }
    if model::assigned_location(state, &id).as_deref() == Some(location.as_str()) {
        return failed(&[UserError::new(
            &["newLocationId"],
            "The fulfillment order is already assigned to this location.",
        )]);
    }
    if matches!(
        text(&record, "status"),
        Some("CLOSED" | "CANCELLED" | "INCOMPLETE")
    ) {
        return failed(&[UserError::new(
            &["id"],
            "Cannot move a closed fulfillment order.",
        )]);
    }
    if matches!(
        text(&record, "requestStatus"),
        Some("SUBMITTED" | "ACCEPTED" | "CANCELLATION_REQUESTED" | "CANCELLATION_REJECTED")
    ) {
        return failed(&[UserError::new(
            &["id"],
            "Cannot move a fulfillment order whose fulfillment was requested.",
        )]);
    }
    let mut errors = Vec::new();
    let items = ctx
        .arg("fulfillmentOrderLineItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let picked = if items.is_empty() {
        the_rest(state, &record, &[])
    } else {
        line_items_of(
            state,
            &record,
            &items,
            &["fulfillmentOrderLineItems"],
            &mut errors,
        )
    };
    if !errors.is_empty() {
        return failed(&errors);
    }
    let shipped = !gids(record.get("fulfillments")).is_empty();
    let whole = the_rest(state, &record, &picked).is_empty();
    let from = model::assigned_location(state, &id);
    let order = text(&record, "order").unwrap_or_default().to_string();
    let state = mutation.state_mut();
    let moved = if whole && !shipped {
        let assigned = model::assigned_location_value(state, &location);
        if let Some(record) = state.get_mut(&id) {
            record.insert("assignedLocation".into(), assigned);
        }
        state.touch(&id);
        id.clone()
    } else {
        let new = split_off(state, &id, &picked, Some(&location), true);
        model::push_key(state, &order, "fulfillmentOrders", &new);
        new
    };
    // The stock the order committed moves along.
    for (fo_line, quantity) in &picked {
        let Some(line) = state
            .get(fo_line)
            .and_then(|item| text(item, "lineItem"))
            .map(str::to_string)
        else {
            continue;
        };
        model::move_line_commitment(state, &line, from.as_deref(), &location, *quantity);
    }
    let requested: Vec<Value> = picked
        .iter()
        .map(|(line, quantity)| json!({"id": line, "quantity": quantity}))
        .collect();
    if let Some(record) = state.get_mut(&moved) {
        record.insert(
            FULFILLMENT_ORDER_EVENT.into(),
            json!({"original": id, "source": from, "lineItems": requested}),
        );
    }
    model::refresh_order(state, &order);
    state.emit("fulfillment_orders/moved", &moved);
    Ok(payload(
        json!({"movedFulfillmentOrder": moved, "originalFulfillmentOrder": id, "remainingFulfillmentOrder": null}),
        &[],
    ))
}

/// `fulfillmentOrderCancel`: a fulfillment request submitted to a fulfillment service.
fn cancel_fulfillment_order(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let failed = |errors: &[UserError]| {
        Ok(payload(
            json!({"fulfillmentOrder": null, "replacementFulfillmentOrder": null}),
            errors,
        ))
    };
    let state = mutation.state();
    let Some(record) = fulfillment_order(state, &id) else {
        return failed(&[UserError::new(&["id"], "Fulfillment order does not exist.")]);
    };
    if !matches!(
        text(&record, "requestStatus"),
        Some("SUBMITTED" | "CANCELLATION_REQUESTED")
    ) || matches!(text(&record, "status"), Some("CLOSED" | "CANCELLED"))
    {
        return failed(&[UserError::new(
            &[],
            "Fulfillment order is not in cancelable request state and can’t be canceled.",
        )]);
    }
    let rest = the_rest(state, &record, &[]);
    let order = text(&record, "order").unwrap_or_default().to_string();
    let state = mutation.state_mut();
    let replacement = split_off(state, &id, &rest, None, false);
    model::push_key(state, &order, "fulfillmentOrders", &replacement);
    if let Some(record) = state.get_mut(&id) {
        record.insert("status".into(), json!("CLOSED"));
        record.insert("requestStatus".into(), json!("CANCELLATION_ACCEPTED"));
        record.insert(
            FULFILLMENT_ORDER_EVENT.into(),
            json!({"replacement": replacement}),
        );
    }
    if let Some(lines) = state.get(&id).map(|record| gids(record.get("lineItems"))) {
        for line in lines {
            if let Some(item) = state.get_mut(&line) {
                item.insert("remainingQuantity".into(), json!(0));
            }
        }
    }
    state.touch(&id);
    model::refresh_order(state, &order);
    state.emit("fulfillment_orders/cancelled", &id);
    Ok(payload(
        json!({"fulfillmentOrder": id, "replacementFulfillmentOrder": replacement}),
        &[],
    ))
}

/// `fulfillmentOrderOpen`.
fn open_fulfillment_order(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let failed = |error: UserError| Ok(payload(json!({"fulfillmentOrder": null}), &[error]));
    let Some(record) = fulfillment_order(mutation.state(), &id) else {
        return failed(UserError::new(&["id"], "Fulfillment order does not exist."));
    };
    // From 2026-01, an in-progress one with progress reported opens again (logistics family).
    if let Some(answer) =
        crate::domains::logistics::fulfillment_orders::stop_progress(mutation, &id)
    {
        return Ok(answer);
    }
    let status = text(&record, "status").unwrap_or("OPEN");
    if status != "SCHEDULED" {
        return failed(UserError::new(
            &["id"],
            format!(
                "Expected fulfillment order status to be scheduled but it was {}.",
                status.to_ascii_lowercase()
            ),
        ));
    }
    let now = mutation.timestamp();
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&id) {
        record.insert("status".into(), json!("OPEN"));
        record.insert("fulfillAt".into(), json!(now));
    }
    model::refresh_fulfillment_order(state, &id);
    state.touch(&id);
    if let Some(order) = model::order_of(state, &id) {
        model::refresh_order(state, &order);
    }
    state.emit("fulfillment_orders/scheduled_fulfillment_order_ready", &id);
    Ok(payload(json!({"fulfillmentOrder": id}), &[]))
}

/// `fulfillmentOrderReschedule`.
fn reschedule(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let failed = |error: UserError| Ok(payload(json!({"fulfillmentOrder": null}), &[error]));
    let Some(record) = fulfillment_order(mutation.state(), &id) else {
        return failed(coded(
            &["id"],
            "The fulfillment order could not be found.",
            "FULFILLMENT_ORDER_NOT_FOUND",
        ));
    };
    if text(&record, "status") != Some("SCHEDULED") {
        // lsf's wording.
        return failed(UserError::new(
            &["id"],
            "Only a scheduled fulfillment order can be rescheduled.",
        ));
    }
    let fulfill_at = ctx.arg_str("fulfillAt").unwrap_or_default().to_string();
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&id) {
        record.insert("fulfillAt".into(), json!(fulfill_at));
    }
    state.touch(&id);
    state.emit("fulfillment_orders/rescheduled", &id);
    Ok(payload(json!({"fulfillmentOrder": id}), &[]))
}
