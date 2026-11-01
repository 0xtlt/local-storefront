//! Customer payment methods: `customerPaymentMethodCreditCardCreate`, `…CreditCardUpdate`,
//! `…PaypalBillingAgreementCreate`, `…PaypalBillingAgreementUpdate`, `…RemoteCreate`,
//! `…Revoke`, `…GetUpdateUrl`, `…SendUpdateEmail`, `paymentInstrumentSendAddEmail`; the
//! `customerPaymentMethod` query and `Customer.paymentMethods`.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - A card comes from a card vault session (`sessionId`): "These values are only obtained
//!   through card imports happening from a PCI compliant environment". lsf's vault is
//!   `POST /__lsf/admin/card-sessions`, which takes the body Shopify's card vault takes
//!   (`{"credit_card": {"number", "first_name", "last_name", "month", "year",
//!   "verification_value"}}`) and answers `{"id"}`: the session holds the card's brand, BIN,
//!   last digits, expiry and name, never its number ([`vault_session`]).
//! - A PayPal billing agreement's id starts with `B-` ("The billing agreement ID (starting with
//!   'B-') must be obtained from PayPal").
//! - A remote method (`stripePaymentMethod`, `braintreePaymentMethod`,
//!   `authorizeNetCustomerPaymentProfile`, exactly one: `EXACTLY_ONE_REMOTE_REFERENCE_REQUIRED`)
//!   is imported at once: Shopify imports it asynchronously, lsf has nothing to wait for.
//! - Revoking fails while an active contract uses the method ("Revocation will fail if the
//!   payment method has active subscription contracts"); a revoked method keeps its record, with
//!   `revokedAt` and `revokedReason: MANUALLY_REVOKED`, and is hidden unless `showRevoked`.
//! - `customerPaymentMethodGetUpdateUrl` "only supports Shop Pay": another instrument is
//!   `INVALID_INSTRUMENT`. The URL is a page of lsf where the customer gives a new card.
//! - The emails are not sent: they are written to the shop's outbox ([`crate::outbox`]), with
//!   the link they would carry; `GET /__lsf/admin/subscriptions` lists them ([`emails`]) with
//!   the charges.
//! - Events: `customer_payment_methods/create`, `/update`, `/revoke`.
//!
//! **What a charge does** with a method is lsf's convention, read when the method is made and
//! kept as `lsfOutcome` (`success`, `challenge`, or the `SubscriptionBillingAttemptErrorCode` the
//! charge fails with): the test cards of Shopify Payments' test mode for a card, the test
//! payment method ids of Stripe and the words they are made of for a remote method, the
//! `inactive` flag of a PayPal agreement. See [`card_outcome`] and [`remote_outcome`].

use base64::Engine;
use serde_json::{Map, Value, json};

use super::model::{CONTRACT, PAYMENT_METHOD, Pointer, invalid_error, object, text};
use crate::domains::catalog::gids;
use crate::graphql::{FieldContext, FieldError, Output};
use crate::outbox::{Channel, Entry};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::{Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("customerPaymentMethodCreditCardCreate", credit_card_create)
        .mutation("customerPaymentMethodCreditCardUpdate", credit_card_update)
        .mutation("customerPaymentMethodGetUpdateUrl", get_update_url)
        .mutation(
            "customerPaymentMethodPaypalBillingAgreementCreate",
            paypal_create,
        )
        .mutation(
            "customerPaymentMethodPaypalBillingAgreementUpdate",
            paypal_update,
        )
        .mutation("customerPaymentMethodRemoteCreate", remote_create)
        .mutation("customerPaymentMethodRevoke", revoke)
        .mutation("customerPaymentMethodSendUpdateEmail", send_update_email)
        .mutation("paymentInstrumentSendAddEmail", send_add_email)
        .query("customerPaymentMethod", payment_method)
        .field("Customer", "paymentMethods", customer_payment_methods)
        .field(
            "CustomerPaymentMethod",
            "subscriptionContracts",
            method_contracts,
        )
        .field("CustomerCreditCard", "expiresSoon", expires_soon)
        .field("CustomerShopPayAgreement", "expiresSoon", expires_soon)
        .field("CustomerCreditCard", "isRevocable", is_revocable)
        .field(
            "CustomerPaypalBillingAgreement",
            "isRevocable",
            is_revocable,
        )
        .field("CustomerShopPayAgreement", "isRevocable", is_revocable)
        .field(
            "SubscriptionContract",
            "customerPaymentMethod",
            contract_method,
        )
        .field(
            "SubscriptionDraft",
            "customerPaymentMethod",
            contract_method,
        )
        .field(
            "SubscriptionBillingCycleEditedContract",
            "customerPaymentMethod",
            contract_method,
        )
        .field(
            "SubscriptionCalculatedContract",
            "customerPaymentMethod",
            contract_method,
        );
}

/// The emails of the family, as the outbox names them (`what`).
pub const EMAILS: [&str; 2] = ["customer_payment_method_update", "payment_instrument_add"];

/// The prefix of lsf's card vault sessions.
const SESSION_PREFIX: &str = "lsf-card-";

// ---- the test-card convention ---------------------------------------------------------------

/// The brand of a card number, as Shopify names it (`Visa`, `Mastercard`, `American Express`,
/// …); the Bogus gateway's numbers (`1`, `2`, `3`) are `Bogus`.
pub fn brand(number: &str) -> &'static str {
    let prefix = |length: usize| {
        number
            .get(..length)
            .and_then(|digits| digits.parse::<u32>().ok())
    };
    if number.len() == 1 {
        return "Bogus";
    }
    match (prefix(1), prefix(2), prefix(3), prefix(4)) {
        (Some(4), ..) => "Visa",
        (_, Some(34 | 37), ..) => "American Express",
        (_, Some(51..=55), ..) | (.., Some(2221..=2720)) => "Mastercard",
        (.., Some(6011)) | (_, Some(65), ..) | (_, _, Some(644..=649), _) => "Discover",
        (_, Some(35), ..) => "JCB",
        (_, Some(30 | 36 | 38), ..) => "Diners Club",
        _ => "Visa",
    }
}

/// What charging a card does, by its number: the test cards of Shopify Payments' test mode
/// (Shopify's Help Center, "Testing Shopify Payments") and of the Bogus gateway (`1` succeeds,
/// `2` is declined, `3` fails); any other number succeeds.
///
/// | Number | A charge |
/// |---|---|
/// | `4242 4242 4242 4242`, `5555 5555 5555 4444`, any other, `1` | succeeds |
/// | `4000 0000 0000 0002`, `2` | `CARD_DECLINED` |
/// | `4000 0000 0000 9995` | `INSUFFICIENT_FUNDS` |
/// | `4000 0000 0000 0069` | `EXPIRED_CARD` |
/// | `4000 0000 0000 0127` | `INCORRECT_CVC` |
/// | `4000 0000 0000 0119`, `3` | `PROCESSING_ERROR` |
/// | `4242 4242 4242 4241` | `INCORRECT_NUMBER` |
/// | `4000 0000 0000 3220`, `4000 0025 0000 3155`, `4000 0000 0000 3063` | needs the customer's 3D Secure challenge |
pub fn card_outcome(number: &str) -> &'static str {
    match number {
        "4000000000000002" | "2" => "CARD_DECLINED",
        "4000000000009995" => "INSUFFICIENT_FUNDS",
        "4000000000000069" => "EXPIRED_CARD",
        "4000000000000127" => "INCORRECT_CVC",
        "4000000000000119" | "3" => "PROCESSING_ERROR",
        "4242424242424241" => "INCORRECT_NUMBER",
        "4000000000003220" | "4000002500003155" | "4000000000003063" => "challenge",
        _ => "success",
    }
}

/// What charging a remote method does, by its gateway id: the words Stripe's test payment
/// method ids are made of (`pm_card_chargeDeclinedInsufficientFunds`,
/// `pm_card_authenticationRequired`, …), in any id: `insufficient`, `expired`, `incorrectcvc`,
/// `processingerror`, `declined`, `authentication`, `threedsecure` or `3ds`; any other id
/// succeeds.
pub fn remote_outcome(id: &str) -> &'static str {
    let id = id.to_ascii_lowercase().replace(['_', '-'], "");
    [
        ("insufficient", "INSUFFICIENT_FUNDS"),
        ("expired", "EXPIRED_CARD"),
        ("incorrectcvc", "INCORRECT_CVC"),
        ("processingerror", "PROCESSING_ERROR"),
        ("declined", "CARD_DECLINED"),
        ("authentication", "challenge"),
        ("threedsecure", "challenge"),
        ("3ds", "challenge"),
    ]
    .iter()
    .find(|(word, _)| id.contains(word))
    .map_or("success", |(_, outcome)| outcome)
}

/// A session of lsf's card vault from a card (`{"number", "first_name", "last_name", "month",
/// "year"}`, Shopify's vault's `credit_card`), or why it is refused. The id holds what the
/// payment method shows of the card and what charging it does, not its number.
pub fn vault_session(card: &Map<String, Value>) -> Result<String, String> {
    let number: String = text(card, "number")
        .unwrap_or_default()
        .chars()
        .filter(char::is_ascii_digit)
        .collect();
    if number.is_empty() {
        return Err("credit_card.number is required".to_string());
    }
    let number_of = |field: &str| -> Option<i64> {
        match card.get(field)? {
            Value::Number(number) => number.as_i64(),
            Value::String(text) => text.trim().parse().ok(),
            _ => None,
        }
    };
    let month = number_of("month").filter(|month| (1..=12).contains(month));
    let Some(month) = month else {
        return Err("credit_card.month must be 1 to 12".to_string());
    };
    let Some(mut year) = number_of("year") else {
        return Err("credit_card.year is required".to_string());
    };
    if year < 100 {
        year += 2000;
    }
    let name = [text(card, "first_name"), text(card, "last_name")]
        .into_iter()
        .flatten()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let last = number
        .get(number.len().saturating_sub(4)..)
        .unwrap_or(&number);
    let session = json!({
        "brand": brand(&number),
        "firstDigits": number.get(..6.min(number.len())),
        "lastDigits": last,
        "expiryMonth": month,
        "expiryYear": year,
        "name": name,
        "outcome": card_outcome(&number),
    });
    let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(session.to_string());
    Ok(format!("{SESSION_PREFIX}{encoded}"))
}

/// The card of a vault session, `None` when the id is not one of lsf's vault.
fn session_card(session: &str) -> Option<Map<String, Value>> {
    let encoded = session.strip_prefix(SESSION_PREFIX)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .ok()?;
    serde_json::from_slice::<Value>(&bytes)
        .ok()?
        .as_object()
        .cloned()
}

// ---- records --------------------------------------------------------------------------------

/// A new id of a payment method: Shopify's are a token of 32 hex digits
/// (`gid://shopify/CustomerPaymentMethod/0eccccc666aac73efcd31094ddc4ebf0`), lsf's a UUID of the
/// shop and a counter, without its dashes.
fn method_gid(state: &mut ShopState) -> String {
    let counter = state.new_gid(PAYMENT_METHOD);
    let token = value::uuid(&format!(
        "customer_payment_method/{}/{counter}",
        state.domain()
    ))
    .replace('-', "");
    format!("gid://shopify/{PAYMENT_METHOD}/{token}")
}

/// The token of a payment method (the last part of its GID): its `token` in the webhooks.
pub fn token(gid: &str) -> &str {
    gid.rsplit('/').next().unwrap_or(gid)
}

/// A `CustomerCreditCardBillingAddress` from a `MailingAddressInput`.
fn card_address(input: Option<&Value>) -> Value {
    let Some(input) = input.and_then(Value::as_object) else {
        return Value::Null;
    };
    let code = text(input, "countryCode").map(str::to_ascii_uppercase);
    let country = code
        .as_deref()
        .and_then(lsf_core::store::reference::country_name)
        .map(str::to_string)
        .or_else(|| text(input, "country").map(str::to_string));
    json!({
        "address1": input.get("address1"),
        "city": input.get("city"),
        "country": country,
        "countryCode": code,
        "firstName": input.get("firstName"),
        "lastName": input.get("lastName"),
        "province": input.get("province").or(input.get("provinceCode")),
        "provinceCode": input.get("provinceCode"),
        "zip": input.get("zip"),
    })
}

/// The `CustomerCreditCard` of a vault session's card.
fn card_instrument(
    card: &Map<String, Value>,
    address: Value,
    method: &str,
    source: Option<&str>,
) -> Value {
    let last = text(card, "lastDigits").unwrap_or("4242");
    json!({
        "__typename": "CustomerCreditCard",
        "brand": card.get("brand").cloned().unwrap_or(json!("Visa")),
        "expiryMonth": card.get("expiryMonth").cloned().unwrap_or(json!(12)),
        "expiryYear": card.get("expiryYear").cloned().unwrap_or(json!(2030)),
        "firstDigits": card.get("firstDigits"),
        "lastDigits": last,
        "maskedNumber": format!("•••• •••• •••• {last}"),
        "name": card.get("name").cloned().unwrap_or(json!("")),
        "source": source,
        "virtualLastDigits": null,
        "billingAddress": address,
        "lsfMethod": method,
    })
}

/// Makes a payment method of a customer, emits `customer_payment_methods/create`, and answers
/// its GID.
pub fn create_method(
    state: &mut ShopState,
    customer: &str,
    instrument: impl FnOnce(&str) -> Value,
    outcome: &str,
) -> String {
    let id = method_gid(state);
    let mut record = Record::new();
    record.insert("__typename".into(), json!(PAYMENT_METHOD));
    record.insert("id".into(), json!(id));
    record.insert("customer".into(), json!(customer));
    record.insert("instrument".into(), instrument(&id));
    record.insert("revokedAt".into(), Value::Null);
    record.insert("revokedReason".into(), Value::Null);
    record.insert("mandates".into(), json!([]));
    record.insert("lsfOutcome".into(), json!(outcome));
    record.insert("lsfCreatedAt".into(), json!(state.timestamp()));
    state.insert_as(id.clone(), record);
    state.emit("customer_payment_methods/create", &id);
    id
}

/// A method that is not revoked, by its GID, or the error of the field that names it.
fn live_method(state: &ShopState, id: &str, field: &str) -> Result<Record, UserError> {
    let Some(method) = state
        .get(id)
        .filter(|_| state.type_of(id) == Some(PAYMENT_METHOD))
    else {
        return Err(UserError::new(
            &[field],
            "Customer payment method does not exist",
        ));
    };
    if method.get("revokedAt").is_some_and(|at| !at.is_null()) {
        // lsf's wording.
        return Err(UserError::new(
            &[field],
            "Customer payment method has been revoked",
        ));
    }
    Ok(method.clone())
}

/// The customer of a `customerId`, or its error.
fn customer_arg(state: &ShopState, ctx: &FieldContext<'_>) -> Result<String, UserError> {
    let id = ctx.arg_str("customerId").unwrap_or_default();
    if state.type_of(id) == Some("Customer") {
        Ok(id.to_string())
    } else {
        Err(UserError::new(&["customerId"], "Customer does not exist"))
    }
}

/// The active contracts that charge a payment method.
fn active_contracts(state: &ShopState, method: &str) -> Vec<String> {
    state
        .all(CONTRACT)
        .filter(|contract| text(contract, "customerPaymentMethod") == Some(method))
        .filter(|contract| text(contract, "status") == Some("ACTIVE"))
        .filter_map(|contract| text(contract, "id").map(str::to_string))
        .collect()
}

// ---- mutations ------------------------------------------------------------------------------

/// `customerPaymentMethodCreditCardCreate`: the card of a vault session, `processing: false`
/// (lsf verifies nothing, so the method is there at once).
fn credit_card_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"customerPaymentMethod": null, "processing": null});
    let state = mutation.state();
    let customer = match customer_arg(state, ctx) {
        Ok(customer) => customer,
        Err(error) => return Ok(payload(empty, &[error])),
    };
    let Some(card) = ctx.arg_str("sessionId").and_then(session_card) else {
        // lsf's wording: Shopify's vault answers the error of a session it does not know.
        return Ok(payload(
            empty,
            &[UserError::new(
                &["sessionId"],
                "Session ID is invalid: make one with POST /__lsf/admin/card-sessions",
            )],
        ));
    };
    let address = card_address(ctx.arg("billingAddress"));
    let outcome = text(&card, "outcome").unwrap_or("success").to_string();
    let id = create_method(
        mutation.state_mut(),
        &customer,
        |method| card_instrument(&card, address, method, None),
        &outcome,
    );
    Ok(payload(
        json!({"customerPaymentMethod": id, "processing": false}),
        &[],
    ))
}

/// `customerPaymentMethodCreditCardUpdate`: the card of a new vault session replaces the
/// method's.
fn credit_card_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"customerPaymentMethod": null, "processing": null});
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let method = match live_method(mutation.state(), &id, "id") {
        Ok(method) => method,
        Err(error) => return Ok(payload(empty, &[error])),
    };
    if method
        .pointer("/instrument/__typename")
        .and_then(Value::as_str)
        != Some("CustomerCreditCard")
    {
        return Ok(payload(
            empty,
            &[UserError::new(
                &["id"],
                "Payment method is not a credit card",
            )],
        ));
    }
    let Some(card) = ctx.arg_str("sessionId").and_then(session_card) else {
        return Ok(payload(
            empty,
            &[UserError::new(
                &["sessionId"],
                "Session ID is invalid: make one with POST /__lsf/admin/card-sessions",
            )],
        ));
    };
    let address = card_address(ctx.arg("billingAddress"));
    update_card(mutation.state_mut(), &id, &card, address);
    Ok(payload(
        json!({"customerPaymentMethod": id, "processing": false}),
        &[],
    ))
}

/// Puts a vault session's card in a method (keeping its billing address when `address` is
/// `null`), with `customer_payment_methods/update`: the mutation's, and the update page's.
pub fn update_card(state: &mut ShopState, id: &str, card: &Map<String, Value>, address: Value) {
    let outcome = text(card, "outcome").unwrap_or("success").to_string();
    if let Some(record) = state.get_mut(id) {
        let address = if address.is_null() {
            record
                .pointer("/instrument/billingAddress")
                .cloned()
                .unwrap_or(Value::Null)
        } else {
            address
        };
        record.insert(
            "instrument".into(),
            card_instrument(card, address, id, None),
        );
        record.insert("lsfOutcome".into(), json!(outcome));
    }
    state.emit("customer_payment_methods/update", id);
}

/// Puts the card of a vault session in a method, from the update page: `Err` with why when the
/// method or the session is not one.
pub fn update_from_session(
    state: &mut ShopState,
    token: &str,
    session: &str,
) -> Result<String, String> {
    let id = format!("gid://shopify/{PAYMENT_METHOD}/{token}");
    live_method(state, &id, "id").map_err(|error| error.message)?;
    let card = session_card(session).ok_or("the session is not one of lsf's card vault")?;
    update_card(state, &id, &card, Value::Null);
    Ok(id)
}

/// `customerPaymentMethodPaypalBillingAgreementCreate`.
fn paypal_create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"customerPaymentMethod": null});
    let state = mutation.state();
    let customer = match customer_arg(state, ctx) {
        Ok(customer) => customer,
        Err(error) => return Ok(payload(empty, &[error.code("INVALID")])),
    };
    let agreement = ctx
        .arg_str("billingAgreementId")
        .unwrap_or_default()
        .trim()
        .to_string();
    if !agreement.starts_with("B-") {
        // lsf's wording of shopify.dev's "The billing agreement ID (starting with 'B-')".
        return Ok(payload(
            empty,
            &[invalid_error(
                &["billingAgreementId"],
                "Billing agreement ID must start with B-",
            )],
        ));
    }
    let taken = state.all(PAYMENT_METHOD).any(|method| {
        method.get("lsfReference").and_then(Value::as_str) == Some(agreement.as_str())
    });
    if taken {
        return Ok(payload(
            empty,
            &[UserError::new(
                &["billingAgreementId"],
                "Billing agreement ID has already been taken",
            )
            .code("TAKEN")],
        ));
    }
    let inactive = ctx.arg_bool("inactive").unwrap_or(false);
    let outcome = if inactive {
        "INVALID_CUSTOMER_BILLING_AGREEMENT"
    } else {
        remote_outcome(&agreement)
    };
    let email = state
        .get(&customer)
        .and_then(|record| text(record, "email"))
        .map(str::to_string);
    let address = paypal_address(ctx.arg("billingAddress"));
    let state = mutation.state_mut();
    let id = create_method(
        state,
        &customer,
        |method| {
            json!({
                "__typename": "CustomerPaypalBillingAgreement",
                "billingAddress": address,
                "inactive": inactive,
                "paypalAccountEmail": email,
                "lsfMethod": method,
            })
        },
        outcome,
    );
    if let Some(record) = state.get_mut(&id) {
        record.insert("lsfReference".into(), json!(agreement));
    }
    Ok(payload(json!({"customerPaymentMethod": id}), &[]))
}

/// A `CustomerPaymentInstrumentBillingAddress` from a `MailingAddressInput`.
fn paypal_address(input: Option<&Value>) -> Value {
    let card = card_address(input);
    if card.is_null() {
        return card;
    }
    let name = [card.get("firstName"), card.get("lastName")]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(" ");
    json!({
        "address1": card.get("address1"),
        "city": card.get("city"),
        "country": card.get("country"),
        "countryCode": card.get("countryCode"),
        "name": if name.is_empty() { Value::Null } else { json!(name) },
        "province": card.get("province"),
        "provinceCode": card.get("provinceCode"),
        "zip": card.get("zip"),
    })
}

/// `customerPaymentMethodPaypalBillingAgreementUpdate`: the agreement's billing address.
fn paypal_update(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"customerPaymentMethod": null});
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let method = match live_method(mutation.state(), &id, "id") {
        Ok(method) => method,
        Err(error) => return Ok(payload(empty, &[error.code("INVALID")])),
    };
    if method
        .pointer("/instrument/__typename")
        .and_then(Value::as_str)
        != Some("CustomerPaypalBillingAgreement")
    {
        return Ok(payload(
            empty,
            &[invalid_error(
                &["id"],
                "Payment method is not a PayPal billing agreement",
            )],
        ));
    }
    let address = paypal_address(ctx.arg("billingAddress"));
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&id) {
        record["instrument"]["billingAddress"] = address;
    }
    state.emit("customer_payment_methods/update", &id);
    Ok(payload(json!({"customerPaymentMethod": id}), &[]))
}

/// `customerPaymentMethodRemoteCreate`: a card imported from Stripe, Braintree or
/// Authorize.Net, its charges following [`remote_outcome`]. Its brand is the id's (`visa`,
/// `mastercard`, `amex` in it), Visa otherwise, its last digits those of the test card of the
/// brand: the gateway is not asked.
fn remote_create(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"customerPaymentMethod": null});
    let state = mutation.state();
    let customer = match customer_arg(state, ctx) {
        Ok(customer) => customer,
        Err(error) => return Ok(payload(empty, &[error.code("INVALID")])),
    };
    let reference = object(ctx.arg("remoteReference"));
    let given: Vec<(&str, &Map<String, Value>)> = [
        ("stripePaymentMethod", "Stripe"),
        ("braintreePaymentMethod", "Braintree"),
        ("authorizeNetCustomerPaymentProfile", "Authorize.Net"),
    ]
    .iter()
    .filter_map(|(field, gateway)| {
        reference
            .get(*field)?
            .as_object()
            .map(|value| (*gateway, value))
    })
    .collect();
    let [(gateway, remote)] = given.as_slice() else {
        let code = "EXACTLY_ONE_REMOTE_REFERENCE_REQUIRED";
        return Ok(payload(
            empty,
            &[UserError::new(
                &["remoteReference"],
                super::model::described("CustomerPaymentMethodRemoteUserErrorCode", code),
            )
            .code(code)],
        ));
    };
    let id = [
        "paymentMethodId",
        "paymentMethodToken",
        "customerPaymentProfileId",
    ]
    .iter()
    .find_map(|field| text(remote, field))
    .unwrap_or_default()
    .to_string();
    let reference_key = format!(
        "{gateway}/{}/{id}",
        remote
            .values()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("/")
    );
    if state.all(PAYMENT_METHOD).any(|method| {
        method.get("lsfReference").and_then(Value::as_str) == Some(reference_key.as_str())
    }) {
        return Ok(payload(
            empty,
            &[UserError::new(
                &["remoteReference"],
                "Remote reference has already been taken",
            )
            .code("TAKEN")],
        ));
    }
    let lower = id.to_ascii_lowercase();
    let (brand, last) = if lower.contains("mastercard") {
        ("Mastercard", "4444")
    } else if lower.contains("amex") {
        ("American Express", "8431")
    } else {
        ("Visa", "4242")
    };
    let name = state
        .get(&customer)
        .and_then(|record| text(record, "displayName"))
        .unwrap_or_default()
        .to_string();
    let year = super::model::date_time(&mutation.timestamp()).map_or(2030, |now| {
        use chrono::Datelike;
        i64::from(now.year()) + 3
    });
    let card = value::object(json!({
        "brand": brand,
        "lastDigits": last,
        "expiryMonth": 12,
        "expiryYear": year,
        "name": name,
    }));
    let outcome = remote_outcome(&id);
    let gateway = gateway.to_string();
    let state = mutation.state_mut();
    let method = create_method(
        state,
        &customer,
        |method| card_instrument(&card, Value::Null, method, None),
        outcome,
    );
    if let Some(record) = state.get_mut(&method) {
        record.insert("lsfReference".into(), json!(reference_key));
        record.insert("lsfGateway".into(), json!(gateway));
    }
    Ok(payload(json!({"customerPaymentMethod": method}), &[]))
}

/// `customerPaymentMethodRevoke`.
fn revoke(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"revokedCustomerPaymentMethodId": null});
    let id = ctx
        .arg_str("customerPaymentMethodId")
        .unwrap_or_default()
        .to_string();
    let state = mutation.state();
    if let Err(error) = live_method(state, &id, "customerPaymentMethodId") {
        return Ok(payload(empty, &[error]));
    }
    if !active_contracts(state, &id).is_empty() {
        // lsf's wording of shopify.dev's "Revocation will fail if the payment method has
        // active subscription contracts".
        return Ok(payload(
            empty,
            &[UserError::new(
                &["customerPaymentMethodId"],
                "Payment method can't be revoked while it has active subscription contracts",
            )],
        ));
    }
    let state = mutation.state_mut();
    let now = state.timestamp();
    if let Some(record) = state.get_mut(&id) {
        record.insert("revokedAt".into(), json!(now));
        record.insert("revokedReason".into(), json!("MANUALLY_REVOKED"));
    }
    state.emit("customer_payment_methods/revoke", &id);
    Ok(payload(json!({"revokedCustomerPaymentMethodId": id}), &[]))
}

/// The URL of lsf's page where a customer gives a new card for a method.
pub fn update_url(origin: &str, shop: &str, method: &str) -> String {
    format!(
        "{origin}/__lsf/admin/payment-methods/{}/update?shop={shop}",
        token(method)
    )
}

/// `customerPaymentMethodGetUpdateUrl`: for a Shop Pay agreement only, as Shopify.
fn get_update_url(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"updatePaymentMethodUrl": null});
    let id = ctx
        .arg_str("customerPaymentMethodId")
        .unwrap_or_default()
        .to_string();
    let error = |code: &str| {
        UserError::new(
            &["customerPaymentMethodId"],
            super::model::described("CustomerPaymentMethodGetUpdateUrlUserErrorCode", code),
        )
        .code(code)
    };
    let state = mutation.state();
    let Ok(method) = live_method(state, &id, "customerPaymentMethodId") else {
        return Ok(payload(empty, &[error("PAYMENT_METHOD_DOES_NOT_EXIST")]));
    };
    if !text(&method, "customer").is_some_and(|customer| state.contains(customer)) {
        return Ok(payload(empty, &[error("CUSTOMER_DOES_NOT_EXIST")]));
    }
    if method
        .pointer("/instrument/__typename")
        .and_then(Value::as_str)
        != Some("CustomerShopPayAgreement")
    {
        return Ok(payload(empty, &[error("INVALID_INSTRUMENT")]));
    }
    let url = update_url(mutation.origin(), mutation.shop(), &id);
    Ok(payload(json!({"updatePaymentMethodUrl": url}), &[]))
}

/// The emails of the family in the shop's outbox, oldest first.
pub fn emails(state: &ShopState) -> Vec<Value> {
    crate::outbox::entries(state, Some(Channel::Email))
        .into_iter()
        .filter(|email| {
            email
                .get("what")
                .and_then(Value::as_str)
                .is_some_and(|what| EMAILS.contains(&what))
        })
        .collect()
}

/// The email of an `EmailInput` (`to`, `subject`, `body`; `from`, `bcc` and `customMessage` in
/// its `data`), to the customer's address and with lsf's subject by default.
fn email_entry(
    what: &str,
    input: &Map<String, Value>,
    to: Option<&str>,
    subject: &str,
    mut data: Map<String, Value>,
) -> Entry {
    let given = |field: &str| input.get(field).and_then(Value::as_str);
    let mut email = Entry::email(what, given("to").or(to).unwrap_or_default())
        .subject(given("subject").unwrap_or(subject));
    if let Some(body) = given("body") {
        email = email.body(body);
    }
    for field in ["from", "bcc", "customMessage"] {
        data.insert(
            field.into(),
            input.get(field).cloned().unwrap_or(Value::Null),
        );
    }
    email.data(Value::Object(data))
}

/// `customerPaymentMethodSendUpdateEmail`: the email with the link to update the method,
/// logged, not sent.
fn send_update_email(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx
        .arg_str("customerPaymentMethodId")
        .unwrap_or_default()
        .to_string();
    let state = mutation.state();
    let method = match live_method(state, &id, "customerPaymentMethodId") {
        Ok(method) => method,
        Err(error) => return Ok(payload(json!({"customer": null}), &[error])),
    };
    let customer = text(&method, "customer").unwrap_or_default().to_string();
    let address = state
        .get(&customer)
        .and_then(|record| text(record, "email"))
        .map(str::to_string);
    let input = object(ctx.arg("email"));
    if address.is_none() && input.get("to").is_none_or(Value::is_null) {
        // lsf's wording.
        return Ok(payload(
            json!({"customer": null}),
            &[UserError::new(
                &["email", "to"],
                "Customer does not have an email address",
            )],
        ));
    }
    let url = update_url(mutation.origin(), mutation.shop(), &id);
    let mut data = Map::new();
    data.insert("customer".into(), json!(customer));
    data.insert("url".into(), json!(url));
    let email = email_entry(
        EMAILS[0],
        &input,
        address.as_deref(),
        "Update your payment method",
        data,
    )
    .about(&id);
    crate::outbox::record(mutation.state_mut(), email);
    mutation.note(
        "customerPaymentMethodSendUpdateEmail: lsf sends no email; it is in the shop's outbox (GET /__lsf/admin/emails)",
    );
    Ok(payload(json!({"customer": customer}), &[]))
}

/// `paymentInstrumentSendAddEmail`: the email asking a customer for a payment method for a
/// contract, an order or a draft order (the mandate's resource), logged, not sent.
fn send_add_email(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let mandate = object(ctx.arg("mandate"));
    let resource = text(&mandate, "resourceId").unwrap_or_default().to_string();
    let kind = text(&mandate, "resourceType").unwrap_or_default();
    let expected = match kind {
        "SUBSCRIPTIONS" => CONTRACT,
        "DRAFT_ORDERS" => "DraftOrder",
        _ => "Order",
    };
    let state = mutation.state();
    let Some(record) = state
        .get(&resource)
        .filter(|_| state.type_of(&resource) == Some(expected))
    else {
        // lsf's wording.
        return Ok(payload(
            json!({"customer": null}),
            &[UserError::new(
                &["mandate", "resourceId"],
                "Resource does not exist",
            )],
        ));
    };
    let Some(customer) = text(record, "customer").map(str::to_string) else {
        return Ok(payload(
            json!({"customer": null}),
            &[UserError::new(
                &["mandate", "resourceId"],
                "Resource has no customer",
            )],
        ));
    };
    let address = state
        .get(&customer)
        .and_then(|record| text(record, "email"))
        .map(str::to_string);
    let input = object(ctx.arg("email"));
    if address.is_none() && input.get("to").is_none_or(Value::is_null) {
        return Ok(payload(
            json!({"customer": null}),
            &[UserError::new(
                &["email", "to"],
                "Customer does not have an email address",
            )],
        ));
    }
    let mut data = Map::new();
    data.insert("customer".into(), json!(customer));
    data.insert("resourceType".into(), json!(kind));
    let email = email_entry(
        EMAILS[1],
        &input,
        address.as_deref(),
        "Add a payment method",
        data,
    )
    .about(&resource);
    crate::outbox::record(mutation.state_mut(), email);
    mutation.note("paymentInstrumentSendAddEmail: lsf sends no email; it is in the shop's outbox (GET /__lsf/admin/emails)");
    Ok(payload(json!({"customer": customer}), &[]))
}

// ---- queries --------------------------------------------------------------------------------

/// Whether a method is revoked.
fn revoked(method: &Record) -> bool {
    method.get("revokedAt").is_some_and(|at| !at.is_null())
}

/// `customerPaymentMethod(id:, showRevoked:)`.
fn payment_method(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default();
    let show = ctx.arg_bool("showRevoked").unwrap_or(false);
    let state = r.state();
    let found = state
        .get(id)
        .filter(|_| state.type_of(id) == Some(PAYMENT_METHOD))
        .filter(|method| show || !revoked(method));
    Ok(Output::object_or_null(found.map(|_| Obj::record(id))))
}

/// `Customer.paymentMethods(showRevoked:)`.
fn customer_payment_methods(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let customer = r
        .map_of(object)
        .and_then(|record| text(record, "id"))
        .unwrap_or_default();
    let show = ctx.arg_bool("showRevoked").unwrap_or(false);
    let items: Vec<Value> = r
        .state()
        .all(PAYMENT_METHOD)
        .filter(|method| text(method, "customer") == Some(customer))
        .filter(|method| show || !revoked(method))
        .filter_map(|method| method.get("id").cloned())
        .collect();
    r.connection(ctx, items)
}

/// `CustomerPaymentMethod.subscriptionContracts`: the contracts it pays for.
fn method_contracts(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let method = r
        .map_of(object)
        .and_then(|record| text(record, "id"))
        .unwrap_or_default();
    let items: Vec<Value> = r
        .state()
        .all(CONTRACT)
        .filter(|contract| text(contract, "customerPaymentMethod") == Some(method))
        .filter_map(|contract| contract.get("id").cloned())
        .collect();
    r.connection(ctx, items)
}

/// `expiresSoon` of a card or a Shop Pay agreement: it expires within three months of the
/// shop's clock (lsf's reading of "Whether the card is about to expire").
fn expires_soon(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    use chrono::Datelike;
    let card = r.map_of(object);
    let month = card
        .and_then(|card| card.get("expiryMonth"))
        .and_then(Value::as_i64)
        .unwrap_or(12);
    let year = card
        .and_then(|card| card.get("expiryYear"))
        .and_then(Value::as_i64)
        .unwrap_or(9999);
    let now = r.state().now();
    let months_left = (year * 12 + month) - (i64::from(now.year()) * 12 + i64::from(now.month()));
    Ok(Output::value(months_left < 3))
}

/// `isRevocable` of an instrument: no active contract uses its method.
fn is_revocable(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let method = r
        .map_of(object)
        .and_then(|record| text(record, "lsfMethod"))
        .unwrap_or_default();
    let state = r.state();
    let revocable = state.get(method).is_some_and(|record| !revoked(record))
        && active_contracts(state, method).is_empty();
    Ok(Output::value(revocable))
}

/// `customerPaymentMethod(showRevoked:)` of a contract-like record: its method, unless revoked.
fn contract_method(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let method = r
        .map_of(object)
        .and_then(|record| text(record, "customerPaymentMethod"))
        .unwrap_or_default();
    let show = ctx.arg_bool("showRevoked").unwrap_or(false);
    let found = r
        .state()
        .get(method)
        .filter(|record| show || !revoked(record));
    Ok(Output::object_or_null(found.map(|_| Obj::record(method))))
}

/// The methods of a customer that are not revoked, in creation order.
pub fn methods_of(state: &ShopState, customer: &str) -> Vec<String> {
    gids(Some(&Value::Array(
        state
            .all(PAYMENT_METHOD)
            .filter(|method| text(method, "customer") == Some(customer) && !revoked(method))
            .filter_map(|method| method.get("id").cloned())
            .collect(),
    )))
}
