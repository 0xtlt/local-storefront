//! Gift cards: `giftCardCreate`, `giftCardUpdate`, `giftCardCredit`, `giftCardDebit`,
//! `giftCardDeactivate`.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - `giftCardCreate` issues a card of `initialAmount` (or the deprecated `initialValue`) in the
//!   shop's currency, with the `code` given (8 to 20 letters and digits, unique) or one it makes
//!   (16 characters, as the reference's example `a5bhbh645b32934f`), answered once in
//!   `giftCardCode`; the card itself shows its `lastCharacters` and `maskedCode` only. A
//!   `customerId` or a recipient that is not a customer is refused (`CUSTOMER_NOT_FOUND`,
//!   `RECIPIENT_NOT_FOUND`).
//! - `giftCardCredit` and `giftCardDebit` add and take an amount, in the card's currency
//!   (`MISMATCHING_CURRENCY`), positive (`NEGATIVE_OR_ZERO_AMOUNT`), a debit no more than the
//!   balance (`INSUFFICIENT_FUNDS`); each records a `GiftCardCreditTransaction` or
//!   `GiftCardDebitTransaction` in the card's `transactions`. A deactivated or expired card
//!   takes neither.
//! - `giftCardDeactivate` disables the card for good (`enabled: false`, `deactivatedAt`).
//! - `isRedeemable` follows: enabled, not expired by the shop's clock, a balance left.
//!
//! The 2026-10 webhook topics have none for gift cards: the events these mutations emit
//! (`gift_cards/create`, `gift_cards/update`) reach no subscription; they tell the server the
//! shop changed. A card created here has the id lsf-core gives a card of its code
//! (`stable_id("gift_card", code)`), but the store sync does not write it (the record does not
//! keep its code); changes to the cards of the store data sync.

use serde_json::{Map, Value, json};

use super::cents_of;
use super::records::{field, text};
use crate::admin::graphql::{FieldContext, FieldError, Output};
use crate::admin::resolve::{Mutation, Registry, UserError, coded, payload};
use crate::admin::state::{Record, ShopState, gid, value};

pub fn register(registry: &mut Registry) {
    registry
        .mutation("giftCardCreate", gift_card_create)
        .mutation("giftCardUpdate", gift_card_update)
        .mutation("giftCardCredit", gift_card_credit)
        .mutation("giftCardDebit", gift_card_debit)
        .mutation("giftCardDeactivate", gift_card_deactivate)
        // Read at the shop's clock, which runs: a card stored redeemable expires all the same.
        .field("GiftCard", "isRedeemable", |r, _ctx, object| {
            let today = today(r.state());
            Ok(Output::value(
                r.map_of(object)
                    .is_some_and(|card| redeemable(card, &today)),
            ))
        });
}

/// The cents of a `MoneyInput` (`{amount, currencyCode}`) and its currency.
fn money_input(value: Option<&Value>) -> Option<(i64, Option<String>)> {
    let value = value?;
    let cents = super::variants::money_cents(value.get("amount")?)?;
    Some((
        cents,
        value
            .get("currencyCode")
            .and_then(Value::as_str)
            .map(str::to_string),
    ))
}

/// The cents of a `MoneyV2` field of a record.
fn cents_in(record: &Record, name: &str) -> i64 {
    cents_of(record.get(name).and_then(|money| money.get("amount"))).unwrap_or(0)
}

/// The shop's date today (`2026-03-15`): its clock in its time zone (`Shop.ianaTimezone`, else
/// the offset the seed read, else UTC), the zone the seed writes `expiresOn` in. A card expires
/// at the end of the shop's day, not of UTC's.
pub fn today(state: &ShopState) -> String {
    let now = state.now();
    if let Some(zone) = state.iana_time_zone() {
        return now.with_timezone(&zone).format("%Y-%m-%d").to_string();
    }
    let offset = state
        .shop()
        .and_then(|shop| shop.get("timezoneOffsetMinutes"))
        .and_then(Value::as_i64)
        .and_then(|minutes| i32::try_from(minutes * 60).ok())
        .and_then(chrono::FixedOffset::east_opt);
    match offset {
        Some(offset) => now.with_timezone(&offset).format("%Y-%m-%d").to_string(),
        None => now.format("%Y-%m-%d").to_string(),
    }
}

/// Whether a card can be redeemed: enabled, not expired by `today`, a balance left.
pub fn redeemable(record: &Record, today: &str) -> bool {
    let enabled = record
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let expired = field(record, "expiresOn").is_some_and(|expires| expires < today);
    enabled && !expired && cents_in(record, "balance") > 0
}

/// The last four characters of a code, and its masked form.
pub fn masked(code: &str) -> (String, String) {
    let last: String = code
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let masked = format!("•••• •••• •••• {last}");
    (last, masked)
}

/// A code for a card: 16 lowercase letters and digits, made from the shop and its cards.
fn new_code(state: &ShopState) -> String {
    let seed = format!("gift-card/{}/{}", state.domain(), state.count("GiftCard"));
    value::uuid(&seed)
        .replace('-', "")
        .chars()
        .take(16)
        .collect()
}

/// The checks of a recipient input.
fn check_recipient(state: &ShopState, input: &Map<String, Value>, errors: &mut Vec<UserError>) {
    if let Some(recipient) = input.get("recipientAttributes").and_then(Value::as_object) {
        let id = text(recipient, "id").unwrap_or_default();
        if !state.is_of(id, "Customer") {
            errors.push(coded(
                &["input", "recipientAttributes", "id"],
                "The recipient could not be found.",
                "RECIPIENT_NOT_FOUND",
            ));
        }
    }
    if let Some(customer) = text(input, "customerId")
        && !state.is_of(customer, "Customer")
    {
        errors.push(coded(
            &["input", "customerId"],
            "The customer could not be found.",
            "CUSTOMER_NOT_FOUND",
        ));
    }
}

/// Sets the fields of a create or update input on a card: those given.
fn apply_fields(record: &mut Record, input: &Map<String, Value>) {
    for name in ["note", "templateSuffix", "expiresOn"] {
        if let Some(given) = input.get(name) {
            record.insert(name.into(), given.clone());
        }
    }
    if let Some(customer) = input.get("customerId") {
        record.insert("customer".into(), customer.clone());
    }
    if let Some(strategy) = input.get("crossCurrencyRedemptionStrategy") {
        record.insert("crossCurrencyRedemptionStrategy".into(), strategy.clone());
    }
    if let Some(recipient) = input.get("recipientAttributes") {
        let recipient = recipient.as_object().map(|recipient| {
            json!({
                "recipient": recipient.get("id"),
                "message": recipient.get("message"),
                "preferredName": recipient.get("preferredName"),
                "sendNotificationAt": recipient.get("sendNotificationAt"),
            })
        });
        record.insert("recipientAttributes".into(), json!(recipient));
    }
}

/// `giftCardCreate`.
fn gift_card_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"giftCard": null, "giftCardCode": null});
    let input = ctx.arg_object_or_default("input");
    let state = mutation.state();
    let shop_currency = state.currency();
    let mut errors = Vec::new();
    let amount = money_input(input.get("initialAmount")).or_else(|| {
        input
            .get("initialValue")
            .and_then(|value| cents_of(Some(value)))
            .map(|cents| (cents, None))
    });
    match &amount {
        None => errors.push(coded(
            &["input", "initialAmount"],
            "Missing a required argument.",
            "MISSING_ARGUMENT",
        )),
        Some((cents, _)) if *cents <= 0 => errors.push(coded(
            &["input", "initialAmount"],
            "Initial value must be greater than 0",
            "GREATER_THAN",
        )),
        Some((_, Some(code))) if !code.eq_ignore_ascii_case(&shop_currency) => errors.push(coded(
            &["input", "initialAmount", "currencyCode"],
            "The input value is invalid.",
            "INVALID",
        )),
        _ => {}
    }
    let code = text(&input, "code").map(|code| code.replace([' ', '-'], ""));
    if let Some(code) = &code {
        if code.chars().count() < 8 {
            errors.push(coded(
                &["input", "code"],
                "Code is too short (minimum is 8 characters)",
                "TOO_SHORT",
            ));
        } else if code.chars().count() > 20 {
            errors.push(coded(
                &["input", "code"],
                "Code is too long (maximum is 20 characters)",
                "TOO_LONG",
            ));
        } else if !code.chars().all(|c| c.is_ascii_alphanumeric()) {
            errors.push(coded(
                &["input", "code"],
                "Code can only contain letters and numbers",
                "INVALID",
            ));
        } else if state.contains(&gid(
            "GiftCard",
            lsf_core::util::stable_id("gift_card", code),
        )) {
            errors.push(coded(
                &["input", "code"],
                "Code has already been taken",
                "TAKEN",
            ));
        }
    }
    check_recipient(state, &input, &mut errors);
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let state = mutation.state_mut();
    let now = state.timestamp();
    let code = code.unwrap_or_else(|| new_code(state));
    let id = gid("GiftCard", lsf_core::util::stable_id("gift_card", &code));
    let cents = amount.map_or(0, |(cents, _)| cents);
    let (last, masked) = masked(&code);
    let mut record = value::object(json!({
        "__typename": "GiftCard",
        "id": id,
        "balance": value::money_v2(cents, &shop_currency),
        "initialValue": value::money_v2(cents, &shop_currency),
        "enabled": true,
        "deactivatedAt": null,
        "expiresOn": null,
        "lastCharacters": last,
        "maskedCode": masked,
        "customer": null,
        "recipientAttributes": null,
        "note": null,
        "templateSuffix": null,
        "transactions": [],
        "createdAt": now,
        "updatedAt": now,
    }));
    apply_fields(&mut record, &input);
    let today = today(state);
    record.insert("isRedeemable".into(), json!(redeemable(&record, &today)));
    state.insert(record);
    state.emit("gift_cards/create", &id);
    Ok(payload(json!({"giftCard": id, "giftCardCode": code}), &[]))
}

/// The card of `id`.
fn card_arg(state: &ShopState, ctx: &FieldContext<'_>) -> Option<String> {
    ctx.arg_str("id")
        .filter(|id| state.is_of(id, "GiftCard"))
        .map(str::to_string)
}

/// Refreshes `isRedeemable` and `updatedAt`, and emits `gift_cards/update`.
fn changed(state: &mut ShopState, card: &str) {
    let now = state.timestamp();
    let today = today(state);
    if let Some(record) = state.get_mut(card) {
        let redeemable = redeemable(record, &today);
        record.insert("isRedeemable".into(), json!(redeemable));
        record.insert("updatedAt".into(), json!(now));
    }
    state.emit("gift_cards/update", card);
}

/// `giftCardUpdate`.
fn gift_card_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let Some(card) = card_arg(state, ctx) else {
        let error = UserError::new(&["id"], "Gift card does not exist");
        return Ok(payload(json!({"giftCard": null}), &[error]));
    };
    let input = ctx.arg_object_or_default("input");
    let mut errors = Vec::new();
    check_recipient(state, &input, &mut errors);
    if !errors.is_empty() {
        return Ok(payload(json!({"giftCard": null}), &errors));
    }
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&card) {
        apply_fields(record, &input);
    }
    changed(state, &card);
    Ok(payload(json!({"giftCard": card}), &[]))
}

/// `giftCardCredit` and `giftCardDebit`.
fn transaction(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    credit: bool,
) -> Result<Value, FieldError> {
    let (answer, input_name, amount_name, type_name) = if credit {
        (
            "giftCardCreditTransaction",
            "creditInput",
            "creditAmount",
            "GiftCardCreditTransaction",
        )
    } else {
        (
            "giftCardDebitTransaction",
            "debitInput",
            "debitAmount",
            "GiftCardDebitTransaction",
        )
    };
    let empty = json!({answer: null});
    let state = mutation.state();
    let Some(card) = card_arg(state, ctx) else {
        let error = coded(
            &["id"],
            "The gift card could not be found.",
            "GIFT_CARD_NOT_FOUND",
        );
        return Ok(payload(empty, &[error]));
    };
    let input = ctx.arg_object_or_default(input_name);
    let record = state.get(&card).cloned().unwrap_or_default();
    let card_currency = record
        .get("balance")
        .and_then(|balance| balance.get("currencyCode"))
        .and_then(Value::as_str)
        .unwrap_or("USD")
        .to_string();
    let mut errors = Vec::new();
    let amount = money_input(input.get(amount_name));
    let at = [input_name, amount_name];
    match &amount {
        None => errors.push(coded(&at, "The input value is invalid.", "INVALID")),
        Some((cents, _)) if *cents <= 0 => errors.push(coded(
            &at,
            "A positive amount must be used.",
            "NEGATIVE_OR_ZERO_AMOUNT",
        )),
        Some((_, Some(code))) if !code.eq_ignore_ascii_case(&card_currency) => errors.push(coded(
            &[input_name, amount_name, "currencyCode"],
            "The currency provided does not match the currency of the gift card.",
            "MISMATCHING_CURRENCY",
        )),
        Some((cents, _)) if !credit && *cents > cents_in(&record, "balance") => errors.push(coded(
            &at,
            "The gift card does not have sufficient funds to satisfy the request.",
            "INSUFFICIENT_FUNDS",
        )),
        // The balance stays within the bound of an amount (`value::MAX_MONEY_CENTS`, lsf's), with
        // the code Shopify has for it and its description.
        Some((cents, _))
            if credit
                && !cents_in(&record, "balance")
                    .checked_add(*cents)
                    .is_some_and(value::money_in_bounds) =>
        {
            errors.push(coded(
                &at,
                "The gift card's value exceeds the allowed limits.",
                "GIFT_CARD_LIMIT_EXCEEDED",
            ))
        }
        _ => {}
    }
    let today = today(mutation.state());
    let enabled = record
        .get("enabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let expired = field(&record, "expiresOn").is_some_and(|expires| expires < today.as_str());
    if errors.is_empty() && (!enabled || expired) {
        errors.push(coded(&["id"], "The input value is invalid.", "INVALID"));
    }
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let cents = amount.map_or(0, |(cents, _)| cents);
    let state = mutation.state_mut();
    let now = state.timestamp();
    let id = state.new_gid(type_name);
    let signed = if credit { cents } else { -cents };
    state.insert(value::object(json!({
        "__typename": type_name,
        "id": id,
        "amount": value::money_v2(signed, &card_currency),
        "giftCard": card,
        "note": input.get("note"),
        "processedAt": text(&input, "processedAt").map(normalize_time).unwrap_or(now),
        "metafields": [],
    })));
    if let Some(record) = state.get_mut(&card) {
        let balance = cents_in(record, "balance") + signed;
        record.insert("balance".into(), value::money_v2(balance, &card_currency));
        if let Some(Value::Array(transactions)) = record.get_mut("transactions") {
            transactions.push(json!(id));
        } else {
            record.insert("transactions".into(), json!([id]));
        }
    }
    changed(state, &card);
    Ok(payload(json!({answer: id}), &[]))
}

/// A `DateTime` input in UTC, as the Admin API answers it (`2024-09-09T12:48:33-04:00` is
/// `2024-09-09T16:48:33Z`, the reference's example).
pub fn normalize_time(time: &str) -> String {
    value::parse_date_time(time).map_or_else(|| time.to_string(), value::date_time)
}

fn gift_card_credit(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    transaction(mutation, ctx, true)
}

fn gift_card_debit(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    transaction(mutation, ctx, false)
}

/// `giftCardDeactivate`.
fn gift_card_deactivate(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let state = mutation.state();
    let Some(card) = card_arg(state, ctx) else {
        let error = coded(
            &["id"],
            "The gift card could not be found.",
            "GIFT_CARD_NOT_FOUND",
        );
        return Ok(payload(json!({"giftCard": null}), &[error]));
    };
    let state = mutation.state_mut();
    let now = state.timestamp();
    if let Some(record) = state.get_mut(&card)
        && record.get("enabled").and_then(Value::as_bool) != Some(false)
    {
        record.insert("enabled".into(), json!(false));
        record.insert("deactivatedAt".into(), json!(now));
    }
    changed(state, &card);
    Ok(payload(json!({"giftCard": card}), &[]))
}
