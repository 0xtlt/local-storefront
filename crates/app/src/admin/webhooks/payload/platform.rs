//! The payloads of themes, rollouts, and the finance topics.
//!
//! - `themes/create`, `themes/update`, `themes/publish`: the REST `Theme` (`id`, `name`, dates,
//!   `role` in lowercase, `theme_store_id`, `previewable`, `processing`), with
//!   `admin_graphql_api_id` as the example writes it, `gid://shopify/Theme/<id>` (REST's
//!   resource name, not GraphQL's `OnlineStoreTheme`). `themes/delete`: the `id`.
//! - `rollouts/create`, `rollouts/update`: the `Rollout` (name, status, traffic allocation,
//!   schedule, dates); `rollouts/delete`: its id and when it was deleted; the
//!   `rollouts/resource_change_*` topics: the rollout, the treatment, the change and the
//!   resource it targets, and when (the event's `lsfChange` on the rollout, else its first
//!   treatment's first change). No mutation of 2026-10 makes rollouts (the admin does): these
//!   are built from a `Rollout` of the shop, or from the sample of `lsf app webhook trigger`.
//! - `finance_app_staff_member/*`: Shopify's reference documents no payload (`{}`, its resource
//!   the REST `User`): lsf sends the staff member as the REST `User` writes one (`id`, names,
//!   `email`, `phone`, `account_owner`, `locale`, `user_type`, `admin_graphql_api_id`).
//!   `finance_kyc_information/update`: the same; lsf sends the `FinanceKycInformation` in
//!   snake_case.

use serde_json::{Map, Value, json};

use super::{boolean, field, id, lower, text, time};

/// The `Theme` of `themes/*`.
pub(super) fn theme(record: &Map<String, Value>) -> Value {
    let number = id(record);
    json!({
        "id": number,
        "name": text(record, "name"),
        "created_at": time(field(record, "createdAt")),
        "updated_at": time(field(record, "updatedAt")),
        "role": lower(field(record, "role")),
        "theme_store_id": field(record, "themeStoreId"),
        "previewable": true,
        "processing": boolean(record, "processing", false),
        "admin_graphql_api_id": match &number {
            Value::Number(n) => json!(format!("gid://shopify/Theme/{n}")),
            _ => Value::Null,
        },
    })
}

/// `rollouts/create`, `rollouts/update`.
pub(super) fn rollout(record: &Map<String, Value>) -> Value {
    let schedule = field(record, "schedule");
    json!({
        "admin_graphql_api_id": text(record, "id"),
        "name": text(record, "name"),
        "status": text(record, "status"),
        "traffic_allocation": field(record, "trafficAllocation"),
        "effective_traffic_allocation": field(record, "effectiveTrafficAllocation")
            .as_f64()
            .map_or(Value::Null, |share| json!(share)),
        "schedule": if schedule.is_object() {
            json!({
                "activate_at": time(&schedule["activateAt"]),
                "conclude_at": time(&schedule["concludeAt"]),
            })
        } else {
            Value::Null
        },
        "started_at": time(field(record, "startedAt")),
        "concluded_at": time(field(record, "concludedAt")),
        "archived_at": time(field(record, "archivedAt")),
        "created_at": time(field(record, "createdAt")),
        "updated_at": time(field(record, "updatedAt")),
    })
}

/// `rollouts/delete`.
pub(super) fn rollout_deleted(record: &Map<String, Value>) -> Value {
    json!({
        "admin_graphql_api_id": text(record, "id"),
        "deleted_at": time(field(record, super::DELETED_AT)),
    })
}

/// The field of a rollout that says which change a `rollouts/resource_change_*` event is about:
/// `{treatment, change, resource, occurredAt}`.
const ROLLOUT_CHANGE: &str = "lsfChange";

/// `rollouts/resource_change_added`, `…_removed`, `…_updated`.
pub(super) fn rollout_change(record: &Map<String, Value>) -> Value {
    let change = match field(record, ROLLOUT_CHANGE) {
        Value::Object(change) => Value::Object(change.clone()),
        _ => {
            // The first treatment's first change.
            let treatment = field(record, "treatments")
                .as_array()
                .and_then(|treatments| treatments.first())
                .cloned()
                .unwrap_or(Value::Null);
            let first = treatment["changes"]
                .as_array()
                .and_then(|changes| changes.first())
                .cloned()
                .unwrap_or(Value::Null);
            json!({
                "treatment": treatment["id"],
                "change": first["id"],
                "resource": first["resourceId"],
                "occurredAt": record.get("updatedAt").cloned().unwrap_or(Value::Null),
            })
        }
    };
    json!({
        "admin_graphql_api_id": text(record, "id"),
        "treatment_id": change["treatment"],
        "change_id": change["change"],
        "resource_id": change["resource"],
        "occurred_at": change["occurredAt"],
    })
}

/// `finance_app_staff_member/*`: the staff member, as the REST `User` writes it.
pub(super) fn finance_staff_member(record: &Map<String, Value>) -> Value {
    json!({
        "id": id(record),
        "first_name": text(record, "firstName"),
        "last_name": text(record, "lastName"),
        "email": text(record, "email"),
        "phone": text(record, "phone"),
        "account_owner": boolean(record, "isShopOwner", false),
        "locale": text(record, "locale"),
        "user_type": if field(record, "accountType") == "COLLABORATOR" { "collaborator" } else { "regular" },
        "admin_graphql_api_id": text(record, "id"),
    })
}

/// `finance_kyc_information/update`: the `FinanceKycInformation`, in snake_case.
pub(super) fn finance_kyc_information(record: &Map<String, Value>) -> Value {
    let address = field(record, "businessAddress");
    let owner = field(record, "shopOwner");
    let industry = field(record, "industry");
    let tax = field(record, "taxIdentification");
    json!({
        "business_address": if address.is_object() {
            json!({
                "address_line1": address["addressLine1"],
                "address_line2": address["addressLine2"],
                "city": address["city"],
                "zone": address["zone"],
                "postal_code": address["postalCode"],
                "country": address["country"],
            })
        } else {
            Value::Null
        },
        "business_type": lower(field(record, "businessType")),
        "industry": if industry.is_object() {
            json!({
                "id": industry["id"],
                "code": industry["code"],
                "category": industry["category"],
                "category_label": industry["categoryLabel"],
                "subcategory_label": industry["subcategoryLabel"],
            })
        } else {
            Value::Null
        },
        "legal_name": text(record, "legalName"),
        "shop_owner": if owner.is_object() {
            json!({
                "id": owner["id"],
                "email": owner["email"],
                "first_name": owner["firstName"],
                "last_name": owner["lastName"],
                "phone": owner["phone"],
            })
        } else {
            Value::Null
        },
        "tax_identification": if tax.is_object() {
            json!({
                "tax_identification_type": lower(&tax["taxIdentificationType"]),
                "value": tax["value"],
            })
        } else {
            Value::Null
        },
    })
}
