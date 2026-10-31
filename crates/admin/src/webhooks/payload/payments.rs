//! The payloads of the payments of orders: payment terms and their schedules, tender
//! transactions, in the key order of the examples of the reference of 2026-10.

use serde_json::{Map, Value, json};

use super::{
    Lookup, field, first, id, lower, money_amount, numeric, records, ref_gid, ref_id, resolve,
    text, time,
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
        "amount": or_zero(money_amount(amount)),
        "balance_due": or_zero(money_amount(field(schedule, "balanceDue"))),
        "completed_at": time(field(schedule, "completedAt")),
        "created_at": created(schedule),
        "currency": currency,
        "due_at": time(field(schedule, "dueAt")),
        "id": id(schedule),
        "issued_at": time(field(schedule, "issuedAt")),
        "payment_terms_id": ref_id(field(schedule, "paymentTerms")),
        "presentment_currency": currency,
        "total_balance": or_zero(money_amount(field(schedule, "totalBalance"))),
        "total_price": or_zero(money_amount(amount)),
        "updated_at": updated(schedule),
        "admin_graphql_api_id": text(schedule, "id"),
    })
}

fn or_zero(price: Value) -> Value {
    match price {
        Value::Null => json!("0.00"),
        price => price,
    }
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

/// `payment_terms/delete`: the id alone.
pub(super) fn payment_terms_deleted(terms: &Map<String, Value>) -> Value {
    json!({"id": id(terms)})
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
        "payment_method": match text(tender, "paymentMethod") {
            Value::Null => json!("unknown"),
            method => method,
        },
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
