//! Billing: the app's subscriptions, their usage records, its one-time purchases, and the
//! merchant's approval of each on a local confirmation page.
//!
//! As shopify.dev's "Subscription billing with the Billing API" and the reference of 2026-10
//! describe it, and as `@shopify/shopify-api` 15.0.1 (`lib/billing`: `request`, `check`,
//! `cancel`, `createUsageRecord`, `updateUsageCappedAmount`) calls it:
//!
//! - `appSubscriptionCreate` makes a `PENDING` subscription with its line items (one recurring
//!   and one usage plan at most: "Only one pricing plan can be defined for each available
//!   type"), and answers a `confirmationUrl` on lsf: `GET /__lsf/admin/charges/<n>?shop=…`
//!   shows the charge, approving redirects to `returnUrl` with `charge_id=<n>`, as Shopify does.
//!   `POST /__lsf/admin/charges/<n>/approve` (or `/decline`) does it without the page.
//! - Approved, the subscription is `ACTIVE` (2026-10: "When a merchant approves an app
//!   subscription, the status immediately transitions from pending to active"), and the app's
//!   other active subscription is `CANCELLED` ("An app can have only one active subscription for
//!   each merchant"). lsf has no billing cycles: a replacement Shopify defers to the next cycle
//!   (`APPLY_ON_NEXT_BILLING_CYCLE`, and the three cases of `STANDARD`) applies at once. Declined,
//!   it is `DECLINED`; pending for more than two days by the shop's clock, `EXPIRED`.
//! - `currentPeriodEnd`, *chosen*: the end of the trial when there is one, else 30 days (365 for
//!   an `ANNUAL` plan) from the approval; `null` while the subscription is not active.
//! - `appSubscriptionCancel` cancels (`prorate` moves no money here), `appSubscriptionTrialExtend`
//!   adds days to an active trial (its codes and their descriptions), `appSubscriptionLineItemUpdate`
//!   asks for a new capped amount, which the merchant approves on the same page.
//! - `appUsageRecordCreate` charges a usage line of an active subscription: the balance used may
//!   not exceed the capped amount (shopify.dev's example: `"Failed to create usage charge"`,
//!   `field: null`); an `idempotencyKey` of 255 characters at most ("Idempotency key exceeds the
//!   maximum length.") answers the record it made the first time. The balance never resets (no
//!   cycles).
//! - `appPurchaseOneTimeCreate` makes a `PENDING` purchase, approved (`ACTIVE`) or declined on
//!   the same page.
//! - Webhooks (shopify.dev: "triggered when a subscription's status or capped amount changes",
//!   "when a one-time purchase status changes", "when usage reaches 90% of the capped amount"):
//!   `app_subscriptions/update`, `app_purchases_one_time/update`,
//!   `app_subscriptions/approaching_capped_amount` (once, when the balance crosses 90%).
//!
//! The ids of subscriptions and purchases share their numbers, so that `charge_id` names one
//! charge; a line item's id is its subscription's with `?v=1&index=<i>`, as Shopify writes it.
//! The messages lsf could not verify are Rails' ("Name can't be blank") or its own, said so where
//! they are written.

use chrono::Duration;
use serde_json::{Value, json};

use super::CHANGED;
use crate::domains::catalog::gids;
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::{Record, ShopState, gid, parse_gid, value};

/// How long a charge waits for the merchant before it expires (`EXPIRED`: "wasn't approved by
/// the merchant within two days of being created").
const EXPIRES_AFTER_DAYS: i64 = 2;

/// The longest idempotency key (shopify.dev: "Maximum length of 255 characters").
const IDEMPOTENCY_KEY_LENGTH: usize = 255;

/// The share of the capped amount past which `app_subscriptions/approaching_capped_amount` is
/// sent ("when merchants reach or exceed 90% of their capped amount").
const APPROACHING: f64 = 0.9;

pub fn register(registry: &mut Registry) {
    registry
        .query("appByKey", app_by_key)
        .field(
            "AppInstallation",
            "activeSubscriptions",
            active_subscriptions,
        )
        .field("AppInstallation", "subscriptions", active_subscriptions)
        .field("AppInstallation", "allSubscriptions", |r, ctx, _object| {
            r.connection(ctx, r.records_of("AppSubscription"))
        })
        .field("AppInstallation", "oneTimePurchases", |r, ctx, _object| {
            r.connection(ctx, r.records_of("AppPurchaseOneTime"))
        })
        .mutation("appSubscriptionCreate", subscription_create)
        .mutation("appSubscriptionCancel", subscription_cancel)
        .mutation("appSubscriptionLineItemUpdate", line_item_update)
        .mutation("appSubscriptionTrialExtend", trial_extend)
        .mutation("appUsageRecordCreate", usage_record_create)
        .mutation("appPurchaseOneTimeCreate", purchase_create);
}

/// `appByKey(apiKey:)`: the app, when the key is its client id.
fn app_by_key(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let key = ctx.arg_str("apiKey").unwrap_or_default();
    Ok(Output::object_or_null(r.find("App", |app| {
        app.get("apiKey").and_then(Value::as_str) == Some(key)
    })))
}

/// `activeSubscriptions` (and the deprecated `subscriptions`): those that are `ACTIVE`.
fn active_subscriptions(
    r: &ShopResolver<'_>,
    _ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    Ok(Output::List(
        state
            .keys("AppSubscription")
            .filter(|key| status(state, key) == Some("ACTIVE"))
            .map(|key| Output::Object(Obj::record(key)))
            .collect(),
    ))
}

// ---- values -------------------------------------------------------------------------------

fn status<'s>(state: &'s ShopState, key: &str) -> Option<&'s str> {
    state
        .get(key)
        .and_then(|record| record.get("status"))
        .and_then(Value::as_str)
}

/// A `MoneyInput` (`{amount, currencyCode}`) as its amount and currency.
fn money_of(input: Option<&Value>) -> Option<(f64, String)> {
    let input = input?;
    let amount = match input.get("amount")? {
        Value::String(text) => text.trim().parse().ok()?,
        Value::Number(number) => number.as_f64()?,
        _ => return None,
    };
    let currency = input.get("currencyCode")?.as_str()?.to_string();
    Some((amount, currency))
}

/// A `MoneyV2` of an amount: `{"amount": "10.0", "currencyCode": "USD"}`.
fn money_v2(amount: f64, currency: &str) -> Value {
    value::money_v2((amount * 100.0).round() as i64, currency)
}

/// The amount of a `MoneyV2`.
fn amount_of(money: Option<&Value>) -> f64 {
    money
        .and_then(|money| money.get("amount"))
        .and_then(|amount| match amount {
            Value::String(text) => text.parse().ok(),
            other => other.as_f64(),
        })
        .unwrap_or(0.0)
}

fn currency_of(money: Option<&Value>) -> String {
    money
        .and_then(|money| money.get("currencyCode"))
        .and_then(Value::as_str)
        .unwrap_or("USD")
        .to_string()
}

/// The next number of a charge: subscriptions and one-time purchases share them.
fn next_charge(state: &ShopState) -> u64 {
    state
        .keys("AppSubscription")
        .chain(state.keys("AppPurchaseOneTime"))
        .filter_map(parse_gid)
        .map(|(_, id)| id)
        .max()
        .unwrap_or(0)
        + 1
}

/// The page where the merchant approves a charge.
pub fn confirmation_url(origin: &str, shop: &str, number: u64) -> String {
    format!("{origin}/__lsf/admin/charges/{number}?shop={shop}")
}

/// The pricing details of a line item record.
fn pricing(state: &ShopState, line: &str) -> Option<Record> {
    state
        .get(line)?
        .get("plan")?
        .get("pricingDetails")?
        .as_object()
        .cloned()
}

/// The subscription a line item belongs to: its id is the subscription's number with
/// `?v=1&index=<i>`.
fn subscription_of_line(line: &str) -> Option<String> {
    let (kind, number) = parse_gid(line)?;
    (kind == "AppSubscriptionLineItem").then(|| gid("AppSubscription", number))
}

/// Marks the pending charges older than two days as `EXPIRED`, and emits their update. Run by
/// the shop's tick (`domains::tick`), and before a charge is made or decided.
pub fn expire_stale(state: &mut ShopState) {
    let now = state.now();
    let stale: Vec<(String, &'static str)> = state
        .keys("AppSubscription")
        .map(|key| (key, "app_subscriptions/update"))
        .chain(
            state
                .keys("AppPurchaseOneTime")
                .map(|key| (key, "app_purchases_one_time/update")),
        )
        .filter(|(key, _)| {
            let Some(record) = state.get(key) else {
                return false;
            };
            record.get("status").and_then(Value::as_str) == Some("PENDING")
                && value::date_time_of(record.get("createdAt"))
                    .is_some_and(|created| created + Duration::days(EXPIRES_AFTER_DAYS) <= now)
        })
        .map(|(key, topic)| (key.to_string(), topic))
        .collect();
    for (key, topic) in stale {
        set_status(state, &key, "EXPIRED");
        state.emit(topic, &key);
    }
}

fn set_status(state: &mut ShopState, key: &str, status: &str) {
    let now = state.timestamp();
    if let Some(record) = state.get_mut(key) {
        record.insert("status".into(), json!(status));
        record.insert("updatedAt".into(), json!(now));
        if status != "ACTIVE"
            && record.get("__typename").and_then(Value::as_str) == Some("AppSubscription")
        {
            record.insert("currentPeriodEnd".into(), Value::Null);
        }
    }
}

// ---- appSubscriptionCreate ----------------------------------------------------------------

/// A line item of an `appSubscriptionCreate` input, checked: its pricing details.
fn line_details(
    line: &Value,
    index: usize,
    interval: &str,
    errors: &mut Vec<UserError>,
) -> Option<Value> {
    let position = index.to_string();
    let plan = line.get("plan");
    let recurring = plan
        .and_then(|plan| plan.get("appRecurringPricingDetails"))
        .filter(|value| !value.is_null());
    let usage = plan
        .and_then(|plan| plan.get("appUsagePricingDetails"))
        .filter(|value| !value.is_null());
    match (recurring, usage) {
        (Some(recurring), None) => {
            let Some((price, currency)) = money_of(recurring.get("price")) else {
                errors.push(UserError::new(
                    &[
                        "lineItems",
                        &position,
                        "plan",
                        "appRecurringPricingDetails",
                        "price",
                    ],
                    "Price can't be blank",
                ));
                return None;
            };
            if price < 0.0 {
                errors.push(UserError::new(
                    &[
                        "lineItems",
                        &position,
                        "plan",
                        "appRecurringPricingDetails",
                        "price",
                    ],
                    "Price must be greater than or equal to 0",
                ));
            }
            let interval = recurring
                .get("interval")
                .and_then(Value::as_str)
                .unwrap_or("EVERY_30_DAYS");
            let discount = recurring
                .get("discount")
                .filter(|value| !value.is_null())
                .map(|discount| {
                    let limit = discount
                        .get("durationLimitInIntervals")
                        .and_then(Value::as_i64);
                    let value = discount.get("value");
                    let (value, after) = match (
                        value.and_then(|value| value.get("amount")).filter(|v| !v.is_null()),
                        value
                            .and_then(|value| value.get("percentage"))
                            .and_then(Value::as_f64),
                    ) {
                        (Some(amount), _) => {
                            let amount = match amount {
                                Value::String(text) => text.parse().unwrap_or(0.0),
                                other => other.as_f64().unwrap_or(0.0),
                            };
                            (
                                json!({
                                    "__typename": "AppSubscriptionDiscountAmount",
                                    "amount": money_v2(amount, &currency),
                                }),
                                (price - amount).max(0.0),
                            )
                        }
                        (None, Some(percentage)) => (
                            json!({
                                "__typename": "AppSubscriptionDiscountPercentage",
                                "percentage": percentage,
                            }),
                            (price * (1.0 - percentage)).max(0.0),
                        ),
                        _ => (
                            json!({"__typename": "AppSubscriptionDiscountPercentage", "percentage": 0.0}),
                            price,
                        ),
                    };
                    json!({
                        "durationLimitInIntervals": limit,
                        "remainingDurationInIntervals": limit,
                        "priceAfterDiscount": money_v2(after, &currency),
                        "value": value,
                    })
                })
                .unwrap_or(Value::Null);
            Some(json!({
                "__typename": "AppRecurringPricing",
                "price": money_v2(price, &currency),
                "interval": interval,
                "planHandle": null,
                "discount": discount,
            }))
        }
        (None, Some(usage)) => {
            let path = ["lineItems", &position, "plan", "appUsagePricingDetails"];
            let Some((capped, currency)) = money_of(usage.get("cappedAmount")) else {
                errors.push(UserError::new(
                    &[path.as_slice(), &["cappedAmount"]].concat(),
                    "Capped amount can't be blank",
                ));
                return None;
            };
            if capped <= 0.0 {
                errors.push(UserError::new(
                    &[path.as_slice(), &["cappedAmount"]].concat(),
                    "Capped amount must be greater than 0",
                ));
            }
            let terms = usage
                .get("terms")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if terms.trim().is_empty() {
                errors.push(UserError::new(
                    &[path.as_slice(), &["terms"]].concat(),
                    "Terms can't be blank",
                ));
            }
            Some(json!({
                "__typename": "AppUsagePricing",
                "balanceUsed": money_v2(0.0, &currency),
                "cappedAmount": money_v2(capped, &currency),
                "interval": interval,
                "terms": terms,
            }))
        }
        _ => {
            errors.push(UserError::new(
                &["lineItems", &position, "plan"],
                "A plan has either appRecurringPricingDetails or appUsagePricingDetails",
            ));
            None
        }
    }
}

fn subscription_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = json!({"appSubscription": null, "confirmationUrl": null});
    let name = ctx.arg_str("name").unwrap_or_default().trim().to_string();
    let return_url = ctx.arg_str("returnUrl").unwrap_or_default().to_string();
    let lines: Vec<Value> = ctx
        .arg("lineItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let trial_days = ctx.arg_i64("trialDays").unwrap_or(0);
    let mut errors = Vec::new();
    if name.is_empty() {
        errors.push(UserError::new(&["name"], "Name can't be blank"));
    }
    if lines.is_empty() {
        errors.push(UserError::new(&["lineItems"], "Line items can't be blank"));
    }
    if trial_days < 0 {
        errors.push(UserError::new(
            &["trialDays"],
            "Trial days must be greater than or equal to 0",
        ));
    }
    let interval = lines
        .iter()
        .find_map(|line| {
            line.pointer("/plan/appRecurringPricingDetails/interval")
                .and_then(Value::as_str)
        })
        .unwrap_or("EVERY_30_DAYS")
        .to_string();
    let details: Vec<Value> = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| line_details(line, index, &interval, &mut errors))
        .collect();
    for kind in ["AppRecurringPricing", "AppUsagePricing"] {
        let count = details
            .iter()
            .filter(|details| details.get("__typename").and_then(Value::as_str) == Some(kind))
            .count();
        if count > 1 {
            errors.push(UserError::new(
                &["lineItems"],
                "Only one pricing plan can be defined for each available type.",
            ));
            break;
        }
    }
    let currencies: Vec<String> = details
        .iter()
        .map(|details| currency_of(details.get("price").or_else(|| details.get("cappedAmount"))))
        .collect();
    if currencies.windows(2).any(|pair| pair[0] != pair[1]) {
        errors.push(UserError::new(
            &["lineItems"],
            "The line items of a subscription have one currency",
        ));
    }
    if !errors.is_empty() {
        return Ok(payload(failed, &errors));
    }

    let origin = mutation.origin().to_string();
    let shop = mutation.shop().to_string();
    let test = ctx.arg_bool("test").unwrap_or(false);
    let behavior = ctx
        .arg_str("replacementBehavior")
        .unwrap_or("STANDARD")
        .to_string();
    let state = mutation.state_mut();
    expire_stale(state);
    let now = state.timestamp();
    let number = next_charge(state);
    let id = gid("AppSubscription", number);
    let mut line_ids = Vec::new();
    for (index, details) in details.into_iter().enumerate() {
        let line = format!("gid://shopify/AppSubscriptionLineItem/{number}?v=1&index={index}");
        state.insert(value::object(json!({
            "__typename": "AppSubscriptionLineItem",
            "id": line,
            "plan": {"pricingDetails": details},
            "usageRecords": [],
        })));
        line_ids.push(line);
    }
    state.insert(value::object(json!({
        "__typename": "AppSubscription",
        "id": id,
        "name": name,
        "status": "PENDING",
        "test": test,
        "trialDays": trial_days,
        "returnUrl": return_url,
        "currentPeriodEnd": null,
        "lineItems": line_ids,
        "replacementBehavior": behavior,
        "createdAt": now,
        "updatedAt": now,
    })));
    // A charge waiting for the merchant is no status change: no webhook, but a change.
    state.emit(CHANGED, &id);
    Ok(payload(
        json!({
            "appSubscription": id,
            "confirmationUrl": confirmation_url(&origin, &shop, number),
        }),
        &[],
    ))
}

// ---- the other subscription mutations -----------------------------------------------------

fn subscription_cancel(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let state = mutation.state();
    if state.type_of(&id) != Some("AppSubscription") {
        let error = UserError::new(&["id"], "Subscription not found");
        return Ok(payload(json!({"appSubscription": null}), &[error]));
    }
    let current = status(state, &id).unwrap_or_default().to_string();
    if matches!(current.as_str(), "CANCELLED" | "DECLINED" | "EXPIRED") {
        let error = UserError::new(
            &["id"],
            format!(
                "The subscription is {} and cannot be cancelled",
                current.to_ascii_lowercase()
            ),
        );
        return Ok(payload(json!({"appSubscription": null}), &[error]));
    }
    if ctx.arg_bool("prorate") == Some(true) {
        mutation.note("appSubscriptionCancel: lsf moves no money: `prorate` issued no credit");
    }
    let state = mutation.state_mut();
    set_status(state, &id, "CANCELLED");
    state.emit("app_subscriptions/update", &id);
    Ok(payload(json!({"appSubscription": id}), &[]))
}

fn trial_extend(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("id").unwrap_or_default().to_string();
    let days = ctx.arg_i64("days").unwrap_or(0);
    let failed = json!({"appSubscription": null});
    let state = mutation.state();
    let error = |field: &str, message: &str, code: &str| {
        Ok(payload(
            failed.clone(),
            &[UserError::new(&[field], message).code(code)],
        ))
    };
    if !(1..=1000).contains(&days) {
        // "The value must be greater than 0 and less than or equal to 1000": no code for it.
        let error = UserError::new(
            &["days"],
            "Days must be greater than 0 and less than or equal to 1000",
        );
        return Ok(payload(failed, &[error]));
    }
    let Some(record) = state
        .get(&id)
        .filter(|_| state.type_of(&id) == Some("AppSubscription"))
    else {
        return error(
            "id",
            "The app subscription wasn't found.",
            "SUBSCRIPTION_NOT_FOUND",
        );
    };
    if record.get("status").and_then(Value::as_str) != Some("ACTIVE") {
        return error(
            "id",
            "The app subscription isn't active.",
            "SUBSCRIPTION_NOT_ACTIVE",
        );
    }
    let trial_ends = value::date_time_of(record.get("trialEndsAt"));
    if !trial_ends.is_some_and(|ends| ends > state.now()) {
        return error("id", "The trial isn't active.", "TRIAL_NOT_ACTIVE");
    }
    let state = mutation.state_mut();
    let now = state.timestamp();
    if let Some(record) = state.get_mut(&id) {
        let trial = record.get("trialDays").and_then(Value::as_i64).unwrap_or(0);
        record.insert("trialDays".into(), json!(trial + days));
        for field in ["trialEndsAt", "currentPeriodEnd"] {
            if let Some(date) = value::date_time_of(record.get(field)) {
                record.insert(
                    field.into(),
                    json!(value::date_time(date + Duration::days(days))),
                );
            }
        }
        record.insert("updatedAt".into(), json!(now));
    }
    state.emit(CHANGED, &id);
    Ok(payload(json!({"appSubscription": id}), &[]))
}

fn line_item_update(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = json!({"appSubscription": null, "confirmationUrl": null});
    let line = ctx.arg_str("id").unwrap_or_default().to_string();
    let state = mutation.state();
    let details = (state.type_of(&line) == Some("AppSubscriptionLineItem"))
        .then(|| pricing(state, &line))
        .flatten();
    let Some(details) = details else {
        let error = UserError::new(&["id"], "Subscription line item not found");
        return Ok(payload(failed, &[error]));
    };
    if details.get("__typename").and_then(Value::as_str) != Some("AppUsagePricing") {
        let error = UserError::new(
            &["id"],
            "Only the capped amount of a usage pricing plan can be updated",
        );
        return Ok(payload(failed, &[error]));
    }
    let Some((capped, currency)) = money_of(ctx.arg("cappedAmount")) else {
        let error = UserError::new(&["cappedAmount"], "Capped amount can't be blank");
        return Ok(payload(failed, &[error]));
    };
    let mut errors = Vec::new();
    if currency != currency_of(details.get("cappedAmount")) {
        errors.push(UserError::new(
            &["cappedAmount", "currencyCode"],
            "The currency of the capped amount cannot be changed",
        ));
    }
    if capped <= 0.0 {
        errors.push(UserError::new(
            &["cappedAmount", "amount"],
            "Capped amount must be greater than 0",
        ));
    }
    let Some(subscription) = subscription_of_line(&line) else {
        return Ok(payload(failed, &[]));
    };
    if status(state, &subscription) != Some("ACTIVE") {
        errors.push(UserError::new(&["id"], "The subscription is not active"));
    }
    if !errors.is_empty() {
        return Ok(payload(failed, &errors));
    }
    let origin = mutation.origin().to_string();
    let shop = mutation.shop().to_string();
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&line) {
        record.insert("pendingCappedAmount".into(), money_v2(capped, &currency));
    }
    state.emit(CHANGED, &line);
    let number = parse_gid(&subscription).map_or(0, |(_, id)| id);
    Ok(payload(
        json!({
            "appSubscription": subscription,
            "confirmationUrl": confirmation_url(&origin, &shop, number),
        }),
        &[],
    ))
}

fn usage_record_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = json!({"appUsageRecord": null});
    let line = ctx
        .arg_str("subscriptionLineItemId")
        .unwrap_or_default()
        .to_string();
    let description = ctx
        .arg_str("description")
        .unwrap_or_default()
        .trim()
        .to_string();
    let key = ctx.arg_str("idempotencyKey").map(str::to_string);
    let state = mutation.state();
    if key
        .as_ref()
        .is_some_and(|key| key.chars().count() > IDEMPOTENCY_KEY_LENGTH)
    {
        let error = UserError::new(
            &["idempotencyKey"],
            "Idempotency key exceeds the maximum length.",
        );
        return Ok(payload(failed, &[error]));
    }
    let details = (state.type_of(&line) == Some("AppSubscriptionLineItem"))
        .then(|| pricing(state, &line))
        .flatten();
    let Some(details) = details else {
        let error = UserError::new(
            &["subscriptionLineItemId"],
            "Subscription line item not found",
        );
        return Ok(payload(failed, &[error]));
    };
    if let Some(key) = &key {
        // The same key answers the record it made, and charges nothing more.
        let existing = state
            .get(&line)
            .and_then(|record| record.get("usageRecords"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .find(|record| {
                state
                    .get(record)
                    .and_then(|record| record.get("idempotencyKey"))
                    .and_then(Value::as_str)
                    == Some(key.as_str())
            })
            .map(str::to_string);
        if let Some(existing) = existing {
            return Ok(payload(json!({"appUsageRecord": existing}), &[]));
        }
    }
    if description.is_empty() {
        let error = UserError::new(&["description"], "Description can't be blank");
        return Ok(payload(failed, &[error]));
    }
    let Some((price, currency)) = money_of(ctx.arg("price")) else {
        let error = UserError::new(&["price"], "Price can't be blank");
        return Ok(payload(failed, &[error]));
    };
    let subscription = subscription_of_line(&line).unwrap_or_default();
    let capped = amount_of(details.get("cappedAmount"));
    let used = amount_of(details.get("balanceUsed"));
    let usable = details.get("__typename").and_then(Value::as_str) == Some("AppUsagePricing")
        && status(state, &subscription) == Some("ACTIVE")
        && currency == currency_of(details.get("cappedAmount"))
        && price > 0.0;
    // shopify.dev's example of a record above the capped amount; lsf answers the same for a
    // line that cannot be charged (not a usage plan, not active, another currency).
    if !usable || used + price > capped + 1e-9 {
        let error = UserError::new(&[], "Failed to create usage charge");
        return Ok(payload(failed, &[error]));
    }

    let state = mutation.state_mut();
    let now = state.timestamp();
    let record = state.new_gid("AppUsageRecord");
    state.insert(value::object(json!({
        "__typename": "AppUsageRecord",
        "id": record,
        "description": description,
        "idempotencyKey": key,
        "price": money_v2(price, &currency),
        "subscriptionLineItem": line,
        "createdAt": now,
    })));
    let balance = used + price;
    if let Some(line_record) = state.get_mut(&line) {
        if let Some(details) = line_record
            .get_mut("plan")
            .and_then(|plan| plan.get_mut("pricingDetails"))
            .and_then(Value::as_object_mut)
        {
            details.insert("balanceUsed".into(), money_v2(balance, &currency));
        }
        if let Some(Value::Array(records)) = line_record.get_mut("usageRecords") {
            records.push(json!(record));
        }
    }
    state.emit(CHANGED, &record);
    if used < APPROACHING * capped && balance >= APPROACHING * capped {
        state.emit("app_subscriptions/approaching_capped_amount", &subscription);
    }
    Ok(payload(json!({"appUsageRecord": record}), &[]))
}

fn purchase_create(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
) -> Result<Value, FieldError> {
    let failed = json!({"appPurchaseOneTime": null, "confirmationUrl": null});
    let name = ctx.arg_str("name").unwrap_or_default().trim().to_string();
    let return_url = ctx.arg_str("returnUrl").unwrap_or_default().to_string();
    let mut errors = Vec::new();
    if name.is_empty() {
        errors.push(UserError::new(&["name"], "Name can't be blank"));
    }
    let price = money_of(ctx.arg("price"));
    match &price {
        None => errors.push(UserError::new(&["price"], "Price can't be blank")),
        Some((amount, _)) if *amount <= 0.0 => errors.push(UserError::new(
            &["price", "amount"],
            "Price must be greater than 0",
        )),
        _ => {}
    }
    let Some((amount, currency)) = price.filter(|_| errors.is_empty()) else {
        return Ok(payload(failed, &errors));
    };
    let origin = mutation.origin().to_string();
    let shop = mutation.shop().to_string();
    let test = ctx.arg_bool("test").unwrap_or(false);
    let state = mutation.state_mut();
    expire_stale(state);
    let now = state.timestamp();
    let number = next_charge(state);
    let id = gid("AppPurchaseOneTime", number);
    state.insert(value::object(json!({
        "__typename": "AppPurchaseOneTime",
        "id": id,
        "name": name,
        "price": money_v2(amount, &currency),
        "status": "PENDING",
        "test": test,
        "returnUrl": return_url,
        "createdAt": now,
        "updatedAt": now,
    })));
    state.emit(CHANGED, &id);
    Ok(payload(
        json!({
            "appPurchaseOneTime": id,
            "confirmationUrl": confirmation_url(&origin, &shop, number),
        }),
        &[],
    ))
}

// ---- the merchant's decision ----------------------------------------------------------------

/// What the merchant decides on the confirmation page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Approve,
    Decline,
}

/// A charge as the confirmation page shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Charge {
    /// `AppSubscription` or `AppPurchaseOneTime`.
    pub kind: &'static str,
    pub id: String,
    pub number: u64,
    pub name: String,
    pub status: String,
    pub test: bool,
    pub trial_days: i64,
    pub return_url: String,
    /// What is charged, a line each: `10.00 USD every 30 days`, `Up to 20.00 USD of usage`.
    pub lines: Vec<String>,
    /// A new capped amount the app asks for, for an active subscription.
    pub pending_capped_amount: Option<String>,
    /// The app's title.
    pub app: String,
}

impl Charge {
    /// Whether the merchant has something to decide.
    pub fn decidable(&self) -> bool {
        self.status == "PENDING" || self.pending_capped_amount.is_some()
    }

    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "chargeId": self.number,
            "kind": self.kind,
            "name": self.name,
            "status": self.status,
            "test": self.test,
            "trialDays": self.trial_days,
            "returnUrl": self.return_url,
            "lines": self.lines,
            "pendingCappedAmount": self.pending_capped_amount,
        })
    }
}

fn money_text(money: Option<&Value>) -> String {
    format!("{:.2} {}", amount_of(money), currency_of(money))
}

/// The charge of number `number` (a subscription or a one-time purchase).
pub fn charge(state: &ShopState, number: u64) -> Option<Charge> {
    let (kind, id) = [
        ("AppSubscription", gid("AppSubscription", number)),
        ("AppPurchaseOneTime", gid("AppPurchaseOneTime", number)),
    ]
    .into_iter()
    .find(|(_, id)| state.contains(id))?;
    let record = state.get(&id)?;
    let text = |field: &str| {
        record
            .get(field)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let mut lines = Vec::new();
    let mut pending = None;
    if kind == "AppSubscription" {
        for line in record
            .get("lineItems")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            let Some(details) = pricing(state, line) else {
                continue;
            };
            if details.get("__typename").and_then(Value::as_str) == Some("AppUsagePricing") {
                lines.push(format!(
                    "Usage charges up to {} every 30 days: {}",
                    money_text(details.get("cappedAmount")),
                    details
                        .get("terms")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                ));
                if let Some(new) = state
                    .get(line)
                    .and_then(|line| line.get("pendingCappedAmount"))
                {
                    pending = Some(money_text(Some(new)));
                }
            } else {
                let every = match details.get("interval").and_then(Value::as_str) {
                    Some("ANNUAL") => "every year",
                    _ => "every 30 days",
                };
                let mut line = format!("{} {every}", money_text(details.get("price")));
                if let Some(after) = details
                    .get("discount")
                    .and_then(|discount| discount.get("priceAfterDiscount"))
                {
                    line.push_str(&format!(" ({} with the discount)", money_text(Some(after))));
                }
                lines.push(line);
            }
        }
    } else {
        lines.push(format!("{} once", money_text(record.get("price"))));
    }
    let app = state
        .all("App")
        .next()
        .and_then(|app| app.get("title"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Some(Charge {
        kind,
        id: id.clone(),
        number,
        name: text("name"),
        status: text("status"),
        test: record.get("test").and_then(Value::as_bool).unwrap_or(false),
        trial_days: record.get("trialDays").and_then(Value::as_i64).unwrap_or(0),
        return_url: text("returnUrl"),
        lines,
        pending_capped_amount: pending.filter(|_| text("status") == "ACTIVE"),
        app,
    })
}

/// Why a decision cannot be made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecisionError {
    NotFound,
    /// The charge is not waiting for the merchant: its status.
    NotPending(String),
}

/// Approves or declines a charge, as the merchant does on Shopify's page, and emits its
/// webhooks. Answers the URL the merchant is sent to: the charge's `returnUrl` with
/// `charge_id`, approved or declined.
pub fn decide(
    state: &mut ShopState,
    number: u64,
    decision: Decision,
) -> Result<String, DecisionError> {
    expire_stale(state);
    let charge = charge(state, number).ok_or(DecisionError::NotFound)?;
    if !charge.decidable() {
        return Err(DecisionError::NotPending(charge.status));
    }
    let approve = decision == Decision::Approve;
    let id = charge.id.clone();
    match charge.kind {
        "AppPurchaseOneTime" => {
            set_status(state, &id, if approve { "ACTIVE" } else { "DECLINED" });
            state.emit("app_purchases_one_time/update", &id);
        }
        _ if charge.status == "ACTIVE" => {
            // A new capped amount.
            let lines = gids(state.get(&id).and_then(|record| record.get("lineItems")));
            for line in lines {
                let Some(record) = state.get_mut(&line) else {
                    continue;
                };
                let Some(new) = record.remove("pendingCappedAmount") else {
                    continue;
                };
                if approve
                    && let Some(details) = record
                        .get_mut("plan")
                        .and_then(|plan| plan.get_mut("pricingDetails"))
                        .and_then(Value::as_object_mut)
                {
                    details.insert("cappedAmount".into(), new);
                }
            }
            let now = state.timestamp();
            if let Some(record) = state.get_mut(&id) {
                record.insert("updatedAt".into(), json!(now));
            }
            if approve {
                state.emit("app_subscriptions/update", &id);
            } else {
                state.emit(CHANGED, &id);
            }
        }
        _ if approve => {
            // The app's other active subscription is replaced (lsf applies every replacement
            // at once: it has no billing cycles).
            let others: Vec<String> = state
                .keys("AppSubscription")
                .filter(|key| *key != id && status(state, key) == Some("ACTIVE"))
                .map(str::to_string)
                .collect();
            for other in others {
                set_status(state, &other, "CANCELLED");
                state.emit("app_subscriptions/update", &other);
            }
            activate(state, &id);
            state.emit("app_subscriptions/update", &id);
        }
        _ => {
            set_status(state, &id, "DECLINED");
            state.emit("app_subscriptions/update", &id);
        }
    }
    Ok(with_charge_id(&charge.return_url, number))
}

/// Activates an approved subscription: its trial starts now, and its period.
fn activate(state: &mut ShopState, id: &str) {
    let now = state.now();
    let annual = state
        .get(id)
        .and_then(|record| record.get("lineItems"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|line| pricing(state, line))
        .any(|details| details.get("interval").and_then(Value::as_str) == Some("ANNUAL"));
    set_status(state, id, "ACTIVE");
    if let Some(record) = state.get_mut(id) {
        let trial = record.get("trialDays").and_then(Value::as_i64).unwrap_or(0);
        let period = if annual { 365 } else { 30 };
        let ends = if trial > 0 {
            now + Duration::days(trial)
        } else {
            now + Duration::days(period)
        };
        record.insert("activatedAt".into(), json!(value::date_time(now)));
        record.insert(
            "trialEndsAt".into(),
            if trial > 0 {
                json!(value::date_time(now + Duration::days(trial)))
            } else {
                Value::Null
            },
        );
        record.insert("currentPeriodEnd".into(), json!(value::date_time(ends)));
    }
}

/// `returnUrl` with `charge_id=<n>` added to its query.
fn with_charge_id(url: &str, number: u64) -> String {
    let (base, fragment) = url
        .split_once('#')
        .map_or((url, None), |(base, fragment)| (base, Some(fragment)));
    let separator = if base.contains('?') { '&' } else { '?' };
    let mut target = format!("{base}{separator}charge_id={number}");
    if let Some(fragment) = fragment {
        target.push('#');
        target.push_str(fragment);
    }
    target
}

/// Cancels the app's active subscriptions, as uninstalling the app does ("When an app is
/// uninstalled, Shopify automatically cancels the subscription"). No webhook: the app is gone.
pub fn cancel_on_uninstall(state: &mut ShopState) {
    let active: Vec<String> = state
        .keys("AppSubscription")
        .filter(|key| matches!(status(state, key), Some("ACTIVE" | "FROZEN")))
        .map(str::to_string)
        .collect();
    for id in active {
        set_status(state, &id, "CANCELLED");
    }
}
