//! `giftCardProductSet`, `giftCardSendNotificationToCustomer`, `giftCardSendNotificationToRecipient`.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - `giftCardProductSet` creates or updates a gift card product as `productSet` does a product
//!   ("For list fields like `variants`, the mutation creates new entries, updates existing ones,
//!   and deletes any entries not included"): lsf runs `productSet` with `giftCard: true`, so its
//!   validation, its variants and options, its operation (`synchronous: false`) and its events
//!   (`products/create`, `products/update`) are those of `productSet`. "The `issuanceCurrency`
//!   and `crossCurrencyRedeemable` fields can only be set during product creation" (the product's
//!   `giftCardSettings`). A product that is not a gift card is refused
//!   (`PRODUCT_MUST_BE_GIFT_CARD`, "The specified product is not a gift card. The
//!   giftCardProductSet mutation can only be used with gift card products."). Its errors take
//!   `productSet`'s codes when the payload's enum has them, `INVALID_INPUT` otherwise.
//! - `giftCardSendNotificationToCustomer` "Sends a gift card notification" (a purchase
//!   confirmation when the card has a recipient); "The gift card must have an assigned customer
//!   with an email address or phone number. Returns an error without sending if the selected
//!   contact source has no contact information. When both an email address and a phone number
//!   are available, the notification is sent by email."
//! - `giftCardSendNotificationToRecipient` "Sends a notification to the designated recipient of
//!   a gift card", by their contact method.
//!
//! lsf sends no email or SMS: a notification is written to the card's log (`lsfNotifications`:
//! `{at, kind, to, channel}`) and said in `extensions.lsf.notes`. Shopify has no webhook topic for
//! a notification: the event is `gift_cards/notification_sent`, which no subscription can name.

use serde_json::{Map, Value, json};

use super::records::field;
use crate::graphql::{FieldContext, FieldError};
use crate::resolve::{Mutation, Registry, UserError, mutation, payload};
use crate::state::ShopState;

pub fn register(registry: &mut Registry) {
    registry
        .mutation("giftCardProductSet", gift_card_product_set)
        .mutation("giftCardSendNotificationToCustomer", |m, ctx| {
            notify(m, ctx, false)
        })
        .mutation("giftCardSendNotificationToRecipient", |m, ctx| {
            notify(m, ctx, true)
        });
}

/// The record field of a gift card with the notifications lsf did not send.
const NOTIFICATIONS: &str = "lsfNotifications";

/// The code enum of `giftCardProductSet`'s errors.
const CODES: &str = "GiftCardProductSetUserErrorCode";

/// A `productSet` error's code, as the gift card payload's enum has it.
fn fitted(code: &Value) -> Value {
    let Some(code) = code.as_str() else {
        return Value::Null;
    };
    let known = crate::schema::admin()
        .get(CODES)
        .is_some_and(|definition| definition.enum_values.contains_key(code));
    json!(if known { code } else { "INVALID_INPUT" })
}

/// The product a `productSet` payload names: its `product`, else its operation's.
fn product_of(state: &ShopState, answer: &Value) -> Option<String> {
    answer
        .get("product")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            let operation = answer.get("productSetOperation")?.as_str()?;
            field(state.get(operation)?, "product").map(str::to_string)
        })
}

/// The product a `ProductSetIdentifiers` names, when it exists: by `id`, `handle` or `customId`.
fn existing(state: &ShopState, identifier: Option<&Value>) -> Option<String> {
    let identifier = identifier?.as_object()?;
    if let Some(id) = identifier.get("id").and_then(Value::as_str) {
        return (state.type_of(id) == Some("Product")).then(|| id.to_string());
    }
    if let Some(handle) = identifier.get("handle").and_then(Value::as_str) {
        return state
            .keys("Product")
            .find(|product| {
                state
                    .get(product)
                    .and_then(|record| field(record, "handle"))
                    == Some(handle)
            })
            .map(str::to_string);
    }
    super::products::product_by_custom_id(state, identifier.get("customId")?)
}

/// `giftCardProductSet`.
fn gift_card_product_set(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let empty = json!({"product": null, "productSetOperation": null});
    let Some(run) = mutation.registry().mutation_fn("productSet") else {
        return Err(mutation::not_simulated("productSet"));
    };
    let mut input: Map<String, Value> = ctx
        .arg("input")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let settings = (
        input.remove("issuanceCurrency"),
        input.remove("crossCurrencyRedeemable"),
    );
    // A product that exists already must be a gift card: `productSet` would make it one.
    if let Some(existing) = existing(mutation.state(), ctx.arg("identifier"))
        && mutation
            .state()
            .get(&existing)
            .and_then(|record| record.get("isGiftCard"))
            .and_then(Value::as_bool)
            != Some(true)
    {
        let error = UserError::new(
            &["identifier"],
            "The specified product is not a gift card. The giftCardProductSet mutation can only be used with gift card products.",
        )
        .code("PRODUCT_MUST_BE_GIFT_CARD");
        return Ok(payload(empty, &[error]));
    }
    input.insert("giftCard".into(), json!(true));
    let mut arguments = ctx.arguments().clone();
    arguments.insert("input".into(), Value::Object(input));
    let before: Vec<String> = mutation
        .state()
        .keys("Product")
        .map(str::to_string)
        .collect();
    let mut answer = run(mutation, &ctx.with_arguments(&arguments))?;
    if let Some(Value::Array(errors)) = answer.get_mut("userErrors")
        && !errors.is_empty()
    {
        for error in errors.iter_mut() {
            let code = fitted(error.get("code").unwrap_or(&Value::Null));
            error["code"] = code;
        }
        return Ok(answer);
    }
    let state = mutation.state();
    let Some(product) = product_of(state, &answer) else {
        return Ok(answer);
    };
    let created = !before.contains(&product);
    let gift_card = state
        .get(&product)
        .and_then(|record| record.get("isGiftCard"))
        .and_then(Value::as_bool)
        == Some(true);
    let error = if !gift_card {
        Some(UserError::new(&["input"], "Product is not valid.").code("INVALID_PRODUCT"))
    } else if !created && (settings.0.is_some() || settings.1.is_some()) {
        // The reference's rule, in lsf's words.
        let name = if settings.0.is_some() {
            "issuanceCurrency"
        } else {
            "crossCurrencyRedeemable"
        };
        Some(
            UserError::new(
                &["input", name],
                format!("{name} can only be set when the gift card product is created"),
            )
            .code("INVALID_INPUT"),
        )
    } else {
        None
    };
    if let Some(error) = error {
        return Ok(payload(empty, &[error]));
    }
    if created {
        let currency = settings.0.filter(|currency| !currency.is_null());
        let cross = settings
            .1
            .and_then(|cross| cross.as_bool())
            .unwrap_or(false);
        if let Some(record) = mutation.state_mut().get_mut(&product) {
            record.insert(
                "giftCardSettings".into(),
                json!({"issuanceCurrency": currency, "crossCurrencyRedeemable": cross}),
            );
        }
    }
    if let Value::Object(map) = &mut answer {
        map.remove("shop");
    }
    Ok(answer)
}

/// The contact of a customer a notification goes to: `(channel, address)`, email first.
fn contact_of(state: &ShopState, customer: &str) -> Option<(&'static str, String)> {
    let record = state.get(customer)?;
    if let Some(email) = field(record, "email").filter(|email| !email.is_empty()) {
        return Some(("email", email.to_string()));
    }
    field(record, "phone")
        .filter(|phone| !phone.is_empty())
        .map(|phone| ("sms", phone.to_string()))
}

/// `giftCardSendNotificationToCustomer` and (`recipient`) `…ToRecipient`.
fn notify(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    recipient: bool,
) -> Result<Value, FieldError> {
    let empty = json!({"giftCard": null});
    let state = mutation.state();
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let Some(card) = state
        .get(&id)
        .filter(|_| state.type_of(&id) == Some("GiftCard"))
    else {
        let error = UserError::new(&["id"], "The gift card could not be found.")
            .code("GIFT_CARD_NOT_FOUND");
        return Ok(payload(empty, &[error]));
    };
    let designated = card
        .get("recipientAttributes")
        .and_then(|attributes| attributes.get("recipient"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let (person, kind) = if recipient {
        (designated.clone(), "gift card")
    } else {
        let kind = if designated.is_some() {
            "purchase confirmation"
        } else {
            "gift card"
        };
        (field(card, "customer").map(str::to_string), kind)
    };
    let Some(person) = person.filter(|person| state.type_of(person) == Some("Customer")) else {
        let error = if recipient {
            UserError::new(&["id"], "The recipient could not be found.").code("RECIPIENT_NOT_FOUND")
        } else {
            UserError::new(&["id"], "The customer could not be found.").code("CUSTOMER_NOT_FOUND")
        };
        return Ok(payload(empty, &[error]));
    };
    let Some((channel, address)) = contact_of(state, &person) else {
        let error = UserError::new(&["id"], "The input value is invalid.").code("INVALID");
        mutation.note(
            "the gift card's customer has no email address nor phone number: nothing would be sent",
        );
        return Ok(payload(empty, &[error]));
    };
    let state = mutation.state_mut();
    let at = state.timestamp();
    if let Some(record) = state.get_mut(&id) {
        let mut log = record
            .get(NOTIFICATIONS)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        log.push(json!({"at": at, "kind": kind, "to": address, "channel": channel}));
        record.insert(NOTIFICATIONS.into(), json!(log));
    }
    state.emit("gift_cards/notification_sent", &id);
    mutation.note(format!(
        "{}: lsf sends no {channel}: the {kind} to {address} is in the gift card's `{NOTIFICATIONS}`",
        ctx.field_name()
    ));
    Ok(payload(json!({"giftCard": id}), &[]))
}
