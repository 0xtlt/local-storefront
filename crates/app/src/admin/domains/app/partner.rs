//! What the Partner API reads of a shop (`crate::admin::partner`): the history of the app on it,
//! and its active Shopify App Pricing subscription.
//!
//! The history is kept as the changes happen, in a memo of the shop (not a record: no GraphQL type
//! of the Admin API, no webhook, no store sync), so that a later change of a subscription does not
//! rewrite what an event said. A reset puts it back to the seed's, which has the install the shop
//! starts with. Each event says which of the Partner API's two event systems shows it
//! (shopify.dev, "Shopify App Pricing", "Partner API event types"):
//!
//! - `eventType` (`EventType`, the root `events` query): `RELATIONSHIP_INSTALLED`,
//!   `RELATIONSHIP_UNINSTALLED`, and, for a subscription of managed pricing (one with a
//!   `planHandle`), `SUBSCRIPTION_CREATED` ("A merchant subscribed to a plan"),
//!   `SUBSCRIPTION_UPDATED` ("A merchant changed their plan") and `SUBSCRIPTION_CANCELED` ("The
//!   subscription was canceled").
//! - `appEventType` (`AppEventTypes`, the older `App.events`): the relationship events too, and one
//!   per change of status of any charge, the Billing API's included, as that field reported them
//!   before managed pricing: `SUBSCRIPTION_CHARGE_ACCEPTED` and `SUBSCRIPTION_CHARGE_ACTIVATED`
//!   when a charge becomes active, `…_CANCELED`, `…_DECLINED`, `…_EXPIRED`, `…_FROZEN`, and the
//!   `ONE_TIME_CHARGE_…` ones of a one-time purchase.
//!
//! lsf's choices where Shopify does not say: an approval records both `…_ACCEPTED` and
//! `…_ACTIVATED` (the REST charge passes through `accepted` before `active`, and Shopify has
//! activated an approved charge at once since 2021); a plan that replaces another is one
//! `SUBSCRIPTION_UPDATED`, with no `SUBSCRIPTION_CANCELED` for the plan it replaces (the page names
//! the update "A merchant changed their plan"); an uninstall records the end of the subscription
//! first, then `RELATIONSHIP_UNINSTALLED`. No `CHARGE_…`, `EARNING_…` or `CREDIT_…` event: no money
//! moves in lsf.

use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};

use super::billing;
use super::pricing;
use crate::admin::state::seed::Seed;
use crate::admin::state::{ShopState, value};

/// The memo the history of the app on the shop is kept in: a JSON array, oldest first.
const HISTORY: &str = "lsf/partner-events";

/// The install the shop starts with, dated as the seed's records are ([`Seed::epoch`]).
pub fn seed(seed: &mut Seed) {
    let at = Seed::epoch();
    push(&mut seed.state, relationship("RELATIONSHIP_INSTALLED", at));
}

/// The app is installed on the shop again (`domains::app::reinstall`).
pub fn installed(state: &mut ShopState) {
    let at = state.now();
    push(state, relationship("RELATIONSHIP_INSTALLED", at));
}

/// The app is uninstalled from the shop (`domains::app::uninstall`), or found uninstalled after a
/// new seed (`domains::app::reseed_uninstalled`).
pub fn uninstalled(state: &mut ShopState) {
    let at = state.now();
    push(state, relationship("RELATIONSHIP_UNINSTALLED", at));
}

fn relationship(kind: &str, at: DateTime<Utc>) -> Value {
    json!({"at": value::date_time(at), "eventType": kind, "appEventType": kind})
}

fn push(state: &mut ShopState, event: Value) {
    let mut history = match state.memo(HISTORY) {
        Some(Value::Array(events)) => events.clone(),
        _ => Vec::new(),
    };
    history.push(event);
    state.set_memo(HISTORY, Value::Array(history));
}

/// The history of the app on the shop, oldest first: each event `{at, eventType, appEventType,
/// charge?, plan?, cancelEffectiveOn?}` (see the module's doc). Its index in the list is its
/// sequence.
pub fn history(state: &ShopState) -> Vec<Value> {
    match state.memo(HISTORY) {
        Some(Value::Array(events)) => events.clone(),
        _ => Vec::new(),
    }
}

/// A charge changed status (`billing::set_status`, once the record has its new status): the events
/// it makes in the history.
pub(super) fn status_changed(state: &mut ShopState, id: &str, before: &str, after: &str) {
    let Some(record) = state.get(id) else {
        return;
    };
    let typename = record
        .get("__typename")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let app_events: &[&str] = match (typename.as_str(), after) {
        ("AppSubscription", "ACTIVE") if before == "FROZEN" => &["SUBSCRIPTION_CHARGE_UNFROZEN"],
        ("AppSubscription", "ACTIVE") => &[
            "SUBSCRIPTION_CHARGE_ACCEPTED",
            "SUBSCRIPTION_CHARGE_ACTIVATED",
        ],
        ("AppSubscription", "CANCELLED") => &["SUBSCRIPTION_CHARGE_CANCELED"],
        ("AppSubscription", "DECLINED") => &["SUBSCRIPTION_CHARGE_DECLINED"],
        ("AppSubscription", "EXPIRED") => &["SUBSCRIPTION_CHARGE_EXPIRED"],
        ("AppSubscription", "FROZEN") => &["SUBSCRIPTION_CHARGE_FROZEN"],
        ("AppPurchaseOneTime", "ACTIVE") => {
            &["ONE_TIME_CHARGE_ACCEPTED", "ONE_TIME_CHARGE_ACTIVATED"]
        }
        ("AppPurchaseOneTime", "DECLINED") => &["ONE_TIME_CHARGE_DECLINED"],
        ("AppPurchaseOneTime", "EXPIRED") => &["ONE_TIME_CHARGE_EXPIRED"],
        _ => &[],
    };
    let at = state.timestamp();
    let charge = charge_of(state, id);
    for kind in app_events {
        push(
            state,
            json!({"at": at, "eventType": null, "appEventType": kind, "charge": charge}),
        );
    }
    if typename != "AppSubscription" {
        return;
    }
    let Some(plan) = plan_of(state, id) else {
        // A subscription of the Billing API: the root `events` reports managed pricing.
        return;
    };
    // Another plan active now: the change of plan (`pricing::select` activates the new plan
    // before it cancels the one it replaces).
    let another = state.keys("AppSubscription").any(|other| {
        other != id
            && billing::status(state, other) == Some("ACTIVE")
            && plan_of(state, other).is_some()
    });
    let (kind, state_name) = match after {
        "ACTIVE" if before == "FROZEN" => ("SUBSCRIPTION_UNFROZEN", "UNFROZEN"),
        "ACTIVE" if another => ("SUBSCRIPTION_UPDATED", "UPDATED"),
        "ACTIVE" => ("SUBSCRIPTION_CREATED", "CREATED"),
        "CANCELLED" if matches!(before, "ACTIVE" | "FROZEN") && !another => {
            ("SUBSCRIPTION_CANCELED", "CANCELED")
        }
        "FROZEN" => ("SUBSCRIPTION_FROZEN", "FROZEN"),
        _ => return,
    };
    let cancel_effective_on =
        (state_name == "CANCELED").then(|| state.now().date_naive().format("%Y-%m-%d").to_string());
    push(
        state,
        json!({
            "at": at,
            "eventType": kind,
            "appEventType": null,
            "state": state_name,
            "subscription": id,
            "plan": plan,
            "cancelEffectiveOn": cancel_effective_on,
        }),
    );
}

/// What an event of `App.events` says of a charge, as it is now: the Partner API's
/// `AppSubscription` (`id`, `name`, `amount`, `test`, `billingOn`) or `AppPurchaseOneTime`.
fn charge_of(state: &ShopState, id: &str) -> Value {
    let Some(record) = state.get(id) else {
        return Value::Null;
    };
    let typename = record
        .get("__typename")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let (amount, currency) = if typename == "AppSubscription" {
        recurring_price(state, id).unwrap_or((0.0, "USD".to_string()))
    } else {
        money(record.get("price")).unwrap_or((0.0, "USD".to_string()))
    };
    let mut charge = json!({
        "__typename": typename,
        "id": id,
        "name": record.get("name").cloned().unwrap_or(Value::Null),
        "amount": partner_money(amount, &currency),
        "test": record.get("test").and_then(Value::as_bool).unwrap_or(false),
    });
    if typename == "AppSubscription" {
        charge["billingOn"] = record
            .get("currentPeriodEnd")
            .cloned()
            .unwrap_or(Value::Null);
    }
    charge
}

/// A `Money` of the Partner API: its `amount` a `Decimal` with two places (`"4.99"`, as the
/// examples of shopify.dev write it), and its `currencyCode`.
pub fn partner_money(amount: f64, currency: &str) -> Value {
    json!({"__typename": "Money", "amount": decimal(amount), "currencyCode": currency})
}

/// A `Decimal` of the Partner API: `"29.00"`.
pub fn decimal(amount: f64) -> String {
    format!("{amount:.2}")
}

/// The amount and currency of a `MoneyV2` of the Admin API.
fn money(money: Option<&Value>) -> Option<(f64, String)> {
    let money = money?;
    let amount = match money.get("amount")? {
        Value::String(text) => text.parse().ok()?,
        Value::Number(number) => number.as_f64()?,
        _ => return None,
    };
    let currency = money.get("currencyCode")?.as_str()?.to_string();
    Some((amount, currency))
}

/// The pricing details of the recurring line of a subscription (`AppRecurringPricing`).
fn recurring_details(state: &ShopState, id: &str) -> Option<serde_json::Map<String, Value>> {
    state
        .get(id)?
        .get("lineItems")?
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .find_map(|line| {
            let details = state.get(line)?.get("plan")?.get("pricingDetails")?;
            (details.get("__typename").and_then(Value::as_str) == Some("AppRecurringPricing"))
                .then(|| details.as_object().cloned())
                .flatten()
        })
}

fn recurring_price(state: &ShopState, id: &str) -> Option<(f64, String)> {
    money(recurring_details(state, id)?.get("price"))
}

/// The plan of a subscription of managed pricing, as an event keeps it (`Plan` of the Partner
/// API: `handle`, `billingPeriod`, `trialDays`, `trialDaysRemaining`, `prices`), `None` for a
/// subscription of the Billing API (no `planHandle`).
fn plan_of(state: &ShopState, id: &str) -> Option<Value> {
    let details = recurring_details(state, id)?;
    let handle = details.get("planHandle")?.as_str()?.to_string();
    let record = state.get(id)?;
    let (amount, currency) = money(details.get("price")).unwrap_or((0.0, "USD".to_string()));
    let trial_days = record.get("trialDays").and_then(Value::as_i64).unwrap_or(0);
    let remaining = value::date_time_of(record.get("trialEndsAt"))
        .map_or(0, |ends| days_left(state.now(), ends));
    Some(json!({
        "__typename": "Plan",
        "handle": handle,
        "billingPeriod": details.get("interval").cloned().unwrap_or(json!("EVERY_30_DAYS")),
        "trialDays": trial_days,
        "trialDaysRemaining": remaining,
        "prices": [{
            "__typename": "FlatRatePlanPrice",
            "amount": decimal(amount),
            "currencyCode": currency,
        }],
    }))
}

/// The whole days left from `now` until `ends`, a part of a day counting as one; 0 once it passed.
fn days_left(now: DateTime<Utc>, ends: DateTime<Utc>) -> i64 {
    let seconds = (ends - now).num_seconds();
    if seconds <= 0 {
        0
    } else {
        (seconds + 86_399) / 86_400
    }
}

/// The shop's active subscription of managed pricing, as `activeSubscription` of the Partner API
/// answers it (without its `app` and `shop`, which the Partner API adds), at the shop's clock.
/// `None` when the shop has none: no subscription, or one of the Billing API ("The shop must have
/// an active managed pricing contract for the app. If no contract exists, `activeSubscription`
/// returns `null`", shopify.dev, "Active subscription").
///
/// - `trialEndsAt` while the trial runs, then `null`; `currentBillingCycle` `null` during the
///   trial, then the cycle the clock is in ("While a subscription is in its trial period,
///   `currentBillingCycle` is `null` … After the trial ends, `trialEndsAt` becomes `null` and
///   `currentBillingCycle` is populated"). lsf has no billing cycles: they are counted in periods
///   of 30 days (365 for an annual plan) from the subscription's `currentPeriodEnd`.
/// - One item, the plan: its handle, its name as the description, a `FlatRatePrice`.
/// - `legacySubscriptionId`: the subscription's Admin API GID while the plans keep the legacy
///   features (`legacy`: the welcome link's `charge_id` names it), `null` with `legacy = false`
///   ("For subscriptions created natively in managed pricing, this field is `null`").
/// - `cancelAtEndOfCycle` is `false` and `pendingUpdate` `null`: lsf changes a plan at once.
pub fn active_subscription(state: &ShopState, legacy: bool) -> Option<Value> {
    let current = pricing::current(state)?;
    let handle = current.plan.clone()?;
    let details = recurring_details(state, &current.id)?;
    let record = state.get(&current.id)?;
    let now = state.now();
    let interval = details
        .get("interval")
        .and_then(Value::as_str)
        .unwrap_or("EVERY_30_DAYS")
        .to_string();
    let period = Duration::days(if interval == "ANNUAL" { 365 } else { 30 });
    let trial_ends = value::date_time_of(record.get("trialEndsAt")).filter(|ends| *ends > now);
    let cycle = match trial_ends {
        Some(_) => Value::Null,
        None => {
            let anchor = value::date_time_of(record.get("currentPeriodEnd"))
                .or_else(|| value::date_time_of(record.get("activatedAt")).map(|at| at + period))
                .unwrap_or(now + period);
            let end = if anchor > now {
                anchor
            } else {
                let passed = (now - anchor).num_seconds() / period.num_seconds() + 1;
                anchor + period * passed as i32
            };
            json!({
                "__typename": "BillingCycle",
                "startTime": value::date_time(end - period),
                "endTime": value::date_time(end),
            })
        }
    };
    let (amount, currency) = money(details.get("price")).unwrap_or((0.0, "USD".to_string()));
    Some(json!({
        "__typename": "ActiveSubscription",
        "billingPeriod": interval,
        "cancelAtEndOfCycle": false,
        "currentBillingCycle": cycle,
        "items": [{
            "__typename": "SubscriptionItem",
            "handle": handle,
            "description": current.name,
            "price": {
                "__typename": "FlatRatePrice",
                "active": true,
                "currency": currency,
                "amount": decimal(amount),
            },
            "discount": null,
            "usage": null,
        }],
        "legacySubscriptionId": if legacy { json!(current.id) } else { Value::Null },
        "pendingUpdate": null,
        "trialEndsAt": trial_ends.map(value::date_time),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_left_counts_a_started_day_as_one() {
        let now = Utc::now();
        assert_eq!(days_left(now, now - Duration::hours(1)), 0);
        assert_eq!(days_left(now, now), 0);
        assert_eq!(days_left(now, now + Duration::hours(1)), 1);
        assert_eq!(days_left(now, now + Duration::days(7)), 7);
        assert_eq!(
            days_left(now, now + Duration::days(6) + Duration::hours(1)),
            7
        );
    }

    #[test]
    fn partner_money_has_two_decimal_places() {
        assert_eq!(
            partner_money(4.99, "USD"),
            json!({"__typename": "Money", "amount": "4.99", "currencyCode": "USD"})
        );
        assert_eq!(decimal(49.0), "49.00");
        assert_eq!(decimal(0.0), "0.00");
    }
}
