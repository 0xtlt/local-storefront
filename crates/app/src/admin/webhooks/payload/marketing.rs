//! The payloads of the customer marketing topics: segments and the customers who join or leave
//! them, customer groups (customer saved searches), merges, purchasing summaries, the WhatsApp
//! consent, the customer account settings, and the Admin API's audit events.
//!
//! Each follows the example of its topic in shopify.dev's webhook reference of 2026-10, which
//! writes some of them in GraphQL's shape rather than REST's: `segments/*` (`creationDate`,
//! `lastEditDate`), `customer.joined_segment` (GIDs), `customers/purchasing_summary`
//! (`customerId`, `amountSpent`).

use serde_json::{Map, Value, json};

use super::{Lookup, field, id, lower, millis, numeric, price_or_zero, text, text_or, time};

/// `segments/create` and `segments/update`.
pub(super) fn segment(record: &Map<String, Value>) -> Value {
    json!({
        "id": id(record),
        "name": text(record, "name"),
        "query": text(record, "query"),
        "creationDate": millis(field(record, "creationDate")),
        "lastEditDate": millis(field(record, "lastEditDate")),
    })
}

/// `customer.joined_segment` and `customer.left_segment`: the GIDs of the shop, the customer and
/// the segment.
pub(super) fn segment_membership(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let shop = match field(record, "shopId") {
        Value::String(shop) => json!(shop),
        _ => lookup
            .shop()
            .and_then(|shop| shop.get("id").cloned())
            .unwrap_or(Value::Null),
    };
    json!({
        "shop_id": shop,
        "customer_id": text(record, "customerId"),
        "segment_id": text(record, "segmentId"),
    })
}

/// `customer_groups/create` and `customer_groups/update`: a customer saved search.
pub(super) fn customer_group(record: &Map<String, Value>) -> Value {
    json!({
        "id": id(record),
        "name": text(record, "name"),
        "created_at": time(field(record, "lsfCreatedAt")),
        "updated_at": time(field(record, "lsfUpdatedAt")),
        "query": text(record, "query"),
    })
}

/// `customers/merge`: the customer kept, the one deleted, the job, and how it went.
pub(super) fn customer_merge(record: &Map<String, Value>) -> Value {
    let status = match field(record, "status") {
        Value::String(status) => json!(status.to_ascii_lowercase()),
        _ => json!("completed"),
    };
    let errors: Vec<Value> = field(record, "customerMergeErrors")
        .as_array()
        .into_iter()
        .flatten()
        .map(|error| {
            json!({
                "customer_ids": error.get("customerIds").cloned().unwrap_or(json!([])),
                "field": error.get("field").map(lower).unwrap_or(Value::Null),
                "message": error.get("message").cloned().unwrap_or(Value::Null),
            })
        })
        .collect();
    json!({
        "admin_graphql_api_customer_kept_id": text(record, "customerKept"),
        "admin_graphql_api_customer_deleted_id": text(record, "customerDeleted"),
        "admin_graphql_api_job_id": text(record, "jobId"),
        "status": status,
        "errors": errors,
    })
}

/// `customers/purchasing_summary`: a customer's sales, as GraphQL writes them.
pub(super) fn purchasing_summary(record: &Map<String, Value>) -> Value {
    let orders = match field(record, "numberOfOrders") {
        Value::String(number) => number.parse::<i64>().map_or(Value::Null, Value::from),
        Value::Number(number) => Value::Number(number.clone()),
        _ => json!(0),
    };
    let spent = field(record, "amountSpent");
    json!({
        "customerId": text(record, "customerId"),
        "numberOfOrders": orders,
        "amountSpent": {
            "amount": price_or_zero(spent.get("amount").unwrap_or(&Value::Null)),
            "currencyCode": spent.get("currencyCode").cloned().unwrap_or(Value::Null),
        },
        "lastOrderId": text(record, "lastOrderId"),
        "occurredAt": millis(field(record, "occurredAt")),
    })
}

/// `customers_whats_app_marketing_consent/update`: the customer's id, phone and WhatsApp
/// consent, enums in lowercase (`collected_from: "other"`, as the example writes it).
pub(super) fn whatsapp_consent_update(record: &Map<String, Value>) -> Value {
    let consent = field(record, "whatsAppMarketingConsent");
    let empty = Map::new();
    let consent = consent.as_object().unwrap_or(&empty);
    let state = match field(consent, "state") {
        Value::String(state) if state != "NEVER_SUBSCRIBED" => json!(state.to_ascii_lowercase()),
        _ => Value::Null,
    };
    let collected = match field(consent, "collectedFrom") {
        Value::String(from) => json!(from.to_ascii_lowercase()),
        _ => json!("other"),
    };
    json!({
        "customer_id": id(record),
        "phone_number": text(record, "phone"),
        "whats_app_marketing_consent": {
            "state": state,
            "opt_in_level": lower(field(consent, "optInLevel")),
            "updated_at": time(field(consent, "updatedAt")),
            "collected_from": collected,
        },
    })
}

/// `customer_account_settings/update`, from the shop: its `customerAccountsV2` (new customer
/// accounts, which lsf serves), else classic accounts and the shop's setting (`DISABLED`,
/// `OPTIONAL`, `REQUIRED`).
pub(super) fn customer_account_settings(record: &Map<String, Value>) -> Value {
    if let Value::Object(v2) = field(record, "customerAccountsV2") {
        return json!({
            "url": text(v2, "url"),
            "customer_accounts_version": lower(field(v2, "customerAccountsVersion")),
            "login_required_at_checkout": field(v2, "loginRequiredAtCheckout").as_bool().unwrap_or(false),
            "login_links_visible_on_storefront_and_checkout": field(v2, "loginLinksVisibleOnStorefrontAndCheckout").as_bool().unwrap_or(true),
        });
    }
    let setting = field(record, "customerAccounts")
        .as_str()
        .unwrap_or("OPTIONAL");
    json!({
        "url": null,
        "customer_accounts_version": "classic",
        "login_required_at_checkout": setting == "REQUIRED",
        "login_links_visible_on_storefront_and_checkout": setting != "DISABLED",
    })
}

/// `audit_events/admin_api_activity`: one Admin API request, from what the server recorded of it
/// (`domains::audit`).
pub(super) fn admin_api_activity(record: &Map<String, Value>) -> Value {
    let get = |name: &str| field(record, name).clone();
    json!({
        "events": [{
            "time": get("time"),
            "event": {
                "context": {
                    "context_type": "network_request",
                    "context_identifier": get("requestId"),
                    "context_metadata": {
                        "client_ip": get("clientIp"),
                        "content_type": "json",
                        "response_time_ms": get("responseTimeMs"),
                        "user_agent": get("userAgent"),
                        "api_request_failed": get("failed"),
                        "response_code": get("responseCode"),
                    },
                },
                "action": "api_request",
                "actor": {
                    "actor_type": "app",
                    "actor_identifier": numeric(field(record, "app")),
                    "actor_metadata": {
                        "app_name": get("appName"),
                        "api_version_requested": get("versionRequested"),
                        "api_version_served": get("versionServed"),
                        "ecosystem_category": text_or(record, "ecosystemCategory", "public"),
                    },
                    "on_behalf_of": match field(record, "userEmail") {
                        Value::String(email) => json!({"user_email": email}),
                        _ => Value::Null,
                    },
                },
                "subject": {
                    "subject_type": "shop",
                    "subject_identifier": get("shop"),
                    "subject_metadata": {},
                },
                "timestamp": get("timestamp"),
                "additional_metadata": {
                    "request_type": "GraphQL",
                    "mutation_names": get("mutationNames"),
                    "query": get("query"),
                    "variables": get("variables"),
                    "error_codes": get("errorCodes"),
                },
            },
        }],
    })
}
