//! The payloads of the rest of the orders family: payment terms and their schedules, tender
//! transactions, risk assessments, Shopify Protect, requested order edits, returns' updates,
//! reverse deliveries and dispositions, in the key order of the examples of the reference of
//! 2026-10.

use serde_json::{Map, Value, json};

use super::{
    Lookup, field, first, id, items, lower, money_amount, numeric, price_or_zero, records, ref_gid,
    ref_id, resolve, text, text_or, time,
};

/// The dates of payment terms and schedules, which their GraphQL types do not have: lsf keeps
/// them under these keys (`domains::orders::payments`), a sample under the GraphQL-like ones.
fn created(record: &Map<String, Value>) -> Value {
    time(first(record, &["lsfCreatedAt", "createdAt"]))
}

fn updated(record: &Map<String, Value>) -> Value {
    time(first(record, &["lsfUpdatedAt", "updatedAt"]))
}

/// `payment_schedules/due`, and each schedule of `payment_terms/…`: a `PaymentSchedule`.
pub(super) fn payment_schedule(schedule: &Map<String, Value>) -> Value {
    let amount = field(schedule, "amount");
    let currency = amount.get("currencyCode").cloned().unwrap_or(Value::Null);
    json!({
        "amount": price_or_zero(&money_amount(amount)),
        "balance_due": price_or_zero(&money_amount(field(schedule, "balanceDue"))),
        "completed_at": time(field(schedule, "completedAt")),
        "created_at": created(schedule),
        "currency": currency,
        "due_at": time(field(schedule, "dueAt")),
        "id": id(schedule),
        "issued_at": time(field(schedule, "issuedAt")),
        "payment_terms_id": ref_id(field(schedule, "paymentTerms")),
        "presentment_currency": currency,
        "total_balance": price_or_zero(&money_amount(field(schedule, "totalBalance"))),
        "total_price": price_or_zero(&money_amount(amount)),
        "updated_at": updated(schedule),
        "admin_graphql_api_id": text(schedule, "id"),
    })
}

/// `payment_terms/create` and `…/update`: `PaymentTerms` with their schedules. REST names the
/// terms by the class Shopify keeps them in (`PaymentsPaymentFlexibilityPaymentTermsPaymentTerms`),
/// as the examples' `admin_graphql_api_id` shows, where GraphQL names them `PaymentTerms`.
pub(super) fn payment_terms(terms: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let terms_id = id(terms);
    json!({
        "id": terms_id,
        "payment_terms_name": text(terms, "paymentTermsName"),
        "payment_terms_type": lower(field(terms, "paymentTermsType")),
        "due_in_days": field(terms, "dueInDays"),
        "created_at": created(terms),
        "updated_at": updated(terms),
        "payment_schedules": records(field(terms, "paymentSchedules"), lookup)
            .iter()
            .map(payment_schedule)
            .collect::<Vec<_>>(),
        "admin_graphql_api_id": match terms_id {
            Value::Null => Value::Null,
            id => json!(format!("gid://shopify/PaymentsPaymentFlexibilityPaymentTermsPaymentTerms/{id}")),
        },
    })
}

/// `tender_transactions/create`: a `TenderTransaction`. Its amount is negative for a refund;
/// REST names its payment method `unknown` when the record has none.
pub(super) fn tender_transaction(tender: &Map<String, Value>) -> Value {
    let amount = field(tender, "amount");
    let details = match field(tender, "transactionDetails") {
        Value::Object(details) => json!({
            "credit_card_number": text(details, "creditCardNumber"),
            "credit_card_company": text(details, "creditCardCompany"),
        }),
        _ => Value::Null,
    };
    json!({
        "id": id(tender),
        "order_id": ref_id(field(tender, "order")),
        "amount": money_amount(amount),
        "currency": amount.get("currencyCode").cloned().unwrap_or(Value::Null),
        "user_id": ref_id(field(tender, "user")),
        "test": field(tender, "test").as_bool().unwrap_or(false),
        "processed_at": time(field(tender, "processedAt")),
        "remote_reference": text(tender, "remoteReference"),
        "payment_details": details,
        "payment_method": text_or(tender, "paymentMethod", "unknown"),
    })
}

/// `orders/risk_assessment_changed`: the newest risk assessment of an order (`provider_id` and
/// `provider_title` `null` when Shopify made it, as the topic's description says).
pub(super) fn risk_assessment(order: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let assessment = field(order, "lsfRiskAssessments")
        .as_array()
        .and_then(|all| all.last())
        .cloned()
        .unwrap_or(Value::Null);
    let provider = assessment.get("provider").cloned().unwrap_or(Value::Null);
    let title = resolve(&provider, lookup).map_or(Value::Null, |app| text(&app, "title"));
    json!({
        "provider_id": numeric(&provider),
        "provider_title": title,
        "risk_level": lower(assessment.get("riskLevel").unwrap_or(&Value::Null)),
        "created_at": time(assessment.get("createdAt").unwrap_or(&Value::Null)),
        "order_id": id(order),
        "admin_graphql_api_order_id": ref_gid(field(order, "id")),
    })
}

/// `orders/shopify_protect_eligibility_changed`: an order's Shopify Protect summary.
pub(super) fn protect_eligibility(order: &Map<String, Value>) -> Value {
    let protect = field(order, "shopifyProtect");
    json!({
        "order_id": id(order),
        "status": lower(protect.get("status").unwrap_or(&Value::Null)),
        "eligibility": {
            "status": lower(protect.pointer("/eligibility/status").unwrap_or(&Value::Null)),
        },
    })
}

/// `requested_order_edit/created`, `…/declined`, `…/resolved`: a `RequestedOrderEdit` with its
/// removals.
pub(super) fn requested_order_edit(edit: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let removals: Vec<Value> = records(
        field(edit, "lineItems")
            .get("removals")
            .unwrap_or(&Value::Null),
        lookup,
    )
    .iter()
    .map(|item| {
        json!({
            "id": id(item),
            "admin_graphql_api_id": text(item, "id"),
            "line_item_id": ref_id(field(item, "lineItem")),
            "quantity": field(item, "quantity"),
            "resolved_quantity": field(item, "resolvedQuantity"),
        })
    })
    .collect();
    json!({
        "id": id(edit),
        "admin_graphql_api_id": text(edit, "id"),
        "status": lower(field(edit, "status")),
        "order_id": ref_id(field(edit, "order")),
        "requested_at": time(field(edit, "requestedAt")),
        "line_items": {"removals": removals},
    })
}

/// `{id, admin_graphql_api_id}` of a reference.
fn reference(value: &Value) -> Value {
    match ref_gid(value) {
        Value::Null => Value::Null,
        gid => json!({"id": ref_id(value), "admin_graphql_api_id": gid}),
    }
}

/// `returns/update`: the return line items a change took off a return (`lsfUpdateRemovals`,
/// `[{id, delta}]`). lsf's returns have no fees a change updates: their lists are empty.
pub(super) fn return_update(record: &Map<String, Value>) -> Value {
    let removals: Vec<Value> = items(field(record, "lsfUpdateRemovals"))
        .iter()
        .map(|removal| {
            json!({
                "admin_graphql_api_id": ref_gid(removal.get("id").unwrap_or(&Value::Null)),
                "delta": removal.get("delta"),
            })
        })
        .collect();
    json!({
        "admin_graphql_api_id": text(record, "id"),
        "return_line_items": {"removals": removals},
        "restocking_fees": {"updates": [], "removals": []},
        "return_shipping_fees": {"updates": [], "removals": []},
    })
}

/// `reverse_deliveries/attach_deliverable`: a `ReverseDelivery`, its return and its tracking
/// and label.
pub(super) fn reverse_delivery(delivery: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let reverse = resolve(field(delivery, "reverseFulfillmentOrder"), lookup);
    let return_ref = reverse.as_ref().map_or(Value::Null, |reverse| {
        reference(field(reverse, "lsfReturn"))
    });
    let deliverable = field(delivery, "deliverable");
    let tracking = deliverable.get("tracking").cloned().unwrap_or(Value::Null);
    let label = deliverable.get("label").cloned().unwrap_or(Value::Null);
    json!({
        "id": id(delivery),
        "admin_graphql_api_id": text(delivery, "id"),
        "return": return_ref,
        "shipping_deliverable": {
            "tracking": {
                "carrier_name": tracking.get("carrierName").cloned().unwrap_or(Value::Null),
                "tracking_number": tracking.get("number").cloned().unwrap_or(Value::Null),
                "tracking_url": tracking.get("url").cloned().unwrap_or(Value::Null),
            },
            "label": {
                "public_file_url": label.get("publicFileUrl").cloned().unwrap_or(Value::Null),
                "created_at": time(label.get("createdAt").unwrap_or(&Value::Null)),
            },
        },
    })
}

/// `reverse_fulfillment_orders/dispose`: a `ReverseFulfillmentOrder` and the dispositions its
/// last change made (`lsfLastDispositions`).
pub(super) fn reverse_dispositions(reverse: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let dispositions: Vec<Value> = records(field(reverse, "lsfLastDispositions"), lookup)
        .iter()
        .map(|disposition| {
            json!({
                "reverse_fulfillment_order_line_item": reference(field(disposition, "lsfLineItem")),
                "reverse_delivery_line_item": Value::Null,
                "type": lower(field(disposition, "type")),
                "location": reference(field(disposition, "location")),
                "quantity": field(disposition, "quantity"),
            })
        })
        .collect();
    let total = dispositions.len();
    json!({
        "id": id(reverse),
        "admin_graphql_api_id": text(reverse, "id"),
        "dispositions": dispositions,
        "total_dispositions": total,
    })
}

/// `disputes/create` and `disputes/update`: a `ShopifyPaymentsDispute`.
pub(super) fn dispute(dispute: &Map<String, Value>) -> Value {
    let amount = field(dispute, "amount");
    let reason = field(dispute, "reasonDetails");
    json!({
        "id": id(dispute),
        "order_id": ref_id(field(dispute, "order")),
        "type": lower(field(dispute, "type")),
        "amount": money_amount(amount),
        "currency": amount.get("currencyCode").cloned().unwrap_or(Value::Null),
        "reason": lower(reason.get("reason").unwrap_or(&Value::Null)),
        "network_reason_code": reason.get("networkReasonCode").cloned().unwrap_or(Value::Null),
        "status": lower(field(dispute, "status")),
        "evidence_due_by": time(field(dispute, "evidenceDueBy")),
        "evidence_sent_on": time(field(dispute, "evidenceSentOn")),
        "finalized_on": time(field(dispute, "finalizedOn")),
        "initiated_at": time(field(dispute, "initiatedAt")),
    })
}

/// `tax_services/create` and `tax_services/update`: the app as the shop's tax service.
pub(super) fn tax_service(record: &Map<String, Value>) -> Value {
    let service = field(record, "lsfService");
    json!({
        "id": service.get("id").cloned().unwrap_or(Value::Null),
        "name": service.get("name").cloned().unwrap_or(Value::Null),
        "url": service.get("url").cloned().unwrap_or(Value::Null),
        "active": service.get("active").and_then(Value::as_bool).unwrap_or(false),
    })
}
