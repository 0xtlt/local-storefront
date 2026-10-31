//! Returns: `returnCreate`, `returnRequest`, `returnApproveRequest`, `returnDeclineRequest`,
//! `returnClose`, `returnReopen`, `returnCancel`, `returnProcess`.
//!
//! As shopify.dev describes them (the reference of 2026-10 and the returns apps tutorials,
//! `apps/build/orders-fulfillment/returns-apps`):
//!
//! - A return takes back units that shipped: fulfillment line items, each up to what shipped
//!   less what returns that were not declined or canceled hold (what `returnableFulfillments`
//!   lists).
//! - Statuses: `returnRequest` makes a `REQUESTED` return, which `returnApproveRequest` opens or
//!   `returnDeclineRequest` declines; `returnCreate` makes an `OPEN` one; `returnClose` closes an
//!   open one, `returnReopen` reopens a closed one, `returnCancel` cancels an open one that
//!   nothing was processed or refunded of; `returnProcess` processes units of an open return,
//!   refunds them when its `financialTransfer` says so, and closes the return once every unit
//!   is processed.
//! - Events: `returns/request`, `returns/approve` (also for `returnCreate`, whose return is open
//!   at once: the topic is about a return becoming `OPEN`, lsf's reading), `returns/decline`,
//!   `returns/close`, `returns/reopen`, `returns/cancel`, `returns/process` (with
//!   `refunds/create` for its refund). The order's `returnStatus` follows.
//! - Not simulated: exchanges, reverse fulfillment orders and deliveries, restocking fees,
//!   return shipping fees (said in `extensions.lsf.notes`). A disposition `RESTOCKED` of
//!   `returnProcess` puts the units back in stock at its location.
//!
//! Messages: "Return is not approvable. Only returns with status REQUESTED can be approved."
//! and "Return is not declinable. Only non-refunded returns with status REQUESTED can be
//! declined." are the reference's examples; "Return line item has an invalid quantity." and
//! "Fulfillment Line Item was not found." are quoted from community.shopify.dev. The others are
//! lsf's wording in the same form.

use serde_json::{Value, json};

use super::model::{self, array, int, text};
use super::refunds::{self, RefundLine, RefundSpec};
use crate::domains::catalog::gids;
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("returnCreate", |m, ctx| {
            create(m, ctx, "returnInput", "OPEN")
        })
        .mutation("returnRequest", |m, ctx| {
            create(m, ctx, "input", "REQUESTED")
        })
        .mutation("returnApproveRequest", approve)
        .mutation("returnDeclineRequest", decline)
        .mutation("returnClose", |m, ctx| {
            change_status(m, ctx, &["OPEN"], "CLOSED", "returns/close")
        })
        .mutation("returnReopen", |m, ctx| {
            change_status(m, ctx, &["CLOSED"], "OPEN", "returns/reopen")
        })
        .mutation("returnCancel", |m, ctx| {
            change_status(m, ctx, &["OPEN", "REQUESTED"], "CANCELED", "returns/cancel")
        })
        .mutation("returnProcess", process);
}

/// The order of a fulfillment line item, through the fulfillment that holds it.
fn order_of_fulfillment_line(state: &ShopState, fulfillment_line: &str) -> Option<String> {
    state.keys("Fulfillment").find_map(|fulfillment| {
        let record = state.get(fulfillment)?;
        if gids(record.get("fulfillmentLineItems"))
            .iter()
            .any(|id| id == fulfillment_line)
        {
            text(record, "order").map(str::to_string)
        } else {
            None
        }
    })
}

/// What is left to return of a fulfillment line item: what shipped, less what the returns that
/// were not declined or canceled hold of it.
pub fn returnable_quantity(state: &ShopState, fulfillment_line: &str) -> i64 {
    let Some(line) = state.get(fulfillment_line) else {
        return 0;
    };
    let shipped = int(line, "quantity");
    let Some(order) = order_of_fulfillment_line(state, fulfillment_line) else {
        return shipped;
    };
    let in_returns: i64 = state
        .get(&order)
        .map(|record| gids(record.get("returns")))
        .unwrap_or_default()
        .iter()
        .filter_map(|id| state.get(id))
        .filter(|record| !matches!(text(record, "status"), Some("DECLINED" | "CANCELED")))
        .flat_map(|record| gids(record.get("returnLineItems")))
        .filter_map(|id| state.get(&id))
        .filter(|item| text(item, "fulfillmentLineItem") == Some(fulfillment_line))
        .map(|item| int(item, "quantity"))
        .sum();
    (shipped - in_returns).max(0)
}

/// A return error with its code.
fn error(field: &[&str], message: &str, code: &str) -> UserError {
    UserError::new(field, message).code(code)
}

/// `returnCreate` (an open return) and `returnRequest` (a requested one).
fn create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    argument: &str,
    status: &str,
) -> Result<Value, FieldError> {
    let input = model::input_of(ctx.arg(argument));
    let failed = |errors: &[UserError]| Ok(payload(json!({"return": null}), errors));
    let order = input
        .get("orderId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let state = mutation.state();
    if state.type_of(&order) != Some("Order") {
        return failed(&[error(
            &[argument, "orderId"],
            "Order does not exist.",
            "NOT_FOUND",
        )]);
    }
    let items = array(&input, "returnLineItems");
    if items.is_empty() {
        return failed(&[error(
            &[argument, "returnLineItems"],
            "Return line items can't be blank.",
            "BLANK",
        )]);
    }
    let mut errors = Vec::new();
    let mut lines: Vec<(String, i64, Value)> = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let index = index.to_string();
        let path = [argument, "returnLineItems", index.as_str()];
        let line = item
            .get("fulfillmentLineItemId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let quantity = item.get("quantity").and_then(Value::as_i64).unwrap_or(0);
        if state.type_of(line) != Some("FulfillmentLineItem")
            || order_of_fulfillment_line(state, line).as_deref() != Some(order.as_str())
        {
            errors.push(error(
                &[&path[..], &["fulfillmentLineItemId"]].concat(),
                "Fulfillment Line Item was not found.",
                "NOT_FOUND",
            ));
            continue;
        }
        let taken: i64 = lines
            .iter()
            .filter(|(known, _, _)| known == line)
            .map(|(_, quantity, _)| quantity)
            .sum();
        if quantity <= 0 || quantity + taken > returnable_quantity(state, line) {
            errors.push(error(
                &[&path[..], &["quantity"]].concat(),
                "Return line item has an invalid quantity.",
                "INVALID",
            ));
            continue;
        }
        lines.push((line.to_string(), quantity, item.clone()));
    }
    if !errors.is_empty() {
        return failed(&errors);
    }
    for (field, what) in [
        ("exchangeLineItems", "exchanges"),
        ("returnShippingFee", "return shipping fees"),
    ] {
        if input
            .get(field)
            .is_some_and(|value| !value.is_null() && value != &json!([]))
        {
            mutation.note(format!(
                "{}: lsf does not simulate {what} yet: `{field}` was ignored",
                ctx.field_name()
            ));
        }
    }
    let state = mutation.state_mut();
    let now = state.timestamp();
    let id = state.new_gid("Return");
    let count = state
        .get(&order)
        .map_or(0, |record| gids(record.get("returns")).len());
    let name = state
        .get(&order)
        .and_then(|record| text(record, "name"))
        .unwrap_or_default()
        .to_string();
    let mut return_lines = Vec::new();
    for (line, quantity, item) in &lines {
        let line_id = state.new_gid("ReturnLineItem");
        let reason = item
            .get("returnReason")
            .and_then(Value::as_str)
            .unwrap_or("UNKNOWN");
        state.insert(value::object(json!({
            "__typename": "ReturnLineItem",
            "id": line_id,
            "fulfillmentLineItem": line,
            "quantity": quantity,
            "processedQuantity": 0,
            "unprocessedQuantity": quantity,
            "processableQuantity": if status == "OPEN" { *quantity } else { 0 },
            "refundableQuantity": quantity,
            "refundedQuantity": 0,
            "returnReason": reason,
            "returnReasonNote": item.get("returnReasonNote").and_then(Value::as_str).unwrap_or_default(),
            "customerNote": item.get("customerNote"),
            "restockingFee": null,
        })));
        return_lines.push(line_id);
    }
    state.insert(value::object(json!({
        "__typename": "Return",
        "id": id,
        "name": format!("{name}-R{}", count + 1),
        "status": status,
        "order": order,
        "returnLineItems": return_lines,
        "totalQuantity": lines.iter().map(|(_, quantity, _)| quantity).sum::<i64>(),
        "createdAt": input.get("requestedAt").and_then(Value::as_str).unwrap_or(&now),
        "closedAt": null,
        "requestApprovedAt": if status == "OPEN" { json!(now) } else { Value::Null },
        "decline": null,
        "refunds": [],
        "transactions": [],
        "exchangeLineItems": [],
        "reverseFulfillmentOrders": [],
        "returnShippingFees": [],
    })));
    model::push_key(state, &order, "returns", &id);
    model::refresh_order(state, &order);
    state.emit(
        if status == "OPEN" {
            "returns/approve"
        } else {
            "returns/request"
        },
        &id,
    );
    Ok(payload(json!({"return": id}), &[]))
}

/// A return of the shop, its status and its order.
fn find_return(state: &ShopState, id: &str) -> Option<(String, String)> {
    let record = state
        .get(id)
        .filter(|_| state.type_of(id) == Some("Return"))?;
    Some((
        text(record, "status").unwrap_or("OPEN").to_string(),
        text(record, "order").unwrap_or_default().to_string(),
    ))
}

/// Sets the processable quantities of a return's lines by its status: what is unprocessed of
/// an open return, nothing otherwise.
fn refresh_return(state: &mut ShopState, id: &str) {
    let Some(record) = state.get(id) else {
        return;
    };
    let open = text(record, "status") == Some("OPEN");
    for line in gids(record.get("returnLineItems")) {
        if let Some(item) = state.get_mut(&line) {
            let unprocessed = int(item, "unprocessedQuantity");
            item.insert(
                "processableQuantity".into(),
                json!(if open { unprocessed } else { 0 }),
            );
        }
    }
}

/// `returnApproveRequest`.
fn approve(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx
        .arg("input")
        .and_then(|input| input.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let failed = |error: UserError| Ok(payload(json!({"return": null}), &[error]));
    let Some((status, order)) = find_return(mutation.state(), &id) else {
        return failed(self::error(
            &["input", "id"],
            "Return does not exist.",
            "NOT_FOUND",
        ));
    };
    if status != "REQUESTED" {
        return failed(self::error(
            &["input", "id"],
            "Return is not approvable. Only returns with status REQUESTED can be approved.",
            "INVALID_STATE",
        ));
    }
    let now = mutation.timestamp();
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&id) {
        record.insert("status".into(), json!("OPEN"));
        record.insert("requestApprovedAt".into(), json!(now));
    }
    refresh_return(state, &id);
    model::refresh_order(state, &order);
    state.emit("returns/approve", &id);
    Ok(payload(json!({"return": id}), &[]))
}

/// `returnDeclineRequest`.
fn decline(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = model::input_of(ctx.arg("input"));
    let id = input
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let failed = |error: UserError| Ok(payload(json!({"return": null}), &[error]));
    let Some((status, order)) = find_return(mutation.state(), &id) else {
        return failed(self::error(
            &["input", "id"],
            "Return does not exist.",
            "NOT_FOUND",
        ));
    };
    if status != "REQUESTED" {
        return failed(self::error(
            &["input", "id"],
            "Return is not declinable. Only non-refunded returns with status REQUESTED can be declined.",
            "INVALID_STATE",
        ));
    }
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&id) {
        record.insert("status".into(), json!("DECLINED"));
        record.insert(
            "decline".into(),
            json!({"reason": input.get("declineReason"), "note": input.get("declineNote")}),
        );
    }
    refresh_return(state, &id);
    model::refresh_order(state, &order);
    state.emit("returns/decline", &id);
    Ok(payload(json!({"return": id}), &[]))
}

/// `returnClose`, `returnReopen`, `returnCancel`: a return from one of `from` to `to`.
fn change_status(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    from: &[&str],
    to: &str,
    topic: &str,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let failed = |error: UserError| Ok(payload(json!({"return": null}), &[error]));
    let state = mutation.state();
    let Some((status, order)) = find_return(state, &id) else {
        return failed(self::error(&["id"], "Return does not exist.", "NOT_FOUND"));
    };
    let processed_or_refunded = state.get(&id).is_some_and(|record| {
        !gids(record.get("refunds")).is_empty()
            || gids(record.get("returnLineItems"))
                .iter()
                .filter_map(|line| state.get(line))
                .any(|line| int(line, "processedQuantity") > 0)
    });
    if !from.contains(&status.as_str()) || (to == "CANCELED" && processed_or_refunded) {
        let message = match to {
            "CLOSED" => "Return is not closable. Only returns with status OPEN can be closed.",
            "OPEN" => "Return is not reopenable. Only returns with status CLOSED can be reopened.",
            _ => {
                "Return is not cancelable. Only returns that were not processed or refunded can be canceled."
            }
        };
        return failed(self::error(&["id"], message, "INVALID_STATE"));
    }
    let now = mutation.timestamp();
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&id) {
        record.insert("status".into(), json!(to));
        record.insert(
            "closedAt".into(),
            if to == "CLOSED" {
                json!(now)
            } else {
                Value::Null
            },
        );
    }
    refresh_return(state, &id);
    model::refresh_order(state, &order);
    state.emit(topic, &id);
    Ok(payload(json!({"return": id}), &[]))
}

/// `returnProcess`.
fn process(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = model::input_of(ctx.arg("input"));
    let id = input
        .get("returnId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let failed = |errors: &[UserError]| Ok(payload(json!({"return": null}), errors));
    let state = mutation.state();
    let Some((status, order)) = find_return(state, &id) else {
        return failed(&[error(
            &["input", "returnId"],
            "Return does not exist.",
            "NOT_FOUND",
        )]);
    };
    if status != "OPEN" {
        return failed(&[error(
            &["input", "returnId"],
            "Return is not processable. Only returns with status OPEN can be processed.",
            "INVALID_STATE",
        )]);
    }
    let return_lines = state
        .get(&id)
        .map(|record| gids(record.get("returnLineItems")))
        .unwrap_or_default();
    let mut errors = Vec::new();
    // (return line item, quantity, restock location when a disposition restocks it)
    let mut processing: Vec<(String, i64, Option<Option<String>>)> = Vec::new();
    for (index, item) in array(&input, "returnLineItems").iter().enumerate() {
        let index = index.to_string();
        let line = item.get("id").and_then(Value::as_str).unwrap_or_default();
        let quantity = item.get("quantity").and_then(Value::as_i64).unwrap_or(0);
        if !return_lines.iter().any(|known| known == line) {
            errors.push(error(
                &["input", "returnLineItems", &index, "id"],
                "Return line item does not exist.",
                "NOT_FOUND",
            ));
            continue;
        }
        let processable = state
            .get(line)
            .map_or(0, |line| int(line, "processableQuantity"));
        if quantity <= 0 || quantity > processable {
            errors.push(error(
                &["input", "returnLineItems", &index, "quantity"],
                "Return line item has an invalid quantity.",
                "INVALID",
            ));
            continue;
        }
        let restock = item
            .get("dispositions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|disposition| {
                disposition.get("dispositionType").and_then(Value::as_str) == Some("RESTOCKED")
            })
            .map(|disposition| {
                disposition
                    .get("locationId")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
        processing.push((line.to_string(), quantity, restock));
    }
    let transfer = model::input_of(input.get("financialTransfer"));
    let refund = model::input_of(transfer.get("issueRefund"));
    let order_record = state.get(&order).cloned().unwrap_or_default();
    let payments = model::refundable_payments(state, &order_record);
    let mut moves = Vec::new();
    for (index, transaction) in array(&refund, "orderTransactions").iter().enumerate() {
        let index = index.to_string();
        let parent = transaction
            .get("parentId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let amount = model::cents(transaction.get("transactionAmount"));
        let Some((_, left)) = payments.iter().find(|(id, _)| id == parent) else {
            errors.push(error(
                &[
                    "input",
                    "financialTransfer",
                    "issueRefund",
                    "orderTransactions",
                    &index,
                    "parentId",
                ],
                "Parent transaction does not exist.",
                "NOT_FOUND",
            ));
            continue;
        };
        let over = refund.get("allowOverRefunding").and_then(Value::as_bool) == Some(true);
        if amount <= 0 || (!over && amount > *left) {
            errors.push(error(
                &[
                    "input",
                    "financialTransfer",
                    "issueRefund",
                    "orderTransactions",
                    &index,
                    "transactionAmount",
                ],
                "Transaction amount is invalid.",
                "INVALID",
            ));
            continue;
        }
        let gateway = state
            .get(parent)
            .and_then(|record| text(record, "gateway"))
            .map(str::to_string);
        moves.push((amount, gateway, Some(parent.to_string())));
    }
    let store_credit = refunds::store_credit_refunds(
        state,
        &order_record,
        &array(&refund, "refundMethods"),
        &["input", "financialTransfer", "issueRefund", "refundMethods"],
        &mut errors,
    );
    if processing.is_empty() && moves.is_empty() && store_credit.is_empty() && errors.is_empty() {
        errors.push(error(
            &["input", "returnLineItems"],
            "Return line items can't be blank.",
            "BLANK",
        ));
    }
    if !errors.is_empty() {
        return failed(&errors);
    }
    if input
        .get("exchangeLineItems")
        .is_some_and(|items| items != &json!([]) && !items.is_null())
    {
        mutation.note(
            "returnProcess: lsf does not simulate exchanges yet: `exchangeLineItems` was ignored",
        );
    }
    let shipping_input = model::input_of(input.get("refundShipping"));
    let state = mutation.state_mut();
    // The units are processed; a refund takes them back when money goes back with them.
    let mut refund_lines = Vec::new();
    for (line, quantity, restock) in &processing {
        let fulfillment_line = state
            .get(line)
            .and_then(|item| text(item, "fulfillmentLineItem"))
            .map(str::to_string);
        if let Some(item) = state.get_mut(line) {
            let processed = int(item, "processedQuantity") + quantity;
            let unprocessed = int(item, "unprocessedQuantity") - quantity;
            item.insert("processedQuantity".into(), json!(processed));
            item.insert("unprocessedQuantity".into(), json!(unprocessed));
            item.insert("processableQuantity".into(), json!(unprocessed));
        }
        let line_item = fulfillment_line
            .as_deref()
            .and_then(|line| state.get(line))
            .and_then(|line| text(line, "lineItem"))
            .map(str::to_string);
        let Some(line_item) = line_item else {
            continue;
        };
        if moves.is_empty() && store_credit.is_empty() {
            // Without a refund, a restocked unit goes back in stock alone.
            if let Some(location) = restock {
                model::restock_line(state, &line_item, location.as_deref(), *quantity, true);
            }
        } else {
            refund_lines.push(RefundLine {
                line: line_item,
                quantity: *quantity,
                restock_type: if restock.is_some() {
                    "RETURN"
                } else {
                    "NO_RESTOCK"
                }
                .into(),
                location: restock.clone().flatten(),
            });
        }
    }
    let mut refund = None;
    if !moves.is_empty() || !store_credit.is_empty() {
        let order_record = state.get(&order).cloned().unwrap_or_default();
        let remaining_shipping = refunds::refundable_shipping(&order_record);
        let shipping = if shipping_input.get("fullRefund").and_then(Value::as_bool) == Some(true) {
            remaining_shipping
        } else {
            model::cents(shipping_input.get("shippingRefundAmount")).min(remaining_shipping)
        };
        refund = Some(refunds::create_refund(
            state,
            &order,
            &RefundSpec {
                lines: refund_lines,
                shipping,
                transactions: moves,
                note: input
                    .get("note")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                processed_at: None,
                return_id: Some(id.clone()),
                store_credit,
                cancellation: false,
            },
        ));
        model::settle_financial_status(state, &order);
    }
    let done = gids(
        state
            .get(&id)
            .and_then(|record| record.get("returnLineItems")),
    )
    .iter()
    .filter_map(|line| state.get(line))
    .all(|line| int(line, "unprocessedQuantity") == 0);
    if done {
        let now = state.timestamp();
        if let Some(record) = state.get_mut(&id) {
            record.insert("status".into(), json!("CLOSED"));
            record.insert("closedAt".into(), json!(now));
        }
        refresh_return(state, &id);
    }
    model::refresh_order(state, &order);
    state.emit("returns/process", &id);
    if refund.is_some() {
        model::order_updated(state, &order);
    }
    Ok(payload(json!({"return": id}), &[]))
}
