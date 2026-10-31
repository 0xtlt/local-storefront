//! The payloads of customers, and the addresses that orders share with them.

use serde_json::{Map, Value, json};

use super::{
    Lookup, boolean, field, first, id, lower, records, resolve, strings, tags, text, time,
};

/// `customers/create`, `/update`, `/enable`, `/disable`: a `Customer`.
///
/// Shopify's example payload of the topic has no `tags` nor marketing consents, but the original
/// payload the same reference shows for `customers/update` (under "Modify payloads") has them: lsf
/// sends them, in that payload's places.
pub(super) fn customer(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let customer_id = id(record);
    let default = resolve(field(record, "defaultAddress"), lookup);
    let default_id = default.as_ref().map(|address| text(address, "id"));
    let addresses: Vec<Value> = records(first(record, &["addressesV2", "addresses"]), lookup)
        .iter()
        .map(|address| {
            let is_default = default_id
                .as_ref()
                .is_some_and(|default| !default.is_null() && *default == text(address, "id"));
            customer_address(address, &customer_id, is_default)
        })
        .collect();
    json!({
        "id": customer_id,
        "created_at": time(field(record, "createdAt")),
        "updated_at": time(field(record, "updatedAt")),
        "first_name": text(record, "firstName"),
        "last_name": text(record, "lastName"),
        "state": state(record),
        "note": text(record, "note"),
        "verified_email": boolean(record, "verifiedEmail", true),
        "multipass_identifier": text(record, "multipassIdentifier"),
        "tax_exempt": boolean(record, "taxExempt", false),
        "tags": tags(field(record, "tags")),
        "email": email(record),
        "phone": phone(record),
        "currency": currency(record, lookup),
        "addresses": addresses,
        "tax_exemptions": strings(field(record, "taxExemptions")),
        "email_marketing_consent": email_consent(field(record, "emailMarketingConsent")),
        "sms_marketing_consent": sms_consent(field(record, "smsMarketingConsent")),
        "admin_graphql_api_id": text(record, "id"),
        "default_address": default
            .as_ref()
            .map_or(Value::Null, |address| customer_address(address, &customer_id, true)),
    })
}

/// The customer of an order or a draft order: the keys of the topic's example payload, without
/// the addresses.
pub(super) fn embedded(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let customer_id = id(record);
    let default = resolve(field(record, "defaultAddress"), lookup);
    json!({
        "id": customer_id,
        "created_at": time(field(record, "createdAt")),
        "updated_at": time(field(record, "updatedAt")),
        "first_name": text(record, "firstName"),
        "last_name": text(record, "lastName"),
        "state": state(record),
        "note": text(record, "note"),
        "verified_email": boolean(record, "verifiedEmail", true),
        "multipass_identifier": text(record, "multipassIdentifier"),
        "tax_exempt": boolean(record, "taxExempt", false),
        "email": email(record),
        "phone": phone(record),
        "currency": currency(record, lookup),
        "tax_exemptions": strings(field(record, "taxExemptions")),
        "admin_graphql_api_id": text(record, "id"),
        "default_address": default
            .as_ref()
            .map_or(Value::Null, |address| customer_address(address, &customer_id, true)),
    })
}

/// `customers/delete`.
pub(super) fn deleted(record: &Map<String, Value>) -> Value {
    json!({
        "id": id(record),
        "tax_exemptions": strings(field(record, "taxExemptions")),
        "admin_graphql_api_id": text(record, "id"),
    })
}

/// `DISABLED` is `disabled`.
fn state(record: &Map<String, Value>) -> Value {
    match lower(field(record, "state")) {
        Value::Null => json!("disabled"),
        state => state,
    }
}

/// `email`, deprecated in favour of `defaultEmailAddress.emailAddress`.
pub(super) fn email(record: &Map<String, Value>) -> Value {
    match text(record, "email") {
        Value::Null => field(record, "defaultEmailAddress")
            .get("emailAddress")
            .cloned()
            .unwrap_or(Value::Null),
        email => email,
    }
}

/// `phone`, deprecated in favour of `defaultPhoneNumber.phoneNumber`.
pub(super) fn phone(record: &Map<String, Value>) -> Value {
    match text(record, "phone") {
        Value::Null => field(record, "defaultPhoneNumber")
            .get("phoneNumber")
            .cloned()
            .unwrap_or(Value::Null),
        phone => phone,
    }
}

/// A customer has no currency in GraphQL: that of what it spent, else the shop's.
fn currency(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    match text(record, "currency") {
        Value::Null => match field(record, "amountSpent").get("currencyCode") {
            Some(Value::String(code)) => json!(code),
            _ => lookup
                .shop()
                .map_or(Value::Null, |shop| text(&shop, "currencyCode")),
        },
        currency => currency,
    }
}

/// `CustomerEmailMarketingConsentState` as REST writes it.
fn email_consent(consent: &Value) -> Value {
    let Value::Object(consent) = consent else {
        return Value::Null;
    };
    json!({
        "state": lower(field(consent, "marketingState")),
        "opt_in_level": lower(field(consent, "marketingOptInLevel")),
        "consent_updated_at": time(field(consent, "consentUpdatedAt")),
    })
}

/// `CustomerSmsMarketingConsentState` as REST writes it.
fn sms_consent(consent: &Value) -> Value {
    let Value::Object(consent) = consent else {
        return Value::Null;
    };
    json!({
        "state": lower(field(consent, "marketingState")),
        "opt_in_level": lower(field(consent, "marketingOptInLevel")),
        "consent_updated_at": time(field(consent, "consentUpdatedAt")),
        "consent_collected_from": match field(consent, "consentCollectedFrom") {
            Value::String(from) => json!(from),
            _ => json!("OTHER"),
        },
    })
}

/// The country code of a `MailingAddress`: `countryCodeV2`, or the deprecated `countryCode`.
fn country_code(address: &Map<String, Value>) -> Value {
    first(address, &["countryCodeV2", "countryCode"]).clone()
}

/// A `MailingAddress` of a customer, as REST's customer addresses write it.
pub(super) fn customer_address(
    address: &Map<String, Value>,
    customer_id: &Value,
    is_default: bool,
) -> Value {
    json!({
        "id": id(address),
        "customer_id": customer_id,
        "first_name": text(address, "firstName"),
        "last_name": text(address, "lastName"),
        "company": text(address, "company"),
        "address1": text(address, "address1"),
        "address2": text(address, "address2"),
        "city": text(address, "city"),
        "province": text(address, "province"),
        "country": text(address, "country"),
        "zip": text(address, "zip"),
        "phone": text(address, "phone"),
        "name": name(address),
        "province_code": text(address, "provinceCode"),
        "country_code": country_code(address),
        "country_name": text(address, "country"),
        "default": is_default,
    })
}

/// A `MailingAddress` of an order (its billing and shipping addresses, the destination of a
/// fulfillment), as REST writes it. `Null` when there is none.
pub(super) fn order_address(address: Option<&Map<String, Value>>) -> Value {
    let Some(address) = address else {
        return Value::Null;
    };
    json!({
        "first_name": text(address, "firstName"),
        "address1": text(address, "address1"),
        "phone": text(address, "phone"),
        "city": text(address, "city"),
        "zip": text(address, "zip"),
        "province": text(address, "province"),
        "country": text(address, "country"),
        "last_name": text(address, "lastName"),
        "address2": text(address, "address2"),
        "company": text(address, "company"),
        "latitude": field(address, "latitude"),
        "longitude": field(address, "longitude"),
        "name": name(address),
        "country_code": country_code(address),
        "province_code": text(address, "provinceCode"),
    })
}

/// The `name` of an address, or its first and last names.
fn name(address: &Map<String, Value>) -> Value {
    match text(address, "name") {
        Value::Null => {
            let parts: Vec<&str> = ["firstName", "lastName"]
                .iter()
                .filter_map(|key| field(address, key).as_str())
                .filter(|part| !part.is_empty())
                .collect();
            if parts.is_empty() {
                Value::Null
            } else {
                json!(parts.join(" "))
            }
        }
        name => name,
    }
}
