//! The life of an order: `orderCreate`, `orderUpdate`, `orderCancel`, `orderClose`, `orderOpen`,
//! `orderMarkAsPaid`, `orderCapture`, `transactionVoid`, `orderCustomerSet`,
//! `orderCustomerRemove`, `orderInvoiceSend`, `orderDelete`.
//!
//! As shopify.dev's reference of 2026-10 describes them (`mutations/<name>`), with what the
//! topics of `WebhookSubscriptionTopic` say they fire on; Shopify documents no order between
//! topics, lsf emits the specific one first, then `orders/updated` (which "also fires in
//! addition to the more specific webhooks", a staff answer on community.shopify.dev):
//!
//! - `orderUpdate` changes the email, phone, note, PO number, tags (replaced), custom attributes
//!   and shipping address given (the fulfillment orders' destination follows): `orders/updated`.
//! - `orderCancel` cancels at once and answers its `job` done, with a `jobResult` the `node`
//!   query reads (Shopify cancels asynchronously, the job `done: false` in its examples). It
//!   voids the authorizations ("Authorized payments will be voided regardless of this
//!   setting"), refunds the payments to their methods when `refundMethod.
//!   originalPaymentMethodsRefund` (or the deprecated `refund`) says so, puts the units that did
//!   not ship back in stock when `restock`, and cancels the open fulfillment orders. What it takes
//!   back is a refund, as Shopify records it, when it restocks or refunds. Events:
//!   `order_transactions/create`, `refunds/create`, `fulfillment_orders/cancelled`,
//!   `orders/cancelled`, `orders/updated`.
//! - `orderClose` and `orderOpen` set `closed` and `closedAt`: `orders/updated`.
//! - `orderMarkAsPaid` captures what authorizations hold, else records a manual sale, of the
//!   outstanding amount ("will either create a new sale transaction for the full outstanding
//!   amount or capture an existing authorized transaction"); refused, with Shopify's
//!   "Order cannot be marked as paid.", once paid: `order_transactions/create`, `orders/paid`,
//!   `orders/updated`.
//! - `orderCapture` captures part or all of an authorization (a capture is final unless
//!   `finalCapture: false`, Shopify's multi-capture): `order_transactions/create`, `orders/paid`
//!   once paid in full, `orders/updated`. `transactionVoid` voids an authorization, with the
//!   messages of `TransactionVoidUserErrorCode`'s descriptions.
//! - `orderCreate` imports an order (see [`order_create`]): `orders/create`, `orders/paid` when
//!   paid, `order_transactions/create` for each transaction.
//! - `orderDelete` deletes a cancelled or closed order ("cancelled or archived orders can be
//!   deleted", Shopify's help center) with what is its alone: `orders/delete`.
//!
//! Messages shopify.dev does not show are lsf's wording, said where they are made.

use lsf_core::util::stable_id;
use serde_json::{Value, json};

use super::model::{self, NewTransaction, array, field_cents, int, text};
use super::refunds::{self, RefundLine, RefundSpec};
use super::{FinancialStatus, FulfillmentState, LineDraft, OrderDraft, ShippingDraft};
use crate::domains::catalog::{gids, tags_of};
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{ShopState, gid, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("orderCreate", order_create)
        .mutation("orderUpdate", order_update)
        .mutation("orderCancel", order_cancel)
        .mutation("orderClose", |m, ctx| set_closed(m, ctx, true))
        .mutation("orderOpen", |m, ctx| set_closed(m, ctx, false))
        .mutation("orderMarkAsPaid", order_mark_as_paid)
        .mutation("orderCapture", order_capture)
        .mutation("transactionVoid", transaction_void)
        .mutation("orderCustomerSet", order_customer_set)
        .mutation("orderCustomerRemove", order_customer_remove)
        .mutation("orderInvoiceSend", order_invoice_send)
        .mutation("orderDelete", order_delete);
}

/// The order a GID names, when it is one of the shop.
pub fn find_order(state: &ShopState, id: &str) -> Option<String> {
    (state.type_of(id) == Some("Order")).then(|| id.to_string())
}

/// lsf's wording of an unknown order: shopify.dev shows none.
pub fn no_order(field: &[&str]) -> UserError {
    UserError::new(field, "Order does not exist").code("NOT_FOUND")
}

/// The id of an `{id}` input (`OrderCloseInput`, `OrderMarkAsPaidInput`, …).
fn input_id(ctx: &FieldContext<'_>) -> String {
    ctx.arg("input")
        .and_then(|input| input.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `orderUpdate`.
fn order_update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = model::input_of(ctx.arg("input"));
    let id = input
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let Some(order) = find_order(mutation.state(), &id) else {
        return Ok(payload(json!({"order": null}), &[no_order(&["id"])]));
    };
    for field in ["metafields", "localizedFields", "localizationExtensions"] {
        if input.get(field).is_some_and(|value| !value.is_null()) {
            mutation.note(format!(
                "orderUpdate: lsf does not simulate `{field}` yet: it was ignored"
            ));
        }
    }
    let state = mutation.state_mut();
    let address_id = state
        .get(&order)
        .and_then(|record| record.get("shippingAddress"))
        .and_then(|address| address.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| address_gid(&order, "shipping"));
    let address = input
        .get("shippingAddress")
        .and_then(|address| model::mailing_address(address, &address_id));
    let Some(record) = state.get_mut(&order) else {
        return Ok(payload(json!({"order": null}), &[]));
    };
    for field in ["email", "phone", "note", "poNumber"] {
        if let Some(value) = input.get(field) {
            let value = value.as_str().filter(|text| !text.is_empty());
            record.insert(field.into(), json!(value));
        }
    }
    if input.contains_key("tags") {
        record.insert("tags".into(), json!(tags_of(input.get("tags"))));
    }
    if input.contains_key("customAttributes") {
        record.insert(
            "customAttributes".into(),
            model::attributes(input.get("customAttributes")),
        );
    }
    if let Some(address) = &address {
        record.insert("shippingAddress".into(), address.clone());
        let billing = record.get("billingAddress").cloned();
        record.insert(
            "billingAddressMatchesShippingAddress".into(),
            json!(billing.is_some_and(|billing| same_place(&billing, address))),
        );
        let email = record.get("email").cloned();
        for fulfillment_order in gids(record.get("fulfillmentOrders")) {
            if let Some(fulfillment_order) = state.get_mut(&fulfillment_order) {
                let id = fulfillment_order
                    .get("destination")
                    .and_then(|destination| destination.get("id"))
                    .cloned()
                    .unwrap_or_else(|| json!(address_gid(&order, "destination")));
                fulfillment_order.insert(
                    "destination".into(),
                    destination(address, id, email.clone().unwrap_or(Value::Null)),
                );
            }
        }
    }
    model::order_updated(state, &order);
    Ok(payload(json!({"order": order}), &[]))
}

/// A GID for an address of an order: `MailingAddress/<n>?model_name=Address`.
fn address_gid(order: &str, kind: &str) -> String {
    format!(
        "gid://shopify/MailingAddress/{}?model_name=Address",
        stable_id("address", &format!("{order}/{kind}"))
    )
}

/// Whether two `MailingAddress`es are the same place (ids aside).
fn same_place(a: &Value, b: &Value) -> bool {
    [
        "firstName",
        "lastName",
        "company",
        "address1",
        "address2",
        "city",
        "provinceCode",
        "countryCodeV2",
        "zip",
    ]
    .iter()
    .all(|key| a.get(key) == b.get(key))
}

/// A `FulfillmentOrderDestination` of a shipping address.
pub fn destination(address: &Value, id: Value, email: Value) -> Value {
    json!({
        "id": id,
        "firstName": address.get("firstName"),
        "lastName": address.get("lastName"),
        "company": address.get("company"),
        "address1": address.get("address1"),
        "address2": address.get("address2"),
        "city": address.get("city"),
        "province": address.get("province"),
        "zip": address.get("zip"),
        "countryCode": address.get("countryCodeV2"),
        "phone": address.get("phone"),
        "email": email,
    })
}

/// `orderClose` and `orderOpen`.
fn set_closed(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    closed: bool,
) -> Result<Value, FieldError> {
    let Some(order) = find_order(mutation.state(), &input_id(ctx)) else {
        return Ok(payload(json!({"order": null}), &[no_order(&["id"])]));
    };
    let now = mutation.timestamp();
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&order) {
        record.insert("closed".into(), json!(closed));
        record.insert(
            "closedAt".into(),
            if closed { json!(now) } else { Value::Null },
        );
    }
    model::order_updated(state, &order);
    Ok(payload(json!({"order": order}), &[]))
}

/// `orderCancel`.
fn order_cancel(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("orderId").unwrap_or_default().to_string();
    let failed = |errors: &[UserError]| {
        let mut answer = payload(json!({"job": null, "jobResult": null}), errors);
        answer["orderCancelUserErrors"] = answer["userErrors"].clone();
        Ok(answer)
    };
    let Some(order) = find_order(mutation.state(), &id) else {
        return failed(&[no_order(&["orderId"])]);
    };
    let state = mutation.state();
    let record = state.get(&order).cloned().unwrap_or_default();
    if model::is_cancelled(&record) {
        // lsf's wording: shopify.dev lists "not already cancelled" among the conditions, not
        // its message.
        return failed(&[
            UserError::new(&["orderId"], "Order has already been cancelled").code("INVALID"),
        ]);
    }
    let staff_note = ctx.arg_str("staffNote").map(str::to_string);
    if staff_note.as_ref().is_some_and(|note| note.chars().count() > 255) {
        return failed(&[UserError::new(
            &["staffNote"],
            "Staff note is too long (maximum is 255 characters)",
        )
        .code("INVALID")]);
    }
    let method = ctx.arg("refundMethod").and_then(Value::as_object);
    let refund_money = method
        .and_then(|method| method.get("originalPaymentMethodsRefund"))
        .and_then(Value::as_bool)
        .or_else(|| ctx.arg_bool("refund"))
        .unwrap_or(false);
    if method.is_some_and(|method| method.get("storeCreditRefund").is_some_and(|v| !v.is_null()))
    {
        mutation.note(
            "orderCancel: lsf does not simulate store credit: `storeCreditRefund` was ignored, \
             nothing was refunded",
        );
    }
    if ctx.arg_bool("notifyCustomer") == Some(true) {
        mutation.note("orderCancel: lsf sends no email: `notifyCustomer` was recorded only");
    }
    let restock = ctx.arg_bool("restock").unwrap_or(false);
    let reason = ctx.arg_str("reason").unwrap_or("OTHER").to_string();

    let state = mutation.state_mut();
    let now = state.timestamp();
    // The authorizations are voided whatever the refund.
    let transactions: Vec<String> = gids(record.get("transactions"));
    for authorization in &transactions {
        let Some(transaction) = state.get(authorization) else {
            continue;
        };
        if text(transaction, "kind") != Some("AUTHORIZATION")
            || text(transaction, "status") != Some("SUCCESS")
        {
            continue;
        }
        let holds = state
            .get(&order)
            .map_or(0, |order| model::capturable_amount(state, order, authorization));
        if holds > 0 {
            let gateway = text(transaction, "gateway").map(str::to_string);
            let amount = field_cents(transaction, "amountSet");
            model::add_transaction(
                state,
                &order,
                &NewTransaction {
                    kind: "VOID",
                    amount,
                    gateway: gateway.as_deref(),
                    parent: Some(authorization),
                    manual: false,
                    test: record.get("test").and_then(Value::as_bool) == Some(true),
                },
            );
        }
    }
    // The open fulfillment orders are cancelled.
    let mut cancelled_orders = Vec::new();
    for fulfillment_order in gids(record.get("fulfillmentOrders")) {
        let Some(fo) = state.get_mut(&fulfillment_order) else {
            continue;
        };
        if matches!(text(fo, "status"), Some("CLOSED" | "CANCELLED")) {
            continue;
        }
        fo.insert("status".into(), json!("CANCELLED"));
        state.touch(&fulfillment_order);
        cancelled_orders.push(fulfillment_order);
    }
    // What is taken back: the units that did not ship (restocked or not), and the money.
    let mut spec = RefundSpec::default();
    for line in gids(record.get("lineItems")) {
        let Some(item) = state.get(&line) else {
            continue;
        };
        let current = int(item, "currentQuantity");
        let unfulfilled = int(item, "unfulfilledQuantity").min(current);
        if restock && unfulfilled > 0 {
            spec.lines.push(RefundLine {
                line: line.clone(),
                quantity: unfulfilled,
                restock_type: "CANCEL".into(),
                location: None,
            });
        } else if refund_money && current > 0 {
            spec.lines.push(RefundLine {
                line: line.clone(),
                quantity: current,
                restock_type: "NO_RESTOCK".into(),
                location: None,
            });
        }
        if restock && refund_money && current > unfulfilled {
            spec.lines.push(RefundLine {
                line,
                quantity: current - unfulfilled,
                restock_type: "NO_RESTOCK".into(),
                location: None,
            });
        }
    }
    if refund_money {
        let order_record = state.get(&order).cloned().unwrap_or_default();
        spec.shipping = refunds::refundable_shipping(&order_record);
        for (payment, left) in model::refundable_payments(state, &order_record) {
            if left > 0 {
                let gateway = state
                    .get(&payment)
                    .and_then(|record| text(record, "gateway"))
                    .map(str::to_string);
                spec.transactions.push((left, gateway, Some(payment)));
            }
        }
    }
    if !spec.lines.is_empty() || !spec.transactions.is_empty() {
        refunds::create_refund(state, &order, &spec);
    }
    if let Some(record) = state.get_mut(&order) {
        record.insert("cancelledAt".into(), json!(now));
        record.insert("cancelReason".into(), json!(reason));
        record.insert("cancellation".into(), json!({"staffNote": staff_note}));
    }
    model::settle_financial_status(state, &order);
    for fulfillment_order in &cancelled_orders {
        state.emit("fulfillment_orders/cancelled", fulfillment_order);
    }
    state.touch(&order);
    state.emit("orders/cancelled", &order);
    state.emit("orders/updated", &order);
    let number = state
        .new_gid("OrderCancelJobResult")
        .rsplit('/')
        .next()
        .and_then(|number| number.parse::<u64>().ok())
        .unwrap_or(1);
    let result = gid("OrderCancelJobResult", number);
    state.insert(value::object(json!({
        "__typename": "OrderCancelJobResult",
        "id": result,
        "done": true,
        "status": "SUCCEEDED",
        "errors": [],
        "order": order,
    })));
    let job = json!({"id": gid("Job", number), "done": true, "query": null});
    Ok(payload(
        json!({"job": job, "jobResult": result, "orderCancelUserErrors": []}),
        &[],
    ))
}

/// `orderMarkAsPaid`.
fn order_mark_as_paid(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let Some(order) = find_order(mutation.state(), &input_id(ctx)) else {
        return Ok(payload(json!({"order": null}), &[no_order(&["id"])]));
    };
    let state = mutation.state();
    let record = state.get(&order).cloned().unwrap_or_default();
    let outstanding = field_cents(&record, "totalOutstandingSet");
    let status = text(&record, "displayFinancialStatus").unwrap_or("PENDING");
    if outstanding <= 0 || status == "PAID" || model::is_cancelled(&record) {
        // Shopify's message (the reference's example "Attempt to mark an already paid order as
        // paid"), with the order.
        let error = UserError::new(&["id"], "Order cannot be marked as paid.");
        return Ok(payload(json!({"order": order}), &[error]));
    }
    let authorizations: Vec<(String, i64)> = gids(record.get("transactions"))
        .iter()
        .filter(|id| {
            state.get(id).is_some_and(|transaction| {
                text(transaction, "kind") == Some("AUTHORIZATION")
                    && text(transaction, "status") == Some("SUCCESS")
            })
        })
        .map(|id| (id.clone(), model::capturable_amount(state, &record, id)))
        .filter(|(_, holds)| *holds > 0)
        .collect();
    let test = record.get("test").and_then(Value::as_bool) == Some(true);
    let state = mutation.state_mut();
    let mut left = outstanding;
    for (authorization, holds) in authorizations {
        if left <= 0 {
            break;
        }
        let amount = left.min(holds);
        let gateway = state
            .get(&authorization)
            .and_then(|record| text(record, "gateway"))
            .map(str::to_string);
        model::add_transaction(
            state,
            &order,
            &NewTransaction {
                kind: "CAPTURE",
                amount,
                gateway: gateway.as_deref(),
                parent: Some(&authorization),
                manual: false,
                test,
            },
        );
        left -= amount;
    }
    if left > 0 {
        model::add_transaction(
            state,
            &order,
            &NewTransaction {
                kind: "SALE",
                amount: left,
                gateway: Some("manual"),
                parent: None,
                manual: true,
                test,
            },
        );
    }
    model::settle_financial_status(state, &order);
    if let Some(record) = state.get_mut(&order) {
        record.insert("displayFinancialStatus".into(), json!("PAID"));
    }
    model::refresh_order(state, &order);
    state.touch(&order);
    state.emit("orders/paid", &order);
    state.emit("orders/updated", &order);
    Ok(payload(json!({"order": order}), &[]))
}

/// `orderCapture`.
fn order_capture(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = model::input_of(ctx.arg("input"));
    let field = |name: &str| input.get(name).and_then(Value::as_str).unwrap_or_default();
    let failed = |error: UserError| Ok(payload(json!({"transaction": null}), &[error]));
    let Some(order) = find_order(mutation.state(), field("id")) else {
        return failed(no_order(&["id"]));
    };
    let state = mutation.state();
    let record = state.get(&order).cloned().unwrap_or_default();
    let parent = field("parentTransactionId").to_string();
    let is_authorization = gids(record.get("transactions")).contains(&parent)
        && state.get(&parent).is_some_and(|transaction| {
            text(transaction, "kind") == Some("AUTHORIZATION")
                && text(transaction, "status") == Some("SUCCESS")
        });
    if !is_authorization {
        // lsf's wording.
        return failed(UserError::new(
            &["parentTransactionId"],
            "Parent transaction must be a successful authorization of the order",
        ));
    }
    let amount = model::cents(input.get("amount"));
    let holds = model::capturable_amount(state, &record, &parent);
    if amount <= 0 {
        return failed(UserError::new(&["amount"], "Amount must be greater than 0"));
    }
    if amount > holds {
        // The input's description: "can't be greater than the amount of the authorized
        // transaction"; the message is lsf's wording of it.
        return failed(UserError::new(
            &["amount"],
            "Amount can't be greater than the amount left to capture on the authorized transaction",
        ));
    }
    let gateway = state
        .get(&parent)
        .and_then(|record| text(record, "gateway"))
        .map(str::to_string);
    let test = record.get("test").and_then(Value::as_bool) == Some(true);
    let final_capture = input.get("finalCapture").and_then(Value::as_bool);
    let state = mutation.state_mut();
    if final_capture == Some(false)
        && let Some(authorization) = state.get_mut(&parent)
    {
        authorization.insert("multiCapturable".into(), json!(true));
    } else if let Some(authorization) = state.get_mut(&parent) {
        authorization.insert("multiCapturable".into(), json!(false));
    }
    let transaction = model::add_transaction(
        state,
        &order,
        &NewTransaction {
            kind: "CAPTURE",
            amount,
            gateway: gateway.as_deref(),
            parent: Some(&parent),
            manual: false,
            test,
        },
    );
    model::settle_financial_status(state, &order);
    state.touch(&order);
    if state
        .get(&order)
        .and_then(|record| text(record, "displayFinancialStatus"))
        == Some("PAID")
    {
        state.emit("orders/paid", &order);
    }
    state.emit("orders/updated", &order);
    Ok(payload(json!({"transaction": transaction}), &[]))
}

/// `transactionVoid`, with the messages of the descriptions of `TransactionVoidUserErrorCode`.
fn transaction_void(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx
        .arg_str("parentTransactionId")
        .unwrap_or_default()
        .to_string();
    let failed = |error: UserError| Ok(payload(json!({"transaction": null}), &[error]));
    let state = mutation.state();
    let Some(transaction) = state
        .get(&id)
        .filter(|_| state.type_of(&id) == Some("OrderTransaction"))
        .cloned()
    else {
        return failed(
            UserError::new(&["parentTransactionId"], "Transaction does not exist.")
                .code("TRANSACTION_NOT_FOUND"),
        );
    };
    if text(&transaction, "kind") != Some("AUTHORIZATION")
        || text(&transaction, "status") != Some("SUCCESS")
    {
        return failed(
            UserError::new(
                &["parentTransactionId"],
                "Transaction must be a successful authorization.",
            )
            .code("AUTH_NOT_SUCCESSFUL"),
        );
    }
    let Some(order) = text(&transaction, "order").map(str::to_string) else {
        return failed(
            UserError::new(&["parentTransactionId"], "Transaction does not exist.")
                .code("TRANSACTION_NOT_FOUND"),
        );
    };
    let holds = state
        .get(&order)
        .map_or(0, |record| model::capturable_amount(state, record, &id));
    if holds <= 0 {
        return failed(
            UserError::new(&["parentTransactionId"], "Transaction must be voidable.")
                .code("AUTH_NOT_VOIDABLE"),
        );
    }
    let gateway = text(&transaction, "gateway").map(str::to_string);
    let test = transaction.get("test").and_then(Value::as_bool) == Some(true);
    let state = mutation.state_mut();
    let void = model::add_transaction(
        state,
        &order,
        &NewTransaction {
            kind: "VOID",
            amount: field_cents(&transaction, "amountSet"),
            gateway: gateway.as_deref(),
            parent: Some(&id),
            manual: false,
            test,
        },
    );
    model::settle_financial_status(state, &order);
    model::order_updated(state, &order);
    Ok(payload(json!({"transaction": void}), &[]))
}

/// `orderCustomerSet`.
fn order_customer_set(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let Some(order) = find_order(mutation.state(), ctx.arg_str("orderId").unwrap_or_default())
    else {
        return Ok(payload(json!({"order": null}), &[no_order(&["orderId"])]));
    };
    let customer = ctx.arg_str("customerId").unwrap_or_default().to_string();
    if mutation.state().type_of(&customer) != Some("Customer") {
        // lsf's wording.
        let error = UserError::new(&["customerId"], "Customer does not exist").code("NOT_FOUND");
        return Ok(payload(json!({"order": null}), &[error]));
    }
    set_customer(mutation.state_mut(), &order, Some(&customer));
    Ok(payload(json!({"order": order}), &[]))
}

/// `orderCustomerRemove`.
fn order_customer_remove(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let Some(order) = find_order(mutation.state(), ctx.arg_str("orderId").unwrap_or_default())
    else {
        return Ok(payload(json!({"order": null}), &[no_order(&["orderId"])]));
    };
    set_customer(mutation.state_mut(), &order, None);
    Ok(payload(json!({"order": order}), &[]))
}

/// Sets the customer of an order, both customers' counters following: `orders/updated`.
fn set_customer(state: &mut ShopState, order: &str, customer: Option<&str>) {
    let previous = state
        .get(order)
        .and_then(|record| text(record, "customer"))
        .map(str::to_string);
    let email = customer
        .and_then(|customer| state.get(customer))
        .and_then(|record| text(record, "email"))
        .map(str::to_string);
    if let Some(record) = state.get_mut(order) {
        record.insert("customer".into(), json!(customer));
        record.insert("purchasingEntity".into(), json!(customer));
        if let Some(email) = email {
            record.insert("email".into(), json!(email));
        }
    }
    if let Some(previous) = previous {
        crate::domains::customers::refresh_customer(state, &previous);
    }
    model::refresh_order(state, order);
    model::order_updated(state, order);
}

/// `orderInvoiceSend`: lsf sends no email; the order is answered as it is.
fn order_invoice_send(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let Some(order) = find_order(mutation.state(), ctx.arg_str("id").unwrap_or_default()) else {
        return Ok(payload(json!({"order": null}), &[no_order(&["id"])]));
    };
    mutation.note("orderInvoiceSend: lsf sends no email; nothing was recorded");
    Ok(payload(json!({"order": order}), &[]))
}

/// `orderDelete`.
fn order_delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("orderId").unwrap_or_default().to_string();
    let Some(order) = find_order(mutation.state(), &id) else {
        return Ok(payload(json!({"deletedId": null}), &[no_order(&["orderId"])]));
    };
    let record = mutation.state().get(&order).cloned().unwrap_or_default();
    let closed = record.get("closed").and_then(Value::as_bool) == Some(true);
    if !closed && !model::is_cancelled(&record) {
        // The rule of Shopify's help center ("You can delete orders that are archived or
        // canceled"); the message is lsf's wording.
        let error = UserError::new(
            &["orderId"],
            "Only cancelled or archived (closed) orders can be deleted",
        )
        .code("INVALID");
        return Ok(payload(json!({"deletedId": null}), &[error]));
    }
    let state = mutation.state_mut();
    delete_order(state, &order);
    Ok(payload(json!({"deletedId": order}), &[]))
}

/// Removes an order and the records that are its alone (line items, fulfillment orders,
/// fulfillments, transactions, refunds, returns), then refreshes its customer. Emits
/// `orders/delete`.
pub fn delete_order(state: &mut ShopState, order: &str) {
    let Some(record) = state.get(order).cloned() else {
        return;
    };
    let mut owned: Vec<String> = Vec::new();
    owned.extend(gids(record.get("lineItems")));
    for fulfillment_order in gids(record.get("fulfillmentOrders")) {
        if let Some(fo) = state.get(&fulfillment_order) {
            owned.extend(gids(fo.get("lineItems")));
        }
        owned.push(fulfillment_order);
    }
    for fulfillment in gids(record.get("fulfillments")) {
        if let Some(record) = state.get(&fulfillment) {
            owned.extend(gids(record.get("fulfillmentLineItems")));
            owned.extend(gids(record.get("events")));
        }
        owned.push(fulfillment);
    }
    for id in gids(record.get("returns")) {
        if let Some(record) = state.get(&id) {
            owned.extend(gids(record.get("returnLineItems")));
        }
        owned.push(id);
    }
    owned.extend(gids(record.get("transactions")));
    owned.extend(gids(record.get("refunds")));
    owned.extend(gids(record.get("metafields")));
    state.delete(order, "orders/delete");
    for key in owned {
        state.remove(&key);
    }
    if let Some(customer) = text(&record, "customer") {
        crate::domains::customers::refresh_customer(state, customer);
    }
}

/// `orderCreate`: an order imported by the app, made the way the seed makes the orders of the
/// store data ([`super::insert_order`]).
///
/// - Lines by `variantId` take the variant's title, SKU, vendor, weight and price (unless
///   `priceSet` gives one); others need a `title` and a `priceSet`.
/// - The customer: `toAssociate` (or the deprecated `customerId`) names one of the shop;
///   `toUpsert` associates the customer of that id or email, and does not create one (said in
///   `extensions.lsf.notes`).
/// - `transactions` are recorded as given, and set the financial status unless
///   `financialStatus` does; a `financialStatus` without transactions records what the seed
///   records for it (a sale of the total for a paid order, an authorization for an authorized
///   one) *(chosen: Shopify records none)*.
/// - `fulfillmentStatus` `FULFILLED` or `PARTIAL` makes a fulfillment, at `fulfillment.
///   locationId` with its tracking; `RESTOCKED` is not simulated.
/// - `options.inventoryBehaviour`: `BYPASS` (the default) leaves the stock alone; the two
///   `DECREMENT_…` commit the units, `DECREMENT_OBEYING_POLICY` refusing a tracked variant that
///   does not have them and does not sell when out of stock (`INVENTORY_CLAIM_FAILED`).
/// - Order-level `taxLines`, one `discountCode`, the first shipping line (others are said in
///   the notes) make the amounts; line-level tax lines add to the order's tax.
fn order_create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = model::input_of(ctx.arg("order"));
    let options = model::input_of(ctx.arg("options"));
    let state = mutation.state();
    let mut errors: Vec<UserError> = Vec::new();
    let failed = |errors: &[UserError]| Ok(payload(json!({"order": null}), errors));
    let shop_currency = model::currency(state);
    if let Some(currency) = input.get("currency").and_then(Value::as_str)
        && currency != shop_currency
    {
        mutation.note(format!(
            "orderCreate: lsf keeps every amount in the shop's currency ({shop_currency}): \
             `currency` {currency} was ignored"
        ));
    }
    let state = mutation.state();
    let behaviour = options
        .get("inventoryBehaviour")
        .and_then(Value::as_str)
        .unwrap_or("BYPASS")
        .to_string();

    // The lines.
    let items = array(&input, "lineItems");
    if items.is_empty() {
        // lsf's wording.
        return failed(&[UserError::new(
            &["order", "lineItems"],
            "An order must have at least one line item",
        )
        .code("INVALID")]);
    }
    let mut lines = Vec::new();
    let mut line_tax = 0;
    for (index, item) in items.iter().enumerate() {
        let index = index.to_string();
        let path = ["order", "lineItems", index.as_str()];
        let get = |name: &str| item.get(name).and_then(Value::as_str);
        let quantity = item.get("quantity").and_then(Value::as_i64).unwrap_or(0);
        if quantity <= 0 {
            errors.push(
                UserError::new(&[&path[..], &["quantity"]].concat(), "Quantity must be greater than 0")
                    .code("INVALID"),
            );
            continue;
        }
        let price = item
            .get("priceSet")
            .and_then(|set| set.get("shopMoney"))
            .map(|money| model::cents(Some(money)));
        let mut line = LineDraft {
            quantity: quantity as u32,
            requires_shipping: item.get("requiresShipping").and_then(Value::as_bool) == Some(true),
            taxable: item.get("taxable").and_then(Value::as_bool) != Some(false),
            gift_card: item.get("giftCard").and_then(Value::as_bool) == Some(true),
            sku: get("sku").map(str::to_string),
            vendor: get("vendor").map(str::to_string),
            attributes: item
                .get("properties")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|property| {
                    Some((
                        property.get("name")?.as_str()?.to_string(),
                        property.get("value")?.as_str()?.to_string(),
                    ))
                })
                .collect(),
            ..LineDraft::default()
        };
        if let Some(variant) = get("variantId") {
            let Some(variant_record) = state
                .get(variant)
                .filter(|_| state.type_of(variant) == Some("ProductVariant"))
            else {
                // lsf's wording.
                errors.push(
                    UserError::new(&[&path[..], &["variantId"]].concat(), "Variant does not exist")
                        .code("INVALID"),
                );
                continue;
            };
            let product = text(variant_record, "product").map(str::to_string);
            let product_record = product.as_deref().and_then(|product| state.get(product));
            let item_gid = text(variant_record, "inventoryItem").map(str::to_string);
            let inventory = item_gid.as_deref().and_then(|item| state.get(item));
            let variant_title = text(variant_record, "title").unwrap_or_default();
            line.product = product.clone();
            line.variant = Some(variant.to_string());
            line.inventory_item = item_gid.clone();
            line.title = get("title")
                .or_else(|| product_record.and_then(|product| text(product, "title")))
                .unwrap_or_default()
                .to_string();
            line.variant_title = (variant_title != "Default Title")
                .then(|| variant_title.to_string())
                .or_else(|| get("variantTitle").map(str::to_string));
            line.sku = line
                .sku
                .take()
                .or_else(|| text(variant_record, "sku").map(str::to_string));
            line.vendor = line
                .vendor
                .take()
                .or_else(|| product_record.and_then(|product| text(product, "vendor")).map(str::to_string));
            line.unit_price = price.unwrap_or_else(|| field_cents(variant_record, "price"));
            line.requires_shipping = item
                .get("requiresShipping")
                .and_then(Value::as_bool)
                .or_else(|| inventory.and_then(|item| item.get("requiresShipping")).and_then(Value::as_bool))
                .unwrap_or(true);
            line.weight = inventory
                .and_then(|item| item.get("measurement"))
                .and_then(|measurement| measurement.get("weight"))
                .cloned();
            line.image = variant_record.get("image").filter(|image| !image.is_null()).cloned();
            line.gift_card |= product_record
                .and_then(|product| product.get("isGiftCard"))
                .and_then(Value::as_bool)
                == Some(true);
            if behaviour == "DECREMENT_OBEYING_POLICY"
                && let Some(item_gid) = &item_gid
                && inventory.and_then(|item| item.get("tracked")).and_then(Value::as_bool) == Some(true)
                && text(variant_record, "inventoryPolicy") != Some("CONTINUE")
                && model::inventory_quantity(state, item_gid, None, "available") < quantity
            {
                // lsf's wording.
                errors.push(
                    UserError::new(
                        &[&path[..], &["quantity"]].concat(),
                        format!("{} is out of stock", line.title),
                    )
                    .code("INVENTORY_CLAIM_FAILED"),
                );
                continue;
            }
        } else {
            match (get("title").filter(|title| !title.trim().is_empty()), price) {
                (Some(title), Some(price)) => {
                    line.title = title.trim().to_string();
                    line.variant_title = get("variantTitle").map(str::to_string);
                    line.unit_price = price;
                }
                (None, _) => {
                    errors.push(
                        UserError::new(&[&path[..], &["title"]].concat(), "Title can't be blank")
                            .code("INVALID"),
                    );
                    continue;
                }
                (_, None) => {
                    // lsf's wording.
                    errors.push(
                        UserError::new(
                            &[&path[..], &["priceSet"]].concat(),
                            "A line item without a variant needs a price",
                        )
                        .code("INVALID"),
                    );
                    continue;
                }
            }
        }
        line_tax += tax_lines_cents(item.get("taxLines"), line.unit_price * quantity);
        lines.push(line);
    }

    // The customer.
    let customer_input = model::input_of(input.get("customer"));
    let associate = model::input_of(customer_input.get("toAssociate"));
    let upsert = model::input_of(customer_input.get("toUpsert"));
    if !customer_input.is_empty() && input.get("customerId").is_some_and(|id| !id.is_null()) {
        errors.push(
            UserError::new(
                &["order", "customerId"],
                "customerId and customer cannot both be provided",
            )
            .code("REDUNDANT_CUSTOMER_FIELDS"),
        );
    }
    let find_customer = |input: &serde_json::Map<String, Value>| -> Option<String> {
        if let Some(id) = input.get("id").and_then(Value::as_str) {
            return (state.type_of(id) == Some("Customer")).then(|| id.to_string());
        }
        let email = input.get("email").and_then(Value::as_str)?;
        state
            .keys("Customer")
            .find(|key| {
                state
                    .get(key)
                    .and_then(|customer| text(customer, "email"))
                    .is_some_and(|known| known.eq_ignore_ascii_case(email))
            })
            .map(str::to_string)
    };
    let customer = if let Some(id) = input.get("customerId").and_then(Value::as_str) {
        let found = (state.type_of(id) == Some("Customer")).then(|| id.to_string());
        if found.is_none() {
            errors.push(
                UserError::new(&["order", "customerId"], "Customer does not exist").code("INVALID"),
            );
        }
        found
    } else if !associate.is_empty() {
        let found = find_customer(&associate);
        if found.is_none() {
            errors.push(
                UserError::new(
                    &["order", "customer", "toAssociate"],
                    "Customer does not exist",
                )
                .code("INVALID"),
            );
        }
        found
    } else if !upsert.is_empty() {
        find_customer(&upsert)
    } else {
        None
    };
    let processed_at = input
        .get("processedAt")
        .and_then(Value::as_str)
        .map(str::to_string);
    let processed = match &processed_at {
        Some(text) => match chrono::DateTime::parse_from_rfc3339(text) {
            Ok(at) if at.with_timezone(&chrono::Utc) <= mutation.now() => {
                Some(at.with_timezone(&chrono::Utc))
            }
            _ => {
                errors.push(
                    UserError::new(
                        &["order", "processedAt"],
                        "Processed at must not be in the future",
                    )
                    .code("PROCESSED_AT_INVALID"),
                );
                None
            }
        },
        None => None,
    };
    let fulfillment_input = model::input_of(input.get("fulfillment"));
    let location = match fulfillment_input.get("locationId").and_then(Value::as_str) {
        Some(location) if state.type_of(location) == Some("Location") => Some(location.to_string()),
        Some(_) => {
            errors.push(
                UserError::new(&["order", "fulfillment", "locationId"], "Location does not exist")
                    .code("INVALID"),
            );
            None
        }
        None => state.keys("Location").next().map(str::to_string),
    };
    if !errors.is_empty() {
        return failed(&errors);
    }
    if !upsert.is_empty() {
        mutation.note(match &customer {
            Some(_) => "orderCreate: lsf associates the customer of `toUpsert` but does not update it",
            None => "orderCreate: lsf does not create customers from `toUpsert` yet: the order has no customer",
        });
    }
    for (field, what) in [
        ("metafields", "`metafields`"),
        ("presentmentCurrency", "`presentmentCurrency`"),
        ("companyLocationId", "B2B orders (`companyLocationId`)"),
    ] {
        if input.get(field).is_some_and(|value| !value.is_null()) {
            mutation.note(format!("orderCreate: lsf does not simulate {what} yet: it was ignored"));
        }
    }
    if options.get("sendReceipt").and_then(Value::as_bool) == Some(true)
        || options.get("sendFulfillmentReceipt").and_then(Value::as_bool) == Some(true)
    {
        mutation.note("orderCreate: lsf sends no email: the receipts were not sent");
    }

    // The shipping, the discount, the tax.
    let shipping_lines = array(&input, "shippingLines");
    if shipping_lines.len() > 1 {
        mutation.note("orderCreate: lsf keeps the first shipping line only");
    }
    let mut shipping = shipping_lines.first().map(|line| {
        let price = model::cents(line.get("priceSet").and_then(|set| set.get("shopMoney")));
        line_tax += tax_lines_cents(line.get("taxLines"), price);
        ShippingDraft {
            title: line
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Shipping")
                .to_string(),
            handle: line.get("code").and_then(Value::as_str).map(str::to_string),
            original: price,
            discounted: price,
        }
    });
    let subtotal: i64 = lines
        .iter()
        .map(|line| line.unit_price * i64::from(line.quantity))
        .sum();
    let discount = model::input_of(input.get("discountCode"));
    let mut discount_codes = Vec::new();
    let mut order_discount = 0;
    if let Some(code) = discount.get("itemPercentageDiscountCode").and_then(Value::as_object) {
        let percentage = code.get("percentage").and_then(Value::as_f64).unwrap_or(0.0);
        for line in &mut lines {
            let total = line.unit_price * i64::from(line.quantity);
            line.discount = ((total as f64) * percentage / 100.0).round() as i64;
        }
        discount_codes.extend(code.get("code").and_then(Value::as_str).map(str::to_string));
    } else if let Some(code) = discount.get("itemFixedDiscountCode").and_then(Value::as_object) {
        order_discount = model::cents(code.get("amountSet").and_then(|set| set.get("shopMoney")))
            .min(subtotal);
        discount_codes.extend(code.get("code").and_then(Value::as_str).map(str::to_string));
    } else if let Some(code) = discount.get("freeShippingDiscountCode").and_then(Value::as_object) {
        if let Some(shipping) = &mut shipping {
            shipping.discounted = 0;
        }
        discount_codes.extend(code.get("code").and_then(Value::as_str).map(str::to_string));
    }
    let tax = tax_lines_cents(input.get("taxLines"), subtotal) + line_tax;

    // The money.
    let transactions = array(&input, "transactions");
    let financial = input
        .get("financialStatus")
        .and_then(Value::as_str)
        .map(FinancialStatus::parse);
    let fulfillment = match input.get("fulfillmentStatus").and_then(Value::as_str) {
        Some("FULFILLED") => FulfillmentState::Fulfilled,
        Some("PARTIAL") => FulfillmentState::Partial,
        Some("RESTOCKED") => {
            mutation.note("orderCreate: lsf does not simulate `fulfillmentStatus: RESTOCKED`: the order is unfulfilled");
            FulfillmentState::Unfulfilled
        }
        _ => FulfillmentState::Unfulfilled,
    };
    let state = mutation.state_mut();
    let now = state.now();
    let id = state.new_gid("Order");
    let number = model::next_order_number(state);
    let name = input
        .get("name")
        .and_then(Value::as_str)
        .map_or_else(|| format!("#{number}"), str::to_string);
    let address = |field: &str, kind: &str| {
        input
            .get(field)
            .and_then(|address| model::mailing_address(address, &address_gid(&id, kind)))
    };
    let customer_record = customer.as_ref().and_then(|customer| state.get(customer));
    let draft = OrderDraft {
        id: id.clone(),
        name,
        number,
        created_at: processed.unwrap_or(now),
        customer: customer.clone(),
        email: input
            .get("email")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| customer_record.and_then(|record| text(record, "email")).map(str::to_string)),
        phone: input.get("phone").and_then(Value::as_str).map(str::to_string),
        accepts_marketing: input
            .get("buyerAcceptsMarketing")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        tax_exempt: customer_record
            .and_then(|record| record.get("taxExempt"))
            .and_then(Value::as_bool)
            .unwrap_or(false),
        lines,
        shipping_address: address("shippingAddress", "shipping"),
        billing_address: address("billingAddress", "billing"),
        shipping,
        tax,
        order_discount,
        discount_codes,
        note: input.get("note").and_then(Value::as_str).map(str::to_string),
        attributes: input
            .get("customAttributes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|attribute| {
                Some((
                    attribute.get("key")?.as_str()?.to_string(),
                    attribute.get("value")?.as_str()?.to_string(),
                ))
            })
            .collect(),
        financial: if transactions.is_empty() {
            financial.unwrap_or(FinancialStatus::Pending)
        } else {
            FinancialStatus::Pending
        },
        fulfillment,
        cancelled: false,
        paid: None,
        gateway: Some("manual".to_string()),
        source_name: input
            .get("sourceName")
            .and_then(Value::as_str)
            .map(str::to_string),
        returns: Vec::new(),
        location: location.clone(),
    };
    let order = super::insert_order(state, &draft);
    let test = input.get("test").and_then(Value::as_bool) == Some(true);
    let now_text = value::date_time(now);
    if let Some(record) = state.get_mut(&order) {
        record.insert("createdAt".into(), json!(now_text));
        record.insert("updatedAt".into(), json!(now_text));
        record.insert("test".into(), json!(test));
        record.insert("tags".into(), json!(tags_of(input.get("tags"))));
        for field in ["poNumber", "sourceIdentifier"] {
            record.insert(field.into(), input.get(field).cloned().unwrap_or(Value::Null));
        }
        if let Some(closed) = input.get("closedAt").and_then(Value::as_str) {
            record.insert("closed".into(), json!(true));
            record.insert("closedAt".into(), json!(closed));
        } else {
            record.insert("closed".into(), json!(false));
        }
        if !draft.discount_codes.is_empty() {
            record.insert(
                "discountCodes".into(),
                json!(draft.discount_codes),
            );
        }
    }
    // The seed's records of a fulfilled order, with the tracking of `fulfillment`.
    let fulfillments = state
        .get(&order)
        .map(|record| gids(record.get("fulfillments")))
        .unwrap_or_default();
    let tracking_number = fulfillment_input
        .get("trackingNumber")
        .and_then(Value::as_str)
        .map(str::to_string);
    for fulfillment in &fulfillments {
        if let Some(record) = state.get_mut(fulfillment) {
            record.insert("createdAt".into(), json!(now_text));
            record.insert("updatedAt".into(), json!(now_text));
            if tracking_number.is_some() {
                record.insert(
                    "trackingInfo".into(),
                    json!([{
                        "company": fulfillment_input.get("trackingCompany"),
                        "number": tracking_number,
                        "url": null,
                    }]),
                );
            }
        }
    }
    state.emit("orders/create", &order);
    let made = state
        .get(&order)
        .map(|record| gids(record.get("transactions")))
        .unwrap_or_default();
    for transaction in made {
        state.emit("order_transactions/create", &transaction);
    }
    for transaction in &transactions {
        let amount = model::cents(transaction.get("amountSet").and_then(|set| set.get("shopMoney")));
        let kind = transaction
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("SALE");
        let status = transaction
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("SUCCESS");
        let gateway = transaction.get("gateway").and_then(Value::as_str);
        let created = model::add_transaction(
            state,
            &order,
            &NewTransaction {
                kind,
                amount,
                gateway,
                parent: None,
                manual: gateway.is_none_or(|gateway| gateway == "manual"),
                test: transaction.get("test").and_then(Value::as_bool) == Some(true) || test,
            },
        );
        if let Some(record) = state.get_mut(&created) {
            record.insert("status".into(), json!(status));
            for (from, to) in [("authorizationCode", "authorizationCode"), ("processedAt", "processedAt")] {
                if let Some(value) = transaction.get(from).filter(|value| !value.is_null()) {
                    record.insert(to.into(), value.clone());
                }
            }
        }
    }
    if !transactions.is_empty() {
        model::settle_financial_status(state, &order);
        if let Some(status) = financial
            && let Some(record) = state.get_mut(&order)
        {
            record.insert("displayFinancialStatus".into(), json!(status.graphql()));
        }
    }
    // The stock the order claims.
    if behaviour != "BYPASS" {
        let lines = state
            .get(&order)
            .map(|record| gids(record.get("lineItems")))
            .unwrap_or_default();
        for line in lines {
            let Some(item_record) = state.get(&line).cloned() else {
                continue;
            };
            let Some(item) = model::inventory_item_of(state, &item_record) else {
                continue;
            };
            let quantity = int(&item_record, "quantity");
            let shipped = quantity - int(&item_record, "unfulfilledQuantity");
            model::commit_inventory(state, &item, location.as_deref(), quantity);
            model::ship_inventory(state, &item, location.as_deref(), shipped);
        }
    }
    model::refresh_order(state, &order);
    if state
        .get(&order)
        .and_then(|record| text(record, "displayFinancialStatus"))
        == Some("PAID")
    {
        state.emit("orders/paid", &order);
    }
    Ok(payload(json!({"order": order}), &[]))
}

/// The tax of `taxLines` (`OrderCreateTaxLineInput`): their `priceSet`, else their `rate` of
/// `base`.
fn tax_lines_cents(lines: Option<&Value>, base: i64) -> i64 {
    lines
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|line| match line.get("priceSet").and_then(|set| set.get("shopMoney")) {
            Some(money) => model::cents(Some(money)),
            None => {
                let rate = line
                    .get("rate")
                    .and_then(|rate| match rate {
                        Value::String(text) => text.parse::<f64>().ok(),
                        other => other.as_f64(),
                    })
                    .unwrap_or(0.0);
                ((base as f64) * rate).round() as i64
            }
        })
        .sum()
}
