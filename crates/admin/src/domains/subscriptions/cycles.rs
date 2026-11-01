//! Billing cycles: computed from a contract's billing policy and the shop's clock, as
//! shopify.dev's "About subscription billing cycles" describes them, and the mutations that edit
//! their schedule: `subscriptionBillingCycleSkip`, `subscriptionBillingCycleUnskip`,
//! `subscriptionBillingCycleScheduleEdit`, `subscriptionBillingCycleEditDelete`,
//! `subscriptionBillingCycleEditsDelete`; the queries `subscriptionBillingCycle` and
//! `subscriptionBillingCycles`.
//!
//! - **Indexes** start at 1 with the contract's creation and never reset: cycle 1 starts when
//!   the contract is created, "the next billing cycle starts on the day following the previous
//!   cycle's end date", and each cycle is billed at its end ("Billing occurs at the end of each
//!   cycle for the upcoming delivery period"): `billingAttemptExpectedDate` is `cycleEndAt`
//!   unless a schedule edit moved it (the reference's examples: a cycle selected by a date it
//!   holds, its expected date its end).
//! - **Without anchors** a cycle is one interval long from the contract's creation (a month
//!   from Jan 31 ends on the last day of February, then on Mar 31). **With anchors** (the
//!   first one: lsf does not split a cycle at several anchors) the first cycle ends at the first
//!   anchor date after the creation, and every cycle after it at the same anchor, at midnight in
//!   the shop's time zone (the webhook examples' `cycle_start_at: 2022-10-01T00:00:00-04:00`).
//!   Shopify does not document the time of day of either; these are lsf's.
//! - **`maxCycles`** ends the cycles; cycles more than a year ahead of the shop's clock are past
//!   the limit ("You can make edits only to the current billing cycle and any billing cycle
//!   that's up to 1 year after the current billing cycle"): `UPCOMING_CYCLE_LIMIT_EXCEEDED`.
//! - **A change of the billing policy** keeps the past cycles and the start of the current one:
//!   its end follows the new interval from its start, or, when that is already past, one new
//!   interval from now ("This prevents billing from being triggered solely by the policy
//!   change"); the cycles after it follow the new cadence. The contract keeps the segments of
//!   its schedule under `lsfSchedule` (`fromIndex`, `start`, `firstEnd`, the interval, the
//!   anchors, the day of the month the months step to).
//! - **Edits** are kept per cycle under `lsfCycleEdits` (`skipped`, `billingAttemptExpectedDate`,
//!   `contract`: an edited contract, `concatenated`): a cycle with one is `edited`; deleting the
//!   edits of a cycle puts it back to its contract. A cycle is `BILLED` once one of its billing
//!   attempts succeeded.
//!
//! Events, with a payload built from the cycle (its key, `gid://shopify/SubscriptionBillingCycle
//! /<contract number>?index=<i>`, names no record): `subscription_billing_cycles/skip` and
//! `/unskip`, `subscription_billing_cycle_edits/create` (the first edit of a cycle's schedule or
//! contract), `/update` (another), `/delete`.

use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde_json::{Map, Value, json};

use super::contracts::{contract_arg, is_terminated};
use super::model::{self, Pointer, cycle_error, date_time, object, text};
use crate::domains::catalog::gids;
use crate::graphql::{FieldContext, FieldError, Output};
use crate::resolve::{Mutation, Obj, Registry, ShopResolver, UserError, payload};
use crate::state::{Record, ShopState, value};

pub fn register(registry: &mut Registry) {
    registry
        .query("subscriptionBillingCycle", billing_cycle)
        .query("subscriptionBillingCycles", billing_cycles)
        .field(
            "SubscriptionBillingCycleEditedContract",
            "billingCycles",
            edited_contract_cycles,
        )
        .field(
            "SubscriptionBillingCycleEditedContract",
            "orders",
            edited_contract_orders,
        )
        .mutation("subscriptionBillingCycleSkip", |m, ctx| skip(m, ctx, true))
        .mutation("subscriptionBillingCycleUnskip", |m, ctx| {
            skip(m, ctx, false)
        })
        .mutation("subscriptionBillingCycleScheduleEdit", schedule_edit)
        .mutation("subscriptionBillingCycleEditDelete", edit_delete)
        .mutation("subscriptionBillingCycleEditsDelete", edits_delete);
}

/// How far ahead cycles can be selected: a year.
const UPCOMING_LIMIT_DAYS: i64 = 366;

/// The most cycles lsf walks to find one (daily cycles over more than a year).
const MAX_WALK: i64 = 2000;

/// The last year a cycle reaches, in UTC: a cycle that would start or end after it is out of range
/// (`CYCLE_INDEX_OUT_OF_RANGE`), whatever its index or its policy's `intervalCount`. Shopify
/// publishes no bound; lsf's keeps the date arithmetic within what a date holds.
const LAST_YEAR: i32 = 9999;

/// One billing cycle of a contract.
#[derive(Clone, Debug, PartialEq)]
pub struct Cycle {
    pub index: i64,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// Why a cycle cannot be selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Miss {
    /// Neither or both of `index` and `date`.
    OneOf,
    /// An index below 1 or past `maxCycles`.
    IndexOutOfRange,
    /// A date before the contract's first cycle, or past its last.
    DateOutOfRange,
    /// More than a year ahead.
    Upcoming,
}

impl Miss {
    /// The code of the error, as the error types of the cycles and of billing attempts name it.
    pub fn code(self) -> &'static str {
        match self {
            Miss::OneOf => "INVALID",
            Miss::IndexOutOfRange => "CYCLE_INDEX_OUT_OF_RANGE",
            Miss::DateOutOfRange => "CYCLE_START_DATE_OUT_OF_RANGE",
            Miss::Upcoming => "UPCOMING_CYCLE_LIMIT_EXCEEDED",
        }
    }

    /// The code of the error in `SubscriptionDraftErrorCode`.
    pub fn draft_code(self) -> &'static str {
        match self {
            Miss::OneOf => "CYCLE_SELECTOR_VALIDATE_ONE_OF",
            other => other.code(),
        }
    }
}

// ---- time -----------------------------------------------------------------------------------

/// The shop's time zone (`Shop.ianaTimezone`), UTC when it has none.
pub fn zone(state: &ShopState) -> Tz {
    state
        .shop()
        .and_then(|shop| text(shop, "ianaTimezone"))
        .and_then(|zone| zone.parse::<Tz>().ok())
        .unwrap_or(Tz::UTC)
}

/// A local date and time of the zone as an instant (the first of an ambiguous time, an hour
/// later for one a change of offset skips).
fn instant(zone: Tz, local: NaiveDateTime) -> DateTime<Utc> {
    zone.from_local_datetime(&local)
        .earliest()
        .or_else(|| {
            zone.from_local_datetime(&local.checked_add_signed(Duration::hours(1))?)
                .earliest()
        })
        .map_or_else(
            || Utc.from_utc_datetime(&local),
            |at| at.with_timezone(&Utc),
        )
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .and_then(|first| first.pred_opt())
        .map_or(28, |last| last.day())
}

/// The date `months` months after `date`, on `day` (or the last day of a shorter month);
/// `None` before year 1 or after [`LAST_YEAR`].
fn add_months(date: NaiveDate, months: i64, day: u32) -> Option<NaiveDate> {
    let total = (i64::from(date.year()) * 12 + i64::from(date.month0())).checked_add(months)?;
    let year = i32::try_from(total.div_euclid(12))
        .ok()
        .filter(|year| (1..=LAST_YEAR).contains(year))?;
    let month = u32::try_from(total.rem_euclid(12)).unwrap_or(0) + 1;
    let day = day.clamp(1, days_in_month(year, month));
    NaiveDate::from_ymd_opt(year, month, day)
}

/// A segment of a schedule: the cycles from `fromIndex` on, the first starting at `start` and
/// ending at `firstEnd`, each next one an interval later.
#[derive(Clone, Debug)]
struct Segment {
    from_index: i64,
    start: DateTime<Utc>,
    first_end: DateTime<Utc>,
    interval: String,
    count: i64,
    /// The day of the month (or of the week, for weeks) the months step to.
    day: u32,
    anchored: bool,
}

impl Segment {
    fn of(value: &Value) -> Option<Segment> {
        Some(Segment {
            from_index: value.get("fromIndex")?.as_i64()?,
            start: date_time(value.get("start")?.as_str()?)?,
            first_end: date_time(value.get("firstEnd")?.as_str()?)?,
            interval: value.get("interval")?.as_str()?.to_string(),
            count: value
                .get("count")
                .and_then(Value::as_i64)
                .unwrap_or(1)
                .max(1),
            day: value
                .get("day")
                .and_then(Value::as_u64)
                .and_then(|day| u32::try_from(day).ok())
                .unwrap_or(1),
            anchored: value
                .get("anchored")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    fn to_value(&self) -> Value {
        json!({
            "fromIndex": self.from_index,
            "start": value::date_time(self.start),
            "firstEnd": value::date_time(self.first_end),
            "interval": self.interval,
            "count": self.count,
            "day": self.day,
            "anchored": self.anchored,
        })
    }

    /// The boundary `k` of the segment: its start for 0, its first end for 1, then an interval
    /// at a time; `None` past [`LAST_YEAR`].
    fn boundary(&self, zone: Tz, k: i64) -> Option<DateTime<Utc>> {
        match k {
            0 => Some(self.start),
            1 => Some(self.first_end),
            _ => step(
                zone,
                self.first_end,
                &self.interval,
                self.count.checked_mul(k - 1)?,
                self.day,
            ),
        }
    }
}

/// `at` moved by `units` of `interval` in the shop's time zone, the months to `day`; `None`
/// past [`LAST_YEAR`].
fn step(
    zone: Tz,
    at: DateTime<Utc>,
    interval: &str,
    units: i64,
    day: u32,
) -> Option<DateTime<Utc>> {
    let local = at.with_timezone(&zone).naive_local();
    let moved = match interval {
        "DAY" => local.checked_add_signed(Duration::try_days(units)?)?,
        "WEEK" => local.checked_add_signed(Duration::try_weeks(units)?)?,
        "YEAR" => add_months(local.date(), units.checked_mul(12)?, day)?.and_time(local.time()),
        _ => add_months(local.date(), units, day)?.and_time(local.time()),
    };
    let at = instant(zone, moved);
    (1..=LAST_YEAR).contains(&at.year()).then_some(at)
}

/// The first date of an anchor strictly after `after`, at midnight in the shop's zone.
fn next_anchor(zone: Tz, after: DateTime<Utc>, anchor: &Value) -> DateTime<Utc> {
    let local = after.with_timezone(&zone).naive_local();
    let day = anchor.get("day").and_then(Value::as_u64).unwrap_or(1) as u32;
    let midnight = |date: NaiveDate| instant(zone, date.and_time(NaiveTime::MIN));
    let mut candidate = match anchor.get("type").and_then(Value::as_str) {
        Some("WEEKDAY") => {
            let today = local.date().weekday().number_from_monday();
            let ahead = (i64::from(day.clamp(1, 7)) - i64::from(today)).rem_euclid(7);
            midnight(local.date() + Duration::days(ahead))
        }
        Some("YEARDAY") => {
            let month = anchor.get("month").and_then(Value::as_u64).unwrap_or(1) as u32;
            let year = local.date().year();
            let month = month.clamp(1, 12);
            let date =
                NaiveDate::from_ymd_opt(year, month, day.clamp(1, days_in_month(year, month)))
                    .unwrap_or(local.date());
            midnight(date)
        }
        _ => {
            let first = NaiveDate::from_ymd_opt(local.date().year(), local.date().month(), 1)
                .unwrap_or(local.date());
            midnight(add_months(first, 0, day).unwrap_or(first))
        }
    };
    let unit = match anchor.get("type").and_then(Value::as_str) {
        Some("WEEKDAY") => "WEEK",
        Some("YEARDAY") => "YEAR",
        _ => "MONTH",
    };
    let mut guard = 0;
    while candidate <= after && guard < 1000 {
        let Some(next) = step(zone, candidate, unit, 1, day) else {
            break;
        };
        candidate = next;
        guard += 1;
    }
    candidate
}

/// The segment a billing policy starts at `start` for the cycles from `from_index` on: its
/// first end one interval later (or at the next anchor), and not before `not_before`.
fn segment(
    zone: Tz,
    policy: &Value,
    from_index: i64,
    start: DateTime<Utc>,
    not_before: Option<DateTime<Utc>>,
) -> Segment {
    let interval = policy
        .get("interval")
        .and_then(Value::as_str)
        .unwrap_or("MONTH")
        .to_string();
    let count = policy
        .get("intervalCount")
        .and_then(Value::as_i64)
        .unwrap_or(1)
        .max(1);
    let anchor = policy
        .get("anchors")
        .and_then(Value::as_array)
        .and_then(|anchors| anchors.first())
        .cloned();
    let (first_end, day, anchored) = match &anchor {
        Some(anchor) => {
            let mut end = next_anchor(zone, start, anchor);
            if let Some(not_before) = not_before {
                while end <= not_before {
                    end = next_anchor(zone, end, anchor);
                }
            }
            let day = anchor.get("day").and_then(Value::as_u64).unwrap_or(1) as u32;
            (end, day, true)
        }
        None => {
            let day = start.with_timezone(&zone).day();
            // An interval past `LAST_YEAR` ends the cycle on the last midnight lsf computes in
            // every zone (December 30, so that it is in that year in UTC too).
            let last = || {
                NaiveDate::from_ymd_opt(LAST_YEAR, 12, 30)
                    .map_or(start, |date| instant(zone, date.and_time(NaiveTime::MIN)))
            };
            let mut end = step(zone, start, &interval, count, day).unwrap_or_else(last);
            if let Some(not_before) = not_before
                && end <= not_before
            {
                let day = not_before.with_timezone(&zone).day();
                return Segment {
                    from_index,
                    start,
                    first_end: step(zone, not_before, &interval, count, day).unwrap_or_else(last),
                    interval,
                    count,
                    day,
                    anchored: false,
                };
            }
            if end <= start {
                end = start + Duration::days(1);
            }
            (end, day, false)
        }
    };
    Segment {
        from_index,
        start,
        first_end,
        interval,
        count,
        day,
        anchored,
    }
}

/// The schedule of a new contract created at `created` with a billing policy.
pub fn new_schedule(state: &ShopState, policy: &Value, created: DateTime<Utc>) -> Value {
    json!([segment(zone(state), policy, 1, created, None).to_value()])
}

/// Follows a change of a contract's billing policy: the current cycle keeps its start and
/// ends one new interval later (not before now), the next ones follow the new cadence.
pub fn reschedule(state: &ShopState, contract: &mut Record, policy: &Value) {
    let now = state.now();
    let zone = zone(state);
    let current = current(state, contract);
    let mut segments = segments(contract);
    segments.retain(|segment| segment.from_index < current.index);
    segments.push(segment(
        zone,
        policy,
        current.index,
        current.start,
        Some(now),
    ));
    contract.insert(
        "lsfSchedule".into(),
        Value::Array(segments.iter().map(Segment::to_value).collect()),
    );
}

fn segments(contract: &Record) -> Vec<Segment> {
    let mut segments: Vec<Segment> = contract
        .get("lsfSchedule")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Segment::of)
        .collect();
    segments.sort_by_key(|segment| segment.from_index);
    segments
}

/// The schedule of a contract, made from its creation when it has none (a contract of the
/// store data's checkout before it was kept).
fn schedule(state: &ShopState, contract: &Record) -> Vec<Segment> {
    let segments = segments(contract);
    if !segments.is_empty() {
        return segments;
    }
    let created = text(contract, "createdAt")
        .and_then(date_time)
        .unwrap_or_else(|| state.now());
    let policy = contract
        .get("billingPolicy")
        .cloned()
        .unwrap_or(Value::Null);
    vec![segment(zone(state), &policy, 1, created, None)]
}

/// The last index a contract has: `maxCycles`, else none.
fn last_index(contract: &Record) -> Option<i64> {
    contract
        .pointer("/billingPolicy/maxCycles")
        .and_then(Value::as_i64)
        .filter(|max| *max > 0)
}

/// The cycle of an index, `None` below 1, past `maxCycles`, or past [`LAST_YEAR`].
pub fn cycle(state: &ShopState, contract: &Record, index: i64) -> Option<Cycle> {
    if index < 1 || last_index(contract).is_some_and(|last| index > last) {
        return None;
    }
    let zone = zone(state);
    let segments = schedule(state, contract);
    let segment = segments
        .iter()
        .rev()
        .find(|segment| segment.from_index <= index)
        .or_else(|| segments.first())?;
    let k = index - segment.from_index;
    Some(Cycle {
        index,
        start: segment.boundary(zone, k)?,
        end: segment.boundary(zone, k.checked_add(1)?)?,
    })
}

/// The cycle a date falls in (`cycleStartAt` ≤ date < `cycleEndAt`).
pub fn cycle_at(state: &ShopState, contract: &Record, date: DateTime<Utc>) -> Result<Cycle, Miss> {
    let first = cycle(state, contract, 1).ok_or(Miss::IndexOutOfRange)?;
    if date < first.start {
        return Err(Miss::DateOutOfRange);
    }
    if date > state.now() + Duration::days(UPCOMING_LIMIT_DAYS) {
        return Err(Miss::Upcoming);
    }
    let mut index = 1;
    while index <= MAX_WALK {
        let Some(cycle) = cycle(state, contract, index) else {
            return Err(Miss::DateOutOfRange);
        };
        if date < cycle.end {
            return Ok(cycle);
        }
        index += 1;
    }
    Err(Miss::Upcoming)
}

/// The cycle a `SubscriptionBillingCycleSelector` (`index` or `date`) selects.
pub fn select(
    state: &ShopState,
    contract: &Record,
    selector: &Map<String, Value>,
) -> Result<Cycle, Miss> {
    let index = selector.get("index").filter(|value| !value.is_null());
    let date = selector.get("date").filter(|value| !value.is_null());
    match (index, date) {
        (Some(index), None) => {
            let index = index.as_i64().unwrap_or(0);
            let cycle = cycle(state, contract, index).ok_or(Miss::IndexOutOfRange)?;
            if cycle.start > state.now() + Duration::days(UPCOMING_LIMIT_DAYS) {
                return Err(Miss::Upcoming);
            }
            Ok(cycle)
        }
        (None, Some(date)) => {
            let date = date
                .as_str()
                .and_then(date_time)
                .ok_or(Miss::DateOutOfRange)?;
            cycle_at(state, contract, date)
        }
        _ => Err(Miss::OneOf),
    }
}

/// The cycle now falls in (the first before the contract starts, the last past `maxCycles`).
pub fn current(state: &ShopState, contract: &Record) -> Cycle {
    let now = state.now();
    match cycle_at(state, contract, now) {
        Ok(cycle) => cycle,
        Err(_) => {
            let last = last_index(contract).unwrap_or(1);
            let first = cycle(state, contract, 1);
            match first {
                Some(first) if now < first.start => first,
                _ => cycle(state, contract, last).unwrap_or(Cycle {
                    index: 1,
                    start: now,
                    end: now,
                }),
            }
        }
    }
}

/// The cycle a billing attempt without a selector bills: the earliest cycle that is due (its
/// expected date reached), not billed and not skipped, else the one now falls in. Shopify says
/// "the current billing cycle" without saying which cycle that is at a cycle's end, when a
/// contract is billed; lsf bills the one whose bill is due.
pub fn due(state: &ShopState, contract: &Record) -> Cycle {
    let now = state.now();
    let current = current(state, contract);
    for index in 1..current.index {
        let Some(cycle) = cycle(state, contract, index) else {
            break;
        };
        if expected_date(contract, &cycle) <= now
            && !is_skipped(contract, index)
            && !is_billed(state, contract, index)
        {
            return cycle;
        }
    }
    current
}

/// The cycles that overlap a range of dates, in order.
pub fn cycles_between(
    state: &ShopState,
    contract: &Record,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Vec<Cycle> {
    let limit = state.now() + Duration::days(UPCOMING_LIMIT_DAYS);
    let mut cycles = Vec::new();
    let mut index = 1;
    while index <= MAX_WALK {
        let Some(cycle) = cycle(state, contract, index) else {
            break;
        };
        if cycle.start > to || cycle.start > limit {
            break;
        }
        if cycle.end > from {
            cycles.push(cycle);
        }
        index += 1;
    }
    cycles
}

// ---- edits ----------------------------------------------------------------------------------

/// The edits of a cycle, when it has some.
pub fn edits(contract: &Record, index: i64) -> Option<&Map<String, Value>> {
    contract
        .get("lsfCycleEdits")
        .and_then(|edits| edits.get(index.to_string()))
        .and_then(Value::as_object)
}

/// Whether a cycle is skipped.
pub fn is_skipped(contract: &Record, index: i64) -> bool {
    edits(contract, index)
        .and_then(|edits| edits.get("skipped"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// When a cycle is to be billed: its edit's date, else its end.
pub fn expected_date(contract: &Record, cycle: &Cycle) -> DateTime<Utc> {
    edits(contract, cycle.index)
        .and_then(|edits| text(edits, "billingAttemptExpectedDate"))
        .and_then(date_time)
        .unwrap_or(cycle.end)
}

/// The billing attempts of a contract's cycle.
pub fn attempts(state: &ShopState, contract: &Record, index: i64) -> Vec<String> {
    gids(contract.get("billingAttempts"))
        .into_iter()
        .filter(|attempt| {
            state
                .get(attempt)
                .and_then(|record| record.get("lsfCycleIndex"))
                .and_then(Value::as_i64)
                == Some(index)
        })
        .collect()
}

/// Whether a billing attempt of a cycle succeeded.
pub fn is_billed(state: &ShopState, contract: &Record, index: i64) -> bool {
    attempts(state, contract, index).iter().any(|attempt| {
        state
            .get(attempt)
            .and_then(|record| record.pointer("/state/__typename"))
            .and_then(Value::as_str)
            == Some("SubscriptionBillingAttemptSuccessState")
    })
}

/// Whether a cycle has a billing attempt still in progress (a challenge not completed).
pub fn has_incomplete_attempt(state: &ShopState, contract: &Record, index: i64) -> bool {
    attempts(state, contract, index).iter().any(|attempt| {
        matches!(
            state
                .get(attempt)
                .and_then(|record| record.pointer("/state/__typename"))
                .and_then(Value::as_str),
            Some(
                "SubscriptionBillingAttemptPendingState"
                    | "SubscriptionBillingAttemptActionRequiredState"
            )
        )
    })
}

/// Sets what a cycle's edit holds (`skipped`, `billingAttemptExpectedDate`, `contract`, …);
/// `None` removes the field.
pub fn set_edit(contract: &mut Record, index: i64, field: &str, value: Option<Value>) {
    let edits = contract.entry("lsfCycleEdits").or_insert_with(|| json!({}));
    if !edits.is_object() {
        *edits = json!({});
    }
    let entry = &mut edits[index.to_string()];
    if !entry.is_object() {
        *entry = json!({});
    }
    match value {
        Some(value) => entry[field] = value,
        None => {
            if let Some(map) = entry.as_object_mut() {
                map.remove(field);
            }
        }
    }
}

/// Removes every edit of a cycle.
pub fn clear_edits(contract: &mut Record, index: i64) {
    if let Some(edits) = contract
        .get_mut("lsfCycleEdits")
        .and_then(Value::as_object_mut)
    {
        edits.remove(&index.to_string());
    }
}

/// The key of a cycle in the events: a GID that names no record.
pub fn cycle_key(contract: &str, index: i64) -> String {
    format!(
        "gid://shopify/SubscriptionBillingCycle/{}?index={index}",
        model::number(contract)
    )
}

/// A cycle as `SubscriptionBillingCycle`: its dates, its edits, its attempts, its status.
pub fn to_value(state: &ShopState, contract_gid: &str, contract: &Record, cycle: &Cycle) -> Value {
    let edits = edits(contract, cycle.index);
    let edited_contract = edits
        .and_then(|edits| edits.get("contract"))
        .filter(|value| !value.is_null())
        .cloned()
        .unwrap_or(Value::Null);
    json!({
        "__typename": "SubscriptionBillingCycle",
        "cycleIndex": cycle.index,
        "cycleStartAt": value::date_time(cycle.start),
        "cycleEndAt": value::date_time(cycle.end),
        "billingAttemptExpectedDate": value::date_time(expected_date(contract, cycle)),
        "skipped": is_skipped(contract, cycle.index),
        "edited": edits.is_some(),
        "editedContract": edited_contract,
        "status": if is_billed(state, contract, cycle.index) { "BILLED" } else { "UNBILLED" },
        "billingAttempts": attempts(state, contract, cycle.index),
        "sourceContract": contract_gid,
        "lsfContract": contract_gid,
    })
}

/// The topic of a cycle skipped or unskipped.
fn skip_topic(skipped: bool) -> &'static str {
    if skipped {
        "subscription_billing_cycles/skip"
    } else {
        "subscription_billing_cycles/unskip"
    }
}

/// The topic of an edit about to be made to a cycle: `/update` when the cycle has one already.
pub fn edit_topic(state: &ShopState, contract: &str, index: i64) -> &'static str {
    if state
        .get(contract)
        .is_some_and(|record| edits(record, index).is_some())
    {
        "subscription_billing_cycle_edits/update"
    } else {
        "subscription_billing_cycle_edits/create"
    }
}

/// Emits an event about a cycle, with the cycle as it is now as what its payload is built from.
pub fn emit_cycle(state: &mut ShopState, topic: &str, contract_gid: &str, index: i64) {
    let Some(contract) = state.get(contract_gid).cloned() else {
        return;
    };
    let Some(cycle) = cycle(state, &contract, index) else {
        return;
    };
    let snapshot = value::object(to_value(state, contract_gid, &contract, &cycle));
    state.emit_with(topic, &cycle_key(contract_gid, index), snapshot);
}

// ---- queries --------------------------------------------------------------------------------

/// `subscriptionBillingCycle(billingCycleInput:)`.
fn billing_cycle(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let input = object(ctx.arg("billingCycleInput"));
    let id = text(&input, "contractId").unwrap_or_default();
    let Some(contract) = contract_arg(r.state(), id) else {
        return Ok(Output::Null);
    };
    match select(r.state(), &contract, &object(input.get("selector"))) {
        Ok(cycle) => r.convert(ctx, &to_value(r.state(), id, &contract, &cycle), false),
        Err(_) => Ok(Output::Null),
    }
}

/// `subscriptionBillingCycles(contractId:, billingCyclesDateRangeSelector:,
/// billingCyclesIndexRangeSelector:)`: the cycles of the range (overlapping a date range), from
/// cycle 1 to a year ahead without one.
fn billing_cycles(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    _object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let id = ctx.arg_str("contractId").unwrap_or_default();
    let Some(contract) = contract_arg(r.state(), id) else {
        return r.connection(ctx, Vec::new());
    };
    let state = r.state();
    let dates = object(ctx.arg("billingCyclesDateRangeSelector"));
    let indexes = object(ctx.arg("billingCyclesIndexRangeSelector"));
    let cycles = if !indexes.is_empty() {
        let start = indexes
            .get("startIndex")
            .and_then(Value::as_i64)
            .unwrap_or(1)
            .max(1);
        let end = indexes
            .get("endIndex")
            .and_then(Value::as_i64)
            .unwrap_or(start);
        let limit = state.now() + Duration::days(UPCOMING_LIMIT_DAYS);
        (start..=end.min(start + MAX_WALK))
            .map_while(|index| cycle(state, &contract, index))
            .take_while(|cycle| cycle.start <= limit)
            .collect()
    } else if !dates.is_empty() {
        let from = text(&dates, "startDate").and_then(date_time);
        let to = text(&dates, "endDate").and_then(date_time);
        match (from, to) {
            (Some(from), Some(to)) => cycles_between(state, &contract, from, to),
            _ => Vec::new(),
        }
    } else {
        cycles_between(
            state,
            &contract,
            DateTime::<Utc>::MIN_UTC,
            state.now() + Duration::days(UPCOMING_LIMIT_DAYS),
        )
    };
    let items: Vec<Value> = cycles
        .iter()
        .map(|cycle| to_value(state, id, &contract, cycle))
        .collect();
    r.connection(ctx, items)
}

/// `SubscriptionBillingCycleEditedContract.billingCycles`: the cycle the edit is of.
fn edited_contract_cycles(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let Some(edited) = r.map_of(object) else {
        return r.connection(ctx, Vec::new());
    };
    let contract_gid = text(edited, "lsfContract").unwrap_or_default().to_string();
    let index = edited
        .get("lsfCycleIndex")
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let state = r.state();
    let items = match contract_arg(state, &contract_gid) {
        Some(contract) => cycle(state, &contract, index)
            .map(|cycle| vec![to_value(state, &contract_gid, &contract, &cycle)])
            .unwrap_or_default(),
        None => Vec::new(),
    };
    r.connection(ctx, items)
}

/// `SubscriptionBillingCycleEditedContract.orders`: the orders the cycle's billing attempts made.
fn edited_contract_orders(
    r: &ShopResolver<'_>,
    ctx: &FieldContext<'_>,
    object: &Obj,
) -> Result<Output<Obj>, FieldError> {
    let state = r.state();
    let items: Vec<Value> = r
        .map_of(object)
        .and_then(|edited| {
            let contract = contract_arg(state, text(edited, "lsfContract")?)?;
            let index = edited.get("lsfCycleIndex")?.as_i64()?;
            Some(
                attempts(state, &contract, index)
                    .iter()
                    .filter_map(|attempt| {
                        state
                            .get(attempt)?
                            .get("order")
                            .filter(|order| order.is_string())
                            .cloned()
                    })
                    .collect(),
            )
        })
        .unwrap_or_default();
    r.connection(ctx, items)
}

// ---- mutations ------------------------------------------------------------------------------

/// The contract and the cycle a `SubscriptionBillingCycleInput` names, or the error (`code` of
/// the payload's error type for what is not a selection problem).
fn input_cycle(
    state: &ShopState,
    input: &Map<String, Value>,
    field: &str,
) -> Result<(String, Record, Cycle), UserError> {
    let id = text(input, "contractId").unwrap_or_default().to_string();
    let Some(contract) = contract_arg(state, &id) else {
        // lsf's wording, as `BillingAttemptUserErrorCode.CONTRACT_NOT_FOUND`'s.
        return Err(model::invalid_error(
            &[field, "contractId"],
            "Subscription contract does not exist",
        ));
    };
    match select(state, &contract, &object(input.get("selector"))) {
        Ok(cycle) => Ok((id, contract, cycle)),
        Err(miss) => Err(cycle_error(&[field, "selector"], miss.code())),
    }
}

/// An error of a payload whose code enum has only `INVALID` (skip, unskip), with the message of
/// the error it stands for.
fn invalid(error: UserError) -> UserError {
    UserError {
        code: Some("INVALID".to_string()),
        ..error
    }
}

/// `subscriptionBillingCycleSkip` and `subscriptionBillingCycleUnskip`: a cycle not billed yet
/// is skipped (no billing attempt can bill it) or billed again. Doing what is already done
/// changes nothing and emits nothing.
fn skip(
    mutation: &mut Mutation<'_>,
    ctx: &FieldContext<'_>,
    skipped: bool,
) -> Result<Value, FieldError> {
    let input = object(ctx.arg("billingCycleInput"));
    let state = mutation.state();
    let (id, contract, cycle) = match input_cycle(state, &input, "billingCycleInput") {
        Ok(found) => found,
        Err(error) => return Ok(payload(json!({"billingCycle": null}), &[invalid(error)])),
    };
    if is_terminated(&contract) {
        return Ok(payload(
            json!({"billingCycle": null}),
            &[model::invalid_error(
                &["billingCycleInput", "contractId"],
                model::described(
                    "SubscriptionContractStatusUpdateErrorCode",
                    "CONTRACT_TERMINATED",
                ),
            )],
        ));
    }
    if is_billed(state, &contract, cycle.index) {
        // lsf's wording: a billed cycle is not skipped.
        return Ok(payload(
            json!({"billingCycle": null}),
            &[model::invalid_error(
                &["billingCycleInput", "selector"],
                "Billing cycle has already been billed",
            )],
        ));
    }
    if has_incomplete_attempt(state, &contract, cycle.index) {
        return Ok(payload(
            json!({"billingCycle": null}),
            &[invalid(cycle_error(
                &["billingCycleInput", "selector"],
                "INCOMPLETE_BILLING_ATTEMPTS",
            ))],
        ));
    }
    if is_skipped(&contract, cycle.index) != skipped {
        let state = mutation.state_mut();
        if let Some(record) = state.get_mut(&id) {
            set_edit(record, cycle.index, "skipped", Some(json!(skipped)));
        }
        emit_cycle(state, skip_topic(skipped), &id, cycle.index);
    }
    let state = mutation.state();
    let contract = contract_arg(state, &id).unwrap_or_default();
    Ok(payload(
        json!({"billingCycle": to_value(state, &id, &contract, &cycle)}),
        &[],
    ))
}

/// `subscriptionBillingCycleScheduleEdit`: moves a cycle's billing date within the cycle
/// (`OUT_OF_BOUNDS` outside `cycleStartAt`…`cycleEndAt`), or skips or unskips it.
fn schedule_edit(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let cycle_input = object(ctx.arg("billingCycleInput"));
    let input = object(ctx.arg("input"));
    let state = mutation.state();
    let (id, contract, cycle) = match input_cycle(state, &cycle_input, "billingCycleInput") {
        Ok(found) => found,
        Err(error) => return Ok(payload(json!({"billingCycle": null}), &[error])),
    };
    let skip = input.get("skip").and_then(Value::as_bool);
    let date = input.get("billingDate").filter(|value| !value.is_null());
    if skip.is_none() && date.is_none() {
        return Ok(payload(
            json!({"billingCycle": null}),
            &[cycle_error(
                &["input"],
                "EMPTY_BILLING_CYCLE_EDIT_SCHEDULE_INPUT",
            )],
        ));
    }
    let date = match date.map(|date| date.as_str().and_then(date_time)) {
        Some(Some(date)) if date < cycle.start || date > cycle.end => {
            return Ok(payload(
                json!({"billingCycle": null}),
                &[cycle_error(&["input", "billingDate"], "OUT_OF_BOUNDS")],
            ));
        }
        Some(Some(date)) => Some(date),
        Some(None) => {
            return Ok(payload(
                json!({"billingCycle": null}),
                &[cycle_error(&["input", "billingDate"], "INVALID_DATE")],
            ));
        }
        None => None,
    };
    let will_skip = skip.unwrap_or_else(|| is_skipped(&contract, cycle.index));
    if date.is_some() && will_skip {
        return Ok(payload(
            json!({"billingCycle": null}),
            &[cycle_error(
                &["input", "billingDate"],
                "BILLING_DATE_SET_ON_SKIPPED",
            )],
        ));
    }
    if is_billed(state, &contract, cycle.index) {
        return Ok(payload(
            json!({"billingCycle": null}),
            &[model::invalid_error(
                &["billingCycleInput", "selector"],
                "Billing cycle has already been billed",
            )],
        ));
    }
    if has_incomplete_attempt(state, &contract, cycle.index) {
        return Ok(payload(
            json!({"billingCycle": null}),
            &[cycle_error(
                &["billingCycleInput", "selector"],
                "INCOMPLETE_BILLING_ATTEMPTS",
            )],
        ));
    }
    let edited_topic = edit_topic(state, &id, cycle.index);
    let was_skipped = is_skipped(&contract, cycle.index);
    let state = mutation.state_mut();
    if let Some(record) = state.get_mut(&id) {
        if let Some(skip) = skip {
            set_edit(record, cycle.index, "skipped", Some(json!(skip)));
        }
        if let Some(date) = date {
            set_edit(
                record,
                cycle.index,
                "billingAttemptExpectedDate",
                Some(json!(value::date_time(date))),
            );
        }
        set_edit(
            record,
            cycle.index,
            "reason",
            Some(input.get("reason").cloned().unwrap_or(Value::Null)),
        );
    }
    if let Some(skip) = skip
        && skip != was_skipped
    {
        emit_cycle(state, skip_topic(skip), &id, cycle.index);
    }
    if date.is_some() {
        emit_cycle(state, edited_topic, &id, cycle.index);
    }
    let state = mutation.state();
    let contract = contract_arg(state, &id).unwrap_or_default();
    Ok(payload(
        json!({"billingCycle": to_value(state, &id, &contract, &cycle)}),
        &[],
    ))
}

/// Deletes the edits of cycles (`indexes`) of a contract, with `subscription_billing_cycle_edits
/// /delete` for each. Returns the cycles as they are now.
fn delete_edits(state: &mut ShopState, id: &str, indexes: &[i64]) -> Vec<Value> {
    for index in indexes {
        emit_cycle(state, "subscription_billing_cycle_edits/delete", id, *index);
        if let Some(record) = state.get_mut(id) {
            clear_edits(record, *index);
        }
    }
    let contract = contract_arg(state, id).unwrap_or_default();
    indexes
        .iter()
        .filter_map(|index| cycle(state, &contract, *index))
        .map(|cycle| to_value(state, id, &contract, &cycle))
        .collect()
}

/// `subscriptionBillingCycleEditDelete`: the schedule and contract edits of one cycle.
fn edit_delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let input = object(ctx.arg("billingCycleInput"));
    let state = mutation.state();
    let (id, contract, cycle) = match input_cycle(state, &input, "billingCycleInput") {
        Ok(found) => found,
        Err(error) => return Ok(payload(json!({"billingCycles": null}), &[error])),
    };
    if edits(&contract, cycle.index).is_none() {
        return Ok(payload(
            json!({"billingCycles": null}),
            &[cycle_error(&["billingCycleInput"], "NO_CYCLE_EDITS")],
        ));
    }
    let cycles = delete_edits(mutation.state_mut(), &id, &[cycle.index]);
    Ok(payload(json!({"billingCycles": cycles}), &[]))
}

/// `subscriptionBillingCycleEditsDelete(targetSelection: ALL)`: the edits of the current and
/// the future cycles.
fn edits_delete(mutation: &mut Mutation<'_>, ctx: &FieldContext<'_>) -> Result<Value, FieldError> {
    let id = ctx.arg_str("contractId").unwrap_or_default().to_string();
    let state = mutation.state();
    let Some(contract) = contract_arg(state, &id) else {
        return Ok(payload(
            json!({"billingCycles": null}),
            &[model::invalid_error(
                &["contractId"],
                "Subscription contract does not exist",
            )],
        ));
    };
    let current = current(state, &contract).index;
    let mut indexes: Vec<i64> = contract
        .get("lsfCycleEdits")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|edits| edits.keys())
        .filter_map(|key| key.parse::<i64>().ok())
        .filter(|index| *index >= current)
        .collect();
    indexes.sort_unstable();
    if indexes.is_empty() {
        return Ok(payload(
            json!({"billingCycles": null}),
            &[cycle_error(&["contractId"], "NO_CYCLE_EDITS")],
        ));
    }
    let cycles = delete_edits(mutation.state_mut(), &id, &indexes);
    Ok(payload(json!({"billingCycles": cycles}), &[]))
}

/// Whether a contract has an edit of its contract on the current or a future cycle: such a
/// contract cannot be changed until they are deleted (`HAS_FUTURE_EDITS`).
pub fn has_future_contract_edits(state: &ShopState, contract: &Record) -> bool {
    let current = current(state, contract).index;
    contract
        .get("lsfCycleEdits")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .any(|(key, edit)| {
            key.parse::<i64>().is_ok_and(|index| index >= current)
                && edit
                    .get("contract")
                    .is_some_and(|contract| !contract.is_null())
        })
}
