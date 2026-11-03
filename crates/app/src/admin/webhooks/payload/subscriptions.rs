//! The payloads of the Subscriptions API: contracts, billing cycles and their edits, billing
//! attempts, customer payment methods.
//!
//! The keys and their order are those of the examples of Shopify's webhook reference for
//! 2026-10, except the billing attempts', which the reference has no example of: theirs are those
//! of the payloads of Shopify's subscriptions reference app (`Shopify/subscriptions-reference-app`
//! on GitHub, `app/types/webhooks.ts` and the fixtures of its tests), the same resource for the
//! three topics.

use chrono::DateTime;
use chrono_tz::Tz;
use serde_json::{Map, Value, json};

use super::{Lookup, boolean, field, id, integer, lower, numeric, text};

/// `subscription_contracts/*`: the contract, its policies, its customer and origin order.
pub(super) fn contract(record: &Map<String, Value>) -> Value {
    let billing = field(record, "billingPolicy");
    let delivery = field(record, "deliveryPolicy");
    let customer = field(record, "customer");
    let origin = field(record, "originOrder");
    json!({
        "admin_graphql_api_id": text(record, "id"),
        "id": id(record),
        "billing_policy": {
            "interval": lower(billing.get("interval").unwrap_or(&Value::Null)),
            "interval_count": billing.get("intervalCount").cloned().unwrap_or(Value::Null),
            "min_cycles": billing.get("minCycles").cloned().unwrap_or(Value::Null),
            "max_cycles": billing.get("maxCycles").cloned().unwrap_or(Value::Null),
        },
        "currency_code": text(record, "currencyCode"),
        "customer_id": numeric(customer),
        "admin_graphql_api_customer_id": customer,
        "delivery_policy": {
            "interval": lower(delivery.get("interval").unwrap_or(&Value::Null)),
            "interval_count": delivery.get("intervalCount").cloned().unwrap_or(Value::Null),
        },
        "status": lower(field(record, "status")),
        "admin_graphql_api_origin_order_id": origin,
        "origin_order_id": numeric(origin),
        "revision_id": match field(record, "revisionId") {
            Value::Number(number) => json!(number.to_string()),
            other => other.clone(),
        },
    })
}

/// A date of a cycle as the cycle webhooks write it: in the shop's time zone, with its offset
/// (`2022-10-01T00:00:00-04:00`).
fn local(value: &Value, zone: Option<Tz>) -> Value {
    let Some(at) = value
        .as_str()
        .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
    else {
        return value.clone();
    };
    let formatted = match zone {
        Some(zone) => at
            .with_timezone(&zone)
            .format("%Y-%m-%dT%H:%M:%S%:z")
            .to_string(),
        None => at.format("%Y-%m-%dT%H:%M:%S%:z").to_string(),
    };
    json!(formatted)
}

/// `subscription_billing_cycles/skip`, `/unskip` and `subscription_billing_cycle_edits/*`: the
/// cycle, from the snapshot its event keeps. `contract_edit` is `null`, as in every example:
/// Shopify does not show what it holds for an edited contract.
pub(super) fn billing_cycle(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let zone = lookup
        .shop()
        .and_then(|shop| {
            shop.get("ianaTimezone")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .and_then(|zone| zone.parse::<Tz>().ok());
    json!({
        "subscription_contract_id": numeric(field(record, "sourceContract")),
        "cycle_start_at": local(field(record, "cycleStartAt"), zone),
        "cycle_end_at": local(field(record, "cycleEndAt"), zone),
        "cycle_index": integer(record, "cycleIndex", Value::Null),
        "contract_edit": null,
        "billing_attempt_expected_date": local(field(record, "billingAttemptExpectedDate"), zone),
        "skipped": boolean(record, "skipped", false),
        "edited": boolean(record, "edited", false),
    })
}

/// `subscription_billing_attempts/success`, `/failure`, `/challenged`: the attempt, its order
/// and contract, its error (the code in lowercase, as REST writes enums).
pub(super) fn billing_attempt(record: &Map<String, Value>) -> Value {
    let order = field(record, "order");
    let contract = field(record, "subscriptionContract");
    json!({
        "id": id(record),
        "admin_graphql_api_id": text(record, "id"),
        "idempotency_key": text(record, "idempotencyKey"),
        "order_id": numeric(order),
        "admin_graphql_api_order_id": order,
        "subscription_contract_id": numeric(contract),
        "admin_graphql_api_subscription_contract_id": contract,
        "ready": boolean(record, "ready", false),
        "error_message": text(record, "errorMessage"),
        "error_code": lower(field(record, "errorCode")),
    })
}

/// `customer_payment_methods/*`: the method's token and customer, its instrument. A card's
/// `payment_instrument` has the example's keys; another instrument's (a PayPal agreement), which
/// Shopify shows no example of, has its GraphQL fields in snake_case.
pub(super) fn payment_method(record: &Map<String, Value>) -> Value {
    let gid = text(record, "id");
    let token = gid
        .as_str()
        .and_then(|gid| gid.rsplit('/').next())
        .map_or(Value::Null, |token| json!(token));
    let customer = field(record, "customer");
    let instrument = record
        .get("instrument")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let kind = text(&instrument, "__typename");
    let payment_instrument = match kind.as_str() {
        Some("CustomerPaypalBillingAgreement") => json!({
            "paypal_account_email": text(&instrument, "paypalAccountEmail"),
            "inactive": boolean(&instrument, "inactive", false),
        }),
        _ => json!({
            "last_digits": text(&instrument, "lastDigits"),
            "month": integer(&instrument, "expiryMonth", Value::Null),
            "year": integer(&instrument, "expiryYear", Value::Null),
            "name": text(&instrument, "name"),
            "brand": text(&instrument, "brand"),
        }),
    };
    json!({
        "admin_graphql_api_id": gid,
        "token": token,
        "customer_id": numeric(customer),
        "admin_graphql_api_customer_id": customer,
        "instrument_type": kind,
        "payment_instrument": payment_instrument,
        "resource_type": "Subscriptions",
    })
}
