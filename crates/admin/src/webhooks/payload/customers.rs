//! The payloads of customers, of the companies they buy for (B2B), and the addresses that orders
//! share with customers.

use serde_json::{Map, Value, json};

use super::{
    Lookup, PREVIOUS, boolean, field, first, id, lower, lower_or, millis, object, records, ref_gid,
    resolve, strings, tags, text, time,
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
    lower_or(field(record, "state"), "disabled")
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

/// The fields of a marketing consent REST writes for email and SMS alike, `null` when the
/// customer has no consent.
fn consent_fields(consent: &Value) -> Map<String, Value> {
    let consent = object(consent);
    let mut fields = Map::new();
    fields.insert("state".into(), lower(field(&consent, "marketingState")));
    fields.insert(
        "opt_in_level".into(),
        lower(field(&consent, "marketingOptInLevel")),
    );
    fields.insert(
        "consent_updated_at".into(),
        time(field(&consent, "consentUpdatedAt")),
    );
    fields
}

/// `CustomerEmailMarketingConsentState` as REST writes it.
fn email_consent(consent: &Value) -> Value {
    if !consent.is_object() {
        return Value::Null;
    }
    Value::Object(consent_fields(consent))
}

/// `CustomerSmsMarketingConsentState` as REST writes it.
fn sms_consent(consent: &Value) -> Value {
    let Value::Object(object) = consent else {
        return Value::Null;
    };
    let mut fields = consent_fields(consent);
    fields.insert(
        "consent_collected_from".into(),
        match field(object, "consentCollectedFrom") {
            Value::String(from) => json!(from),
            _ => json!("OTHER"),
        },
    );
    Value::Object(fields)
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

/// `customer.tags_added` and `customer.tags_removed`: `{customerId, tags, occurredAt}`, in
/// camelCase as Shopify's example writes these event topics. `tags` are those the change added
/// (or removed): the difference with the record before it ([`PREVIOUS`]), ignoring case as tags
/// do; without it (a webhook triggered on demand), the tags the customer has. `occurredAt` is
/// the time of the change, which `updatedAt` holds, with milliseconds as the example has them.
pub(super) fn tags_changed(record: &Map<String, Value>, added: bool) -> Value {
    let now = tag_list(record);
    let tags = match field(record, PREVIOUS) {
        Value::Object(before) => {
            let before = tag_list(before);
            let (from, against) = if added {
                (&now, &before)
            } else {
                (&before, &now)
            };
            from.iter()
                .filter(|tag| !against.iter().any(|other| other.eq_ignore_ascii_case(tag)))
                .cloned()
                .collect()
        }
        _ => now,
    };
    json!({
        "customerId": text(record, "id"),
        "tags": tags,
        "occurredAt": millis(field(record, "updatedAt")),
    })
}

/// The tags of a record: an array, or REST's comma-separated string.
fn tag_list(record: &Map<String, Value>) -> Vec<String> {
    match field(record, "tags") {
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Value::String(text) => text
            .split(',')
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// `customers_email_marketing_consent/update`: the customer's id, email and consent.
pub(super) fn email_consent_update(record: &Map<String, Value>) -> Value {
    json!({
        "customer_id": id(record),
        "email_address": email(record),
        "email_marketing_consent": consent_fields(field(record, "emailMarketingConsent")),
    })
}

/// `customers_marketing_consent/update` (SMS): the customer's id, phone and consent, where
/// `consent_collected_from` is in lowercase, as the topic's example writes it.
pub(super) fn sms_consent_update(record: &Map<String, Value>) -> Value {
    let consent = field(record, "smsMarketingConsent");
    let mut fields = consent_fields(consent);
    fields.insert(
        "consent_collected_from".into(),
        match field(&object(consent), "consentCollectedFrom") {
            Value::String(from) => json!(from.to_ascii_lowercase()),
            _ => json!("other"),
        },
    );
    json!({
        "id": id(record),
        "phone": phone(record),
        "sms_marketing_consent": fields,
    })
}

/// `companies/create`, `/update`, `/delete`, and the company a contact or a location embeds: a
/// `Company`, which REST names by GID alone (no numeric `id`).
pub(super) fn company(record: &Map<String, Value>) -> Value {
    json!({
        "name": text(record, "name"),
        "note": text(record, "note"),
        "external_id": text(record, "externalId"),
        "main_contact_admin_graphql_api_id": ref_gid(field(record, "mainContact")),
        "created_at": time(field(record, "createdAt")),
        "updated_at": time(field(record, "updatedAt")),
        "customer_since": time(field(record, "customerSince")),
        "admin_graphql_api_id": text(record, "id"),
    })
}

/// The company a record names, embedded; `null` when it is gone.
fn company_of(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    resolve(field(record, "company"), lookup).map_or(Value::Null, |found| company(&found))
}

/// `company_contacts/create`, `/update`, `/delete`: a `CompanyContact` with its company.
pub(super) fn company_contact(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    json!({
        "customer_admin_graphql_api_id": ref_gid(field(record, "customer")),
        "title": text(record, "title"),
        "locale": text(record, "locale"),
        "created_at": time(field(record, "createdAt")),
        "updated_at": time(field(record, "updatedAt")),
        "admin_graphql_api_id": text(record, "id"),
        "company": company_of(record, lookup),
    })
}

/// `company_locations/create`, `/update`, `/delete`: a `CompanyLocation` with its company, its
/// addresses and its tax settings.
///
/// `buyer_experience_configuration` is `null`, as in Shopify's example: how REST writes a
/// configuration is not documented. `tax_settings.tax_exempt` is the location's (`false`
/// unless set), where the example has `null`.
pub(super) fn company_location(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let settings = object(field(record, "taxSettings"));
    let exemptions = match field(&settings, "taxExemptions") {
        Value::Null => strings(field(record, "taxExemptions")),
        exemptions => strings(exemptions),
    };
    let registration = match text(&settings, "taxRegistrationId") {
        Value::Null => text(record, "taxRegistrationId"),
        registration => registration,
    };
    let company_gid = ref_gid(field(record, "company"));
    let address = |key: &str| match field(record, key) {
        Value::Object(address) => company_address(address, &company_gid),
        _ => Value::Null,
    };
    json!({
        "name": text(record, "name"),
        "external_id": text(record, "externalId"),
        "phone": text(record, "phone"),
        "locale": text(record, "locale"),
        "created_at": time(field(record, "createdAt")),
        "updated_at": time(field(record, "updatedAt")),
        "note": text(record, "note"),
        "buyer_experience_configuration": null,
        "admin_graphql_api_id": text(record, "id"),
        "tax_exemptions": exemptions,
        "tax_settings": {
            "tax_registration_id": registration,
            "tax_exempt": boolean(&settings, "taxExempt", false),
            "tax_exemptions": exemptions,
        },
        "company": company_of(record, lookup),
        "billing_address": address("billingAddress"),
        "shipping_address": address("shippingAddress"),
        "tax_registration": match &registration {
            Value::Null => Value::Null,
            registration => json!({"tax_id": registration}),
        },
    })
}

/// A `CompanyAddress` as REST writes it.
fn company_address(address: &Map<String, Value>, company: &Value) -> Value {
    json!({
        "address1": text(address, "address1"),
        "city": text(address, "city"),
        "province": text(address, "province"),
        "country": text(address, "country"),
        "zip": text(address, "zip"),
        "recipient": text(address, "recipient"),
        "first_name": text(address, "firstName"),
        "last_name": text(address, "lastName"),
        "address2": text(address, "address2"),
        "phone": text(address, "phone"),
        "zone_code": text(address, "zoneCode"),
        "country_code": text(address, "countryCode"),
        "created_at": time(field(address, "createdAt")),
        "updated_at": time(field(address, "updatedAt")),
        "admin_graphql_api_id": text(address, "id"),
        "company_admin_graphql_api_id": company,
    })
}

/// `company_contact_roles/assign` and `/revoke`: a `CompanyContactRoleAssignment`, as the
/// contact, the location and the role it joins.
pub(super) fn role_assignment(record: &Map<String, Value>, lookup: &dyn Lookup) -> Value {
    let contact = resolve(field(record, "companyContact"), lookup)
        .map_or(Value::Null, |contact| company_contact(&contact, lookup));
    let location = resolve(field(record, "companyLocation"), lookup)
        .map_or(Value::Null, |location| company_location(&location, lookup));
    let role = resolve(field(record, "role"), lookup)
        .map_or(Value::Null, |role| json!({"name": text(&role, "name")}));
    json!({
        "company_contact": contact,
        "company_location": location,
        "company_contact_role": role,
    })
}
