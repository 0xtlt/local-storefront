//! The payment sessions of a device: `pointOfSaleDevicePaymentSessionOpen`, `…Count`,
//! `…Adjust`, `…Close`; the activities they record and what derives from them (the drawer's
//! balance, the session's totals, `CashDrawer.totalAdjustments(dateRange:)`, …); the summaries
//! (`cashManagementLocationSummary`, `cashManagementShopSummary`); the deprecated cash tracking
//! sessions (`cashTrackingSession`, `cashTrackingSessions`), read from the same sessions.
//!
//! As shopify.dev's reference of 2026-10 describes them:
//!
//! - `…Open(pointOfSaleDeviceId:, staffMemberId:, balance:, note:, reasonCodeId:, time:)` opens a
//!   session on a device that has a drawer (`NO_CASH_DRAWER_ASSOCIATED`) and no open session
//!   (`ALREADY_OPEN`); "The counted cash drawer balance when the session was opened" is
//!   `cashCountedAtOpen`, the drawer's balance `expectedCashAtOpen`. "Defaults to the current
//!   time." The session is the device's `activePaymentSession`.
//! - `…Count` "Records a mid-session cash count"; `…Adjust` "Adds an adjustment" (cash added or
//!   removed); `…Close` closes it with "The counted cash drawer balance". "Payments can't be
//!   processed or refunded during a closed session, and the cash drawer balance can't be
//!   adjusted or corrected" (`SESSION_ALREADY_CLOSED`); a time before the opening is
//!   `TIME_BEFORE_SESSION_OPEN`; an amount in another currency than the drawer's
//!   `CURRENCY_MISMATCH`.
//! - A count is a `CashCountActivity` (`cashCounted`, `cashExpected`, `cashDiscrepancy`, its
//!   `cash` the discrepancy: what the drawer's balance moved by); the drawer's balance becomes
//!   what was counted. An adjustment is a `CashAdjustmentActivity` (`ADJUSTMENT`) of `cash`.
//!   `totalDiscrepancy` is "The total discrepancy for the point of sale device payment session",
//!   `closingAdjustment` "The amount that the cash drawer balance was adjusted when the session
//!   was closed": the closing count's discrepancy.
//! - Summaries: sessions opened and closed in the period, the drawers' balances at its start
//!   and end (replayed from their activities), the net cash (sales and adjustments) and the
//!   discrepancies of the period. Shopify returns them "only if the location has a POS Pro
//!   subscription": lsf answers them for every location.

use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{Value, json};

use super::{cents, is_reason_code, money, tell_device, text};
use crate::domains::orders::model::currency;
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::search::SearchValue;
use crate::state::{Record, ShopState, gid, parse_gid, value};

/// The type of a session.
const SESSION: &str = "PointOfSaleDevicePaymentSession";

pub fn register(registry: &mut Registry) {
    registry
        .mutation("pointOfSaleDevicePaymentSessionOpen", open)
        .mutation("pointOfSaleDevicePaymentSessionCount", |m, ctx| {
            count(m, ctx, "MID_SESSION")
        })
        .mutation("pointOfSaleDevicePaymentSessionClose", |m, ctx| {
            count(m, ctx, "CLOSING")
        })
        .mutation("pointOfSaleDevicePaymentSessionAdjust", adjust)
        .field("CashDrawer", "cashActivities", drawer_activities)
        .field("CashDrawer", "netSales", |r, ctx, o| {
            drawer_total(r, ctx, o, Total::Sales)
        })
        .field("CashDrawer", "totalSales", |r, ctx, o| {
            drawer_total(r, ctx, o, Total::Sales)
        })
        .field("CashDrawer", "totalRefunds", |r, ctx, o| {
            drawer_total(r, ctx, o, Total::Refunds)
        })
        .field("CashDrawer", "totalAdjustments", |r, ctx, o| {
            drawer_total(r, ctx, o, Total::Adjustments)
        })
        .field("CashDrawer", "totalDiscrepancies", |r, ctx, o| {
            drawer_total(r, ctx, o, Total::Discrepancies)
        })
        .query("cashManagementLocationSummary", location_summary)
        .query("cashManagementShopSummary", shop_summary)
        .query("cashTrackingSession", tracking_session)
        .query("cashTrackingSessions", tracking_sessions)
        .search_field(SESSION, "is_open", |_r, _item, record| {
            Some(vec![SearchValue::Bool(
                text(record, "status") == Some("OPEN"),
            )])
        })
        .search_field(SESSION, "opening_date", |_r, _item, record| {
            date_value(record, "openingTime")
        })
        .search_field(SESSION, "opening_time", |_r, _item, record| {
            date_value(record, "openingTime")
        })
        .search_field(SESSION, "closing_date", |_r, _item, record| {
            date_value(record, "closingTime")
        })
        .search_field(SESSION, "closing_time", |_r, _item, record| {
            date_value(record, "closingTime")
        });
}

/// A date of a record, as a search value.
fn date_value(record: &Record, name: &str) -> Option<Vec<SearchValue>> {
    Some(
        text(record, name)
            .and_then(|date| DateTime::parse_from_rfc3339(date).ok())
            .map(|date| vec![SearchValue::Date(date.with_timezone(&Utc))])
            .unwrap_or_default(),
    )
}

/// An error of a session mutation, by its code: the code's description is the message.
fn coded(field: &[&str], code: &str) -> UserError {
    let message = match code {
        "ALREADY_OPEN" => {
            "A point of sale device payment session is already open for this point of sale device."
        }
        "CURRENCY_MISMATCH" => "The currency code does not match the cash drawer currency.",
        "NO_CASH_DRAWER_ASSOCIATED" => {
            "Point of sale device has no cash drawer associated with it."
        }
        "PAYMENT_SESSION_NOT_FOUND" => "Point of sale device payment session not found.",
        "POINT_OF_SALE_DEVICE_NOT_FOUND" => "The point of sale device was not found.",
        "REASON_CODE_NOT_FOUND" => "Reason code not found.",
        "SESSION_ALREADY_CLOSED" => "Session is already closed.",
        "STAFF_MEMBER_NOT_FOUND" => "Staff member not found.",
        "TIME_BEFORE_SESSION_OPEN" => "Time cannot be before session opening time.",
        _ => "Unexpected internal error happened.",
    };
    UserError::new(field, message).code(code)
}

/// The time of an argument (`time`), in UTC as the records write dates, else now.
fn time_arg(mutation: &Mutation<'_>, ctx: &FieldContext<'_>) -> String {
    ctx.arg_str("time")
        .and_then(|time| DateTime::parse_from_rfc3339(time).ok())
        .map(|time| value::date_time(time.with_timezone(&Utc)))
        .unwrap_or_else(|| mutation.timestamp())
}

/// The checks every session mutation shares: the staff member, the reason code, the currency of
/// an amount. `amount` is the argument holding a `MoneyInput`.
fn check_common(
    state: &ShopState,
    ctx: &FieldContext<'_>,
    amount: &str,
    errors: &mut Vec<UserError>,
) {
    let staff = ctx.arg_str("staffMemberId").unwrap_or_default();
    if state.type_of(staff) != Some("StaffMember") {
        errors.push(coded(&["staffMemberId"], "STAFF_MEMBER_NOT_FOUND"));
    }
    if let Some(code) = ctx.arg_str("reasonCodeId")
        && !is_reason_code(state, code)
    {
        errors.push(coded(&["reasonCodeId"], "REASON_CODE_NOT_FOUND"));
    }
    if let Some(given) = ctx
        .arg(amount)
        .and_then(|money| money.get("currencyCode"))
        .and_then(Value::as_str)
        && given != currency(state)
    {
        errors.push(coded(&[amount], "CURRENCY_MISMATCH"));
    }
}

/// A drawer's balance, in cents.
fn balance_of(state: &ShopState, drawer: &str) -> i64 {
    cents(state.get(drawer).and_then(|record| record.get("balance")))
}

/// Sets a drawer's balance, in cents.
fn set_balance(state: &mut ShopState, drawer: &str, balance: i64) {
    let money = money(state, balance);
    if let Some(record) = state.get_mut(drawer) {
        record.insert("balance".into(), money);
    }
    state.emit("cash_drawers/update", drawer);
}

/// Adds an activity to a session.
fn record_activity(state: &mut ShopState, session: &str, activity: Value) {
    if let Some(Value::Array(activities)) = state
        .get_mut(session)
        .and_then(|record| record.get_mut("cashActivities"))
    {
        activities.push(activity);
    }
}

/// Adds cents to a `MoneyV2` field of a session.
fn add_to(state: &mut ShopState, session: &str, name: &str, amount: i64) {
    let current = cents(state.get(session).and_then(|record| record.get(name)));
    let money = money(state, current + amount);
    if let Some(record) = state.get_mut(session) {
        record.insert(name.into(), money);
    }
}

/// A count activity.
fn count_activity(
    state: &ShopState,
    ctx: &FieldContext<'_>,
    session: &str,
    kind: &str,
    counted: i64,
    expected: i64,
    time: &str,
) -> Value {
    json!({
        "__typename": "CashCountActivity",
        "type": kind,
        "cash": money(state, counted - expected),
        "cashCounted": money(state, counted),
        "cashExpected": money(state, expected),
        "cashDiscrepancy": money(state, counted - expected),
        "note": ctx.arg_str("note"),
        "reasonCode": ctx.arg_str("reasonCodeId"),
        "staffMember": ctx.arg_str("staffMemberId"),
        "paymentSession": session,
        "time": time,
    })
}

/// `pointOfSaleDevicePaymentSessionOpen`.
fn open(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"pointOfSaleDevicePaymentSession": null});
    let state = mutation.state();
    let device = ctx
        .arg_str("pointOfSaleDeviceId")
        .unwrap_or_default()
        .to_string();
    let Some(device_record) = state
        .get(&device)
        .filter(|_| state.type_of(&device) == Some("PointOfSaleDevice"))
    else {
        let error = coded(&["pointOfSaleDeviceId"], "POINT_OF_SALE_DEVICE_NOT_FOUND");
        return Ok(payload(empty, &[error]));
    };
    let Some(drawer) = text(device_record, "cashDrawer").map(str::to_string) else {
        let error = coded(&["pointOfSaleDeviceId"], "NO_CASH_DRAWER_ASSOCIATED");
        return Ok(payload(empty, &[error]));
    };
    if text(device_record, "activePaymentSession").is_some() {
        return Ok(payload(
            empty,
            &[coded(&["pointOfSaleDeviceId"], "ALREADY_OPEN")],
        ));
    }
    let mut errors = Vec::new();
    check_common(state, ctx, "balance", &mut errors);
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let time = time_arg(mutation, ctx);
    let state = mutation.state();
    let expected = balance_of(state, &drawer);
    let counted = ctx
        .arg("balance")
        .map_or(expected, |money| cents(Some(money)));
    let location = state
        .get(&drawer)
        .and_then(|record| text(record, "location"))
        .map(str::to_string);
    let state = mutation.state_mut();
    let session = state.new_gid(SESSION);
    let zero = money(state, 0);
    state.insert(value::object(json!({
        "__typename": SESSION,
        "id": session,
        "status": "OPEN",
        "currency": currency(state),
        "cashDrawer": drawer,
        "location": location,
        "pointOfSaleDevice": device,
        "openingTime": time,
        "openingStaffMember": ctx.arg_str("staffMemberId"),
        "openingNote": ctx.arg_str("note"),
        "cashCountedAtOpen": money(state, counted),
        "expectedCashAtOpen": money(state, expected),
        "closingTime": null,
        "closingStaffMember": null,
        "closingNote": null,
        "cashCountedAtClose": null,
        "expectedCashAtClose": null,
        "closingBalance": null,
        "closingAdjustment": null,
        "netCashSales": zero,
        "netSales": zero,
        "totalAdjustments": zero,
        "totalCashRefunds": zero,
        "totalCashSales": zero,
        "totalDiscrepancy": money(state, counted - expected),
        "totalRefunds": zero,
        "totalSales": zero,
        "totalsReady": true,
        "cashActivities": [],
    })));
    let activity = count_activity(state, ctx, &session, "OPENING", counted, expected, &time);
    record_activity(state, &session, activity);
    if counted != expected {
        set_balance(state, &drawer, counted);
    }
    if let Some(record) = state.get_mut(&device) {
        record.insert("activePaymentSession".into(), json!(session));
    }
    state.emit("point_of_sale_device_payment_sessions/create", &session);
    let note = tell_device(
        state,
        &device,
        "payment_session_opened",
        &session,
        format!("payment session {session} opened"),
    );
    mutation.note(format!("pointOfSaleDevicePaymentSessionOpen: {note}"));
    Ok(payload(
        json!({"pointOfSaleDevicePaymentSession": session}),
        &[],
    ))
}

/// The open session an argument names, or the error that refuses it.
fn open_session(
    state: &ShopState,
    ctx: &FieldContext<'_>,
    time: &str,
) -> Result<(String, String), UserError> {
    let session = ctx
        .arg_str("pointOfSaleDevicePaymentSessionId")
        .unwrap_or_default()
        .to_string();
    let field = ["pointOfSaleDevicePaymentSessionId"];
    let Some(record) = state
        .get(&session)
        .filter(|_| state.type_of(&session) == Some(SESSION))
    else {
        return Err(coded(&field, "PAYMENT_SESSION_NOT_FOUND"));
    };
    if text(record, "status") != Some("OPEN") {
        return Err(coded(&field, "SESSION_ALREADY_CLOSED"));
    }
    let Some(drawer) = text(record, "cashDrawer").filter(|drawer| state.contains(drawer)) else {
        return Err(coded(&field, "NO_CASH_DRAWER_ASSOCIATED"));
    };
    if text(record, "openingTime").is_some_and(|opening| time < opening) {
        return Err(coded(&["time"], "TIME_BEFORE_SESSION_OPEN"));
    }
    Ok((session, drawer.to_string()))
}

/// `pointOfSaleDevicePaymentSessionCount` (`MID_SESSION`) and `…Close` (`CLOSING`).
fn count(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    kind: &str,
) -> Result<Value, FieldError> {
    let empty = json!({"pointOfSaleDevicePaymentSession": null});
    let time = time_arg(mutation, ctx);
    let state = mutation.state();
    let (session, drawer) = match open_session(state, ctx, &time) {
        Ok(found) => found,
        Err(error) => return Ok(payload(empty, &[error])),
    };
    let mut errors = Vec::new();
    check_common(state, ctx, "balance", &mut errors);
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let expected = balance_of(state, &drawer);
    let counted = cents(ctx.arg("balance"));
    let state = mutation.state_mut();
    let activity = count_activity(state, ctx, &session, kind, counted, expected, &time);
    record_activity(state, &session, activity);
    add_to(state, &session, "totalDiscrepancy", counted - expected);
    if counted != expected {
        set_balance(state, &drawer, counted);
    }
    if kind == "CLOSING" {
        let device = state
            .get(&session)
            .and_then(|record| text(record, "pointOfSaleDevice"))
            .map(str::to_string);
        let (counted_money, expected_money, discrepancy) = (
            money(state, counted),
            money(state, expected),
            money(state, counted - expected),
        );
        if let Some(record) = state.get_mut(&session) {
            for (name, value) in [
                ("status", json!("CLOSED")),
                ("closingTime", json!(time)),
                ("closingStaffMember", json!(ctx.arg_str("staffMemberId"))),
                ("closingNote", json!(ctx.arg_str("note"))),
                ("cashCountedAtClose", counted_money.clone()),
                ("expectedCashAtClose", expected_money),
                ("closingBalance", counted_money),
                ("closingAdjustment", discrepancy),
            ] {
                record.insert(name.into(), value);
            }
        }
        if let Some(device) = device {
            if let Some(record) = state.get_mut(&device) {
                record.insert("activePaymentSession".into(), Value::Null);
            }
            let note = tell_device(
                state,
                &device,
                "payment_session_closed",
                &session,
                format!("payment session {session} closed"),
            );
            mutation.note(format!("pointOfSaleDevicePaymentSessionClose: {note}"));
        }
    }
    mutation
        .state_mut()
        .emit("point_of_sale_device_payment_sessions/update", &session);
    Ok(payload(
        json!({"pointOfSaleDevicePaymentSession": session}),
        &[],
    ))
}

/// `pointOfSaleDevicePaymentSessionAdjust`: its errors are `UserError`s, which have no code.
fn adjust(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let empty = json!({"pointOfSaleDevicePaymentSession": null});
    let time = time_arg(mutation, ctx);
    let state = mutation.state();
    let (session, drawer) = match open_session(state, ctx, &time) {
        Ok(found) => found,
        Err(error) => return Ok(payload(empty, &[error])),
    };
    let mut errors = Vec::new();
    check_common(state, ctx, "cash", &mut errors);
    if !errors.is_empty() {
        return Ok(payload(empty, &errors));
    }
    let cash = cents(ctx.arg("cash"));
    let balance = balance_of(state, &drawer);
    let state = mutation.state_mut();
    let activity = json!({
        "__typename": "CashAdjustmentActivity",
        "type": "ADJUSTMENT",
        "cash": money(state, cash),
        "giftCard": null,
        "note": ctx.arg_str("note"),
        "reasonCode": ctx.arg_str("reasonCodeId"),
        "staffMember": ctx.arg_str("staffMemberId"),
        "paymentSession": session,
        "time": time,
    });
    record_activity(state, &session, activity);
    add_to(state, &session, "totalAdjustments", cash);
    set_balance(state, &drawer, balance + cash);
    state.emit("point_of_sale_device_payment_sessions/update", &session);
    Ok(payload(
        json!({"pointOfSaleDevicePaymentSession": session}),
        &[],
    ))
}

// --- Reading ---

/// The sessions of a drawer.
fn sessions_of(state: &ShopState, drawer: &str) -> Vec<String> {
    state
        .keys(SESSION)
        .filter(|session| {
            state
                .get(session)
                .and_then(|record| text(record, "cashDrawer"))
                == Some(drawer)
        })
        .map(str::to_string)
        .collect()
}

/// The activities of sessions.
fn activities_of(state: &ShopState, sessions: &[String]) -> Vec<Value> {
    sessions
        .iter()
        .filter_map(|session| state.get(session))
        .flat_map(|record| {
            record
                .get("cashActivities")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .collect()
}

/// `CashDrawer.cashActivities`: the activities of its sessions.
fn drawer_activities(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let mut items = activities_of(state, &sessions_of(state, object.key().unwrap_or_default()));
    if let Some(staff) = ctx.arg_str("staffMemberId") {
        items.retain(|activity| activity.get("staffMember").and_then(Value::as_str) == Some(staff));
    }
    r.connection(ctx, items)
}

/// A total of a drawer's activities.
#[derive(Clone, Copy)]
enum Total {
    Sales,
    Refunds,
    Adjustments,
    Discrepancies,
}

/// Whether a date is in a `CashDrawerDateRangeInput`.
fn in_range(time: Option<&str>, range: Option<&Value>) -> bool {
    let (Some(time), Some(range)) = (time, range) else {
        return time.is_some();
    };
    let at = |name: &str| range.get(name).and_then(Value::as_str);
    let parse = |date: &str| {
        DateTime::parse_from_rfc3339(date)
            .ok()
            .map(|d| d.with_timezone(&Utc))
    };
    let Some(time) = parse(time) else {
        return false;
    };
    at("from").and_then(parse).is_none_or(|from| time >= from)
        && at("to").and_then(parse).is_none_or(|to| time <= to)
}

/// The sum of a kind of activity, in cents.
fn total_of(activities: &[Value], total: Total, range: Option<&Value>) -> i64 {
    activities
        .iter()
        .filter(|activity| in_range(activity.get("time").and_then(Value::as_str), range))
        .map(|activity| {
            let kind = activity.get("__typename").and_then(Value::as_str);
            let amount = cents(activity.get("cash"));
            match (total, kind) {
                (Total::Adjustments, Some("CashAdjustmentActivity")) => amount,
                (Total::Discrepancies, Some("CashCountActivity")) => amount,
                // lsf has no POS sales: no `CashTransactionActivity`.
                (Total::Sales | Total::Refunds, Some("CashTransactionActivity")) => amount,
                _ => 0,
            }
        })
        .sum()
}

/// `CashDrawer.totalSales(dateRange:)`, `netSales`, `totalRefunds`, `totalAdjustments`,
/// `totalDiscrepancies`.
fn drawer_total(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
    total: Total,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let activities = activities_of(state, &sessions_of(state, object.key().unwrap_or_default()));
    let amount = total_of(&activities, total, ctx.arg("dateRange"));
    r.convert(ctx, &money(state, amount), false)
}

/// The start and the end of a period of `Date`s: from the start of the first day to the end of
/// the last, days of the shop's time zone (`search::Options`).
fn period(
    state: &ShopState,
    ctx: &FieldContext<'_>,
) -> (Option<DateTime<Utc>>, Option<DateTime<Utc>>) {
    let zone = crate::search::Options::of(state);
    let day = |name: &str, end: bool| {
        let date = NaiveDate::parse_from_str(ctx.arg_str(name)?, "%Y-%m-%d").ok()?;
        Some(if end {
            zone.day_end(date)
        } else {
            zone.day_start(date)
        })
    };
    (day("startDate", false), day("endDate", true))
}

/// The summary of the sessions of some drawers over a period.
fn summary(
    state: &ShopState,
    drawers: &[String],
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
) -> Value {
    let parse = |date: Option<&str>| {
        date.and_then(|date| DateTime::parse_from_rfc3339(date).ok())
            .map(|date| date.with_timezone(&Utc))
    };
    let within = |date: Option<DateTime<Utc>>| {
        date.is_some_and(|date| start.is_none_or(|s| date >= s) && end.is_none_or(|e| date <= e))
    };
    let mut opened = 0;
    let mut closed = 0;
    let mut net = 0;
    let mut discrepancies = 0;
    let mut at_start = 0;
    let mut at_end = 0;
    for drawer in drawers {
        let sessions = sessions_of(state, drawer);
        for session in sessions.iter().filter_map(|session| state.get(session)) {
            opened += i64::from(within(parse(text(session, "openingTime"))));
            closed += i64::from(within(parse(text(session, "closingTime"))));
        }
        let activities = activities_of(state, &sessions);
        let balance = balance_of(state, drawer);
        // The balance at a time: the drawer's balance less what the activities after it moved
        // (before the first activity, none: the drawer started at 0).
        let moved_after = |time: Option<DateTime<Utc>>| -> i64 {
            activities
                .iter()
                .filter(|activity| {
                    let at = parse(activity.get("time").and_then(Value::as_str));
                    time.is_none_or(|time| at.is_some_and(|at| at > time))
                })
                .map(|activity| cents(activity.get("cash")))
                .sum()
        };
        at_start += balance - moved_after(start);
        at_end += balance - end.map_or(0, |end| moved_after(Some(end)));
        for activity in &activities {
            if !within(parse(activity.get("time").and_then(Value::as_str))) {
                continue;
            }
            let amount = cents(activity.get("cash"));
            match activity.get("__typename").and_then(Value::as_str) {
                Some("CashCountActivity") => discrepancies += amount,
                Some("CashAdjustmentActivity" | "CashTransactionActivity") => net += amount,
                _ => {}
            }
        }
    }
    json!({
        "cashBalanceAtStart": money(state, at_start),
        "cashBalanceAtEnd": money(state, at_end),
        "netCash": money(state, net),
        "sessionsOpened": opened,
        "sessionsClosed": closed,
        "totalDiscrepancies": money(state, discrepancies),
    })
}

/// `cashManagementLocationSummary(locationId:, startDate:, endDate:)`.
fn location_summary(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let location = ctx.arg_str("locationId").unwrap_or_default();
    let drawers: Vec<String> = state
        .keys("CashDrawer")
        .filter(|drawer| {
            state
                .get(drawer)
                .and_then(|record| text(record, "location"))
                == Some(location)
        })
        .map(str::to_string)
        .collect();
    let (start, end) = period(state, ctx);
    r.convert(ctx, &summary(state, &drawers, start, end), false)
}

/// `cashManagementShopSummary(currencyCode:, startDate:, endDate:)`: every drawer is in the
/// shop's currency.
fn shop_summary(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let drawers: Vec<String> = if ctx.arg_str("currencyCode") == Some(currency(state).as_str()) {
        state.keys("CashDrawer").map(str::to_string).collect()
    } else {
        Vec::new()
    };
    let (start, end) = period(state, ctx);
    r.convert(ctx, &summary(state, &drawers, start, end), false)
}

/// A session as the deprecated `CashTrackingSession` reads it: `gid://shopify/CashTrackingSession/<n>`
/// of the session's number.
fn tracking_of(state: &ShopState, session: &str) -> Option<Value> {
    let record = state.get(session)?;
    let number = parse_gid(session)?.1;
    let adjustments: Vec<Value> = record
        .get("cashActivities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|activity| {
            activity.get("__typename").and_then(Value::as_str) == Some("CashAdjustmentActivity")
        })
        .enumerate()
        .map(|(index, activity)| {
            json!({
                "id": gid("CashTrackingAdjustment", number * 1000 + index as u64 + 1),
                "cash": activity.get("cash"),
                "note": activity.get("note"),
                "staffMember": activity.get("staffMember"),
                "time": activity.get("time"),
            })
        })
        .collect();
    let drawer = text(record, "cashDrawer").unwrap_or_default();
    let open = text(record, "status") == Some("OPEN");
    let expected = if open {
        money(state, balance_of(state, drawer))
    } else {
        record
            .get("expectedCashAtClose")
            .cloned()
            .unwrap_or(Value::Null)
    };
    let register = text(record, "pointOfSaleDevice")
        .and_then(|device| state.get(device))
        .and_then(|_| state.get(drawer))
        .and_then(|drawer| text(drawer, "name"))
        .unwrap_or_default();
    Some(json!({
        "__typename": "CashTrackingSession",
        "id": gid("CashTrackingSession", number),
        "adjustments": adjustments,
        "cashTrackingEnabled": true,
        "cashTransactions": [],
        "closingBalance": record.get("closingBalance"),
        "closingNote": record.get("closingNote"),
        "closingStaffMember": record.get("closingStaffMember"),
        "closingTime": record.get("closingTime"),
        "expectedBalance": expected,
        "expectedClosingBalance": record.get("expectedCashAtClose"),
        "expectedOpeningBalance": record.get("expectedCashAtOpen"),
        "location": record.get("location"),
        "netCashSales": record.get("netCashSales"),
        "openingBalance": record.get("cashCountedAtOpen"),
        "openingNote": record.get("openingNote"),
        "openingStaffMember": record.get("openingStaffMember"),
        "openingTime": record.get("openingTime"),
        "registerName": register,
        "totalAdjustments": record.get("totalAdjustments"),
        "totalCashRefunds": record.get("totalCashRefunds"),
        "totalCashSales": record.get("totalCashSales"),
        "totalDiscrepancy": record.get("totalDiscrepancy"),
        "lsfStatus": record.get("status"),
        "lsfDevice": record.get("pointOfSaleDevice"),
    }))
}

/// `cashTrackingSession(id:)`.
fn tracking_session(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let Some((_, number)) = ctx.arg_str("id").and_then(parse_gid) else {
        return Ok(Output::Null);
    };
    match tracking_of(state, &gid(SESSION, number)) {
        Some(session) => r.convert(ctx, &session, true),
        None => Ok(Output::Null),
    }
}

/// `cashTrackingSessions(query:, sortKey:)`: `status:open`, `location_id:`, the times.
fn tracking_sessions(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let mut items: Vec<Value> = state
        .keys(SESSION)
        .filter_map(|session| tracking_of(state, session))
        .collect();
    let sort = ctx.arg_str("sortKey").unwrap_or("ID");
    let key = |item: &Value, name: &str| {
        item.get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    match sort {
        "OPENING_TIME_ASC" => items.sort_by_key(|item| key(item, "openingTime")),
        "OPENING_TIME_DESC" => {
            items.sort_by_key(|item| std::cmp::Reverse(key(item, "openingTime")))
        }
        "CLOSING_TIME_ASC" => items.sort_by_key(|item| key(item, "closingTime")),
        "CLOSING_TIME_DESC" => {
            items.sort_by_key(|item| std::cmp::Reverse(key(item, "closingTime")))
        }
        "TOTAL_DISCREPANCY_ASC" => items.sort_by_key(|item| cents(item.get("totalDiscrepancy"))),
        "TOTAL_DISCREPANCY_DESC" => {
            items.sort_by_key(|item| std::cmp::Reverse(cents(item.get("totalDiscrepancy"))))
        }
        _ => {}
    }
    if let Some(query) = ctx.arg_str("query") {
        for term in query.split_whitespace() {
            if let Some(status) = term.strip_prefix("status:") {
                items.retain(|item| key(item, "lsfStatus").eq_ignore_ascii_case(status));
            } else if let Some(location) = term.strip_prefix("location_id:") {
                items.retain(|item| {
                    item.get("location")
                        .and_then(Value::as_str)
                        .and_then(parse_gid)
                        .is_some_and(|(_, id)| {
                            id.to_string() == location || item["location"] == json!(location)
                        })
                });
            }
        }
    }
    r.connection(ctx, items)
}
