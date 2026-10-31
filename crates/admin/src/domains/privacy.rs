//! Privacy: a customer's data erasure (`customerRequestDataErasure`,
//! `customerCancelDataErasure`), the sale of their data (`dataSaleOptOut`), their WhatsApp
//! marketing consent (`customerWhatsAppMarketingConsentUpdate`), and the shop's consent policies
//! and privacy features (`consentPolicyUpdate`, `privacyFeaturesDisable`; the `consentPolicy`,
//! `consentPolicyRegions` and `privacySettings` queries).
//!
//! As shopify.dev's reference of 2026-10 and the Help Center ("Processing customer data
//! requests") describe them:
//!
//! - **Data erasure**: the request "enqueues" the erasure, which the merchant can cancel for 10
//!   days ("you have 10 days to cancel the request"); then Shopify redacts the customer's
//!   personal information ("such as the customer's name and address"), and their profile and
//!   order history remain. lsf keeps the request (`privacy/erasure/<customer GID>`, a memo) and
//!   processes it once the shop's clock is 10 days past it (`process_due_erasures`, which the
//!   clock control endpoint runs): the names, the email, the phone, the note and the addresses
//!   are cleared, the consents `REDACTED`, and the outbox records the erasure, with the
//!   `customers/redact` Shopify would send the app. Errors: a customer that does not exist
//!   (`DOES_NOT_EXIST`), a request already pending (`FAILED_TO_REQUEST`), a cancellation of
//!   nothing pending (`NOT_BEING_ERASED`); the messages are lsf's. A pending erasure blocks a
//!   merge (`PENDING_DATA_REQUEST`).
//! - **`dataSaleOptOut(email:)`**: the customer of that email gets `dataSaleOptOut: true`; an
//!   email no customer has gets a customer of its own, as the storefront's opt-out page makes one
//!   *(chosen: Shopify does not say)*; an email that is not one is `FAILED`. Webhooks:
//!   `customers/update` (or `customers/create`).
//! - **WhatsApp consent**: as the SMS consent (`domains::customers::consent`): the customer needs
//!   a phone, the state is `SUBSCRIBED`, `UNSUBSCRIBED` or `PENDING` (`INCLUSION` otherwise), the
//!   opt-in level defaults to `SINGLE_OPT_IN`, `updatedAt` to now, and the consent is collected
//!   from `OTHER`. It shows in `Customer.defaultPhoneNumber.whatsAppMarketingConsent`. Webhooks:
//!   `customers/update`, then `customers_whats_app_marketing_consent/update`.
//! - **Consent policies**: `consentPolicyUpdate` creates or updates a policy per country (and
//!   region); Shopify's own cookie banner must be disabled first
//!   (`SHOPIFY_COOKIE_BANNER_NOT_DISABLED`, with `privacyFeaturesDisable([COOKIE_BANNER])`), a
//!   country is required (`COUNTRY_CODE_REQUIRED`), and a region must be one of the country's
//!   (`REGION_CODE_MUST_MATCH_COUNTRY_CODE`: lsf checks it is two or three letters or digits, it
//!   has no list of subdivisions). `consentPolicy` lists them, filtered by its arguments;
//!   `consentPolicyRegions` the countries a policy can be made for (every `PrivacyCountryCode`).
//! - **Privacy features**: the cookie banner, the data sale opt-out page and the privacy policy
//!   are managed by Shopify (`autoManaged: true`, the banner `enabled`) until
//!   `privacyFeaturesDisable` turns them off; the privacy policy's `supportedLocales` are the
//!   shop's languages.

use serde_json::{Map, Value, json};

use crate::domains::outbox;
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::{Record, ShopState};

/// How long a data erasure waits before it is processed, and can be cancelled.
pub const ERASURE_DELAY_DAYS: i64 = 10;

const SETTINGS_MEMO: &str = "privacy/settings";

pub fn register(registry: &mut Registry) {
    registry
        .mutation("customerRequestDataErasure", request_data_erasure)
        .mutation("customerCancelDataErasure", cancel_data_erasure)
        .mutation("dataSaleOptOut", data_sale_opt_out)
        .mutation(
            "customerWhatsAppMarketingConsentUpdate",
            whatsapp_consent_update,
        )
        .mutation("consentPolicyUpdate", consent_policy_update)
        .mutation("privacyFeaturesDisable", privacy_features_disable)
        .query("consentPolicy", consent_policy)
        .query("consentPolicyRegions", consent_policy_regions)
        .query("privacySettings", privacy_settings);
}

fn erasure_memo(customer: &str) -> String {
    format!("privacy/erasure/{customer}")
}

/// Whether a customer's data erasure is pending.
pub fn erasure_pending(state: &ShopState, customer: &str) -> bool {
    state
        .memo(&erasure_memo(customer))
        .is_some_and(|request| request.get("status").and_then(Value::as_str) == Some("PENDING"))
}

fn is_customer(state: &ShopState, id: &str) -> bool {
    state.type_of(id) == Some("Customer")
}

fn request_data_erasure(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let customer = ctx.arg_str("customerId").unwrap_or_default().to_string();
    if !is_customer(m.state(), &customer) {
        let error =
            UserError::new(&["customerId"], "Customer does not exist").code("DOES_NOT_EXIST");
        return Ok(payload(json!({"customerId": null}), &[error]));
    }
    if erasure_pending(m.state(), &customer) {
        let error = UserError::new(
            &["customerId"],
            "An erasure of this customer's data is already requested.",
        )
        .code("FAILED_TO_REQUEST");
        return Ok(payload(json!({"customerId": null}), &[error]));
    }
    let now = m.now();
    let due = now + chrono::Duration::days(ERASURE_DELAY_DAYS);
    let state = m.state_mut();
    state.set_memo(
        erasure_memo(&customer),
        json!({
            "status": "PENDING",
            "requestedAt": crate::state::value::date_time(now),
            "dueAt": crate::state::value::date_time(due),
        }),
    );
    let email = state
        .get(&customer)
        .and_then(|record| record.get("email"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    outbox::record(
        state,
        "data_erasure",
        &email,
        &format!(
            "Data erasure requested: the customer's personal data is erased on {} unless the request is cancelled",
            crate::state::value::date_time(due)
        ),
        &customer,
    );
    // No webhook: Shopify sends `customers/redact` when it erases, not when it is asked.
    state.emit(crate::domains::app::CHANGED, &customer);
    Ok(payload(json!({"customerId": customer}), &[]))
}

fn cancel_data_erasure(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let customer = ctx.arg_str("customerId").unwrap_or_default().to_string();
    if !is_customer(m.state(), &customer) {
        let error =
            UserError::new(&["customerId"], "Customer does not exist").code("DOES_NOT_EXIST");
        return Ok(payload(json!({"customerId": null}), &[error]));
    }
    if !erasure_pending(m.state(), &customer) {
        let error = UserError::new(
            &["customerId"],
            "The customer's data is not scheduled for erasure.",
        )
        .code("NOT_BEING_ERASED");
        return Ok(payload(json!({"customerId": null}), &[error]));
    }
    let state = m.state_mut();
    state.set_memo(erasure_memo(&customer), Value::Null);
    outbox::record(
        state,
        "data_erasure",
        "",
        "Data erasure cancelled",
        &customer,
    );
    state.emit(crate::domains::app::CHANGED, &customer);
    Ok(payload(json!({"customerId": customer}), &[]))
}

/// Processes the data erasures whose 10 days passed by the shop's clock: the customers'
/// personal data is redacted. Answers the customers erased, for the `customers/redact` the
/// server sends.
pub fn process_due_erasures(state: &mut ShopState) -> Vec<String> {
    let now = state.now();
    let due: Vec<String> = state
        .keys("Customer")
        .filter(|customer| {
            state
                .memo(&erasure_memo(customer))
                .filter(|request| request.get("status").and_then(Value::as_str) == Some("PENDING"))
                .and_then(|request| request.get("dueAt").and_then(Value::as_str))
                .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
                .is_some_and(|at| at <= now)
        })
        .map(str::to_string)
        .collect();
    for customer in &due {
        let email = state
            .get(customer)
            .and_then(|record| record.get("email"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if let Some(record) = state.get_mut(customer) {
            for field in ["firstName", "lastName", "email", "phone", "note"] {
                record.insert(field.into(), Value::Null);
            }
            record.insert("addresses".into(), json!([]));
            record.insert("addressesV2".into(), json!([]));
            record.insert("defaultAddress".into(), Value::Null);
            for field in ["emailMarketingConsent", "smsMarketingConsent"] {
                record.insert(
                    field.into(),
                    json!({"marketingState": "REDACTED", "marketingOptInLevel": null, "consentUpdatedAt": crate::state::value::date_time(now)}),
                );
            }
            crate::domains::customers::contact_fields(record);
        }
        state.touch(customer);
        state.set_memo(
            erasure_memo(customer),
            json!({"status": "ERASED", "erasedAt": crate::state::value::date_time(now)}),
        );
        outbox::record(
            state,
            "data_erasure",
            &email,
            "Customer data erased: names, email, phone, note and addresses redacted; customers/redact sent to the app",
            customer,
        );
        state.emit("customers/update", customer);
    }
    due
}

fn data_sale_opt_out(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let email = ctx.arg_str("email").unwrap_or_default().trim().to_string();
    if !crate::domains::customers::customer::is_email(&email) {
        let error = UserError::new(&["email"], "Email is invalid").code("FAILED");
        return Ok(payload(json!({"customerId": null}), &[error]));
    }
    let existing = m
        .state()
        .all("Customer")
        .find(|customer| {
            customer
                .get("email")
                .and_then(Value::as_str)
                .is_some_and(|known| known.eq_ignore_ascii_case(&email))
        })
        .and_then(|customer| customer.get("id").and_then(Value::as_str))
        .map(str::to_string);
    let locale = crate::domains::customers::primary_locale(m.state());
    let now = m.timestamp();
    let state = m.state_mut();
    let customer = match existing {
        Some(customer) => {
            if let Some(record) = state.get_mut(&customer) {
                record.insert("dataSaleOptOut".into(), json!(true));
            }
            state.touch(&customer);
            state.emit("customers/update", &customer);
            customer
        }
        None => {
            let id = state.new_gid("Customer");
            let mut record = crate::state::value::object(json!({
                "__typename": "Customer",
                "id": id,
                "firstName": null,
                "lastName": null,
                "email": email,
                "phone": null,
                "emailMarketingConsent": crate::domains::customers::consent::default_email_consent(),
                "addresses": [],
                "addressesV2": [],
                "defaultAddress": null,
                "state": "DISABLED",
                "verifiedEmail": false,
                "taxExempt": false,
                "taxExemptions": [],
                "dataSaleOptOut": true,
                "tags": [],
                "note": null,
                "locale": locale,
                "companyContactProfiles": [],
                "orders": [],
                "createdAt": now,
                "updatedAt": now,
                "metafields": [],
            }));
            crate::domains::customers::contact_fields(&mut record);
            state.insert_as(id.clone(), record);
            crate::domains::customers::refresh_customer(state, &id);
            state.emit("customers/create", &id);
            id
        }
    };
    Ok(payload(json!({"customerId": customer}), &[]))
}

/// The WhatsApp consent of a customer that never gave one.
pub fn default_whatsapp_consent() -> Value {
    json!({
        "state": "NEVER_SUBSCRIBED",
        "optInLevel": null,
        "updatedAt": null,
        "collectedFrom": "OTHER",
        "sourceLocation": null,
    })
}

fn whatsapp_consent_update(
    m: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let input = ctx
        .arg("input")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let customer = input
        .get("customerId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let state = m.state();
    let Some(record) = state
        .get(&customer)
        .filter(|_| is_customer(state, &customer))
    else {
        let error =
            UserError::new(&["input", "customerId"], "Customer does not exist").code("INVALID");
        return Ok(payload(json!({"customerPhoneNumber": null}), &[error]));
    };
    let mut errors = Vec::new();
    let has_phone = record
        .get("phone")
        .and_then(Value::as_str)
        .is_some_and(|phone| !phone.is_empty());
    if !has_phone {
        // lsf's words, as the SMS consent's.
        errors.push(
            UserError::new(
                &["input", "customerId"],
                "A phone number is required to set the WhatsApp consent state.",
            )
            .code("MISSING_ARGUMENT"),
        );
    }
    let consent: Map<String, Value> = input
        .get("whatsAppMarketingConsent")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let wanted = consent
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !["SUBSCRIBED", "UNSUBSCRIBED", "PENDING"].contains(&wanted) {
        // Rails' message for an inclusion validation, as the SMS consent's.
        errors.push(
            UserError::new(
                &["input", "whatsAppMarketingConsent", "state"],
                "State is not included in the list",
            )
            .code("INCLUSION"),
        );
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"customerPhoneNumber": null}), &errors));
    }
    let source = consent
        .get("sourceLocationId")
        .and_then(Value::as_str)
        .filter(|id| state.type_of(id) == Some("Location"))
        .map(str::to_string);
    let value = json!({
        "state": wanted,
        "optInLevel": consent.get("optInLevel").and_then(Value::as_str).unwrap_or("SINGLE_OPT_IN"),
        "updatedAt": consent
            .get("updatedAt")
            .and_then(Value::as_str)
            .map_or_else(|| m.timestamp(), str::to_string),
        "collectedFrom": "OTHER",
        "sourceLocation": source,
    });
    let state = m.state_mut();
    if let Some(record) = state.get_mut(&customer) {
        record.insert("whatsAppMarketingConsent".into(), value);
        crate::domains::customers::contact_fields(record);
    }
    state.touch(&customer);
    state.emit("customers/update", &customer);
    state.emit("customers_whats_app_marketing_consent/update", &customer);
    let phone_number = m
        .state()
        .get(&customer)
        .and_then(|record| record.get("defaultPhoneNumber"))
        .cloned()
        .unwrap_or(Value::Null);
    Ok(payload(json!({"customerPhoneNumber": phone_number}), &[]))
}

/// The shop's privacy features, as `privacySettings` reads them.
fn settings(state: &ShopState) -> Value {
    if let Some(settings) = state.memo(SETTINGS_MEMO).filter(|value| value.is_object()) {
        return settings.clone();
    }
    json!({
        "banner": {"autoManaged": true, "enabled": true},
        "dataSaleOptOutPage": {"autoManaged": true},
        "privacyPolicy": {"autoManaged": true},
    })
}

fn privacy_features_disable(
    m: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let features: Vec<String> = ctx
        .arg("featuresToDisable")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    let mut settings = settings(m.state());
    for feature in &features {
        match feature.as_str() {
            "COOKIE_BANNER" => {
                settings["banner"] = json!({"autoManaged": false, "enabled": false});
            }
            "DATA_SALE_OPT_OUT_PAGE" => {
                settings["dataSaleOptOutPage"] = json!({"autoManaged": false});
            }
            "PRIVACY_POLICY" => settings["privacyPolicy"] = json!({"autoManaged": false}),
            _ => {}
        }
    }
    let state = m.state_mut();
    state.set_memo(SETTINGS_MEMO, settings);
    let shop = state.shop_gid().unwrap_or_default().to_string();
    // No webhook topic: says that the shop changed.
    state.emit(crate::domains::app::CHANGED, &shop);
    Ok(payload(json!({"featuresDisabled": features}), &[]))
}

/// `privacySettings`.
fn privacy_settings(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let mut settings = settings(state);
    let locales: Vec<Value> = state
        .all("ShopLocale")
        .filter_map(|locale| locale.get("locale").cloned())
        .collect();
    settings["privacyPolicy"]["supportedLocales"] = json!(locales);
    settings["banner"]["translations"] = json!([]);
    let value = settings.as_object().cloned().unwrap_or_default();
    Ok(Output::Object(Obj::built("PrivacySettings", value)))
}

fn consent_policy_update(
    m: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let policies: Vec<Map<String, Value>> = ctx
        .arg("consentPolicies")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .cloned()
        .collect();
    let mut errors = Vec::new();
    if settings(m.state())["banner"]["enabled"] == json!(true) {
        // lsf's words for the code's meaning.
        errors.push(
            UserError::new(
                &["consentPolicies"],
                "Shopify's cookie banner must be disabled before consent policies are updated.",
            )
            .code("SHOPIFY_COOKIE_BANNER_NOT_DISABLED"),
        );
    }
    for (index, policy) in policies.iter().enumerate() {
        let index = index.to_string();
        let country = policy.get("countryCode").and_then(Value::as_str);
        let region = policy.get("regionCode").and_then(Value::as_str);
        if country.is_none() {
            errors.push(
                UserError::new(
                    &["consentPolicies", &index, "countryCode"],
                    "Country code is required.",
                )
                .code("COUNTRY_CODE_REQUIRED"),
            );
        }
        if let Some(region) = region
            && (region.is_empty()
                || region.len() > 3
                || !region.chars().all(|c| c.is_ascii_alphanumeric()))
        {
            errors.push(
                UserError::new(
                    &["consentPolicies", &index, "regionCode"],
                    "Region code must match the country code.",
                )
                .code("REGION_CODE_MUST_MATCH_COUNTRY_CODE"),
            );
        }
    }
    if !errors.is_empty() {
        return Ok(payload(json!({"updatedPolicies": null}), &errors));
    }
    let shop = m.state().shop_gid().map(str::to_string);
    let state = m.state_mut();
    let mut updated = Vec::new();
    for policy in policies {
        let country = policy.get("countryCode").cloned().unwrap_or(Value::Null);
        let region = policy.get("regionCode").cloned().unwrap_or(Value::Null);
        let existing = state
            .all("ConsentPolicy")
            .find(|known| {
                known.get("countryCode") == Some(&country)
                    && known.get("regionCode").unwrap_or(&Value::Null) == &region
            })
            .and_then(|known| known.get("id").and_then(Value::as_str))
            .map(str::to_string);
        let id = match existing {
            Some(id) => id,
            None => {
                let id = state.new_gid("ConsentPolicy");
                state.insert_as(
                    id.clone(),
                    crate::state::value::object(json!({
                        "__typename": "ConsentPolicy",
                        "id": id,
                        "countryCode": country,
                        "regionCode": region,
                        "consentRequired": null,
                        "dataSaleOptOutRequired": null,
                        "shopId": shop,
                    })),
                );
                id
            }
        };
        if let Some(record) = state.get_mut(&id) {
            for field in ["consentRequired", "dataSaleOptOutRequired"] {
                if let Some(value) = policy.get(field) {
                    record.insert(field.into(), value.clone());
                }
            }
        }
        state.emit(crate::domains::app::CHANGED, &id);
        updated.push(json!(id));
    }
    Ok(payload(json!({"updatedPolicies": updated}), &[]))
}

/// `consentPolicy(…)`: the policies that match every argument given.
fn consent_policy(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let wanted = |record: &Record| {
        [
            "consentRequired",
            "countryCode",
            "dataSaleOptOutRequired",
            "regionCode",
            "id",
        ]
        .iter()
        .all(|field| match ctx.arg(field) {
            None | Some(Value::Null) => true,
            Some(value) => record.get(*field) == Some(value),
        })
    };
    let policies = r
        .state()
        .all("ConsentPolicy")
        .filter(|record| wanted(record))
        .filter_map(|record| record.get("id").and_then(Value::as_str))
        .map(|id| Output::Object(Obj::record(id)))
        .collect();
    Ok(Output::List(policies))
}

/// `consentPolicyRegions`: every country of `PrivacyCountryCode`.
fn consent_policy_regions(
    _r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let regions = ctx
        .schema()
        .enum_values("PrivacyCountryCode")
        .into_iter()
        .flatten()
        .map(|value| {
            let mut region = Record::new();
            region.insert("countryCode".into(), json!(value.name));
            region.insert("regionCode".into(), Value::Null);
            Output::Object(Obj::built("ConsentPolicyRegion", region))
        })
        .collect();
    Ok(Output::List(regions))
}
