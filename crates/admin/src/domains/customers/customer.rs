//! `customerCreate`, `customerUpdate`, `customerSet`, `customerDelete`,
//! `customerGenerateAccountActivationUrl`, `customerSendAccountInviteEmail`, and the tax
//! exemption mutations.
//!
//! As shopify.dev's reference of 2026-10 (and the REST reference of the same resource, where it
//! shows what GraphQL does not) describes them:
//!
//! - **Required**: "Customer must have a name, phone number or email address" (`field: null`,
//!   the `customerCreate` example).
//! - **Email and phone** are unique among the customers: "Email has already been taken" (the
//!   Storefront API's `TAKEN` answer and the REST reference's `{"phone": ["has already been
//!   taken"]}`, as Rails writes them whole), and must be valid: "Email is invalid", "Phone is
//!   invalid" (Rails' wording; Shopify's is not published). An email is compared ignoring case.
//!   A phone is kept as given without its separators (`+1 (613) 555-0123` is `+16135550123`):
//!   Shopify writes it in E.164 with the shop country's calling code, which lsf cannot add
//!   *(chosen)*; it must have 7 to 15 digits.
//! - **A new customer** is `DISABLED` (the `customers/create` example's `state`), with
//!   `verifiedEmail: true` (the same), the shop's primary locale, no orders, `amountSpent` 0 in
//!   the shop's currency (the example's `"0.0"`), not subscribed to email or SMS marketing unless
//!   the input says so (the examples' `NOT_SUBSCRIBED`, `SINGLE_OPT_IN`).
//! - **`customerUpdate`** changes the fields given (`tags` replace the tags; `addresses`, deprecated,
//!   replace the address book). "Customer does not exist" (`field: ["id"]`) for an id that is
//!   not a customer's (the example).
//! - **`customerSet`** creates without `identifier`, updates the customer it names by `id`, and
//!   upserts by `email`, `phone` or `customId` (created with that value when there is none).
//! - **`customerDelete`**: "You can only delete customers who haven't placed any orders" (the
//!   reference). The customer's company contacts go with it.
//! - **`customerGenerateAccountActivationUrl`**: a URL on the shop's domain in REST's format
//!   (`/account/activate/<id>/<token>-<expiry>`), refused for an enabled account ("account
//!   already enabled", REST's error). Nothing changes, no event.
//! - **`customerSendAccountInviteEmail`**: the customer is `INVITED`; the email is not sent (said
//!   in `extensions.lsf.notes`).
//! - **Events**: `customers/create` (and `customer.tags_added` for the tags it was given);
//!   `customers/update`, then `customer.tags_added`/`customer.tags_removed` for the tags that
//!   changed and the consent topics for the consents that changed; `customers/delete`. Their
//!   order is lsf's: Shopify does not document it.
//!
//! Where a message is not published, the comment where it is written says so.

use serde_json::{Map, Value, json};

use super::addresses::replace_addresses;
use super::consent::{Channel, consent_of, missing_contact};
use super::metafields;
use super::{
    contact_fields, exemptions_changed, gids_of, given_text, object_arg, primary_locale,
    refresh_customer,
};
use crate::domains::catalog::tags_of;
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, payload};
use crate::state::{Record, ShopState, parse_gid, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("customerCreate", customer_create)
        .mutation("customerUpdate", customer_update)
        .mutation("customerSet", customer_set)
        .mutation("customerDelete", customer_delete)
        .mutation(
            "customerGenerateAccountActivationUrl",
            generate_account_activation_url,
        )
        .mutation("customerSendAccountInviteEmail", send_account_invite_email)
        .mutation("customerAddTaxExemptions", add_tax_exemptions)
        .mutation("customerRemoveTaxExemptions", remove_tax_exemptions)
        .mutation("customerReplaceTaxExemptions", replace_tax_exemptions);
}

/// Whether an email has the form of one: something, `@`, a domain with a dot, no spaces.
pub fn is_email(email: &str) -> bool {
    let Some((local, domain)) = email.rsplit_once('@') else {
        return false;
    };
    !local.is_empty()
        && !email.chars().any(char::is_whitespace)
        && domain.split('.').filter(|part| !part.is_empty()).count() >= 2
        && !domain.starts_with('.')
        && !domain.ends_with('.')
}

/// A phone as lsf keeps it: its digits, after a `+` when it had one, without separators; `None`
/// when it is not a phone number (fewer than 7 or more than 15 digits, letters).
fn normalized_phone(phone: &str) -> Option<String> {
    let trimmed = phone.trim();
    let plus = trimmed.starts_with('+');
    let rest = trimmed.trim_start_matches('+');
    if rest
        .chars()
        .any(|c| !(c.is_ascii_digit() || " -().".contains(c)))
    {
        return None;
    }
    let digits: String = rest.chars().filter(char::is_ascii_digit).collect();
    (7..=15)
        .contains(&digits.len())
        .then(|| if plus { format!("+{digits}") } else { digits })
}

/// Whether two values of `field` (`email`, `phone`) are the same: emails ignoring case, phones
/// without their separators (the store data may write `+1 555 010 9988`).
fn same(field: &str, a: &str, b: &str) -> bool {
    if field == "phone"
        && let (Some(a), Some(b)) = (normalized_phone(a), normalized_phone(b))
    {
        return a == b;
    }
    a.eq_ignore_ascii_case(b)
}

/// The first customer other than `except` whose `field` (`email`, `phone`) is `value`.
fn holder<'s>(
    state: &'s ShopState,
    field: &str,
    value: &str,
    except: Option<&str>,
) -> Option<&'s str> {
    state.keys("Customer").find(|key| {
        Some(*key) != except
            && state
                .get(key)
                .and_then(|customer| customer.get(field))
                .and_then(Value::as_str)
                .is_some_and(|other| same(field, other, value))
    })
}

/// An error, with the code the payload's error type has (`CustomerSetUserError`), which the
/// others ignore.
fn error(field: &[&str], message: &str, code: &str) -> UserError {
    UserError::new(field, message).code(code)
}

/// What a customer input (`CustomerInput`, `CustomerSetInput`) changes, checked: the email and
/// the phone as kept, the consents as the record keeps them.
struct Checked {
    email: Option<Option<String>>,
    phone: Option<Option<String>>,
    email_consent: Option<Value>,
    sms_consent: Option<Value>,
}

/// Checks a customer input against the shop, for the customer `existing` (`None`: one being
/// created). Errors in `errors`.
fn check(
    state: &ShopState,
    input: &Map<String, Value>,
    existing: Option<&str>,
    errors: &mut Vec<UserError>,
) -> Checked {
    let record = existing.and_then(|existing| state.get(existing));
    let email = given_text(input, "email");
    if let Some(Some(email)) = &email {
        if !is_email(email) {
            errors.push(error(&["email"], "Email is invalid", "INVALID"));
        } else if holder(state, "email", email, existing).is_some() {
            errors.push(error(&["email"], "Email has already been taken", "TAKEN"));
        }
    }
    let phone = given_text(input, "phone").map(|phone| match phone {
        Some(given) => match normalized_phone(&given) {
            Some(phone) => {
                if holder(state, "phone", &phone, existing).is_some() {
                    errors.push(error(&["phone"], "Phone has already been taken", "TAKEN"));
                }
                Some(phone)
            }
            None => {
                errors.push(error(&["phone"], "Phone is invalid", "INVALID"));
                None
            }
        },
        None => None,
    });
    // What the customer will have: what is given, else what it has.
    let current = |field: &str| {
        record
            .and_then(|record| record.get(field))
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    };
    let will_have = |given: &Option<Option<String>>, field: &str| match given {
        Some(value) => value.clone(),
        None => current(field),
    };
    let final_email = will_have(&email, "email");
    let final_phone = will_have(&phone, "phone");
    let named = ["firstName", "lastName"]
        .iter()
        .any(|field| will_have(&given_text(input, field), field).is_some());
    if final_email.is_none() && final_phone.is_none() && !named {
        errors.push(error(
            &[],
            "Customer must have a name, phone number or email address",
            "BLANK",
        ));
    }
    let mut consent = |channel: Channel, has: bool| {
        let given = input.get(channel.field()).and_then(Value::as_object)?;
        if !has {
            errors.push(error(
                &[channel.field()],
                missing_contact(channel),
                "INVALID",
            ));
        }
        Some(consent_of(
            state,
            channel,
            given,
            false,
            &[channel.field()],
            errors,
        ))
    };
    let email_consent = consent(Channel::Email, final_email.is_some());
    let sms_consent = consent(Channel::Sms, final_phone.is_some());
    metafields::check(state, existing, input.get("metafields"), errors);
    Checked {
        email,
        phone,
        email_consent,
        sms_consent,
    }
}

/// Notes the inputs lsf does not simulate.
fn note_ignored(mutation: &mut Mutation<'_>, name: &str, input: &Map<String, Value>) {
    if input
        .get("whatsAppMarketingConsent")
        .is_some_and(|value| !value.is_null())
    {
        mutation.note(format!(
            "{name}: lsf does not simulate `whatsAppMarketingConsent` yet: it was ignored"
        ));
    }
}

/// Sets the fields of a customer record from a checked input: those given, and only those.
fn apply(record: &mut Record, input: &Map<String, Value>, checked: &Checked) {
    for field in ["firstName", "lastName", "note", "multipassIdentifier"] {
        if let Some(text) = given_text(input, field) {
            record.insert(field.into(), json!(text));
        }
    }
    if let Some(locale) = given_text(input, "locale").flatten() {
        record.insert("locale".into(), json!(locale));
    }
    if let Some(email) = &checked.email {
        record.insert("email".into(), json!(email));
    }
    if let Some(phone) = &checked.phone {
        record.insert("phone".into(), json!(phone));
    }
    if input.contains_key("tags") {
        record.insert("tags".into(), json!(tags_of(input.get("tags"))));
    }
    if let Some(exempt) = input.get("taxExempt").and_then(Value::as_bool) {
        record.insert("taxExempt".into(), json!(exempt));
    }
    if let Some(exemptions) = input.get("taxExemptions") {
        let exemptions = exemptions.as_array().cloned().unwrap_or_default();
        record.insert("taxExemptions".into(), json!(exemptions));
    }
    if let Some(consent) = &checked.email_consent {
        record.insert(Channel::Email.field().into(), consent.clone());
    }
    if let Some(consent) = &checked.sms_consent {
        record.insert(Channel::Sms.field().into(), consent.clone());
    }
    contact_fields(record);
}

/// A new customer record, before its input applies.
fn new_customer(state: &mut ShopState) -> Record {
    let id = state.new_gid("Customer");
    let now = state.timestamp();
    value::object(json!({
        "__typename": "Customer",
        "id": id,
        "firstName": null,
        "lastName": null,
        "displayName": "",
        "email": null,
        "phone": null,
        "note": null,
        "multipassIdentifier": null,
        "defaultEmailAddress": null,
        "defaultPhoneNumber": null,
        "emailMarketingConsent": null,
        "smsMarketingConsent": null,
        "addresses": [],
        "addressesV2": [],
        "defaultAddress": null,
        "state": "DISABLED",
        "verifiedEmail": true,
        "taxExempt": false,
        "taxExemptions": [],
        "dataSaleOptOut": false,
        "tags": [],
        "locale": primary_locale(state),
        "companyContactProfiles": [],
        "orders": [],
        "createdAt": now,
        "updatedAt": now,
        "metafields": [],
    }))
}

/// Creates a customer from a checked input and emits its events. Returns its GID.
fn create(
    state: &mut ShopState,
    input: &Map<String, Value>,
    checked: &Checked,
    custom_id: Option<&Value>,
) -> String {
    let mut record = new_customer(state);
    apply(&mut record, input, checked);
    let customer = state.insert(record);
    if let Some(addresses) = input.get("addresses").and_then(Value::as_array) {
        replace_addresses(state, &customer, addresses);
    }
    metafields::apply(state, &customer, "CUSTOMER", input.get("metafields"));
    if let Some(custom_id) = custom_id {
        metafields::apply(state, &customer, "CUSTOMER", Some(custom_id));
    }
    refresh_customer(state, &customer);
    state.emit("customers/create", &customer);
    let tagged = state
        .get(&customer)
        .and_then(|record| record.get("tags"))
        .and_then(Value::as_array)
        .is_some_and(|tags| !tags.is_empty());
    if tagged {
        let mut before = state.get(&customer).cloned().unwrap_or_default();
        before.insert("tags".into(), json!([]));
        state.emit_with("customer.tags_added", &customer, before);
    }
    customer
}

/// Updates a customer from a checked input and emits its events.
fn update(state: &mut ShopState, customer: &str, input: &Map<String, Value>, checked: &Checked) {
    let Some(before) = state.get(customer).cloned() else {
        return;
    };
    if let Some(record) = state.get_mut(customer) {
        apply(record, input, checked);
    }
    if let Some(addresses) = input.get("addresses").and_then(Value::as_array) {
        replace_addresses(state, customer, addresses);
    }
    metafields::apply(state, customer, "CUSTOMER", input.get("metafields"));
    state.touch(customer);
    state.emit("customers/update", customer);
    let after = state.get(customer).cloned().unwrap_or_default();
    let tags = |record: &Record| tags_of(record.get("tags"));
    let (old, new) = (tags(&before), tags(&after));
    let missing = |from: &[String], against: &[String]| {
        from.iter()
            .any(|tag| !against.iter().any(|other| other.eq_ignore_ascii_case(tag)))
    };
    if missing(&new, &old) {
        state.emit_with("customer.tags_added", customer, before.clone());
    }
    if missing(&old, &new) {
        state.emit_with("customer.tags_removed", customer, before.clone());
    }
    for channel in [Channel::Email, Channel::Sms] {
        if before.get(channel.field()) != after.get(channel.field()) {
            state.emit(channel.topic(), customer);
        }
    }
}

/// Checks the customer fields of another input (a company contact's `email`, `firstName`,
/// `lastName`, `phone`), its errors under `path` (`["input"]`). `existing`: the customer it
/// changes, `None` for one being created.
pub(super) fn check_fields(
    state: &ShopState,
    input: &Map<String, Value>,
    existing: Option<&str>,
    path: &[&str],
    errors: &mut Vec<UserError>,
) {
    let mut own = Vec::new();
    check(state, input, existing, &mut own);
    for mut error in own {
        if let Some(field) = &mut error.field {
            let mut full: Vec<String> = path.iter().map(|part| part.to_string()).collect();
            full.append(field);
            *field = full;
        }
        errors.push(error);
    }
}

/// Creates a customer from an input [`check_fields`] found right (a company contact's), with
/// its events. Returns its GID.
pub(super) fn create_customer(state: &mut ShopState, input: &Map<String, Value>) -> String {
    let checked = check(state, input, None, &mut Vec::new());
    create(state, input, &checked, None)
}

/// Updates a customer from an input [`check_fields`] found right, with its events.
pub(super) fn update_customer(state: &mut ShopState, customer: &str, input: &Map<String, Value>) {
    let checked = check(state, input, Some(customer), &mut Vec::new());
    update(state, customer, input, &checked);
}

fn customer_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let input = object_arg(ctx, "input");
    note_ignored(mutation, "customerCreate", &input);
    let mut errors = Vec::new();
    let checked = check(mutation.state(), &input, None, &mut errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"customer": null}), &errors));
    }
    let customer = create(mutation.state_mut(), &input, &checked, None);
    Ok(payload(json!({"customer": customer}), &[]))
}

fn customer_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let input = object_arg(ctx, "input");
    note_ignored(mutation, "customerUpdate", &input);
    let state = mutation.state();
    let id = input.get("id").and_then(Value::as_str).unwrap_or_default();
    if state.type_of(id) != Some("Customer") {
        let error = UserError::new(&["id"], "Customer does not exist");
        return Ok(payload(json!({"customer": null}), &[error]));
    }
    let customer = id.to_string();
    let mut errors = Vec::new();
    let checked = check(state, &input, Some(&customer), &mut errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"customer": null}), &errors));
    }
    update(mutation.state_mut(), &customer, &input, &checked);
    Ok(payload(json!({"customer": customer}), &[]))
}

/// The customer a `customId` names: the owner of the metafield of that namespace, key and
/// value.
fn by_custom_id(state: &ShopState, custom: &Map<String, Value>) -> Option<String> {
    let text = |name: &str| custom.get(name).and_then(Value::as_str);
    let app = state
        .keys("App")
        .next()
        .and_then(parse_gid)
        .map_or(0, |(_, id)| id);
    let namespace =
        crate::domains::content::admin_namespace(app, text("namespace").unwrap_or("$app"));
    state.keys("Customer").find_map(|customer| {
        let found = gids_of(state.get(customer), "metafields")
            .iter()
            .filter_map(|metafield| state.get(metafield))
            .any(|metafield| {
                let get = |name: &str| metafield.get(name).and_then(Value::as_str);
                get("namespace") == Some(namespace.as_str())
                    && get("key") == text("key")
                    && get("value") == text("value")
            });
        found.then(|| customer.to_string())
    })
}

fn customer_set(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let mut input = object_arg(ctx, "input");
    note_ignored(mutation, "customerSet", &input);
    let identifier = object_arg(ctx, "identifier");
    let state = mutation.state();
    let failed = |errors: Vec<UserError>| Ok(payload(json!({"customer": null}), &errors));
    // Not published: the codes are the payload's, the messages lsf's.
    if input.get("id").is_some_and(|id| !id.is_null()) && !identifier.is_empty() {
        return failed(vec![error(
            &["input", "id"],
            "The id field is not allowed if identifier is provided",
            "ID_NOT_ALLOWED",
        )]);
    }
    let mut custom_id = None;
    let target = if let Some(id) = identifier
        .get("id")
        .or_else(|| input.get("id"))
        .and_then(Value::as_str)
    {
        if state.type_of(id) != Some("Customer") {
            return failed(vec![error(
                &["identifier", "id"],
                "Resource matching the identifier was not found",
                "NOT_FOUND",
            )]);
        }
        Some(id.to_string())
    } else if let Some((key, wanted)) = ["email", "phone"]
        .iter()
        .find_map(|key| Some((*key, identifier.get(*key)?.as_str()?.to_string())))
    {
        let wanted = if key == "phone" {
            normalized_phone(&wanted).unwrap_or(wanted)
        } else {
            wanted
        };
        if let Some(given) = given_text(&input, key).flatten()
            && !same(key, &given, &wanted)
        {
            return failed(vec![error(
                &["input", key],
                "The identifier value does not match the value of the corresponding field in the input",
                "INPUT_MISMATCH",
            )]);
        }
        let found = holder(state, key, &wanted, None).map(str::to_string);
        // Created with that value when there is none; the one found keeps its own.
        if found.is_none() {
            input.insert(key.to_string(), json!(wanted));
        }
        found
    } else if let Some(custom) = identifier.get("customId").and_then(Value::as_object) {
        let found = by_custom_id(state, custom);
        if found.is_none() {
            custom_id = Some(json!([{
                "namespace": custom.get("namespace").and_then(Value::as_str).unwrap_or("$app"),
                "key": custom.get("key"),
                "type": "id",
                "value": custom.get("value"),
            }]));
        }
        found
    } else {
        None
    };
    let mut errors = Vec::new();
    let checked = check(state, &input, target.as_deref(), &mut errors);
    if let Some(custom_id) = &custom_id {
        metafields::check(state, None, Some(custom_id), &mut errors);
    }
    if !errors.is_empty() {
        return failed(errors);
    }
    let state = mutation.state_mut();
    let customer = match target {
        Some(customer) => {
            update(state, &customer, &input, &checked);
            customer
        }
        None => create(state, &input, &checked, custom_id.as_ref()),
    };
    Ok(payload(json!({"customer": customer}), &[]))
}

fn customer_delete(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx
        .arg("input")
        .and_then(|input| input.get("id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let state = mutation.state();
    let failed = |message: &str| {
        let error = UserError::new(&["id"], message);
        Ok(payload(json!({"deletedCustomerId": null}), &[error]))
    };
    let Some(record) = state.get_of(&id, "Customer") else {
        return failed("Customer does not exist");
    };
    if !gids_of(Some(record), "orders").is_empty() {
        // "You can only delete customers who haven't placed any orders" (the reference); the
        // message of the error is not published.
        return failed("Customer can\u{2019}t be deleted because they have associated orders");
    }
    let record = record.clone();
    let state = mutation.state_mut();
    for contact in gids_of(Some(&record), "companyContactProfiles") {
        super::contacts::delete_contact(state, &contact);
    }
    metafields::remove_all(state, &record);
    state.delete(&id, "customers/delete");
    Ok(payload(json!({"deletedCustomerId": id}), &[]))
}

/// The customer of a `customerId` argument, or the error of one that does not exist.
fn customer_arg(state: &ShopState, ctx: &FieldContext<'_>) -> Result<String, UserError> {
    let id = ctx.arg_str("customerId").unwrap_or_default();
    if state.type_of(id) == Some("Customer") {
        Ok(id.to_string())
    } else {
        Err(error(&["customerId"], "Customer does not exist", "INVALID"))
    }
}

/// Whether a customer's account is enabled.
fn enabled(state: &ShopState, customer: &str) -> bool {
    state
        .get(customer)
        .and_then(|record| record.get("state"))
        .and_then(Value::as_str)
        == Some("ENABLED")
}

fn generate_account_activation_url(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let customer = match customer_arg(state, ctx) {
        Ok(customer) => customer,
        Err(error) => return Ok(payload(json!({"accountActivationUrl": null}), &[error])),
    };
    if enabled(state, &customer) {
        // REST's error is `["account already enabled"]`.
        let error = UserError::new(&["customerId"], "Account already enabled");
        return Ok(payload(json!({"accountActivationUrl": null}), &[error]));
    }
    let origin = state
        .shop()
        .and_then(|shop| shop.get("url"))
        .and_then(Value::as_str)
        .map_or_else(|| format!("https://{}", state.domain()), str::to_string);
    let number = parse_gid(&customer).map_or(0, |(_, id)| id);
    // REST's example: `…/account/activate/207119551/e7c01ca8ca79cd849f7c09f9e5450f9c-1790864247`,
    // a token and the time it expires, 30 days on.
    let expires = mutation.now().timestamp() + 30 * 24 * 3600;
    let token = value::uuid(&format!("activation/{customer}/{expires}")).replace('-', "");
    let url = format!("{origin}/account/activate/{number}/{token}-{expires}");
    Ok(payload(json!({"accountActivationUrl": url}), &[]))
}

fn send_account_invite_email(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let customer = match customer_arg(state, ctx) {
        Ok(customer) => customer,
        Err(error) => return Ok(payload(json!({"customer": null}), &[error])),
    };
    let email = state
        .get(&customer)
        .and_then(|record| record.get("email"))
        .and_then(Value::as_str)
        .filter(|email| !email.is_empty())
        .map(str::to_string);
    // Not published: lsf's words, with the payload's only code.
    let problem = if enabled(state, &customer) {
        Some("Account already enabled")
    } else if email.is_none()
        && ctx
            .arg("email")
            .and_then(|email| email.get("to"))
            .is_none_or(Value::is_null)
    {
        Some("Customer has no email address")
    } else {
        None
    };
    if let Some(message) = problem {
        let error = error(&["customerId"], message, "INVALID");
        return Ok(payload(json!({"customer": null}), &[error]));
    }
    let to = ctx
        .arg("email")
        .and_then(|email| email.get("to"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(email)
        .unwrap_or_default();
    mutation.note(format!(
        "customerSendAccountInviteEmail: lsf does not send emails: the invitation to {to} was \
         recorded (the customer is INVITED), not sent"
    ));
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&customer) {
        record.insert("state".into(), json!("INVITED"));
    }
    state.touch(&customer);
    state.emit("customers/update", &customer);
    Ok(payload(json!({"customer": customer}), &[]))
}

fn add_tax_exemptions(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    tax_exemptions(mutation, ctx, |current, given| {
        exemptions_changed(current, given, &[])
    })
}

fn remove_tax_exemptions(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    tax_exemptions(mutation, ctx, |current, given| {
        exemptions_changed(current, &[], given)
    })
}

fn replace_tax_exemptions(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    tax_exemptions(mutation, ctx, |_, given| json!(given))
}

/// `customerAddTaxExemptions`, `customerRemoveTaxExemptions`, `customerReplaceTaxExemptions`:
/// `customers/update`.
fn tax_exemptions(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    change: fn(Option<&Value>, &[Value]) -> Value,
) -> Result<Value, FieldError> {
    let customer = match customer_arg(mutation.state(), ctx) {
        Ok(customer) => customer,
        Err(error) => return Ok(payload(json!({"customer": null}), &[error])),
    };
    let given = ctx
        .arg("taxExemptions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&customer) {
        let exemptions = change(record.get("taxExemptions"), &given);
        record.insert("taxExemptions".into(), exemptions);
    }
    state.touch(&customer);
    state.emit("customers/update", &customer);
    Ok(payload(json!({"customer": customer}), &[]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_emails_and_phones() {
        assert!(is_email("jane@example.com"));
        assert!(is_email("j.doe+tag@mail.example.co.uk"));
        assert!(!is_email("jane"));
        assert!(!is_email("jane@example"));
        assert!(!is_email("jane doe@example.com"));
        assert!(!is_email("@example.com"));
        assert_eq!(
            normalized_phone("+1 (613) 555-0123").as_deref(),
            Some("+16135550123")
        );
        assert_eq!(
            normalized_phone("06 12 34 56 78").as_deref(),
            Some("0612345678")
        );
        assert_eq!(normalized_phone("12345"), None);
        assert_eq!(normalized_phone("call me"), None);
    }
}
