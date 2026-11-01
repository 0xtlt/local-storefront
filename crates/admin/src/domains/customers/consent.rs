//! Email and SMS marketing consent: `customerEmailMarketingConsentUpdate`,
//! `customerSmsMarketingConsentUpdate`, and the consents `customerCreate` and `customerUpdate`
//! take.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - The customer must have an email address (a phone number) for its email (SMS) consent. Only
//!   `SUBSCRIBED`, `UNSUBSCRIBED` and `PENDING` can be set through the consent mutations
//!   ("NOT_SUBSCRIBED, REDACTED, and INVALID cannot be set via this mutation"); a customer's input
//!   also takes `NOT_SUBSCRIBED`, the state of a new customer.
//! - The opt-in level defaults to `SINGLE_OPT_IN`, and `consentUpdatedAt` to now (the examples'
//!   responses show the time of the request when none is given); an SMS consent is
//!   `consentCollectedFrom: OTHER` (the examples' value for consent an app collects).
//! - Events: `customers/update` (its payload has the consents), then
//!   `customers_email_marketing_consent/update` or `customers_marketing_consent/update` (SMS).
//!   The order is lsf's: Shopify does not document it.
//!
//! The messages of a customer without an email or a phone are not published: lsf's are said
//! where they are written.

use serde_json::{Map, Value, json};

use super::contact_fields;
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::ShopState;

pub fn register(registry: &mut Registry) {
    registry
        .mutation(
            "customerEmailMarketingConsentUpdate",
            email_marketing_consent_update,
        )
        .mutation(
            "customerSmsMarketingConsentUpdate",
            sms_marketing_consent_update,
        );
}

/// The email consent of a customer created without one.
pub fn default_email_consent() -> Value {
    json!({
        "marketingState": "NOT_SUBSCRIBED",
        "marketingOptInLevel": "SINGLE_OPT_IN",
        "consentUpdatedAt": null,
    })
}

/// The SMS consent of a customer created without one.
pub fn default_sms_consent() -> Value {
    json!({
        "marketingState": "NOT_SUBSCRIBED",
        "marketingOptInLevel": "SINGLE_OPT_IN",
        "consentUpdatedAt": null,
        "consentCollectedFrom": "OTHER",
    })
}

/// Which consent: their record field, what they need, and the states their input takes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Email,
    Sms,
}

impl Channel {
    pub fn field(self) -> &'static str {
        match self {
            Channel::Email => "emailMarketingConsent",
            Channel::Sms => "smsMarketingConsent",
        }
    }

    /// The customer field the consent is about.
    fn contact(self) -> &'static str {
        match self {
            Channel::Email => "email",
            Channel::Sms => "phone",
        }
    }

    /// The topic of a change of the consent.
    pub fn topic(self) -> &'static str {
        match self {
            Channel::Email => "customers_email_marketing_consent/update",
            Channel::Sms => "customers_marketing_consent/update",
        }
    }
}

/// A consent input (`CustomerEmailMarketingConsentInput`, `CustomerSmsMarketingConsentInput`)
/// as the record keeps it, or what is wrong with it. `mutation`: whether it comes from a consent
/// mutation, which takes fewer states than a customer's input. `path` is the input's.
pub fn consent_of(
    state: &ShopState,
    channel: Channel,
    input: &Map<String, Value>,
    mutation: bool,
    path: &[&str],
    errors: &mut Vec<UserError>,
) -> Value {
    let marketing_state = input
        .get("marketingState")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let allowed: &[&str] = if mutation {
        &["SUBSCRIBED", "UNSUBSCRIBED", "PENDING"]
    } else {
        &["SUBSCRIBED", "UNSUBSCRIBED", "PENDING", "NOT_SUBSCRIBED"]
    };
    if !allowed.contains(&marketing_state) {
        let field: Vec<&str> = path.iter().copied().chain(["marketingState"]).collect();
        // Rails' message for an inclusion validation, with the code the payload's error type
        // has for it: Shopify's exact words are not published.
        errors.push(
            UserError::new(&field, "Marketing state is not included in the list").code("INCLUSION"),
        );
    }
    let source = input
        .get("sourceLocationId")
        .and_then(Value::as_str)
        .filter(|id| state.type_of(id) == Some("Location"));
    let mut consent = json!({
        "marketingState": marketing_state,
        "marketingOptInLevel": input
            .get("marketingOptInLevel")
            .and_then(Value::as_str)
            .unwrap_or("SINGLE_OPT_IN"),
        "consentUpdatedAt": input
            .get("consentUpdatedAt")
            .and_then(Value::as_str)
            .map_or_else(|| state.timestamp(), str::to_string),
        "sourceLocation": source,
    });
    if channel == Channel::Sms {
        consent["consentCollectedFrom"] = json!("OTHER");
    }
    consent
}

/// The message of a consent for a customer without the email or the phone it is about. Not
/// published by Shopify: lsf's words.
pub fn missing_contact(channel: Channel) -> &'static str {
    match channel {
        Channel::Email => "An email address is required to set the email marketing consent state.",
        Channel::Sms => "A phone number is required to set the SMS consent state.",
    }
}

fn email_marketing_consent_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    update(mutation, ctx, Channel::Email)
}

fn sms_marketing_consent_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    update(mutation, ctx, Channel::Sms)
}

/// `customerEmailMarketingConsentUpdate` and `customerSmsMarketingConsentUpdate`.
fn update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    channel: Channel,
) -> Result<Value, FieldError> {
    let input = super::object_arg(ctx, "input");
    let customer = input
        .get("customerId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let state = mutation.state();
    let Some(record) = state
        .get(&customer)
        .filter(|_| state.type_of(&customer) == Some("Customer"))
    else {
        let error =
            UserError::new(&["input", "customerId"], "Customer does not exist").code("INVALID");
        return Ok(payload(json!({"customer": null}), &[error]));
    };
    let mut errors = Vec::new();
    let has_contact = record
        .get(channel.contact())
        .and_then(Value::as_str)
        .is_some_and(|contact| !contact.is_empty());
    if !has_contact {
        errors.push(
            UserError::new(&["input", "customerId"], missing_contact(channel))
                .code("MISSING_ARGUMENT"),
        );
    }
    let consent_input = input
        .get(channel.field())
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let consent = consent_of(
        state,
        channel,
        &consent_input,
        true,
        &["input", channel.field()],
        &mut errors,
    );
    if !errors.is_empty() {
        return Ok(payload(json!({"customer": null}), &errors));
    }

    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&customer) {
        record.insert(channel.field().into(), consent);
        contact_fields(record);
    }
    state.touch(&customer);
    state.emit("customers/update", &customer);
    state.emit(channel.topic(), &customer);
    Ok(payload(json!({"customer": customer}), &[]))
}
