//! Requested order edits: a buyer asks to remove units of an order that did not ship
//! (`requestedOrderEditCreate`, made on the buyer's behalf), and the merchant resolves or declines
//! the request (`requestedOrderEditResolve`, `requestedOrderEditDecline`).
//!
//! As shopify.dev's reference of 2026-10 describes them (`objects/RequestedOrderEdit`, the
//! webhook examples of `requested_order_edit/…`):
//!
//! - A request lists its removals: line items of the order and how many units, at most what is
//!   left to fulfill of each ("a buyer-initiated request to edit unfulfilled line items").
//! - Resolving marks it `RESOLVED` (`requestResolvedAt`); each removal's `resolvedQuantity` is
//!   what the merchant took off the line since the request (an order edit, a refund), at most
//!   what was asked: resolving does not edit the order itself. Declining marks it `DECLINED`
//!   (`requestDeclinedAt`), and the buyer's notice, with the note, goes to the shop's outbox.
//! - `Order.displayRequestedEditStatus`: `REQUESTED` while one is pending, else the status of the
//!   latest, `NONE` without one. `Order.requestedOrderEdits` lists them.
//! - `requestedOrderEditCalculate` answers what removing the units would take off the order:
//!   each line's subtotal (after the order's discounts) and tax, the order's discount they bore,
//!   the total.
//! - Events: `requested_order_edit/created`, `…/declined`, `…/resolved`.
//!
//! Messages: lsf's wording, with the codes of the payloads' error types.

use serde_json::{Value, json};

use super::model::{self, int, text};
use crate::domains::catalog::gids;
use crate::graphql::{FieldContext, FieldError, Output};
use crate::outbox::{self, Entry};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::resolve::{Obj, ShopResolver};
use crate::state::{ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("requestedOrderEditCreate", create)
        .mutation("requestedOrderEditDecline", |m, ctx| {
            close(m, ctx, "DECLINED")
        })
        .mutation("requestedOrderEditResolve", |m, ctx| {
            close(m, ctx, "RESOLVED")
        })
        .query("requestedOrderEditCalculate", calculate)
        .field("Order", "displayRequestedEditStatus", |r, _ctx, object| {
            let state = r.state();
            let statuses: Vec<String> = r
                .map_of(object)
                .map(|order| gids(order.get("requestedOrderEdits")))
                .unwrap_or_default()
                .iter()
                .filter_map(|id| state.get(id).and_then(|edit| text(edit, "status")))
                .map(str::to_string)
                .collect();
            let status = if statuses.iter().any(|status| status == "REQUESTED") {
                "REQUESTED"
            } else {
                statuses.last().map_or("NONE", String::as_str)
            };
            Ok(Output::value(status))
        });
}

/// The units of a line item at the time of the request, kept on each removal: what
/// `resolvedQuantity` counts from.
const AT_REQUEST: &str = "lsfQuantityAtRequest";

/// The note a declined request was declined with.
const DECLINE_NOTE: &str = "lsfDeclineNote";

/// `requestedOrderEditCreate`.
fn create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let failed = |field: &[&str], message: &str, code: &str| {
        Ok(payload(
            json!({"requestedOrderEdit": null}),
            &[UserError::new(field, message).code(code)],
        ))
    };
    let input = model::input_of(ctx.arg("input"));
    let order = input
        .get("orderId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let state = mutation.state();
    let Some(record) = state.get_of(&order, "Order").cloned() else {
        return failed(&["input", "orderId"], "Order does not exist", "NOT_FOUND");
    };
    if model::is_cancelled(&record) {
        return failed(
            &["input", "orderId"],
            "A cancelled order cannot be edited",
            "INVALID",
        );
    }
    let removals = input
        .get("lineItems")
        .and_then(|items| items.get("removals"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if removals.is_empty() {
        return failed(
            &["input", "lineItems", "removals"],
            "Removals can't be blank",
            "BLANK",
        );
    }
    let lines = gids(record.get("lineItems"));
    let mut taken: Vec<(String, i64, i64)> = Vec::new();
    for (index, removal) in removals.iter().enumerate() {
        let index = index.to_string();
        let path = ["input", "lineItems", "removals", index.as_str()];
        let line = removal
            .get("lineItemId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(item) = state
            .get(line)
            .filter(|_| lines.iter().any(|known| known == line))
        else {
            return failed(
                &[&path[..], &["lineItemId"]].concat(),
                "Line item does not exist on the order",
                "NOT_FOUND",
            );
        };
        let quantity = removal.get("quantity").and_then(Value::as_i64).unwrap_or(0);
        let unfulfilled = int(item, "unfulfilledQuantity");
        let already: i64 = taken
            .iter()
            .filter(|(known, _, _)| known == line)
            .map(|(_, quantity, _)| quantity)
            .sum();
        if quantity <= 0 || quantity + already > unfulfilled {
            return failed(
                &[&path[..], &["quantity"]].concat(),
                "Quantity must be greater than 0 and at most what is left to fulfill of the line item",
                "INVALID",
            );
        }
        taken.push((line.to_string(), quantity, int(item, "currentQuantity")));
    }
    let state = mutation.state_mut();
    let now = state.timestamp();
    let id = state.new_gid("RequestedOrderEdit");
    let mut items = Vec::new();
    for (line, quantity, current) in taken {
        let item = state.new_gid("RequestedOrderEditLineItem");
        state.insert(value::object(json!({
            "__typename": "RequestedOrderEditLineItem",
            "id": item,
            "lineItem": line,
            "quantity": quantity,
            "resolvedQuantity": 0,
            AT_REQUEST: current,
        })));
        items.push(item);
    }
    state.insert(value::object(json!({
        "__typename": "RequestedOrderEdit",
        "id": id,
        "order": order,
        "status": "REQUESTED",
        "lineItems": {"removals": items},
        "requestedAt": now,
        "createdAt": now,
        "updatedAt": now,
        "requestDeclinedAt": null,
        "requestResolvedAt": null,
    })));
    model::push_key(state, &order, "requestedOrderEdits", &id);
    state.emit("requested_order_edit/created", &id);
    Ok(payload(json!({"requestedOrderEdit": id}), &[]))
}

/// `requestedOrderEditDecline` and `requestedOrderEditResolve`: a pending request to `status`.
fn close(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    status: &str,
) -> Result<Value, FieldError> {
    let failed = |field: &[&str], message: &str, code: &str| {
        Ok(payload(
            json!({"requestedOrderEdit": null}),
            &[UserError::new(field, message).code(code)],
        ))
    };
    let input = model::input_of(ctx.arg("input"));
    let id = input
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let state = mutation.state();
    let Some(record) = state.get_of(&id, "RequestedOrderEdit").cloned() else {
        return failed(
            &["input", "id"],
            "Requested order edit does not exist",
            "NOT_FOUND",
        );
    };
    if text(&record, "status") != Some("REQUESTED") {
        return failed(
            &["input", "id"],
            "Only a requested order edit that is pending can be resolved or declined",
            "INVALID",
        );
    }
    let order = text(&record, "order").unwrap_or_default().to_string();
    let note = input
        .get("declineNote")
        .and_then(Value::as_str)
        .map(str::to_string);
    let state = mutation.state_mut();
    let now = state.timestamp();
    let removals = record
        .get("lineItems")
        .and_then(|items| items.get("removals"))
        .map(|removals| gids(Some(removals)))
        .unwrap_or_default();
    if status == "RESOLVED" {
        for removal in &removals {
            let Some(item) = state.get(removal).cloned() else {
                continue;
            };
            let current = text(&item, "lineItem")
                .and_then(|line| state.get(line))
                .map_or(0, |line| int(line, "currentQuantity"));
            let resolved = (int(&item, AT_REQUEST) - current).clamp(0, int(&item, "quantity"));
            if let Some(item) = state.get_mut(removal) {
                item.insert("resolvedQuantity".into(), json!(resolved));
            }
        }
    }
    if let Some(record) = state.get_mut(&id) {
        record.insert("status".into(), json!(status));
        record.insert("updatedAt".into(), json!(now));
        let field = if status == "RESOLVED" {
            "requestResolvedAt"
        } else {
            "requestDeclinedAt"
        };
        record.insert(field.into(), json!(now));
        if status == "DECLINED" {
            record.insert(DECLINE_NOTE.into(), json!(note));
        }
    }
    if status == "DECLINED"
        && let Some(email) = state
            .get(&order)
            .and_then(|order| text(order, "email"))
            .map(str::to_string)
    {
        let name = state
            .get(&order)
            .and_then(|order| text(order, "name"))
            .unwrap_or_default()
            .to_string();
        notify_decline(state, &email, &name, note.as_deref(), &id);
    }
    state.emit(
        if status == "RESOLVED" {
            "requested_order_edit/resolved"
        } else {
            "requested_order_edit/declined"
        },
        &id,
    );
    Ok(payload(json!({"requestedOrderEdit": id}), &[]))
}

/// The buyer's notice that their request was declined, in the outbox.
fn notify_decline(state: &mut ShopState, to: &str, name: &str, note: Option<&str>, id: &str) {
    let body = match note {
        Some(note) => format!("Your request to edit order {name} was declined: {note}"),
        None => format!("Your request to edit order {name} was declined."),
    };
    outbox::record(
        state,
        Entry::email("requested_order_edit_declined", to)
            .subject(format!("Your request to edit order {name}"))
            .body(body)
            .about(id),
    );
}

/// `requestedOrderEditCalculate`. Lines that are not of the order, or more units than are left
/// to fulfill, answer a GraphQL error.
fn calculate(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let input = model::input_of(ctx.arg("input"));
    let state = r.state();
    let order_id = input
        .get("orderId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let Some(order) = state.get_of(&order_id, "Order") else {
        return Ok(Output::Null);
    };
    let currency = model::currency(state);
    let bag = |cents: i64| value::money_bag(cents, &currency);
    let lines = gids(order.get("lineItems"));
    let (mut subtotal, mut before, mut tax) = (0, 0, 0);
    let mut removals = Vec::new();
    for (index, removal) in input
        .get("lineItems")
        .and_then(|items| items.get("removals"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
    {
        let line = removal
            .get("lineItemId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let quantity = removal.get("quantity").and_then(Value::as_i64).unwrap_or(0);
        let Some(item) = state
            .get(line)
            .filter(|_| lines.iter().any(|known| known == line))
        else {
            return Err(FieldError::new(format!(
                "lineItems.removals[{index}]: the line item is not of the order"
            )));
        };
        if quantity <= 0 || quantity > int(item, "unfulfilledQuantity") {
            return Err(FieldError::new(format!(
                "lineItems.removals[{index}]: the quantity is more than what is left to fulfill"
            )));
        }
        let (_, line_subtotal, line_tax) =
            super::refunds::line_amounts(state, order, line, quantity);
        subtotal += line_subtotal;
        tax += line_tax;
        before += model::share(
            model::field_cents(item, "discountedTotalSet"),
            quantity,
            int(item, "quantity").max(1),
        );
        removals.push(json!({
            "lineItem": line,
            "quantity": quantity,
            "subtotalSet": bag(line_subtotal),
            "totalTaxSet": bag(line_tax),
        }));
    }
    Ok(Output::Object(Obj::built(
        "CalculatedRequestedOrderEdit",
        value::object(json!({
            "lineItems": {"removals": removals},
            "financialSummary": {
                "editSubtotalBeforeTargetAllDiscountsSet": bag(before),
                "editOrderLevelDiscountSubtotalSet": bag((before - subtotal).max(0)),
                "editSubtotalSet": bag(before),
                "editSubtotalWithCartDiscountSet": bag(subtotal),
                "editTotalTaxSet": bag(tax),
                "editTotalSet": bag(subtotal + tax),
            },
        })),
    )))
}
