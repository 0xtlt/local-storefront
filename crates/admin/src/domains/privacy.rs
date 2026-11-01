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

use crate::graphql::{FieldContext, FieldError, Output};
use crate::outbox::{self, Channel, Entry};
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

/// The prefix of the memos of the data erasures, by customer.
const ERASURE_PREFIX: &str = "privacy/erasure/";

fn erasure_memo(customer: &str) -> String {
    format!("{ERASURE_PREFIX}{customer}")
}

fn is_pending(request: &Value) -> bool {
    request.get("status").and_then(Value::as_str) == Some("PENDING")
}

/// Whether a customer's data erasure is pending.
pub fn erasure_pending(state: &ShopState, customer: &str) -> bool {
    state.memo(&erasure_memo(customer)).is_some_and(is_pending)
}

fn is_customer(state: &ShopState, id: &str) -> bool {
    state.type_of(id) == Some("Customer")
}

/// The answer of the erasure mutations for a `customerId` that is no customer's.
fn no_such_customer() -> Value {
    let error = UserError::new(&["customerId"], "Customer does not exist").code("DOES_NOT_EXIST");
    payload(json!({"customerId": null}), &[error])
}

/// The email of a customer, `""` without one: who the outbox entries of an erasure went to.
fn email_of(state: &ShopState, customer: &str) -> String {
    state
        .get(customer)
        .and_then(|record| record.get("email"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn request_data_erasure(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let customer = ctx.arg_str("customerId").unwrap_or_default().to_string();
    if !is_customer(m.state(), &customer) {
        return Ok(no_such_customer());
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
    let email = email_of(state, &customer);
    outbox::record(
        state,
        Entry::new(Channel::DataErasure, "data_erasure_requested")
            .to(email)
            .about(&customer)
            .body(format!(
                "Data erasure requested: the customer's personal data is erased on {} unless the request is cancelled",
                crate::state::value::date_time(due)
            )),
    );
    // No webhook: Shopify sends `customers/redact` when it erases, not when it is asked.
    state.emit(crate::domains::app::CHANGED, &customer);
    Ok(payload(json!({"customerId": customer}), &[]))
}

fn cancel_data_erasure(m: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let customer = ctx.arg_str("customerId").unwrap_or_default().to_string();
    if !is_customer(m.state(), &customer) {
        return Ok(no_such_customer());
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
        Entry::new(Channel::DataErasure, "data_erasure_cancelled")
            .about(&customer)
            .body("Data erasure cancelled"),
    );
    state.emit(crate::domains::app::CHANGED, &customer);
    Ok(payload(json!({"customerId": customer}), &[]))
}

/// The customers whose data erasure is due by the shop's clock, in the order of the customers.
/// Cheap when none is pending, as it is checked before every request.
pub fn due_erasures(state: &ShopState) -> Vec<String> {
    let pending = state
        .memos_with_prefix(ERASURE_PREFIX)
        .any(|(_, request)| is_pending(request));
    if !pending {
        return Vec::new();
    }
    let now = state.now();
    state
        .keys("Customer")
        .filter(|customer| {
            state
                .memo(&erasure_memo(customer))
                .filter(|request| is_pending(request))
                .and_then(|request| request.get("dueAt").and_then(Value::as_str))
                .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
                .is_some_and(|at| at <= now)
        })
        .map(str::to_string)
        .collect()
}

/// Processes the data erasures whose 10 days passed by the shop's clock: the customers'
/// personal data is redacted. Answers the customers erased as they were before, for the
/// `customers/redact` the server sends.
pub fn process_due_erasures(state: &mut ShopState) -> Vec<Record> {
    let now = state.now();
    let due = due_erasures(state);
    let mut erased = Vec::new();
    for customer in &due {
        if let Some(record) = state.get(customer) {
            erased.push(record.clone());
        }
        let email = email_of(state, customer);
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
            Entry::new(Channel::DataErasure, "customer_data_erased")
                .to(email)
                .about(customer)
                .body("Customer data erased: names, email, phone, note and addresses redacted; customers/redact sent to the app"),
        );
        state.emit("customers/update", customer);
    }
    erased
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
    let input = ctx.arg_object_or_default("input");
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

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::domains::segments::tests::{customer, data, demo, run, topics};

    #[test]
    fn a_data_erasure_waits_ten_days_and_can_be_cancelled() {
        let mut shop = demo();
        let alex = customer(&shop, "alex.morgan@example.com");
        let request = "mutation($id: ID!) { customerRequestDataErasure(customerId: $id) { customerId userErrors { field message code } } }";
        let answer = run(&mut shop, request, json!({"id": alex}));
        assert_eq!(
            data(&answer, "customerRequestDataErasure"),
            &json!({"customerId": alex, "userErrors": []})
        );
        assert!(super::erasure_pending(&shop, &alex));
        let outbox = crate::outbox::entries(&shop, None);
        assert_eq!(outbox[0]["channel"], "data_erasure");
        assert_eq!(outbox[0]["what"], "data_erasure_requested");
        assert!(outbox[0]["body"].as_str().unwrap().contains("2026-11-10"));
        // A second request, and nothing changes.
        let before = shop.picture();
        let again = run(&mut shop, request, json!({"id": alex}));
        assert_eq!(
            data(&again, "customerRequestDataErasure")["userErrors"][0]["code"],
            "FAILED_TO_REQUEST"
        );
        assert_eq!(shop.picture(), before);
        // Not yet due: nothing is erased.
        assert!(super::process_due_erasures(&mut shop).is_empty());

        let cancel = "mutation($id: ID!) { customerCancelDataErasure(customerId: $id) { customerId userErrors { field message code } } }";
        let cancelled = run(&mut shop, cancel, json!({"id": alex}));
        assert_eq!(
            data(&cancelled, "customerCancelDataErasure")["customerId"],
            json!(alex)
        );
        assert!(!super::erasure_pending(&shop, &alex));
        let nothing = run(&mut shop, cancel, json!({"id": alex}));
        assert_eq!(
            data(&nothing, "customerCancelDataErasure")["userErrors"][0]["code"],
            "NOT_BEING_ERASED"
        );

        // Requested again, then ten days later: the personal data is gone.
        run(&mut shop, request, json!({"id": alex}));
        let later = shop.now() + chrono::Duration::days(11);
        shop.set_clock(crate::state::Clock::Frozen(later));
        shop.take_events();
        let erased = super::process_due_erasures(&mut shop);
        assert_eq!(erased.len(), 1);
        assert_eq!(erased[0]["id"], json!(alex));
        assert_eq!(erased[0]["email"], "alex.morgan@example.com");
        let record = shop.get(&alex).unwrap();
        assert_eq!(record["email"], Value::Null);
        assert_eq!(record["emailMarketingConsent"], Value::Null);
        assert_eq!(topics(&mut shop), ["customers/update"]);
        let missing = run(
            &mut shop,
            request,
            json!({"id": "gid://shopify/Customer/1"}),
        );
        assert_eq!(
            data(&missing, "customerRequestDataErasure")["userErrors"],
            json!([{"field": ["customerId"], "message": "Customer does not exist", "code": "DOES_NOT_EXIST"}])
        );
    }

    #[test]
    fn data_sale_opt_out_marks_or_makes_the_customer() {
        let mut shop = demo();
        let opt_out = "mutation($email: String!) { dataSaleOptOut(email: $email) { customerId userErrors { field message code } } }";
        let jane = customer(&shop, "jane.doe@example.com");
        let answer = run(&mut shop, opt_out, json!({"email": "JANE.DOE@example.com"}));
        assert_eq!(data(&answer, "dataSaleOptOut")["customerId"], json!(jane));
        assert_eq!(shop.get(&jane).unwrap()["dataSaleOptOut"], true);
        assert_eq!(topics(&mut shop), ["customers/update"]);
        let created = run(&mut shop, opt_out, json!({"email": "someone@example.com"}));
        let id = data(&created, "dataSaleOptOut")["customerId"]
            .as_str()
            .unwrap()
            .to_string();
        let read = run(
            &mut shop,
            "query($id: ID!) { customer(id: $id) { email dataSaleOptOut state } }",
            json!({"id": id}),
        );
        assert_eq!(
            data(&read, "customer"),
            &json!({"email": "someone@example.com", "dataSaleOptOut": true, "state": "DISABLED"})
        );
        assert_eq!(topics(&mut shop), ["customers/create"]);
        let before = shop.picture();
        let refused = run(&mut shop, opt_out, json!({"email": "not an email"}));
        assert_eq!(
            data(&refused, "dataSaleOptOut")["userErrors"][0]["code"],
            "FAILED"
        );
        assert_eq!(shop.picture(), before);
    }

    #[test]
    fn the_whatsapp_consent_is_the_phone_numbers() {
        let mut shop = demo();
        let jane = customer(&shop, "jane.doe@example.com");
        let update = "mutation($input: CustomerWhatsAppMarketingConsentUpdateInput!) {
            customerWhatsAppMarketingConsentUpdate(input: $input) {
                customerPhoneNumber { phoneNumber whatsAppMarketingConsent { state optInLevel updatedAt collectedFrom } }
                userErrors { field message code } } }";
        let answer = run(
            &mut shop,
            update,
            json!({"input": {"customerId": jane, "whatsAppMarketingConsent": {"state": "SUBSCRIBED"}}}),
        );
        assert_eq!(
            data(&answer, "customerWhatsAppMarketingConsentUpdate")["customerPhoneNumber"]["whatsAppMarketingConsent"],
            json!({"state": "SUBSCRIBED", "optInLevel": "SINGLE_OPT_IN", "updatedAt": "2026-10-31T12:00:00Z", "collectedFrom": "OTHER"})
        );
        assert_eq!(
            topics(&mut shop),
            [
                "customers/update",
                "customers_whats_app_marketing_consent/update"
            ]
        );
        let payload = crate::webhooks::payload::payload(
            "customers_whats_app_marketing_consent/update",
            shop.get(&jane).unwrap(),
            &crate::api::samples::ShopLookup(&shop),
        )
        .unwrap();
        assert_eq!(
            payload["whats_app_marketing_consent"]["state"],
            "subscribed"
        );
        // No phone, and a state the mutation does not set: nothing changes.
        let alex = customer(&shop, "alex.morgan@example.com");
        let before = shop.picture();
        let refused = run(
            &mut shop,
            update,
            json!({"input": {"customerId": alex, "whatsAppMarketingConsent": {"state": "REDACTED"}}}),
        );
        let codes: Vec<Value> =
            data(&refused, "customerWhatsAppMarketingConsentUpdate")["userErrors"]
                .as_array()
                .unwrap()
                .iter()
                .map(|error| error["code"].clone())
                .collect();
        assert_eq!(codes, [json!("MISSING_ARGUMENT"), json!("INCLUSION")]);
        assert_eq!(shop.picture(), before);
    }

    #[test]
    fn consent_policies_need_the_cookie_banner_off() {
        let mut shop = demo();
        let update = "mutation($policies: [ConsentPolicyInput!]!) { consentPolicyUpdate(consentPolicies: $policies) {
            updatedPolicies { id countryCode regionCode consentRequired dataSaleOptOutRequired shopId } userErrors { field message code } } }";
        let policies = json!({"policies": [{"countryCode": "FR", "consentRequired": true, "dataSaleOptOutRequired": false}]});
        let before = shop.picture();
        let refused = run(&mut shop, update, policies.clone());
        assert_eq!(
            data(&refused, "consentPolicyUpdate")["userErrors"][0]["code"],
            "SHOPIFY_COOKIE_BANNER_NOT_DISABLED"
        );
        assert_eq!(shop.picture(), before);
        let disabled = run(
            &mut shop,
            "mutation { privacyFeaturesDisable(featuresToDisable: [COOKIE_BANNER]) { featuresDisabled userErrors { message } } }",
            json!({}),
        );
        assert_eq!(
            data(&disabled, "privacyFeaturesDisable")["featuresDisabled"],
            json!(["COOKIE_BANNER"])
        );
        let settings = run(
            &mut shop,
            "{ privacySettings { banner { enabled autoManaged } dataSaleOptOutPage { autoManaged } privacyPolicy { autoManaged supportedLocales } } }",
            json!({}),
        );
        assert_eq!(
            data(&settings, "privacySettings")["banner"],
            json!({"enabled": false, "autoManaged": false})
        );
        assert!(settings["extensions"].get("lsf").is_none(), "{settings}");
        let updated = run(&mut shop, update, policies);
        let policy = &data(&updated, "consentPolicyUpdate")["updatedPolicies"][0];
        assert_eq!(policy["countryCode"], "FR");
        assert_eq!(policy["consentRequired"], true);
        assert_eq!(policy["shopId"], json!(shop.shop_gid()));
        let listed = run(
            &mut shop,
            "{ consentPolicy(countryCode: FR) { countryCode } all: consentPolicy { id } consentPolicyRegions { countryCode } }",
            json!({}),
        );
        assert_eq!(
            data(&listed, "consentPolicy"),
            &json!([{"countryCode": "FR"}])
        );
        assert!(
            listed["data"]["consentPolicyRegions"]
                .as_array()
                .unwrap()
                .len()
                > 200
        );
        let missing = run(
            &mut shop,
            update,
            json!({"policies": [{"consentRequired": true}]}),
        );
        assert_eq!(
            data(&missing, "consentPolicyUpdate")["userErrors"][0]["code"],
            "COUNTRY_CODE_REQUIRED"
        );
    }
}
