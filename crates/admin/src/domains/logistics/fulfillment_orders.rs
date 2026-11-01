//! The fulfillment order mutations the orders family leaves: `fulfillmentOrderSplit`,
//! `fulfillmentOrderMerge`, `fulfillmentOrderReportProgress`,
//! `fulfillmentOrderLineItemsPreparedForPickup`, `fulfillmentOrdersReroute`,
//! `fulfillmentOrdersSetFulfillmentDeadline`, and `FulfillmentOrder.fulfillmentOrdersForMerge`.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - **Split** "moves the specified line items and quantities into a new fulfillment order"
//!   (`remainingFulfillmentOrder`); the original keeps the rest (`fulfillmentOrder`). A line
//!   moved whole keeps its id, as the reference's example shows; a part of one is a new line.
//!   The new fulfillment order is at the same location, so the stock the order committed stays
//!   where it is. `fulfillment_orders/split`. Codes: `FulfillmentOrderSplitUserErrorCode`.
//! - **Merge** puts the line items of each set of fulfillment orders (all their remaining units,
//!   or the quantities given) into the first of the set; a fulfillment order left with nothing
//!   is closed. The fulfillment orders of a set belong to one order and one location, with one
//!   delivery method (`fulfillmentOrdersForMerge` lists those a fulfillment order can merge
//!   with) *(lsf's message when they do not: Shopify documents no code for it)*.
//!   `fulfillment_orders/merged`.
//! - **Report progress** marks an open fulfillment order in progress (`REPORT_PROGRESS`:
//!   "marking as in progress if it's not already in progress") with the notes given:
//!   `fulfillment_orders/progress_reported`. From 2026-01, `fulfillmentOrderOpen` opens it again
//!   ("this will also mark a fulfillment order as open when it is assigned to a merchant managed
//!   location and has had progress reported"): `fulfillment_orders/
//!   manually_reported_progress_stopped` ([`stop_progress`]).
//! - **Prepared for pickup** marks the line items of pickup fulfillment orders ready, which
//!   "automatically sends a 'Ready For Pickup' notification to the customer": lsf records it in
//!   the shop's outbox, sends nothing. `fulfillment_orders/line_items_prepared_for_pickup`. A
//!   local delivery fulfillment order is prepared the same way, with
//!   `…/line_items_prepared_for_local_delivery` *(lsf's choice: 2026-10 has no other mutation
//!   for it)*.
//! - **Reroute** assigns fulfillment orders "to an alternative location, according to the shop's
//!   order routing settings": lsf's routing is Shopify's default rule, the first active location
//!   that fulfills online orders and has every unit available, within `includedLocationIds` and
//!   outside `excludedLocationIds`. The order's committed stock moves along.
//!   `fulfillment_orders/moved` and `fulfillment_orders/order_routing_complete`.
//! - **Set fulfillment deadline** sets `fulfillBy`: the event is
//!   `fulfillment_orders/fulfillment_deadline_set`, which no subscription can name (Shopify has
//!   no topic for it).

use serde_json::{Map, Value, json};

use super::{coded, emit_change, int, keys_of, objects, push, text};
use crate::domains::catalog::{self, gids};
use crate::domains::orders::model;
use crate::graphql::{FieldContext, FieldError, Output};
use crate::outbox::{self, Channel, Entry};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::{Record, ShopState};
use crate::webhooks::payload::FULFILLMENT_ORDER_EVENT;

pub fn register(registry: &mut Registry) {
    registry
        .mutation("fulfillmentOrderSplit", split)
        .mutation("fulfillmentOrderMerge", merge)
        .mutation("fulfillmentOrderReportProgress", report_progress)
        .mutation("fulfillmentOrderLineItemsPreparedForPickup", prepared)
        .mutation("fulfillmentOrdersReroute", reroute)
        .mutation("fulfillmentOrdersSetFulfillmentDeadline", set_deadline)
        .field(
            "FulfillmentOrder",
            "fulfillmentOrdersForMerge",
            orders_for_merge,
        );
}

/// The record field where a fulfillment order keeps the progress reported on it: `{reasonNotes,
/// app, initialStatus}`. While it is set, the fulfillment order stays in progress.
pub const PROGRESS: &str = "lsfProgress";

/// The statuses a fulfillment order can be split, merged or rerouted in.
const WORKABLE: &[&str] = &["OPEN", "IN_PROGRESS", "SCHEDULED", "ON_HOLD"];

fn fulfillment_order(state: &ShopState, id: &str) -> Option<Record> {
    state.get_of(id, "FulfillmentOrder").cloned()
}

/// Whether progress was reported on a fulfillment order (and not stopped since).
pub fn progress_reported(record: &Record) -> bool {
    record.get(PROGRESS).is_some_and(Value::is_object)
}

/// The actions the family's mutations add to `FulfillmentOrder.supportedActions` of a
/// merchant-managed fulfillment order: `REPORT_PROGRESS` while it is open or in progress,
/// `MERGE` when another can merge with it, `MARK_AS_OPEN` once progress was reported.
pub fn extra_actions(state: &ShopState, id: &str) -> Vec<&'static str> {
    let Some(record) = state.get(id) else {
        return Vec::new();
    };
    let mut actions = Vec::new();
    if matches!(text(record, "status"), Some("OPEN" | "IN_PROGRESS")) {
        actions.push("REPORT_PROGRESS");
    }
    if progress_reported(record) {
        actions.push("MARK_AS_OPEN");
    }
    if !mergeable_with(state, id).is_empty() {
        actions.push("MERGE");
    }
    actions
}

/// The delivery method type of a fulfillment order.
fn method_type(record: &Record) -> Option<&str> {
    record.get("deliveryMethod")?.get("methodType")?.as_str()
}

/// The fulfillment orders of the same order, location and delivery method that a fulfillment
/// order can merge with.
fn mergeable_with(state: &ShopState, id: &str) -> Vec<String> {
    let Some(record) = state.get(id) else {
        return Vec::new();
    };
    if !WORKABLE.contains(&text(record, "status").unwrap_or_default()) {
        return Vec::new();
    }
    let order = text(record, "order").unwrap_or_default();
    let location = model::assigned_location(state, id);
    keys_of(state, order, "fulfillmentOrders")
        .into_iter()
        .filter(|other| other != id)
        .filter(|other| {
            state.get(other).is_some_and(|candidate| {
                WORKABLE.contains(&text(candidate, "status").unwrap_or_default())
                    && model::assigned_location(state, other) == location
                    && method_type(candidate) == method_type(record)
                    && text(candidate, "requestStatus") == text(record, "requestStatus")
            })
        })
        .collect()
}

/// `FulfillmentOrder.fulfillmentOrdersForMerge`.
fn orders_for_merge(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let Some(id) = object.key() else {
        return r.generic(ctx, object);
    };
    let items = mergeable_with(r.state(), id)
        .into_iter()
        .map(Value::String)
        .collect();
    r.connection(ctx, items)
}

/// `(fulfillment order line item, quantity)` of `[FulfillmentOrderLineItemInput]`, checked
/// against a fulfillment order with the codes `GREATER_THAN` and `INVALID_LINE_ITEM_QUANTITY`.
fn picked_lines(
    ctx: &FieldContext<'_>,
    state: &ShopState,
    record: &Record,
    items: &[Map<String, Value>],
    path: &[String],
    errors: &mut Vec<UserError>,
) -> Vec<(String, i64)> {
    let lines = gids(record.get("lineItems"));
    let at: Vec<&str> = path.iter().map(String::as_str).collect();
    let mut picked: Vec<(String, i64)> = Vec::new();
    for item in items {
        let line = text(item, "id").unwrap_or_default();
        let quantity = item.get("quantity").and_then(Value::as_i64).unwrap_or(0);
        if quantity <= 0 {
            errors.push(coded(ctx, &at, "GREATER_THAN"));
            continue;
        }
        let already: i64 = picked
            .iter()
            .filter(|(known, _)| known == line)
            .map(|(_, units)| units)
            .sum();
        let remaining = state
            .get(line)
            .map_or(0, |line| int(line, "remainingQuantity"));
        if !lines.iter().any(|known| known == line) || already + quantity > remaining {
            errors.push(coded(ctx, &at, "INVALID_LINE_ITEM_QUANTITY"));
            continue;
        }
        match picked.iter_mut().find(|(known, _)| known == line) {
            Some((_, units)) => *units += quantity,
            None => picked.push((line.to_string(), quantity)),
        }
    }
    picked
}

/// Every remaining unit of a fulfillment order: `(line, quantity)`.
fn all_remaining(state: &ShopState, record: &Record) -> Vec<(String, i64)> {
    gids(record.get("lineItems"))
        .into_iter()
        .filter_map(|line| {
            let remaining = int(state.get(&line)?, "remainingQuantity");
            (remaining > 0).then_some((line, remaining))
        })
        .collect()
}

/// Moves units of fulfillment order line items from one fulfillment order to another: a line
/// moved whole (nothing of it fulfilled, all of it moved) goes with its id; otherwise its units
/// join the target's line of the same order line, or a new one. The stock the order committed
/// does not move: both are at one location. Returns what the target received, `(line, units)`.
fn move_lines(
    state: &mut ShopState,
    from: &str,
    to: &str,
    units: &[(String, i64)],
) -> Vec<(String, i64)> {
    let mut moved = Vec::new();
    for (line, quantity) in units {
        let Some(record) = state.get(line).cloned() else {
            continue;
        };
        let total = int(&record, "totalQuantity");
        let remaining = int(&record, "remainingQuantity");
        let order_line = text(&record, "lineItem").unwrap_or_default().to_string();
        let joined = keys_of(state, to, "lineItems").into_iter().find(|known| {
            state
                .get(known)
                .is_some_and(|other| text(other, "lineItem") == Some(order_line.as_str()))
        });
        let whole = *quantity == remaining && remaining == total;
        match (whole, joined) {
            (true, None) => {
                super::pull(state, from, "lineItems", line);
                push(state, to, "lineItems", line);
                moved.push((line.clone(), *quantity));
                continue;
            }
            (_, Some(target)) => {
                if let Some(target_record) = state.get_mut(&target) {
                    let total = int(target_record, "totalQuantity") + quantity;
                    let remaining = int(target_record, "remainingQuantity") + quantity;
                    target_record.insert("totalQuantity".into(), json!(total));
                    target_record.insert("remainingQuantity".into(), json!(remaining));
                }
                moved.push((target, *quantity));
            }
            (false, None) => {
                let mut copy = record.clone();
                let id = state.new_gid("FulfillmentOrderLineItem");
                copy.insert("id".into(), json!(id));
                copy.insert("totalQuantity".into(), json!(quantity));
                copy.insert("remainingQuantity".into(), json!(quantity));
                state.insert(copy);
                push(state, to, "lineItems", &id);
                moved.push((id, *quantity));
            }
        }
        if whole {
            super::pull(state, from, "lineItems", line);
            state.remove(line);
        } else if let Some(record) = state.get_mut(line) {
            record.insert("remainingQuantity".into(), json!(remaining - quantity));
            record.insert("totalQuantity".into(), json!((total - quantity).max(0)));
        }
    }
    moved
}

/// A new fulfillment order like `from`, without line items: the target of a split.
fn new_like(state: &mut ShopState, from: &str) -> String {
    let Some(mut record) = state.get(from).cloned() else {
        return String::new();
    };
    let now = state.timestamp();
    let id = state.new_gid("FulfillmentOrder");
    record.insert("id".into(), json!(id));
    record.insert("lineItems".into(), json!([]));
    record.insert("fulfillments".into(), json!([]));
    record.insert("fulfillmentHolds".into(), json!([]));
    record.insert("createdAt".into(), json!(now));
    record.insert("updatedAt".into(), json!(now));
    if text(&record, "status") == Some("IN_PROGRESS") && !progress_reported(&record) {
        record.insert("status".into(), json!("OPEN"));
    }
    record.remove(FULFILLMENT_ORDER_EVENT);
    record.remove(PROGRESS);
    state.insert(record);
    if let Some(order) = model::order_of(state, from) {
        model::push_key(state, &order, "fulfillmentOrders", &id);
    }
    id
}

/// What a change of fulfillment orders leaves to refresh: each one's status, and its order.
fn refreshed(state: &mut ShopState, fulfillment_orders: &[&str]) {
    let mut orders = Vec::new();
    for id in fulfillment_orders {
        model::refresh_fulfillment_order(state, id);
        state.touch(id);
        if let Some(order) = model::order_of(state, id)
            && !orders.contains(&order)
        {
            orders.push(order);
        }
    }
    for order in orders {
        model::refresh_order(state, &order);
    }
}

fn split(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"fulfillmentOrderSplits": null});
    let splits = objects(ctx.arg("fulfillmentOrderSplits"));
    let state = mutation.state();
    let mut errors = Vec::new();
    let mut plans: Vec<(String, Vec<(String, i64)>)> = Vec::new();
    for (index, input) in splits.iter().enumerate() {
        let path = |field: &str| -> Vec<String> {
            vec![
                "fulfillmentOrderSplits".into(),
                index.to_string(),
                field.into(),
            ]
        };
        let id = text(input, "fulfillmentOrderId").unwrap_or_default();
        let Some(record) = fulfillment_order(state, id) else {
            let at = path("fulfillmentOrderId");
            let at: Vec<&str> = at.iter().map(String::as_str).collect();
            errors.push(coded(ctx, &at, "FULFILLMENT_ORDER_NOT_FOUND"));
            continue;
        };
        let items = objects(input.get("fulfillmentOrderLineItems"));
        if items.is_empty() {
            let at = path("fulfillmentOrderLineItems");
            let at: Vec<&str> = at.iter().map(String::as_str).collect();
            errors.push(coded(ctx, &at, "NO_LINE_ITEMS_PROVIDED_TO_SPLIT"));
            continue;
        }
        let picked = picked_lines(
            ctx,
            state,
            &record,
            &items,
            &path("fulfillmentOrderLineItems"),
            &mut errors,
        );
        plans.push((id.to_string(), picked));
    }
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let state = mutation.state_mut();
    let mut results = Vec::new();
    for (id, picked) in plans {
        let new = new_like(state, &id);
        move_lines(state, &id, &new, &picked);
        let split_lines: Vec<Value> = picked
            .iter()
            .map(|(line, quantity)| json!({"id": line, "quantity": quantity}))
            .collect();
        if let Some(record) = state.get_mut(&new) {
            record.insert(
                FULFILLMENT_ORDER_EVENT.into(),
                json!({"original": id, "lineItems": split_lines, "replacement": null}),
            );
        }
        refreshed(state, &[&id, &new]);
        state.emit("fulfillment_orders/split", &new);
        results.push(json!({
            "fulfillmentOrder": id,
            "remainingFulfillmentOrder": new,
            "replacementFulfillmentOrder": null,
        }));
    }
    Ok(payload(json!({"fulfillmentOrderSplits": results}), &[]))
}

/// A fulfillment order of a merge, with the units `(line, quantity)` it gives.
type Member = (String, Vec<(String, i64)>);

fn merge(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"fulfillmentOrderMerges": null});
    let inputs = objects(ctx.arg("fulfillmentOrderMergeInputs"));
    let state = mutation.state();
    let mut errors = Vec::new();
    // Per set: the target, then each fulfillment order with the units it gives.
    let mut plans: Vec<(String, Vec<Member>)> = Vec::new();
    for (set, input) in inputs.iter().enumerate() {
        let intents = objects(input.get("mergeIntents"));
        let mut members: Vec<Member> = Vec::new();
        for (index, intent) in intents.iter().enumerate() {
            let path = |field: &str| -> Vec<String> {
                vec![
                    "fulfillmentOrderMergeInputs".into(),
                    set.to_string(),
                    "mergeIntents".into(),
                    index.to_string(),
                    field.into(),
                ]
            };
            let id = text(intent, "fulfillmentOrderId").unwrap_or_default();
            let Some(record) = fulfillment_order(state, id)
                .filter(|record| WORKABLE.contains(&text(record, "status").unwrap_or_default()))
            else {
                let at = path("fulfillmentOrderId");
                let at: Vec<&str> = at.iter().map(String::as_str).collect();
                errors.push(coded(ctx, &at, "FULFILLMENT_ORDER_NOT_FOUND"));
                continue;
            };
            let units = match intent.get("fulfillmentOrderLineItems") {
                Some(Value::Array(_)) => picked_lines(
                    ctx,
                    state,
                    &record,
                    &objects(intent.get("fulfillmentOrderLineItems")),
                    &path("fulfillmentOrderLineItems"),
                    &mut errors,
                ),
                _ => all_remaining(state, &record),
            };
            members.push((id.to_string(), units));
        }
        let Some((target, _)) = members.first() else {
            continue;
        };
        let target = target.clone();
        let compatible = mergeable_with(state, &target);
        if members.len() < 2
            || members
                .iter()
                .skip(1)
                .any(|(id, _)| !compatible.contains(id))
        {
            // lsf's wording: Shopify documents no code for fulfillment orders that cannot merge.
            errors.push(UserError::new(
                &[
                    "fulfillmentOrderMergeInputs",
                    &set.to_string(),
                    "mergeIntents",
                ],
                "The fulfillment orders to merge must be at least two, of the same order and \
                 location, with the same delivery method.",
            ));
            continue;
        }
        plans.push((target, members));
    }
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let state = mutation.state_mut();
    let mut results = Vec::new();
    for (target, members) in plans {
        let intents: Vec<Value> = members
            .iter()
            .map(|(id, units)| {
                let lines: Vec<Value> = units
                    .iter()
                    .map(|(line, quantity)| json!({"id": line, "quantity": quantity}))
                    .collect();
                json!({"fulfillmentOrder": id, "lineItems": lines})
            })
            .collect();
        let mut touched: Vec<String> = vec![target.clone()];
        for (id, units) in members.iter().skip(1) {
            move_lines(state, id, &target, units);
            let left: i64 = keys_of(state, id, "lineItems")
                .iter()
                .filter_map(|line| state.get(line))
                .map(|line| int(line, "remainingQuantity"))
                .sum();
            if left == 0
                && let Some(record) = state.get_mut(id)
            {
                record.insert("status".into(), json!("CLOSED"));
            }
            touched.push(id.clone());
        }
        let touched: Vec<&str> = touched.iter().map(String::as_str).collect();
        refreshed(state, &touched);
        emit_change(
            state,
            "fulfillment_orders/merged",
            &target,
            json!({"intents": intents}),
        );
        results.push(json!({"fulfillmentOrder": target}));
    }
    Ok(payload(json!({"fulfillmentOrderMerges": results}), &[]))
}

/// Whether a fulfillment order is assigned to a fulfillment service's location.
fn at_service(state: &ShopState, id: &str) -> bool {
    model::assigned_location(state, id)
        .and_then(|location| state.get(&location).cloned())
        .and_then(|location| {
            location
                .get("isFulfillmentService")
                .and_then(Value::as_bool)
        })
        .unwrap_or(false)
}

fn report_progress(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"fulfillmentOrder": null});
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let state = mutation.state();
    let Some(record) = fulfillment_order(state, &id) else {
        return Ok(payload(
            empty,
            &[coded(ctx, &["id"], "FULFILLMENT_ORDER_NOT_FOUND")],
        ));
    };
    let status = text(&record, "status").unwrap_or_default().to_string();
    if !matches!(status.as_str(), "OPEN" | "IN_PROGRESS") {
        return Ok(payload(
            empty,
            &[coded(ctx, &["id"], "FULFILLMENT_ORDER_STATUS_INVALID")],
        ));
    }
    if method_type(&record).is_some_and(|kind| matches!(kind, "PICK_UP" | "PICKUP_POINT")) {
        return Ok(payload(
            empty,
            &[coded(
                ctx,
                &["id"],
                "FULFILLMENT_ORDER_INVALID_DELIVERY_METHOD",
            )],
        ));
    }
    if at_service(state, &id) && !matches!(text(&record, "requestStatus"), Some("ACCEPTED")) {
        return Ok(payload(
            empty,
            &[coded(ctx, &["id"], "FULFILLMENT_ORDER_UNSUPPORTED_ACTION")],
        ));
    }
    let notes = ctx
        .arg("progressReport")
        .and_then(|report| report.get("reasonNotes"))
        .cloned()
        .unwrap_or(Value::Null);
    let state = mutation.state_mut();
    let app = state.app_gid().map(str::to_string);
    let initial = record
        .get(PROGRESS)
        .and_then(|progress| progress.get("initialStatus"))
        .cloned()
        .unwrap_or_else(|| json!(status));
    let progress =
        json!({"reasonNotes": notes, "app": app, "user": null, "initialStatus": initial});
    if let Some(record) = state.get_mut(&id) {
        record.insert("status".into(), json!("IN_PROGRESS"));
        record.insert(PROGRESS.into(), progress.clone());
    }
    refreshed(state, &[&id]);
    emit_change(state, "fulfillment_orders/progress_reported", &id, progress);
    Ok(payload(json!({"fulfillmentOrder": id}), &[]))
}

/// What `fulfillmentOrderOpen` does from 2026-01 to an in-progress fulfillment order with
/// progress reported, at a merchant-managed location: opens it again, with
/// `fulfillment_orders/manually_reported_progress_stopped`. `None` when it does not apply (the
/// orders family's rules then answer).
pub fn stop_progress(mutation: &mut Mutation<'_>, id: &str) -> Option<Value> {
    if mutation.api_version() < "2026-01" {
        return None;
    }
    let record = fulfillment_order(mutation.state(), id)?;
    if text(&record, "status") != Some("IN_PROGRESS")
        || !progress_reported(&record)
        || at_service(mutation.state(), id)
    {
        return None;
    }
    let state = mutation.state_mut();
    let app = state.app_gid().map(str::to_string);
    if let Some(record) = state.get_mut(id) {
        record.insert("status".into(), json!("OPEN"));
        record.remove(PROGRESS);
    }
    refreshed(state, &[id]);
    emit_change(
        state,
        "fulfillment_orders/manually_reported_progress_stopped",
        id,
        json!({"app": app, "user": null}),
    );
    Some(payload(json!({"fulfillmentOrder": id}), &[]))
}

fn prepared(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = super::input(ctx, "input");
    let groups = objects(input.get("lineItemsByFulfillmentOrder"));
    let state = mutation.state();
    let mut errors = Vec::new();
    let mut ready = Vec::new();
    for (index, group) in groups.iter().enumerate() {
        let index = index.to_string();
        let path = [
            "input",
            "lineItemsByFulfillmentOrder",
            index.as_str(),
            "fulfillmentOrderId",
        ];
        let id = text(group, "fulfillmentOrderId").unwrap_or_default();
        let Some(record) = fulfillment_order(state, id).filter(|record| {
            matches!(method_type(record), Some("PICK_UP" | "LOCAL"))
                && matches!(text(record, "status"), Some("OPEN" | "IN_PROGRESS"))
        }) else {
            errors.push(coded(ctx, &path, "FULFILLMENT_ORDER_INVALID"));
            continue;
        };
        if all_remaining(state, &record).is_empty() {
            errors.push(coded(
                ctx,
                &path,
                "NO_LINE_ITEMS_TO_PREPARE_FOR_FULFILLMENT_ORDER",
            ));
            continue;
        }
        ready.push((id.to_string(), method_type(&record) == Some("LOCAL")));
    }
    if !errors.is_empty() {
        return Ok(payload(json!({}), &errors));
    }
    let state = mutation.state_mut();
    for (id, local) in ready {
        let order = model::order_of(state, &id);
        let customer = order
            .as_deref()
            .and_then(|order| state.get(order))
            .and_then(|order| text(order, "email"))
            .map(str::to_string);
        if let Some(record) = state.get_mut(&id) {
            record.insert("lsfPrepared".into(), json!(true));
        }
        state.touch(&id);
        let (topic, notification) = if local {
            (
                "fulfillment_orders/line_items_prepared_for_local_delivery",
                "out_for_delivery",
            )
        } else {
            (
                "fulfillment_orders/line_items_prepared_for_pickup",
                "ready_for_pickup",
            )
        };
        let mut email = Entry::new(Channel::Email, notification)
            .about(&id)
            .data(json!({"order": order}));
        if let Some(customer) = customer {
            email = email.to(customer);
        }
        outbox::record(state, email);
        emit_change(state, topic, &id, json!({"preparable": true}));
    }
    if !groups.is_empty() {
        mutation.note(
            "fulfillmentOrderLineItemsPreparedForPickup: lsf sends no email: the notification \
             is in the shop's outbox (GET /__lsf/admin/outbox)",
        );
    }
    Ok(payload(json!({}), &[]))
}

/// The location lsf's routing assigns fulfillment orders to: the first active location that
/// fulfills online orders (not a fulfillment service's), other than `current`, allowed by the
/// included and excluded lists, with every unit available.
fn route(
    state: &ShopState,
    current: Option<&str>,
    units: &[(String, i64)],
    included: Option<&[String]>,
    excluded: &[String],
) -> Option<String> {
    state
        .keys("Location")
        .filter(|location| Some(*location) != current)
        .filter(|location| included.is_none_or(|included| included.iter().any(|id| id == location)))
        .filter(|location| !excluded.iter().any(|id| id == location))
        .filter(|location| {
            state.get(location).is_some_and(|record| {
                record.get("isActive").and_then(Value::as_bool) == Some(true)
                    && record.get("fulfillsOnlineOrders").and_then(Value::as_bool) != Some(false)
                    && record.get("isFulfillmentService").and_then(Value::as_bool) != Some(true)
            })
        })
        .find(|location| {
            units.iter().all(|(line, quantity)| {
                let item = state
                    .get(line)
                    .and_then(|line| text(line, "lineItem"))
                    .and_then(|line| state.get(line))
                    .and_then(|line| model::inventory_item_of(state, line));
                let Some(item) = item else {
                    return true;
                };
                let tracked = super::stock::tracked(state, &item);
                let level = catalog::level_at(state, &item, location);
                match level.and_then(|level| state.get(&level)) {
                    Some(level) => !tracked || catalog::available_of(level) >= *quantity,
                    None => false,
                }
            })
        })
        .map(str::to_string)
}

fn reroute(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"movedFulfillmentOrders": null});
    let ids = gids(ctx.arg("fulfillmentOrderIds"));
    let state = mutation.state();
    let fail = |code: &str, field: &[&str]| Ok(payload(empty.clone(), &[coded(ctx, field, code)]));
    if ids.is_empty() {
        return fail("NO_FULFILLMENT_ORDER_IDS", &["fulfillmentOrderIds"]);
    }
    let mut records = Vec::new();
    for id in &ids {
        match fulfillment_order(state, id) {
            Some(record) => records.push(record),
            None => return fail("FULFILLMENT_ORDER_NOT_FOUND", &["fulfillmentOrderIds"]),
        }
    }
    let orders: Vec<Option<&str>> = records.iter().map(|record| text(record, "order")).collect();
    if orders.iter().any(|order| *order != orders[0]) {
        return fail(
            "FULFILLMENT_ORDERS_NOT_FROM_THE_SAME_ORDER",
            &["fulfillmentOrderIds"],
        );
    }
    let current = model::assigned_location(state, &ids[0]);
    if ids
        .iter()
        .any(|id| model::assigned_location(state, id) != current)
    {
        return fail(
            "FULFILLMENT_ORDERS_MUST_BELONG_TO_SAME_LOCATION",
            &["fulfillmentOrderIds"],
        );
    }
    if records.iter().any(|record| {
        !matches!(
            text(record, "status"),
            Some("OPEN" | "IN_PROGRESS" | "ON_HOLD" | "SCHEDULED")
        ) || !matches!(text(record, "requestStatus"), Some("UNSUBMITTED") | None)
    }) {
        return fail(
            "FULFILLMENT_ORDERS_STATE_NOT_SUPPORTED",
            &["fulfillmentOrderIds"],
        );
    }
    if records.iter().any(progress_reported) {
        return fail(
            "CANNOT_MOVE_FULFILLMENT_ORDER_WITH_REPORTED_PROGRESS",
            &["fulfillmentOrderIds"],
        );
    }
    if records.iter().any(|record| {
        matches!(
            method_type(record),
            Some("PICK_UP" | "PICKUP_POINT" | "RETAIL")
        )
    }) {
        return fail(
            "DELIVERY_METHOD_TYPE_NOT_SUPPORTED",
            &["fulfillmentOrderIds"],
        );
    }
    let fulfilling = state
        .all("Location")
        .filter(|location| {
            location.get("isActive").and_then(Value::as_bool) == Some(true)
                && location
                    .get("isFulfillmentService")
                    .and_then(Value::as_bool)
                    != Some(true)
        })
        .count();
    if fulfilling < 2 {
        return fail(
            "SINGLE_LOCATION_SHOP_NOT_SUPPORTED",
            &["fulfillmentOrderIds"],
        );
    }
    let included = ctx.arg("includedLocationIds").map(|ids| gids(Some(ids)));
    let excluded = gids(ctx.arg("excludedLocationIds"));
    let units: Vec<(String, i64)> = records
        .iter()
        .flat_map(|record| all_remaining(state, record))
        .collect();
    let Some(location) = route(
        state,
        current.as_deref(),
        &units,
        included.as_deref(),
        &excluded,
    ) else {
        return fail(
            "CANNOT_REASSIGN_LOCATION_FOR_FULFILLMENT_ORDERS",
            &["fulfillmentOrderIds"],
        );
    };
    let state = mutation.state_mut();
    let assigned = model::assigned_location_value(state, &location);
    for record in &records {
        let id = text(record, "id").unwrap_or_default().to_string();
        let units = all_remaining(state, record);
        if let Some(record) = state.get_mut(&id) {
            record.insert("assignedLocation".into(), assigned.clone());
        }
        for (fo_line, quantity) in &units {
            let Some(line) = state
                .get(fo_line)
                .and_then(|item| text(item, "lineItem"))
                .map(str::to_string)
            else {
                continue;
            };
            model::move_line_commitment(state, &line, current.as_deref(), &location, *quantity);
        }
        let requested: Vec<Value> = units
            .iter()
            .map(|(line, quantity)| json!({"id": line, "quantity": quantity}))
            .collect();
        if let Some(record) = state.get_mut(&id) {
            record.insert(
                FULFILLMENT_ORDER_EVENT.into(),
                json!({"original": id, "source": current, "lineItems": requested}),
            );
        }
        refreshed(state, &[&id]);
        state.emit("fulfillment_orders/moved", &id);
        emit_change(
            state,
            "fulfillment_orders/order_routing_complete",
            &id,
            json!({}),
        );
    }
    Ok(payload(json!({"movedFulfillmentOrders": ids}), &[]))
}

fn set_deadline(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let ids = gids(ctx.arg("fulfillmentOrderIds"));
    let deadline = ctx
        .arg_str("fulfillmentDeadline")
        .and_then(crate::state::value::parse_date_time)
        .map(crate::state::value::date_time);
    let state = mutation.state();
    if ids.is_empty() || ids.iter().any(|id| fulfillment_order(state, id).is_none()) {
        return Ok(payload(
            json!({"success": false}),
            &[coded(
                ctx,
                &["fulfillmentOrderIds"],
                "FULFILLMENT_ORDERS_NOT_FOUND",
            )],
        ));
    }
    let state = mutation.state_mut();
    for id in &ids {
        if let Some(record) = state.get_mut(id) {
            record.insert("fulfillBy".into(), json!(deadline));
        }
        state.touch(id);
        state.emit("fulfillment_orders/fulfillment_deadline_set", id);
    }
    Ok(payload(json!({"success": true}), &[]))
}
