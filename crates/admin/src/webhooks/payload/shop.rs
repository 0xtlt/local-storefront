//! The payloads of the shop and of the app: the shop, the app's installation, its subscriptions,
//! its bulk operations; metaobjects and discounts; and the compliance topics.

use serde_json::{Map, Value, json};

use super::customers::{email, phone};
use super::{
    DELETED_AT, Lookup, PREVIOUS_ACCESS_SCOPES, boolean, field, first, id, items, lower, merge,
    millis, money_amount, object, records, ref_gid, ref_id, resolve, text, text_or, time,
    weight_unit,
};
use crate::state::parse_gid;

/// The `Shop` a payload is about: the record itself when it is one, else the shop of the lookup.
fn the_shop(record: &Map<String, Value>, lookup: &dyn Lookup) -> Map<String, Value> {
    let is_shop = field(record, "id")
        .as_str()
        .and_then(parse_gid)
        .is_some_and(|(kind, _)| kind == "Shop");
    if is_shop {
        record.clone()
    } else {
        lookup.shop().unwrap_or_default()
    }
}

/// `shop/update`, `app/uninstalled`: the `Shop`.
pub(super) fn shop(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let shop = the_shop(record, lookup);
    let address = object(first(&shop, &["shopAddress", "billingAddress"]));
    let formats = object(field(&shop, "currencyFormats"));
    let plan = object(field(&shop, "plan"));
    let iana = text(&shop, "ianaTimezone");
    // In chunks: one `json!` of every field is deeper than the macro may go.
    merge([
        json!({
            "id": id(&shop),
            "name": text(&shop, "name"),
            "email": text(&shop, "email"),
            "domain": match field(&shop, "primaryDomain") {
                Value::Object(domain) => text(domain, "host"),
                _ => Value::Null,
            },
            "province": text(&address, "province"),
            "country": first(&address, &["countryCodeV2", "countryCode"]),
            "address1": text(&address, "address1"),
            "zip": text(&address, "zip"),
            "city": text(&address, "city"),
            "source": Value::Null,
            "phone": text(&address, "phone"),
            "latitude": field(&address, "latitude"),
            "longitude": field(&address, "longitude"),
            "primary_locale": text_or(&shop, "primaryLocale", "en"),
            "address2": text(&address, "address2"),
        }),
        json!({
            "created_at": time(field(&shop, "createdAt")),
            "updated_at": time(field(&shop, "updatedAt")),
            "country_code": first(&address, &["countryCodeV2", "countryCode"]),
            "country_name": text(&address, "country"),
            "currency": text(&shop, "currencyCode"),
            "customer_email": first(&shop, &["contactEmail", "email"]),
            "timezone": timezone(&shop, &iana),
            "iana_timezone": iana,
            "shop_owner": text(&shop, "shopOwnerName"),
            "money_format": text(&formats, "moneyFormat"),
            "money_with_currency_format": text(&formats, "moneyWithCurrencyFormat"),
            "weight_unit": match field(&shop, "weightUnit") {
                Value::Null => Value::Null,
                unit => weight_unit(unit),
            },
        }),
        json!({
            "province_code": text(&address, "provinceCode"),
            "taxes_included": field(&shop, "taxesIncluded"),
            "auto_configure_tax_inclusivity": Value::Null,
            "tax_shipping": field(&shop, "taxShipping"),
            "county_taxes": Value::Null,
            "plan_display_name": first(&plan, &["publicDisplayName", "displayName"]),
            "plan_name": plan_name(&plan),
            "has_discounts": false,
            "has_gift_cards": field(&shop, "features").get("giftCards").and_then(Value::as_bool).unwrap_or(false),
            "myshopify_domain": text(&shop, "myshopifyDomain"),
            "google_apps_domain": Value::Null,
            "google_apps_login_enabled": Value::Null,
            "money_in_emails_format": text(&formats, "moneyInEmailsFormat"),
            "money_with_currency_in_emails_format": text(&formats, "moneyWithCurrencyInEmailsFormat"),
        }),
        json!({
            "eligible_for_payments": true,
            "requires_extra_payments_agreement": false,
            "password_enabled": field(&shop, "passwordEnabled"),
            "has_storefront": true,
            "finances": true,
            "primary_location_id": ref_id(field(&shop, "primaryLocation")),
            "checkout_api_supported": boolean(&shop, "checkoutApiSupported", true),
            "multi_location_enabled": true,
            "setup_required": boolean(&shop, "setupRequired", false),
            "pre_launch_enabled": false,
            "enabled_presentment_currencies": match field(&shop, "enabledPresentmentCurrencies") {
                Value::Array(currencies) => Value::Array(currencies.clone()),
                _ => match text(&shop, "currencyCode") {
                    Value::Null => json!([]),
                    currency => json!([currency]),
                },
            },
            "marketing_sms_consent_enabled_at_checkout": boolean(&shop, "marketingSmsConsentEnabledAtCheckout", false),
            "marketing_email_consent_enabled_at_checkout": false,
            "transactional_sms_disabled": boolean(&shop, "transactionalSmsDisabled", false),
        }),
    ])
}

/// REST's `timezone`: `(GMT-05:00) America/New_York`, from `timezoneOffset` (`-0500`).
fn timezone(shop: &Map<String, Value>, iana: &Value) -> Value {
    let Some(name) = iana.as_str() else {
        return Value::Null;
    };
    let offset = field(shop, "timezoneOffset")
        .as_str()
        .filter(|offset| offset.len() == 5)
        .map_or_else(
            || "+00:00".to_string(),
            |offset| format!("{}:{}", &offset[..3], &offset[3..]),
        );
    json!(format!("(GMT{offset}) {name}"))
}

/// REST's `plan_name`, which GraphQL does not give: what the plan's flags say.
fn plan_name(plan: &Map<String, Value>) -> Value {
    if plan.is_empty() {
        return Value::Null;
    }
    if field(plan, "partnerDevelopment").as_bool() == Some(true) {
        json!("partner_test")
    } else if field(plan, "shopifyPlus").as_bool() == Some(true) {
        json!("shopify_plus")
    } else {
        json!("basic")
    }
}

/// `app/scopes_update`: the `AppInstallation`, whose `accessScopes` are those it has now and
/// whose [`PREVIOUS_ACCESS_SCOPES`] those it had.
pub(super) fn scopes_update(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let handles = |value: &Value| {
        Value::Array(
            items(value)
                .into_iter()
                .filter_map(|scope| match scope {
                    Value::String(handle) => Some(json!(handle)),
                    Value::Object(scope) => scope.get("handle").cloned(),
                    _ => None,
                })
                .collect(),
        )
    };
    let shop = lookup.shop().unwrap_or_default();
    json!({
        "id": id(record),
        "shop_id": text(&shop, "id"),
        "previous": handles(field(record, PREVIOUS_ACCESS_SCOPES)),
        "current": handles(field(record, "accessScopes")),
        "updated_at": millis(field(record, "updatedAt")),
    })
}

/// The pricing details of the line items of an `AppSubscription`: its lines are records
/// (`AppSubscriptionLineItem`s by GID) or embedded.
fn subscription_pricing(
    record: &Map<String, Value>,
    lookup: &dyn Lookup,
) -> Vec<Map<String, Value>> {
    records(field(record, "lineItems"), lookup)
        .iter()
        .filter_map(|line| line.get("plan")?.get("pricingDetails").cloned())
        .map(|details| object(&details))
        .collect()
}

/// `app_subscriptions/update`: an `AppSubscription`, its price and its capped amount from its
/// line items.
pub(super) fn app_subscription(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let pricing = subscription_pricing(record, lookup);
    let recurring = pricing
        .iter()
        .find(|details| details.contains_key("interval"));
    let usage = pricing
        .iter()
        .find(|details| details.contains_key("cappedAmount"));
    let shop = lookup.shop().unwrap_or_default();
    json!({
        "app_subscription": {
            "admin_graphql_api_id": text(record, "id"),
            "name": text(record, "name"),
            "status": text(record, "status"),
            "admin_graphql_api_shop_id": text(&shop, "id"),
            "created_at": time(field(record, "createdAt")),
            "updated_at": time(first(record, &["updatedAt", "createdAt"])),
            "currency": recurring
                .and_then(|details| details.get("price"))
                .or_else(|| usage.and_then(|details| details.get("cappedAmount")))
                .and_then(|money| money.get("currencyCode"))
                .cloned()
                .unwrap_or(Value::Null),
            "capped_amount": usage
                .and_then(|details| details.get("cappedAmount"))
                .and_then(|money| money.get("amount"))
                .cloned()
                .unwrap_or(Value::Null),
            "price": recurring
                .and_then(|details| details.get("price"))
                .map_or(Value::Null, money_amount),
            "interval": recurring.map_or(Value::Null, |details| lower(field(details, "interval"))),
            "plan_handle": recurring.map_or(Value::Null, |details| text(details, "planHandle")),
        }
    })
}

/// `app_subscriptions/approaching_capped_amount`: an `AppSubscription`, the balance used and
/// the capped amount of its usage line (`balance_used` a number, `capped_amount` the `Decimal`,
/// as shopify.dev's example writes them).
pub(super) fn approaching_capped_amount(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let pricing = subscription_pricing(record, lookup);
    let usage = pricing
        .iter()
        .find(|details| details.contains_key("cappedAmount"));
    let shop = lookup.shop().unwrap_or_default();
    let balance = usage
        .and_then(|details| details.get("balanceUsed"))
        .and_then(|money| money.get("amount"))
        .and_then(|amount| match amount {
            Value::String(text) => text.parse::<f64>().ok(),
            other => other.as_f64(),
        })
        .unwrap_or(0.0);
    let balance = if balance.fract() == 0.0 {
        json!(balance as i64)
    } else {
        json!(balance)
    };
    let capped = usage.and_then(|details| details.get("cappedAmount"));
    json!({
        "app_subscription": {
            "admin_graphql_api_id": text(record, "id"),
            "name": text(record, "name"),
            "balance_used": balance,
            "capped_amount": capped
                .and_then(|money| money.get("amount"))
                .cloned()
                .unwrap_or(Value::Null),
            "currency_code": capped
                .and_then(|money| money.get("currencyCode"))
                .cloned()
                .unwrap_or(Value::Null),
            "admin_graphql_api_shop_id": text(&shop, "id"),
            "created_at": time(field(record, "createdAt")),
            "updated_at": time(first(record, &["updatedAt", "createdAt"])),
        }
    })
}

/// `app_purchases_one_time/update`: an `AppPurchaseOneTime`.
pub(super) fn app_purchase_one_time(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let shop = lookup.shop().unwrap_or_default();
    json!({
        "app_purchase_one_time": {
            "admin_graphql_api_id": text(record, "id"),
            "name": text(record, "name"),
            "status": text(record, "status"),
            "admin_graphql_api_shop_id": text(&shop, "id"),
            "created_at": time(field(record, "createdAt")),
            "updated_at": time(first(record, &["updatedAt", "createdAt"])),
        }
    })
}

/// `discounts/redeemcode_added`, `discounts/redeemcode_removed`: the code discount and the code
/// (`redeemCode` of the record the event keeps, else the discount's first code), and when.
pub(super) fn redeem_code(record: &Map<String, Value>) -> Value {
    let code = match field(record, "redeemCode") {
        Value::Object(code) => code.clone(),
        _ => field(record, "codeDiscount")
            .get("codes")
            .and_then(Value::as_array)
            .and_then(|codes| codes.first())
            .map(object)
            .unwrap_or_default(),
    };
    let updated = match field(record, "updatedAt") {
        Value::Null => field(record, "codeDiscount")
            .get("updatedAt")
            .cloned()
            .unwrap_or(Value::Null),
        updated => updated.clone(),
    };
    json!({
        "admin_graphql_api_id": text(record, "id"),
        "redeem_code": {"id": text(&code, "id"), "code": text(&code, "code")},
        "updated_at": millis(&updated),
    })
}

/// `bulk_operations/finish`: a `BulkOperation`.
pub(super) fn bulk_operation(record: &Map<String, Value>) -> Value {
    json!({
        "admin_graphql_api_id": text(record, "id"),
        "completed_at": time(field(record, "completedAt")),
        "created_at": time(field(record, "createdAt")),
        "error_code": lower(field(record, "errorCode")),
        "status": lower(field(record, "status")),
        "type": lower(field(record, "type")),
    })
}

/// `metaobjects/create`, `metaobjects/update`: a `Metaobject`, its fields as `{key: value}`.
pub(super) fn metaobject(record: &Map<String, Value>) -> Value {
    let fields: Map<String, Value> = items(field(record, "fields"))
        .iter()
        .filter_map(|entry| {
            let key = entry.get("key")?.as_str()?.to_string();
            Some((key, entry.get("value").cloned().unwrap_or(Value::Null)))
        })
        .collect();
    let capabilities = match field(record, "capabilities").pointer("/publishable/status") {
        Some(status) => json!({"publishable": {"status": lower(status)}}),
        None => json!({}),
    };
    json!({
        "type": text(record, "type"),
        "handle": text(record, "handle"),
        "created_at": time(field(record, "createdAt")),
        "updated_at": time(field(record, "updatedAt")),
        "display_name": first(record, &["displayName", "handle"]),
        "id": text(record, "id"),
        "definition_id": ref_gid(field(record, "definition")),
        "fields": fields,
        "created_by_staff_id": ref_gid(field(record, "createdByStaff")),
        "created_by_app_id": ref_gid(first(record, &["createdByApp", "createdBy"])),
        "capabilities": capabilities,
    })
}

/// `metaobjects/delete`.
pub(super) fn metaobject_deleted(record: &Map<String, Value>) -> Value {
    json!({
        "id": text(record, "id"),
        "type": text(record, "type"),
        "handle": text(record, "handle"),
        "created_by_app_id": ref_gid(first(record, &["createdByApp", "createdBy"])),
    })
}

/// `discounts/create`, `discounts/update`: a `DiscountAutomaticNode` or a `DiscountCodeNode`,
/// the title, status and dates of its discount. A record of the discount itself works too.
pub(super) fn discount(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let discount = resolve(
        first(record, &["automaticDiscount", "codeDiscount", "discount"]),
        lookup,
    )
    .unwrap_or_else(|| record.clone());
    json!({
        "admin_graphql_api_id": text(record, "id"),
        "title": text(&discount, "title"),
        "status": text(&discount, "status"),
        "created_at": time(field(&discount, "createdAt")),
        "updated_at": time(first(&discount, &["updatedAt", "createdAt"])),
    })
}

/// `discounts/delete`: the node and when it was deleted ([`DELETED_AT`]).
pub(super) fn discount_deleted(record: &Map<String, Value>) -> Value {
    json!({
        "admin_graphql_api_id": text(record, "id"),
        "deleted_at": time(first(record, &[DELETED_AT, "updatedAt"])),
    })
}

/// The shop fields of the compliance payloads.
fn shop_fields(record: &Map<String, Value>, lookup: &dyn Lookup) -> (Value, Value) {
    let shop = the_shop(record, lookup);
    (id(&shop), text(&shop, "myshopifyDomain"))
}

/// The customer of the compliance payloads, and the ids of its orders.
fn compliance_customer(record: &Map<String, Value>) -> (Value, Value) {
    let orders: Vec<Value> = items(field(record, "orders"))
        .iter()
        .map(ref_id)
        .filter(|id| !id.is_null())
        .collect();
    (
        json!({"id": id(record), "email": email(record), "phone": phone(record)}),
        Value::Array(orders),
    )
}

/// `customers/data_request`: a `Customer` whose data the shop asks for. The id of the request
/// is `dataRequestId` of the record, else the customer's.
pub(super) fn customer_data_request(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let (shop_id, shop_domain) = shop_fields(&Map::new(), lookup);
    let (customer, orders) = compliance_customer(record);
    let request_id = match field(record, "dataRequestId") {
        Value::Null => id(record),
        request => request.clone(),
    };
    json!({
        "shop_id": shop_id,
        "shop_domain": shop_domain,
        "customer": customer,
        "orders_requested": orders,
        "data_request": {"id": request_id},
    })
}

/// `customers/redact`: a `Customer` whose data the app is to delete.
pub(super) fn customer_redact(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let (shop_id, shop_domain) = shop_fields(&Map::new(), lookup);
    let (customer, orders) = compliance_customer(record);
    json!({
        "shop_id": shop_id,
        "shop_domain": shop_domain,
        "customer": customer,
        "orders_to_redact": orders,
    })
}

/// `shop/redact`: the `Shop`.
pub(super) fn shop_redact(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let (shop_id, shop_domain) = shop_fields(record, lookup);
    json!({"shop_id": shop_id, "shop_domain": shop_domain})
}
